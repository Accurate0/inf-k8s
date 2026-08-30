use crate::crd::DEFAULT_GATEWAY;
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

const CONFIG_YAML: &str = include_str!(concat!(env!("OUT_DIR"), "/config.merged.yaml"));

pub const CONFIG_SCHEMA_VERSION: u32 = 2;

const CONFIG_PATH_ENV: &str = "WORKFLOWS_CONFIGMAP_PATH";

#[derive(Deserialize)]
struct Probe {
    version: u32,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,

    #[serde(default)]
    pub defaults: Defaults,

    #[serde(default)]
    pub ignored_rule_ids: BTreeSet<String>,

    #[serde(default = "default_cooldown")]
    pub manual_unblock_cooldown: Span,

    #[serde(default)]
    pub never_block: Vec<String>,

    #[serde(default)]
    pub allowlist_sources: Vec<AllowlistSource>,

    #[serde(default = "default_allowlist_refresh")]
    pub allowlist_refresh: Span,

    #[serde(default)]
    pub workflows: Vec<WorkflowDef>,
}

impl Config {
    pub fn load() -> Self {
        let Ok(path) = std::env::var(CONFIG_PATH_ENV) else {
            tracing::info!("{CONFIG_PATH_ENV} unset; using baked-in workflow config");
            return Self::baked_in();
        };

        let merged = yaml_include::Transformer::new(path.clone().into(), true)
            .expect("failed to read workflow ConfigMap")
            .to_string();

        let probe: Probe =
            serde_yaml::from_str(&merged).expect("workflow ConfigMap has no readable version");

        if probe.version != CONFIG_SCHEMA_VERSION {
            tracing::warn!(
                configmap_version = probe.version,
                code_version = CONFIG_SCHEMA_VERSION,
                "workflow ConfigMap version incompatible with this binary; using baked-in config"
            );
            return Self::baked_in();
        }

        let config: Config =
            serde_yaml::from_str(&merged).expect("failed to parse workflow ConfigMap");

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

    #[serde(default = "default_window")]
    pub window: Span,

    #[serde(default = "default_candidate_limit")]
    pub candidate_limit: usize,

    #[serde(default = "default_max_blocklist_cidrs")]
    pub max_blocklist_cidrs: usize,

    #[serde(default = "default_max_block_duration")]
    pub max_block_duration: Span,

    #[serde(default = "default_escalation")]
    pub escalation: Vec<Span>,

    #[serde(default = "default_escalation_decay")]
    pub escalation_decay: Span,

    #[serde(default = "default_fast_candidate_limit")]
    pub fast_candidate_limit: usize,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            gateway: default_gateway(),
            window: default_window(),
            candidate_limit: default_candidate_limit(),
            max_blocklist_cidrs: default_max_blocklist_cidrs(),
            max_block_duration: default_max_block_duration(),
            escalation: default_escalation(),
            escalation_decay: default_escalation_decay(),
            fast_candidate_limit: default_fast_candidate_limit(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AllowlistSource {
    pub name: String,

    pub url: String,

    pub format: SourceFormat,

    #[serde(default)]
    pub fields: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceFormat {
    GithubMeta,
    CloudflareIps,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDef {
    pub name: String,

    #[serde(default)]
    pub enabled: Enabled,

    #[serde(default)]
    pub tier: Tier,

    #[serde(default)]
    pub window: Option<Span>,

    #[serde(default)]
    pub gateway: Option<String>,

    pub duration: BlockDuration,

    pub reason: String,

    #[serde(default)]
    pub candidates: Option<Template>,

    #[serde(default)]
    pub signals: BTreeMap<String, Template>,

    #[serde(rename = "when")]
    pub matcher: Matcher,
}

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

    pub fn block_span(&self, strikes: i32, defaults: &Defaults) -> std::time::Duration {
        let cap = defaults.max_block_duration.0;

        let own = match self.duration {
            BlockDuration::Forever => cap,
            BlockDuration::For(span) => span.0,
        };

        let rung = defaults
            .escalation
            .get(strikes.max(1) as usize - 1)
            .or_else(|| defaults.escalation.last())
            .map(|span| span.0)
            .unwrap_or(own);

        own.max(rung).min(cap)
    }

    pub fn expires_at(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        strikes: i32,
        defaults: &Defaults,
    ) -> Option<String> {
        let span = self.block_span(strikes, defaults);
        let at = now + chrono::Duration::from_std(span).unwrap_or(chrono::Duration::zero());

        Some(at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Tier {
    Fast,
    #[default]
    Standard,
}

impl Tier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Tier::Fast => "fast",
            Tier::Standard => "standard",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Enabled {
    Active,
    #[default]
    DryRun,
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

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LeafMatcher {
    Detections {
        #[serde(default)]
        min: u64,
        #[serde(default)]
        max: Option<u64>,
    },

    DistinctRules {
        min: usize,
    },

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
        let haystack = haystack.to_ascii_lowercase();
        let pattern = pattern.to_ascii_lowercase();

        match self {
            StringMatchMode::Contains => haystack.contains(&pattern),
            StringMatchMode::Prefix => haystack.starts_with(&pattern),
            StringMatchMode::Exact => haystack == pattern,
        }
    }
}

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

fn default_max_blocklist_cidrs() -> usize {
    20000
}

fn default_max_block_duration() -> Span {
    Span(std::time::Duration::from_secs(180 * 24 * 60 * 60))
}

fn default_escalation() -> Vec<Span> {
    vec![
        Span::from_secs(24 * 60 * 60),
        Span::from_secs(7 * 24 * 60 * 60),
        Span::from_secs(30 * 24 * 60 * 60),
        default_max_block_duration(),
    ]
}

fn default_escalation_decay() -> Span {
    Span::from_secs(90 * 24 * 60 * 60)
}

fn default_fast_candidate_limit() -> usize {
    10
}

fn default_allowlist_refresh() -> Span {
    Span::from_secs(12 * 60 * 60)
}

fn default_cooldown() -> Span {
    Span::from_secs(7 * 24 * 60 * 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_probe_tolerates_fields_this_binary_does_not_know() {
        let probe: Probe = serde_yaml::from_str(
            r#"
version: 99
some_field_from_the_future: true
workflows: []
"#,
        )
        .unwrap();

        assert_eq!(probe.version, 99);
    }

    #[test]
    fn the_baked_in_config_matches_the_binary_version() {
        assert_eq!(Config::baked_in().version, CONFIG_SCHEMA_VERSION);
    }

    #[test]
    fn baked_in_config_parses() {
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
    fn forever_is_clamped_to_the_max_block_duration() {
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

        let defaults = Defaults::default();
        let now = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        assert_eq!(
            workflow.expires_at(now, 1, &defaults).as_deref(),
            Some("2026-06-30T00:00:00Z")
        );
    }

    #[test]
    fn repeat_offenders_climb_the_escalation_ladder() {
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

        let defaults = Defaults::default();
        let now = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        assert_eq!(
            workflow.expires_at(now, 1, &defaults).as_deref(),
            Some("2026-01-02T00:00:00Z")
        );
        assert_eq!(
            workflow.expires_at(now, 2, &defaults).as_deref(),
            Some("2026-01-08T00:00:00Z")
        );
        assert_eq!(
            workflow.expires_at(now, 3, &defaults).as_deref(),
            Some("2026-01-31T00:00:00Z")
        );
        assert_eq!(
            workflow.expires_at(now, 99, &defaults).as_deref(),
            Some("2026-06-30T00:00:00Z")
        );
    }

    #[test]
    fn a_long_workflow_duration_is_never_shortened_by_a_low_strike_count() {
        let workflow: WorkflowDef = serde_yaml::from_str(
            r#"
name: strict
duration: 30d
reason: because
when:
  type: detections
  min: 1
"#,
        )
        .unwrap();

        let defaults = Defaults::default();
        let now = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        assert_eq!(
            workflow.expires_at(now, 1, &defaults).as_deref(),
            Some("2026-01-31T00:00:00Z")
        );
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
            workflow.expires_at(now, 1, &Defaults::default()).as_deref(),
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
        assert_eq!(workflow.tier, Tier::Standard);
    }

    #[test]
    fn a_workflow_can_opt_into_the_fast_tier() {
        let workflow: WorkflowDef = serde_yaml::from_str(
            r#"
name: urgent
tier: fast
duration: 1h
reason: because
when:
  type: detections
  min: 1
"#,
        )
        .unwrap();

        assert_eq!(workflow.tier, Tier::Fast);
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
