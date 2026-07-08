//! Admin CLI for the feature-flags service. Talks to the `Admin` gRPC API; point
//! `--url` at a port-forwarded backend (`kubectl -n feature-flags port-forward
//! svc/api 50051:50051`). Results are rendered as JSON.

use anyhow::{Context as _, bail};
use clap::{Parser, Subcommand, ValueEnum};
use console::style;
use feature_flags::flag_config::{
    ConstraintDoc, DistDoc, FlagDoc, RuleDoc, SegmentDoc, SegmentsFile,
};
use feature_flags::pb;
use pb::admin_client::AdminClient;
use prost_types::value::Kind;
use serde_json::{Value as Json, json};
use similar::{ChangeTag, TextDiff};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use tonic::Request;
use tonic::transport::Channel;

#[derive(Parser)]
#[command(name = "ffctl", about = "feature-flags admin CLI")]
struct Cli {
    #[arg(long, env = "FFCTL_URL", default_value = "http://localhost:50051")]
    url: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Manage flags
    Flag {
        #[command(subcommand)]
        action: FlagAction,
    },
    /// Manage a flag's variants
    Variant {
        #[command(subcommand)]
        action: VariantAction,
    },
    /// Manage targeting segments
    Segment {
        #[command(subcommand)]
        action: SegmentAction,
    },
    /// Replace the ordered targeting rules for a flag
    Rules {
        #[command(subcommand)]
        action: RulesAction,
    },
    /// Declarative, Terraform-style management from a config directory
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Show the diff between the config directory and the live service without writing.
    Plan {
        /// Directory holding `flags/*.yaml` and `segments.yaml`.
        #[arg(long, default_value = "config")]
        dir: String,
    },
    /// Reconcile the live service to the config directory. Flags and segments absent
    /// from the config are deleted. Prints the plan and asks to confirm.
    Apply {
        #[arg(long, default_value = "config")]
        dir: String,
        /// Identity recorded in the audit log; falls back to `$FFCTL_ACTOR` then
        /// `git config user.email`.
        #[arg(long, env = "FFCTL_ACTOR")]
        actor: Option<String>,
        /// Skip the interactive confirmation prompt.
        #[arg(long)]
        auto_approve: bool,
    },
    /// Dump the live flags and segments into the config directory layout, so current
    /// state can be captured as code before enabling prune-on-apply.
    Export {
        #[arg(long, default_value = "config")]
        dir: String,
    },
}

#[derive(Subcommand)]
enum FlagAction {
    /// Create a flag. Variants are `key=value` pairs; values are parsed as JSON,
    /// falling back to a bare string (e.g. `on=true`, `model=gpt-4`).
    Create {
        key: String,
        #[arg(long, value_enum)]
        r#type: FlagType,
        /// Whether targeting is enabled (otherwise the default variant always serves).
        #[arg(long)]
        enabled: bool,
        /// The variant key served when no rule matches or targeting is disabled.
        #[arg(long)]
        default: String,
        /// A variant, as `key=json`; repeatable.
        #[arg(long = "variant")]
        variants: Vec<String>,
    },
    /// Fetch a single flag
    Get { key: String },
    /// List flags
    List {
        #[arg(long)]
        archived: bool,
    },
    /// Update a flag's enabled state and/or default variant
    Update {
        key: String,
        #[arg(long)]
        enabled: bool,
        #[arg(long)]
        default: String,
    },
    /// Archive (or restore with `--restore`) a flag
    Archive {
        key: String,
        #[arg(long)]
        restore: bool,
    },
    /// Permanently delete a flag
    Delete { key: String },
}

#[derive(Subcommand)]
enum VariantAction {
    /// Add or update a variant. `value` is parsed as JSON, falling back to a string.
    Set {
        flag_key: String,
        variant_key: String,
        value: String,
    },
    /// Remove a variant from a flag
    Delete {
        flag_key: String,
        variant_key: String,
    },
}

#[derive(Subcommand)]
enum SegmentAction {
    /// Create or update a segment from a JSON document, e.g.
    /// `{"key":"beta","name":"Beta","constraints":[{"attribute":"plan","operator":"IN","values":["pro"]}]}`.
    /// Operators are the short names from the proto (EQ, IN, STARTS_WITH, ...).
    Set { json: String },
    /// Fetch a single segment
    Get { key: String },
    /// List segments
    List,
    /// Delete a segment
    Delete { key: String },
}

