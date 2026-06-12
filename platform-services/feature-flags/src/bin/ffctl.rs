//! Admin CLI for the feature-flags service. Talks to the `Admin` gRPC API; point
//! `--url` at a port-forwarded backend (`kubectl -n feature-flags port-forward
//! svc/api 50051:50051`). Results are rendered as JSON.

use anyhow::{Context as _, bail};
use clap::{Parser, Subcommand, ValueEnum};
use feature_flags::pb;
use pb::admin_client::AdminClient;
use prost_types::value::Kind;
use serde_json::{Value as Json, json};
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
