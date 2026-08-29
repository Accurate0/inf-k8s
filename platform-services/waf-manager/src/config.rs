use crate::crd::DEFAULT_GATEWAY;
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

/// Baked in at build time by `build.rs`; the fallback whenever no usable
/// ConfigMap is mounted.
const CONFIG_YAML: &str = include_str!(concat!(env!("OUT_DIR"), "/config.merged.yaml"));

/// Config format version this binary understands. Bump on any breaking change.
/// A ConfigMap whose `version` differs is rejected in favour of the baked-in
/// config. See [`Config::load`].
pub const CONFIG_SCHEMA_VERSION: u32 = 1;

/// Env var pointing at an externally-mounted `config.yaml` (a ConfigMap volume).
/// When unset, the baked-in config is used.
const CONFIG_PATH_ENV: &str = "WORKFLOWS_CONFIGMAP_PATH";

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,

    #[serde(default)]
    pub defaults: Defaults,

    /// Anomaly-score aggregates and body-parse noise: consequences of other rules
    /// rather than findings in their own right, so they are hidden from the
    /// ranking and ignored by `distinct_rules`.
    #[serde(default)]
    pub ignored_rule_ids: BTreeSet<String>,

    /// How long a workflow stays off a CIDR after a human unblocks it. Without
    /// this the next tick would undo the unblock.
    #[serde(default = "default_cooldown")]
    pub manual_unblock_cooldown: Span,

    #[serde(default)]
    pub workflows: Vec<WorkflowDef>,
}

impl Config {
    /// The ConfigMap at `WORKFLOWS_CONFIGMAP_PATH` wins when its `version` matches
    /// [`CONFIG_SCHEMA_VERSION`]; a version mismatch (breaking schema change) falls
    /// back to the baked-in config, while a malformed ConfigMap panics so the pod
    /// fails to start and the previous ReplicaSet keeps serving.
    pub fn load() -> Self {
        let Ok(path) = std::env::var(CONFIG_PATH_ENV) else {
            tracing::info!("{CONFIG_PATH_ENV} unset; using baked-in workflow config");
            return Self::baked_in();
        };

        // Resolve `!include`s the same way `build.rs` does, so the ConfigMap can
        // bundle the raw `config.yaml` + `workflows/*.yaml` rather than a
        // pre-merged file.
        let merged = yaml_include::Transformer::new(path.clone().into(), true)
            .expect("failed to read workflow ConfigMap")
            .to_string();
        let config: Config =
            serde_yaml::from_str(&merged).expect("failed to parse workflow ConfigMap");

        if config.version != CONFIG_SCHEMA_VERSION {
            tracing::warn!(
                configmap_version = config.version,
                code_version = CONFIG_SCHEMA_VERSION,
                "workflow ConfigMap version incompatible with this binary; using baked-in config"
            );
            return Self::baked_in();
        }

        tracing::info!(
            path,
            workflows = config.workflows.len(),
            "loaded workflow config from ConfigMap"
        );

        config
    }

    fn baked_in() -> Self {
        serde_yaml::from_str(CONFIG_YAML).expect("baked-in config deserialization failed")
    }