#[derive(Subcommand)]
enum RulesAction {
    /// Replace a flag's rules from a JSON array, ordered by priority, e.g.
    /// `[{"segment_key":"beta","variant_key":"on"},{"constraint_groups":[[{"attribute":"country","operator":"IN","values":["AU","NZ"]}],[{"attribute":"plan","operator":"EQ","values":["pro"]}]],"variant_key":"on"}]`.
    /// `constraint_groups` is CNF: groups are AND-combined, constraints within a group OR-combined; `constraints` (a flat array) is sugar for plain AND. Both work with or without a `segment_key`.
    /// A `FLAG_MATCHES` operator depends on another flag: `attribute` is the flag key and `values` the variant keys it must resolve to (a prerequisite).
    Set { flag_key: String, json: String },
}

#[derive(Clone, Copy, ValueEnum)]
enum FlagType {
    Bool,
    String,
    Int,
    Float,
    Object,
}

impl From<FlagType> for pb::ValueType {
    fn from(t: FlagType) -> Self {
        match t {
            FlagType::Bool => pb::ValueType::Boolean,
            FlagType::String => pb::ValueType::String,
            FlagType::Int => pb::ValueType::Integer,
            FlagType::Float => pb::ValueType::Float,
            FlagType::Object => pb::ValueType::Object,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let mut admin = AdminClient::connect(cli.url.clone())
        .await
        .with_context(|| format!("connecting to {}", cli.url))?;

    match cli.command {
        Command::Flag { action } => flag(&mut admin, action).await,
        Command::Variant { action } => variant(&mut admin, action).await,
        Command::Segment { action } => segment(&mut admin, action).await,
        Command::Rules { action } => rules(&mut admin, action).await,
        Command::Config { action } => config(&mut admin, action).await,
    }
}

async fn flag(admin: &mut AdminClient<Channel>, action: FlagAction) -> anyhow::Result<()> {
    match action {
        FlagAction::Create {
            key,
            r#type,
            enabled,
            default,
            variants,
        } => {
            let variants = variants
                .iter()
                .map(|spec| parse_variant(spec))
                .collect::<anyhow::Result<_>>()?;
            let flag = admin
                .create_flag(pb::CreateFlagRequest {
                    key,
                    value_type: pb::ValueType::from(r#type) as i32,
                    enabled,
                    default_variant_key: default,
                    variants,
                })
                .await?
                .into_inner();
            print(flag_to_json(&flag))
        }
        FlagAction::Get { key } => {
            let flag = admin
                .get_flag(pb::GetFlagRequest { key })
                .await?
                .into_inner();
            print(flag_to_json(&flag))
        }
        FlagAction::List { archived } => {
            let flags = admin
                .list_flags(pb::ListFlagsRequest {
                    include_archived: archived,
                })
                .await?
                .into_inner()
                .flags;
            print(Json::Array(flags.iter().map(flag_to_json).collect()))
        }
        FlagAction::Update {
            key,
            enabled,
            default,
        } => {
            let flag = admin
                .update_flag(pb::UpdateFlagRequest {
                    key,
                    enabled,
                    default_variant_key: default,
                })
                .await?
                .into_inner();
            print(flag_to_json(&flag))
        }
        FlagAction::Archive { key, restore } => {
            let flag = admin
                .archive_flag(pb::ArchiveFlagRequest {
                    key,
                    archived: !restore,
                })
                .await?
                .into_inner();
            print(flag_to_json(&flag))
        }
        FlagAction::Delete { key } => {
            admin.delete_flag(pb::DeleteFlagRequest { key }).await?;
            println!("ok");
            Ok(())
        }
    }
}

async fn variant(admin: &mut AdminClient<Channel>, action: VariantAction) -> anyhow::Result<()> {
    match action {
        VariantAction::Set {
            flag_key,
            variant_key,
            value,
        } => {
            let flag = admin
                .upsert_variant(pb::UpsertVariantRequest {
                    flag_key,
                    variant: Some(pb::Variant {
                        key: variant_key,
                        value: Some(json_to_value(&parse_json(&value))),
                    }),
                })
                .await?
                .into_inner();
            print(flag_to_json(&flag))
        }
        VariantAction::Delete {
            flag_key,
            variant_key,
        } => {
            let flag = admin
                .delete_variant(pb::DeleteVariantRequest {
                    flag_key,
                    variant_key,
                })
                .await?
                .into_inner();
            print(flag_to_json(&flag))
        }
    }
}

async fn segment(admin: &mut AdminClient<Channel>, action: SegmentAction) -> anyhow::Result<()> {
    match action {
        SegmentAction::Set { json } => {
            let segment = parse_segment(&parse_json(&json))?;
            let saved = admin
                .update_segment(pb::UpdateSegmentRequest {
                    segment: Some(segment),
                })
                .await?
                .into_inner();
            print(segment_to_json(&saved))
        }
        SegmentAction::Get { key } => {
            let segment = admin
                .get_segment(pb::GetSegmentRequest { key })
                .await?
                .into_inner();
            print(segment_to_json(&segment))
        }
        SegmentAction::List => {
            let segments = admin
                .list_segments(pb::ListSegmentsRequest {})
                .await?
                .into_inner()
                .segments;
            print(Json::Array(segments.iter().map(segment_to_json).collect()))
        }
        SegmentAction::Delete { key } => {
            admin
                .delete_segment(pb::DeleteSegmentRequest { key })
                .await?;
            println!("ok");
            Ok(())
        }
    }
}

async fn rules(admin: &mut AdminClient<Channel>, action: RulesAction) -> anyhow::Result<()> {
    let RulesAction::Set { flag_key, json } = action;
    let Json::Array(items) = parse_json(&json) else {
        bail!("rules must be a JSON array");
    };
    let rules = items
        .iter()
        .map(parse_rule)
        .collect::<anyhow::Result<_>>()?;
    let flag = admin
        .set_flag_rules(pb::SetFlagRulesRequest { flag_key, rules })
        .await?
        .into_inner();
    print(flag_to_json(&flag))
}

// -- Declarative config (`ffctl config`) ------------------------------------------

async fn config(admin: &mut AdminClient<Channel>, action: ConfigAction) -> anyhow::Result<()> {
    match action {
        ConfigAction::Plan { dir } => {
            let (flags, segments) = load_config(&dir)?;
            let resp = admin
                .apply_config(pb::ApplyConfigRequest {
                    flags: flags.clone(),
                    segments: segments.clone(),
                    dry_run: true,
                    expected_version: 0,
                })
                .await?
                .into_inner();
            let live = fetch_live(admin).await?;
            render_plan(&resp.changes, &flags, &segments, &live);
            Ok(())
        }
        ConfigAction::Apply {
            dir,
            actor,
            auto_approve,
        } => {
            let actor = resolve_actor(actor);
            let (flags, segments) = load_config(&dir)?;
            let plan = admin
                .apply_config(request_with_actor(
                    pb::ApplyConfigRequest {
                        flags: flags.clone(),
                        segments: segments.clone(),
                        dry_run: true,
                        expected_version: 0,
                    },
                    &actor,
                ))
                .await?
                .into_inner();
            let live = fetch_live(admin).await?;
            render_plan(&plan.changes, &flags, &segments, &live);
            if plan.changes.is_empty() {
                return Ok(());
            }
            if !auto_approve && !confirm("Apply these changes?")? {
                println!("Aborted.");
                return Ok(());
            }
            let resp = admin
                .apply_config(request_with_actor(
                    pb::ApplyConfigRequest {
                        flags,
                        segments,
                        dry_run: false,
                        expected_version: plan.from_version,
                    },
                    &actor,
                ))
                .await
                .context("apply failed (config may have drifted; re-run plan)")?
                .into_inner();
            println!(
                "Applied {} change(s); config version {} -> {}.",
                resp.changes.len(),
                resp.from_version,
                resp.to_version
            );
            Ok(())
        }
        ConfigAction::Export { dir } => {
            let flags = admin
                .list_flags(pb::ListFlagsRequest {
                    include_archived: false,
                })
                .await?
                .into_inner()
                .flags;
            let segments = admin
                .list_segments(pb::ListSegmentsRequest {})
                .await?
                .into_inner()
                .segments;
            export_config(&dir, &flags, &segments)?;
            println!(
                "Wrote {} flag(s) and {} segment(s) to {dir}/.",
                flags.len(),
                segments.len()
            );
            Ok(())
        }
    }
}

/// Load every `flags/*.yaml` plus an optional `segments.yaml` from `dir` and convert
/// them to the wire types the `ApplyConfig` RPC expects.
fn load_config(dir: &str) -> anyhow::Result<(Vec<pb::Flag>, Vec<pb::Segment>)> {
    let flags_dir = Path::new(dir).join("flags");
    let mut flags = Vec::new();
    if flags_dir.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(&flags_dir)
            .with_context(|| format!("reading {}", flags_dir.display()))?
            .collect::<Result<_, _>>()?;
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            let path = entry.path();
            if !matches!(path.extension().and_then(|e| e.to_str()), Some("yaml" | "yml")) {
                continue;
            }
            let text = fs::read_to_string(&path)?;
            let doc: FlagDoc = serde_yaml::from_str(&text)
                .with_context(|| format!("parsing {}", path.display()))?;
            flags.push(flag_doc_to_pb(&doc).with_context(|| format!("in {}", path.display()))?);
        }
    }

    let mut segments = Vec::new();
    let segments_path = Path::new(dir).join("segments.yaml");
    if segments_path.is_file() {
        let text = fs::read_to_string(&segments_path)?;
        let file: SegmentsFile = serde_yaml::from_str(&text)
            .with_context(|| format!("parsing {}", segments_path.display()))?;
        for doc in &file.segments {
            segments.push(segment_doc_to_pb(doc)?);
        }
    }
    Ok((flags, segments))
}

fn export_config(dir: &str, flags: &[pb::Flag], segments: &[pb::Segment]) -> anyhow::Result<()> {
    let flags_dir = Path::new(dir).join("flags");
    fs::create_dir_all(&flags_dir)?;
    for flag in flags {
        let yaml = serde_yaml::to_string(&flag_to_doc(flag))?;
        fs::write(flags_dir.join(format!("{}.yaml", flag.key)), yaml)?;
    }
    let file = SegmentsFile {
        segments: segments.iter().map(segment_to_doc).collect(),
    };
    fs::write(Path::new(dir).join("segments.yaml"), serde_yaml::to_string(&file)?)?;
    Ok(())
}

fn flag_doc_to_pb(doc: &FlagDoc) -> anyhow::Result<pb::Flag> {
    let value_type = value_type_from_name(&doc.value_type)?;
    let variants = doc
        .variants
        .iter()
        .map(|(k, v)| pb::Variant {
            key: k.clone(),
            value: Some(json_to_value(v)),
        })
        .collect();
    let rules = doc
        .rules
        .iter()
        .enumerate()
        .map(|(i, r)| rule_doc_to_pb(i as u32, r))
        .collect::<anyhow::Result<_>>()?;
    Ok(pb::Flag {
        key: doc.key.clone(),
        value_type: value_type as i32,
        enabled: doc.enabled,
        default_variant_key: doc.default.clone(),
        archived: false,
        variants,
        rules,
    })
}

fn rule_doc_to_pb(rank: u32, r: &RuleDoc) -> anyhow::Result<pb::Rule> {
    let constraint_groups = if !r.constraint_groups.is_empty() {
        r.constraint_groups
            .iter()
            .map(|g| {
                Ok(pb::ConstraintGroup {
                    constraints: g
                        .iter()
                        .map(constraint_doc_to_pb)
                        .collect::<anyhow::Result<_>>()?,
                })
            })
            .collect::<anyhow::Result<_>>()?
    } else {
        r.constraints
            .iter()
            .map(|c| {
                Ok(pb::ConstraintGroup {
                    constraints: vec![constraint_doc_to_pb(c)?],
                })
            })
            .collect::<anyhow::Result<_>>()?
    };
    Ok(pb::Rule {
        rank,
        segment_key: r.segment.clone().unwrap_or_default(),
        variant_key: r.variant.clone().unwrap_or_default(),
        distributions: r
            .distributions
            .iter()
            .map(|d| pb::Distribution {
                variant_key: d.variant.clone(),
                weight: d.weight,
            })
            .collect(),
        constraint_groups,
        bucket_salt: r.bucket_salt.clone(),
    })
}

fn constraint_doc_to_pb(c: &ConstraintDoc) -> anyhow::Result<pb::Constraint> {
    Ok(pb::Constraint {
        attribute: c.attribute.clone(),
        operator: operator_from_name(&c.operator)? as i32,
        values: c.values.iter().map(json_to_value).collect(),
    })
}

fn segment_doc_to_pb(doc: &SegmentDoc) -> anyhow::Result<pb::Segment> {
    Ok(pb::Segment {
        key: doc.key.clone(),
        name: doc.name.clone(),
        constraints: doc
            .constraints
            .iter()
            .map(constraint_doc_to_pb)
            .collect::<anyhow::Result<_>>()?,
    })
}

fn flag_to_doc(f: &pb::Flag) -> FlagDoc {
    FlagDoc {
        key: f.key.clone(),
        value_type: value_type_name(f.value_type).to_owned(),
        enabled: f.enabled,
        default: f.default_variant_key.clone(),
        variants: f
            .variants
            .iter()
            .map(|v| {
                (
                    v.key.clone(),
                    v.value.as_ref().map(value_to_json).unwrap_or(Json::Null),
                )
            })
            .collect(),
        rules: f.rules.iter().map(rule_to_doc).collect(),
    }
}

fn rule_to_doc(r: &pb::Rule) -> RuleDoc {
    // Collapse to the flat `constraints` form when every group is a single constraint
    // (plain AND); otherwise keep the CNF `constraint_groups` form.
    let flat = r.constraint_groups.iter().all(|g| g.constraints.len() == 1);
    let (constraints, constraint_groups) = if flat {
        (
            r.constraint_groups
                .iter()
                .map(|g| constraint_to_doc(&g.constraints[0]))
                .collect(),
            Vec::new(),
        )
    } else {
        (
            Vec::new(),
            r.constraint_groups
                .iter()
                .map(|g| g.constraints.iter().map(constraint_to_doc).collect())
                .collect(),
        )
    };
    RuleDoc {
        segment: (!r.segment_key.is_empty()).then(|| r.segment_key.clone()),
        variant: (!r.variant_key.is_empty()).then(|| r.variant_key.clone()),
        distributions: r
            .distributions
            .iter()
            .map(|d| DistDoc {
                variant: d.variant_key.clone(),
                weight: d.weight,
            })
            .collect(),
        constraints,
        constraint_groups,
        bucket_salt: r.bucket_salt.clone(),
    }
}

fn segment_to_doc(s: &pb::Segment) -> SegmentDoc {
    SegmentDoc {
        key: s.key.clone(),
        name: s.name.clone(),
        constraints: s.constraints.iter().map(constraint_to_doc).collect(),
    }
}

fn constraint_to_doc(c: &pb::Constraint) -> ConstraintDoc {
    ConstraintDoc {
        attribute: c.attribute.clone(),
        operator: operator_name(c.operator),
        values: c.values.iter().map(value_to_json).collect(),
    }
}

fn value_type_from_name(s: &str) -> anyhow::Result<pb::ValueType> {
    Ok(match s {
        "boolean" => pb::ValueType::Boolean,
        "string" => pb::ValueType::String,
        "integer" => pb::ValueType::Integer,
        "float" => pb::ValueType::Float,
        "object" => pb::ValueType::Object,
        other => bail!("unknown flag type `{other}`"),
    })
}

fn value_type_name(v: i32) -> &'static str {
    match pb::ValueType::try_from(v).unwrap_or_default() {
        pb::ValueType::Boolean => "boolean",
        pb::ValueType::String => "string",
        pb::ValueType::Integer => "integer",
        pb::ValueType::Float => "float",
        pb::ValueType::Object => "object",
        pb::ValueType::Unspecified => "unspecified",
    }
}

/// Resolve the short operator name (e.g. `in`, `starts_with`) to its proto enum.
fn operator_from_name(s: &str) -> anyhow::Result<pb::ConstraintOperator> {
    let name = format!("CONSTRAINT_OPERATOR_{}", s.to_uppercase());
    pb::ConstraintOperator::from_str_name(&name).with_context(|| format!("unknown operator `{s}`"))
}

fn operator_name(op: i32) -> String {
    pb::ConstraintOperator::try_from(op)
        .unwrap_or_default()
        .as_str_name()
        .trim_start_matches("CONSTRAINT_OPERATOR_")
        .to_lowercase()
}

/// Live flags and segments keyed by name, used to render before/after diffs.
struct LiveState {
    flags: BTreeMap<String, pb::Flag>,
    segments: BTreeMap<String, pb::Segment>,
}

/// Fetch the full live state (including archived flags) so the plan can show a
/// before/after diff of every changed resource.
async fn fetch_live(admin: &mut AdminClient<Channel>) -> anyhow::Result<LiveState> {
    let flags = admin
        .list_flags(pb::ListFlagsRequest {
            include_archived: true,
        })
        .await?
        .into_inner()
        .flags
        .into_iter()
        .map(|f| (f.key.clone(), f))
        .collect();
    let segments = admin
        .list_segments(pb::ListSegmentsRequest {})
        .await?
        .into_inner()
        .segments
        .into_iter()
        .map(|s| (s.key.clone(), s))
        .collect();
    Ok(LiveState { flags, segments })
}