    /// Workflows that may act, in file order. `disabled` ones never reach the
    /// engine, so they cost no Loki queries.
    pub fn active_workflows(&self) -> impl Iterator<Item = &WorkflowDef> {
        self.workflows
            .iter()
            .filter(|w| w.enabled != Enabled::Disabled)
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    #[serde(default = "default_gateway")]
    pub gateway: String,

    /// Lookback for the Loki queries when a workflow does not set its own.
    #[serde(default = "default_window")]
    pub window: Span,

    /// How many top-detecting client IPs to consider each tick.
    #[serde(default = "default_candidate_limit")]
    pub candidate_limit: usize,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            gateway: default_gateway(),
            window: default_window(),
            candidate_limit: default_candidate_limit(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDef {
    pub name: String,

    #[serde(default)]
    pub enabled: Enabled,

    /// Lookback for this workflow's Loki queries. Workflows sharing a window
    /// share their queries.
    #[serde(default)]
    pub window: Option<Span>,

    #[serde(default)]
    pub gateway: Option<String>,

    /// How long the block it creates lasts.
    pub duration: BlockDuration,

    /// Recorded on the WafBlock so the dashboard says why it appeared.
    pub reason: String,

    /// Replaces the built-in "top detecting client IPs" query. Must return series
    /// carrying a `client_ip` label; the value becomes `detections`. Lets a
    /// workflow draw its candidates from somewhere other than raw Coraza volume.
    #[serde(default)]
    pub candidates: Option<Template>,

    /// Named LogQL instant queries, evaluated per candidate IP and folded to a
    /// number the `signal` matcher compares against. This is the escape hatch for
    /// anything the built-in matchers do not express.
    #[serde(default)]
    pub signals: BTreeMap<String, Template>,

    #[serde(rename = "when")]
    pub matcher: Matcher,
}

/// A LogQL query with `{{name}}` placeholders. `{{window}}`, `{{limit}}` and the
/// fragments from [`crate::loki::Loki::fragments`] are always available;
/// `{{client_ip}}`, `{{prefilter}}` and `{{exact_client}}` only in a signal, which
/// is evaluated against one address.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct Template(pub String);

impl Template {
    pub fn render(&self, vars: &BTreeMap<&str, String>) -> String {
        let mut out = self.0.clone();
        for (name, value) in vars {
            out = out.replace(&format!("{{{{{name}}}}}"), value);
        }

        out
    }

    /// Placeholders with no value, so a typo is reported rather than sent to Loki
    /// as a literal `{{typo}}`.
    ///
    /// Only bare identifiers count. LogQL's own `label_format` templates are also
    /// written `{{ .field }}`, and those must pass through untouched rather than
    /// be mistaken for a missing variable.
    pub fn unresolved(&self, vars: &BTreeMap<&str, String>) -> Vec<String> {
        let mut missing = Vec::new();
        let mut rest = self.0.as_str();

        while let Some(start) = rest.find("{{") {
            let after = &rest[start + 2..];
            let Some(end) = after.find("}}") else {
                break;
            };

            let name = &after[..end];
            let is_identifier =
                !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');

            if is_identifier && !vars.contains_key(name) {
                missing.push(name.to_string());
            }

            rest = &after[end + 2..];
        }

        missing
    }
}

impl WorkflowDef {
    pub fn window(&self, defaults: &Defaults) -> Span {
        self.window.unwrap_or(defaults.window)
    }

    pub fn gateway<'a>(&'a self, defaults: &'a Defaults) -> &'a str {
        self.gateway.as_deref().unwrap_or(&defaults.gateway)
    }

    /// RFC 3339 expiry, or `None` for a permanent block.
    pub fn expires_at(&self, now: chrono::DateTime<chrono::Utc>) -> Option<String> {
        match self.duration {
            BlockDuration::Forever => None,
            BlockDuration::For(span) => {
                let at =
                    now + chrono::Duration::from_std(span.0).unwrap_or(chrono::Duration::zero());
                Some(at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Enabled {
    /// Creates blocks.
    Active,
    /// Logs what it would block and records a metric, but creates nothing.
    #[default]
    DryRun,
    /// Never evaluated.
    Disabled,
}

impl Enabled {
    pub fn as_str(&self) -> &'static str {
        match self {
            Enabled::Active => "active",
            Enabled::DryRun => "dry-run",
            Enabled::Disabled => "disabled",
        }
    }
}

/// A duration like `30m`, `24h` or `7d`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Span(pub std::time::Duration);

impl JsonSchema for Span {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Span".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        string_schema(r"^\d+[smhd]$", "A duration such as `30m`, `12h` or `7d`.")
    }
}

impl Span {
    pub fn from_secs(secs: u64) -> Self {
        Self(std::time::Duration::from_secs(secs))
    }

    /// LogQL range vectors and the `since` parameter both accept a plain second
    /// count, so there is no need to preserve the unit the operator wrote.
    pub fn to_logql(self) -> String {
        format!("{}s", self.0.as_secs())
    }

    fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        let (digits, unit) = raw.split_at(
            raw.find(|c: char| !c.is_ascii_digit())
                .ok_or_else(|| format!("duration {raw:?} is missing a unit (s, m, h or d)"))?,
        );

        let value: u64 = digits
            .parse()
            .map_err(|_| format!("duration {raw:?} does not start with a number"))?;

        let multiplier = match unit {
            "s" => 1,
            "m" => 60,
            "h" => 60 * 60,
            "d" => 24 * 60 * 60,
            other => return Err(format!("unknown duration unit {other:?} in {raw:?}")),
        };

        Ok(Self::from_secs(value * multiplier))
    }
}

impl std::fmt::Display for Span {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let secs = self.0.as_secs();
        for (unit, size) in [("d", 86400), ("h", 3600), ("m", 60)] {
            if secs.is_multiple_of(size) && secs >= size {
                return write!(f, "{}{unit}", secs / size);
            }
        }

        write!(f, "{secs}s")
    }
}

impl<'de> Deserialize<'de> for Span {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// `forever` for a permanent block, otherwise a [`Span`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockDuration {
    Forever,
    For(Span),
}

impl JsonSchema for BlockDuration {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "BlockDuration".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        string_schema(
            r"^(forever|\d+[smhd])$",
            "How long a block lasts: a duration such as `24h`, or `forever`.",
        )
    }
}

impl std::fmt::Display for BlockDuration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlockDuration::Forever => write!(f, "forever"),
            BlockDuration::For(span) => write!(f, "{span}"),
        }
    }
}