/// Print a Terraform-style plan: one header per changed resource (`+` create, `~`
/// update, `-` delete) followed by a unified YAML diff of its live vs desired form.
fn render_plan(
    changes: &[pb::ConfigChange],
    desired_flags: &[pb::Flag],
    desired_segments: &[pb::Segment],
    live: &LiveState,
) {
    if changes.is_empty() {
        println!("No changes. Live state already matches the config.");
        return;
    }
    let desired_flags: BTreeMap<&str, &pb::Flag> =
        desired_flags.iter().map(|f| (f.key.as_str(), f)).collect();
    let desired_segments: BTreeMap<&str, &pb::Segment> = desired_segments
        .iter()
        .map(|s| (s.key.as_str(), s))
        .collect();

    let (mut create, mut update, mut delete) = (0, 0, 0);
    for c in changes {
        let (sym, count) = match c.op() {
            pb::ChangeOp::Create => ("+", &mut create),
            pb::ChangeOp::Update => ("~", &mut update),
            pb::ChangeOp::Delete => ("-", &mut delete),
            pb::ChangeOp::Unspecified => ("?", &mut update),
        };
        *count += 1;

        let (before, after) = match c.target_kind.as_str() {
            "flag" => (
                live.flags.get(&c.target_key).map(yaml_of_flag).unwrap_or_default(),
                desired_flags.get(c.target_key.as_str()).map(|f| yaml_of_flag(f)).unwrap_or_default(),
            ),
            _ => (
                live.segments.get(&c.target_key).map(yaml_of_segment).unwrap_or_default(),
                desired_segments.get(c.target_key.as_str()).map(|s| yaml_of_segment(s)).unwrap_or_default(),
            ),
        };

        let header = format!("{sym} {} {}", c.target_kind, c.target_key);
        let header = match c.op() {
            pb::ChangeOp::Create => style(header).green(),
            pb::ChangeOp::Delete => style(header).red(),
            _ => style(header).yellow(),
        };
        println!("\n{}", header.bold());
        print_yaml_diff(&before, &after);
    }
    println!(
        "\nPlan: {} to create, {} to update, {} to delete.",
        style(create).green(),
        style(update).yellow(),
        style(delete).red(),
    );
}

fn yaml_of_flag(f: &pb::Flag) -> String {
    serde_yaml::to_string(&flag_to_doc(f)).unwrap_or_default()
}

fn yaml_of_segment(s: &pb::Segment) -> String {
    serde_yaml::to_string(&segment_to_doc(s)).unwrap_or_default()
}

/// Render a line-oriented unified diff, indented under the resource header.
fn print_yaml_diff(before: &str, after: &str) {
    let diff = TextDiff::from_lines(before, after);
    for change in diff.iter_all_changes() {
        let line = change.value();
        let line = line.strip_suffix('\n').unwrap_or(line);
        let rendered = match change.tag() {
            ChangeTag::Delete => style(format!("  - {line}")).red(),
            ChangeTag::Insert => style(format!("  + {line}")).green(),
            ChangeTag::Equal => style(format!("    {line}")).dim(),
        };
        println!("{rendered}");
    }
}

fn confirm(prompt: &str) -> anyhow::Result<bool> {
    use std::io::Write as _;
    print!("{prompt} [y/N]: ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes" | "Yes"))
}

/// Resolve the audit actor: explicit value (or `$FFCTL_ACTOR` via clap) first, then
/// the local git email, falling back to a constant so an audit row is always stamped.
fn resolve_actor(actor: Option<String>) -> String {
    if let Some(a) = actor.filter(|s| !s.is_empty()) {
        return a;
    }
    std::process::Command::new("git")
        .args(["config", "user.email"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "ffctl".to_owned())
}

/// Wrap a message in a request carrying the `actor` metadata header the Admin service
/// records in the audit log.
fn request_with_actor<T>(message: T, actor: &str) -> Request<T> {
    let mut request = Request::new(message);
    if let Ok(value) = actor.parse() {
        request.metadata_mut().insert("actor", value);
    }
    request
}

/// Parse a `key=value` variant spec, reading `value` as JSON with a bare-string fallback.
fn parse_variant(spec: &str) -> anyhow::Result<pb::Variant> {
    let (key, value) = spec
        .split_once('=')
        .with_context(|| format!("variant must be key=value, got `{spec}`"))?;
    Ok(pb::Variant {
        key: key.to_owned(),
        value: Some(json_to_value(&parse_json(value))),
    })
}

fn parse_segment(json: &Json) -> anyhow::Result<pb::Segment> {
    let obj = json.as_object().context("segment must be a JSON object")?;
    let constraints = obj
        .get("constraints")
        .and_then(Json::as_array)
        .map(|cs| {
            cs.iter()
                .map(parse_constraint)
                .collect::<anyhow::Result<_>>()
        })
        .transpose()?
        .unwrap_or_default();
    Ok(pb::Segment {
        key: str_field(obj, "key")?.to_owned(),
        name: obj
            .get("name")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_owned(),
        constraints,
    })
}

fn parse_constraint(json: &Json) -> anyhow::Result<pb::Constraint> {
    let obj = json
        .as_object()
        .context("constraint must be a JSON object")?;
    let op_name = format!(
        "CONSTRAINT_OPERATOR_{}",
        str_field(obj, "operator")?.to_uppercase()
    );
    let operator = pb::ConstraintOperator::from_str_name(&op_name)
        .with_context(|| format!("unknown operator `{}`", &op_name))?;
    let values = obj
        .get("values")
        .and_then(Json::as_array)
        .map(|vs| vs.iter().map(json_to_value).collect())
        .unwrap_or_default();
    Ok(pb::Constraint {
        attribute: str_field(obj, "attribute")?.to_owned(),
        operator: operator as i32,
        values,
    })
}

fn parse_rule(json: &Json) -> anyhow::Result<pb::Rule> {
    let obj = json.as_object().context("rule must be a JSON object")?;
    let distributions = obj
        .get("distributions")
        .and_then(Json::as_array)
        .map(|ds| {
            ds.iter()
                .map(|d| {
                    let d = d
                        .as_object()
                        .context("distribution must be a JSON object")?;
                    Ok(pb::Distribution {
                        variant_key: str_field(d, "variant_key")?.to_owned(),
                        weight: d.get("weight").and_then(Json::as_u64).unwrap_or(0) as u32,
                    })
                })
                .collect::<anyhow::Result<_>>()
        })
        .transpose()?
        .unwrap_or_default();
    let constraint_groups = parse_constraint_groups(obj)?;
    Ok(pb::Rule {
        // Rank is assigned by position on the server; array order is the priority.
        rank: 0,
        segment_key: obj
            .get("segment_key")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_owned(),
        variant_key: obj
            .get("variant_key")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_owned(),
        distributions,
        constraint_groups,
        bucket_salt: obj
            .get("bucket_salt")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_owned(),
    })
}

/// Parse a rule's inline constraint groups. Accepts `constraint_groups` (an array of
/// arrays, each inner array an OR-group) or `constraints` (a flat array sugar where
/// each constraint becomes its own group, i.e. plain AND).
fn parse_constraint_groups(
    obj: &serde_json::Map<String, Json>,
) -> anyhow::Result<Vec<pb::ConstraintGroup>> {
    if let Some(groups) = obj.get("constraint_groups").and_then(Json::as_array) {
        return groups
            .iter()
            .map(|g| {
                let constraints = g
                    .as_array()
                    .context("constraint group must be a JSON array")?
                    .iter()
                    .map(parse_constraint)
                    .collect::<anyhow::Result<_>>()?;
                Ok(pb::ConstraintGroup { constraints })
            })
            .collect();
    }
    if let Some(constraints) = obj.get("constraints").and_then(Json::as_array) {
        return constraints
            .iter()
            .map(|c| {
                Ok(pb::ConstraintGroup {
                    constraints: vec![parse_constraint(c)?],
                })
            })
            .collect();
    }
    Ok(Vec::new())
}

fn str_field<'a>(obj: &'a serde_json::Map<String, Json>, field: &str) -> anyhow::Result<&'a str> {
    obj.get(field)
        .and_then(Json::as_str)
        .with_context(|| format!("missing string field `{field}`"))
}