impl<'de> Deserialize<'de> for BlockDuration {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        if raw.trim() == "forever" {
            return Ok(BlockDuration::Forever);
        }

        Span::parse(&raw)
            .map(BlockDuration::For)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Matcher {
    Combinator(Combinator),
    Leaf(LeafMatcher),
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub enum Combinator {
    #[serde(rename = "all")]
    All(Vec<Matcher>),
    #[serde(rename = "any")]
    Any(Vec<Matcher>),
    #[serde(rename = "not")]
    Not(Box<Matcher>),
}

/// Every leaf reads one of the three fact sources gathered per candidate IP:
/// the detection total, the per-rule breakdown, or the requested URIs.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LeafMatcher {
    /// Total Coraza detections in the window.
    Detections {
        #[serde(default)]
        min: u64,
        #[serde(default)]
        max: Option<u64>,
    },

    /// How many distinct rule ids the IP tripped; scanners fan out widely.
    DistinctRules { min: usize },

    RuleId {
        values: Vec<String>,
        #[serde(default = "one")]
        min_count: u64,
    },

    RuleMsgMatches {
        patterns: Vec<String>,
        #[serde(default)]
        mode: StringMatchMode,
        #[serde(default = "one")]
        min_count: u64,
    },

    Severity {
        values: Vec<String>,
        #[serde(default = "one")]
        min_count: u64,
    },

    UriMatches {
        patterns: Vec<String>,
        #[serde(default)]
        mode: StringMatchMode,
        #[serde(default = "one")]
        min_count: u64,
    },

    /// The value of one of the workflow's own `signals` queries.
    Signal {
        name: String,
        #[serde(default)]
        min: u64,
        #[serde(default)]
        max: Option<u64>,
    },
}

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StringMatchMode {
    #[default]
    Contains,
    Prefix,
    Exact,
}

impl StringMatchMode {
    pub fn matches(&self, haystack: &str, pattern: &str) -> bool {
        // Coraza logs the URI as sent; comparing case-insensitively stops
        // `/.ENV` from walking past a rule written in lowercase.
        let haystack = haystack.to_ascii_lowercase();
        let pattern = pattern.to_ascii_lowercase();

        match self {
            StringMatchMode::Contains => haystack.contains(&pattern),
            StringMatchMode::Prefix => haystack.starts_with(&pattern),
            StringMatchMode::Exact => haystack == pattern,
        }
    }
}

/// Both duration types are strings in YAML, so their schemas are the same shape:
/// a pattern the editor can check as you type.
fn string_schema(pattern: &str, description: &str) -> schemars::Schema {
    serde_json::from_value(serde_json::json!({
        "type": "string",
        "pattern": pattern,
        "description": description,
    }))
    .expect("static schema must deserialize")
}

fn one() -> u64 {
    1
}

fn default_gateway() -> String {
    DEFAULT_GATEWAY.to_string()
}

fn default_window() -> Span {
    Span::from_secs(60 * 60)
}

fn default_candidate_limit() -> usize {
    50
}

fn default_cooldown() -> Span {
    Span::from_secs(7 * 24 * 60 * 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baked_in_config_parses() {
        // A malformed config.yaml fails here rather than in a crash loop.
        let config = Config::baked_in();
        assert_eq!(config.version, CONFIG_SCHEMA_VERSION);
        assert!(!config.workflows.is_empty());
    }

    #[test]
    fn spans_parse_every_unit() {
        assert_eq!(Span::parse("45s").unwrap(), Span::from_secs(45));
        assert_eq!(Span::parse("30m").unwrap(), Span::from_secs(1800));
        assert_eq!(Span::parse("12h").unwrap(), Span::from_secs(43200));
        assert_eq!(Span::parse("7d").unwrap(), Span::from_secs(604800));
    }

    #[test]
    fn malformed_spans_are_rejected() {
        assert!(Span::parse("").is_err());
        assert!(Span::parse("24").is_err());
        assert!(Span::parse("24y").is_err());
        assert!(Span::parse("soon").is_err());
    }

    #[test]
    fn spans_render_back_to_the_largest_whole_unit() {
        assert_eq!(Span::from_secs(604800).to_string(), "7d");
        assert_eq!(Span::from_secs(43200).to_string(), "12h");
        assert_eq!(Span::from_secs(90).to_string(), "90s");
    }

    #[test]
    fn windows_become_second_counts_for_logql() {
        assert_eq!(Span::from_secs(3600).to_logql(), "3600s");
    }

    #[test]
    fn forever_produces_no_expiry() {
        let workflow: WorkflowDef = serde_yaml::from_str(
            r#"
name: permanent
duration: forever
reason: because
when:
  type: detections
  min: 1
"#,
        )
        .unwrap();

        assert_eq!(workflow.expires_at(chrono::Utc::now()), None);
    }

    #[test]
    fn a_fixed_duration_becomes_an_rfc3339_expiry() {
        let workflow: WorkflowDef = serde_yaml::from_str(
            r#"
name: temporary
duration: 24h
reason: because
when:
  type: detections
  min: 1
"#,
        )
        .unwrap();

        let now = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        assert_eq!(
            workflow.expires_at(now).as_deref(),
            Some("2026-01-02T00:00:00Z")
        );
    }

    #[test]
    fn workflows_default_to_dry_run() {
        let workflow: WorkflowDef = serde_yaml::from_str(
            r#"
name: unspecified
duration: 1h
reason: because
when:
  type: detections
  min: 1
"#,
        )
        .unwrap();

        assert_eq!(workflow.enabled, Enabled::DryRun);
    }

    #[test]
    fn disabled_workflows_are_not_active() {
        let config: Config = serde_yaml::from_str(
            r#"
version: 1
workflows:
  - name: off
    enabled: disabled
    duration: 1h
    reason: because
    when:
      type: detections
      min: 1
  - name: on
    enabled: active
    duration: 1h
    reason: because
    when:
      type: detections
      min: 1
"#,
        )
        .unwrap();

        let names: Vec<&str> = config.active_workflows().map(|w| w.name.as_str()).collect();
        assert_eq!(names, ["on"]);
    }

    #[test]
    fn unknown_fields_are_rejected() {
        // A typo in a workflow must fail the build, not silently do nothing.
        let err = serde_yaml::from_str::<WorkflowDef>(
            r#"
name: typo
duration: 1h
reason: because
expires_after: 2h
when:
  type: detections
  min: 1
"#,
        );

        assert!(err.is_err());
    }

    #[test]
    fn logql_label_format_templates_are_not_mistaken_for_placeholders() {
        // `{{ .x_envoy_origin_path }}` is LogQL's own templating, not ours.
        let template = Template(
            "{{access_selector}} | label_format path=`{{ .x_envoy_origin_path }}` {{typo}}".into(),
        );
        let vars = [("access_selector", "sel".to_string())]
            .into_iter()
            .collect();

        assert_eq!(template.unresolved(&vars), ["typo"]);
    }

    #[test]
    fn every_baked_in_query_renders() {
        // A placeholder typo would otherwise reach Loki as literal `{{name}}`
        // and the workflow would silently never fire.
        let config = Config::baked_in();
        let vars: BTreeMap<&str, String> = crate::loki::Loki::fragments()
            .into_iter()
            .map(|(k, v)| (k, v.to_string()))
            .chain([
                ("window", "3600s".to_string()),
                ("limit", "50".to_string()),
                ("client_ip", "203.0.113.4".to_string()),
                ("prefilter", "|= `203.0.113.4`".to_string()),
                ("exact_client", "| client_ip = `203.0.113.4`".to_string()),
            ])
            .collect();

        for workflow in &config.workflows {
            for (name, template) in &workflow.signals {
                assert_eq!(
                    template.unresolved(&vars),
                    Vec::<String>::new(),
                    "{}/{name}",
                    workflow.name
                );
            }

            if let Some(candidates) = &workflow.candidates {
                assert_eq!(
                    candidates.unresolved(&vars),
                    Vec::<String>::new(),
                    "{} candidates",
                    workflow.name
                );
            }
        }
    }

    #[test]
    fn string_match_modes_are_case_insensitive() {
        assert!(StringMatchMode::Contains.matches("/API/.ENV", "/.env"));
        assert!(StringMatchMode::Prefix.matches("/.env.backup", "/.ENV"));
        assert!(StringMatchMode::Exact.matches("/.env", "/.env"));
        assert!(!StringMatchMode::Exact.matches("/.env.backup", "/.env"));
    }
}