fn parse_json(s: &str) -> Json {
    serde_json::from_str(s).unwrap_or_else(|_| Json::String(s.to_owned()))
}

fn print(value: Json) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn flag_to_json(flag: &pb::Flag) -> Json {
    json!({
        "key": flag.key,
        "value_type": pb::ValueType::try_from(flag.value_type)
            .unwrap_or_default()
            .as_str_name(),
        "enabled": flag.enabled,
        "default_variant_key": flag.default_variant_key,
        "archived": flag.archived,
        "variants": flag.variants.iter().map(|v| json!({
            "key": v.key,
            "value": v.value.as_ref().map(value_to_json).unwrap_or(Json::Null),
        })).collect::<Vec<_>>(),
        "rules": flag.rules.iter().map(|r| json!({
            "rank": r.rank,
            "segment_key": r.segment_key,
            "variant_key": r.variant_key,
            "bucket_salt": r.bucket_salt,
            "distributions": r.distributions.iter().map(|d| json!({
                "variant_key": d.variant_key,
                "weight": d.weight,
            })).collect::<Vec<_>>(),
            "constraint_groups": r.constraint_groups.iter().map(|g| {
                g.constraints.iter().map(|c| json!({
                    "attribute": c.attribute,
                    "operator": pb::ConstraintOperator::try_from(c.operator)
                        .unwrap_or_default()
                        .as_str_name(),
                    "values": c.values.iter().map(value_to_json).collect::<Vec<_>>(),
                })).collect::<Vec<_>>()
            }).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

fn segment_to_json(segment: &pb::Segment) -> Json {
    json!({
        "key": segment.key,
        "name": segment.name,
        "constraints": segment.constraints.iter().map(|c| json!({
            "attribute": c.attribute,
            "operator": pb::ConstraintOperator::try_from(c.operator)
                .unwrap_or_default()
                .as_str_name(),
            "values": c.values.iter().map(value_to_json).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

fn value_to_json(v: &prost_types::Value) -> Json {
    match &v.kind {
        Some(Kind::NullValue(_)) | None => Json::Null,
        Some(Kind::BoolValue(b)) => Json::Bool(*b),
        Some(Kind::NumberValue(n)) => json!(n),
        Some(Kind::StringValue(s)) => Json::String(s.clone()),
        Some(Kind::ListValue(l)) => Json::Array(l.values.iter().map(value_to_json).collect()),
        Some(Kind::StructValue(s)) => Json::Object(
            s.fields
                .iter()
                .map(|(k, v)| (k.clone(), value_to_json(v)))
                .collect(),
        ),
    }
}

fn json_to_value(j: &Json) -> prost_types::Value {
    let kind = match j {
        Json::Null => Kind::NullValue(0),
        Json::Bool(b) => Kind::BoolValue(*b),
        Json::Number(n) => Kind::NumberValue(n.as_f64().unwrap_or(0.0)),
        Json::String(s) => Kind::StringValue(s.clone()),
        Json::Array(a) => Kind::ListValue(prost_types::ListValue {
            values: a.iter().map(json_to_value).collect(),
        }),
        Json::Object(o) => Kind::StructValue(prost_types::Struct {
            fields: o
                .iter()
                .map(|(k, v)| (k.clone(), json_to_value(v)))
                .collect(),
        }),
    };
    prost_types::Value { kind: Some(kind) }
}
