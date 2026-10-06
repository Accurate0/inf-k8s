use crate::flag_config::{ConstraintDoc, DistDoc, FlagDoc, RuleDoc, SegmentDoc};
use anyhow::{Context as _, bail};
use feature_flag_proto as pb;
use prost_types::value::Kind;
use serde_json::{Map, Value as Json, json};

pub struct Convert;

impl Convert {
    pub fn flag_doc_to_pb(doc: &FlagDoc) -> anyhow::Result<pb::Flag> {
        let value_type = Self::value_type_from_name(&doc.value_type)?;

        let variants = doc
            .variants
            .iter()
            .map(|(key, value)| pb::Variant {
                key: key.clone(),
                value: Some(Self::json_to_value(value)),
            })
            .collect();

        let rules = doc
            .rules
            .iter()
            .enumerate()
            .map(|(rank, rule)| Self::rule_doc_to_pb(rank as u32, rule))
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

    pub fn rule_doc_to_pb(rank: u32, rule: &RuleDoc) -> anyhow::Result<pb::Rule> {
        let constraint_groups = if rule.constraint_groups.is_empty() {
            rule.constraints
                .iter()
                .map(|constraint| {
                    Ok(pb::ConstraintGroup {
                        constraints: vec![Self::constraint_doc_to_pb(constraint)?],
                    })
                })
                .collect::<anyhow::Result<_>>()?
        } else {
            rule.constraint_groups
                .iter()
                .map(|group| {
                    Ok(pb::ConstraintGroup {
                        constraints: group
                            .iter()
                            .map(Self::constraint_doc_to_pb)
                            .collect::<anyhow::Result<_>>()?,
                    })
                })
                .collect::<anyhow::Result<_>>()?
        };

        Ok(pb::Rule {
            rank,
            segment_key: rule.segment.clone().unwrap_or_default(),
            variant_key: rule.variant.clone().unwrap_or_default(),
            distributions: rule
                .distributions
                .iter()
                .map(|dist| pb::Distribution {
                    variant_key: dist.variant.clone(),
                    weight: dist.weight,
                })
                .collect(),
            constraint_groups,
            bucket_salt: rule.bucket_salt.clone(),
        })
    }

    pub fn constraint_doc_to_pb(constraint: &ConstraintDoc) -> anyhow::Result<pb::Constraint> {
        Ok(pb::Constraint {
            attribute: constraint.attribute.clone(),
            operator: Self::operator_from_name(&constraint.operator)? as i32,
            values: constraint.values.iter().map(Self::json_to_value).collect(),
        })
    }

    pub fn segment_doc_to_pb(doc: &SegmentDoc) -> anyhow::Result<pb::Segment> {
        Ok(pb::Segment {
            key: doc.key.clone(),
            name: doc.name.clone(),
            constraints: doc
                .constraints
                .iter()
                .map(Self::constraint_doc_to_pb)
                .collect::<anyhow::Result<_>>()?,
        })
    }

    pub fn flag_to_doc(flag: &pb::Flag) -> FlagDoc {
        FlagDoc {
            key: flag.key.clone(),
            value_type: Self::value_type_name(flag.value_type).to_owned(),
            enabled: flag.enabled,
            default: flag.default_variant_key.clone(),
            variants: flag
                .variants
                .iter()
                .map(|variant| {
                    (
                        variant.key.clone(),
                        variant
                            .value
                            .as_ref()
                            .map(Self::value_to_json)
                            .unwrap_or(Json::Null),
                    )
                })
                .collect(),
            rules: flag.rules.iter().map(Self::rule_to_doc).collect(),
        }
    }

    pub fn rule_to_doc(rule: &pb::Rule) -> RuleDoc {
        let flat = rule
            .constraint_groups
            .iter()
            .all(|group| group.constraints.len() == 1);

        let (constraints, constraint_groups) = if flat {
            (
                rule.constraint_groups
                    .iter()
                    .map(|group| Self::constraint_to_doc(&group.constraints[0]))
                    .collect(),
                Vec::new(),
            )
        } else {
            (
                Vec::new(),
                rule.constraint_groups
                    .iter()
                    .map(|group| {
                        group
                            .constraints
                            .iter()
                            .map(Self::constraint_to_doc)
                            .collect()
                    })
                    .collect(),
            )
        };

        RuleDoc {
            segment: (!rule.segment_key.is_empty()).then(|| rule.segment_key.clone()),
            variant: (!rule.variant_key.is_empty()).then(|| rule.variant_key.clone()),
            distributions: rule
                .distributions
                .iter()
                .map(|dist| DistDoc {
                    variant: dist.variant_key.clone(),
                    weight: dist.weight,
                })
                .collect(),
            constraints,
            constraint_groups,
            bucket_salt: rule.bucket_salt.clone(),
        }
    }

    pub fn segment_to_doc(segment: &pb::Segment) -> SegmentDoc {
        SegmentDoc {
            key: segment.key.clone(),
            name: segment.name.clone(),
            constraints: segment
                .constraints
                .iter()
                .map(Self::constraint_to_doc)
                .collect(),
        }
    }

    pub fn constraint_to_doc(constraint: &pb::Constraint) -> ConstraintDoc {
        ConstraintDoc {
            attribute: constraint.attribute.clone(),
            operator: Self::operator_name(constraint.operator),
            values: constraint.values.iter().map(Self::value_to_json).collect(),
        }
    }

    pub fn flag_yaml(flag: &pb::Flag) -> String {
        serde_yaml::to_string(&Self::flag_to_doc(flag)).unwrap_or_default()
    }

    pub fn segment_yaml(segment: &pb::Segment) -> String {
        serde_yaml::to_string(&Self::segment_to_doc(segment)).unwrap_or_default()
    }

    pub fn flag_from_yaml(text: &str) -> anyhow::Result<pb::Flag> {
        let doc: FlagDoc = serde_yaml::from_str(text).context("parsing flag yaml")?;

        Self::flag_doc_to_pb(&doc)
    }

    pub fn segment_from_yaml(text: &str) -> anyhow::Result<pb::Segment> {
        let doc: SegmentDoc = serde_yaml::from_str(text).context("parsing segment yaml")?;

        Self::segment_doc_to_pb(&doc)
    }

    pub fn value_type_from_name(name: &str) -> anyhow::Result<pb::ValueType> {
        Ok(match name {
            "boolean" => pb::ValueType::Boolean,
            "string" => pb::ValueType::String,
            "integer" => pb::ValueType::Integer,
            "float" => pb::ValueType::Float,
            "object" => pb::ValueType::Object,
            other => bail!("unknown flag type `{other}`"),
        })
    }

    pub fn value_type_name(value_type: i32) -> &'static str {
        match pb::ValueType::try_from(value_type).unwrap_or_default() {
            pb::ValueType::Boolean => "boolean",
            pb::ValueType::String => "string",
            pb::ValueType::Integer => "integer",
            pb::ValueType::Float => "float",
            pb::ValueType::Object => "object",
            pb::ValueType::Unspecified => "unspecified",
        }
    }

    pub fn operator_from_name(name: &str) -> anyhow::Result<pb::ConstraintOperator> {
        let upper = name.to_uppercase();
        let short = upper.trim_start_matches("CONSTRAINT_OPERATOR_");
        let full = format!("CONSTRAINT_OPERATOR_{short}");

        pb::ConstraintOperator::from_str_name(&full)
            .with_context(|| format!("unknown operator `{name}`"))
    }

    pub fn operator_name(operator: i32) -> String {
        pb::ConstraintOperator::try_from(operator)
            .unwrap_or_default()
            .as_str_name()
            .trim_start_matches("CONSTRAINT_OPERATOR_")
            .to_lowercase()
    }

    pub fn reason_name(reason: i32) -> String {
        pb::Reason::try_from(reason)
            .unwrap_or_default()
            .as_str_name()
            .trim_start_matches("REASON_")
            .to_lowercase()
    }

    pub fn parse_json(text: &str) -> Json {
        serde_json::from_str(text).unwrap_or_else(|_| Json::String(text.to_owned()))
    }

    pub fn parse_variant(spec: &str) -> anyhow::Result<pb::Variant> {
        let (key, value) = spec
            .split_once('=')
            .with_context(|| format!("variant must be key=value, got `{spec}`"))?;

        Ok(pb::Variant {
            key: key.to_owned(),
            value: Some(Self::json_to_value(&Self::parse_json(value))),
        })
    }

    pub fn parse_segment(json: &Json) -> anyhow::Result<pb::Segment> {
        let obj = json.as_object().context("segment must be a JSON object")?;

        let constraints = obj
            .get("constraints")
            .and_then(Json::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(Self::parse_constraint)
                    .collect::<anyhow::Result<_>>()
            })
            .transpose()?
            .unwrap_or_default();

        Ok(pb::Segment {
            key: Self::str_field(obj, "key")?.to_owned(),
            name: obj
                .get("name")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_owned(),
            constraints,
        })
    }

    pub fn parse_constraint(json: &Json) -> anyhow::Result<pb::Constraint> {
        let obj = json
            .as_object()
            .context("constraint must be a JSON object")?;

        let operator = Self::operator_from_name(Self::str_field(obj, "operator")?)?;

        let values = obj
            .get("values")
            .and_then(Json::as_array)
            .map(|values| values.iter().map(Self::json_to_value).collect())
            .unwrap_or_default();

        Ok(pb::Constraint {
            attribute: Self::str_field(obj, "attribute")?.to_owned(),
            operator: operator as i32,
            values,
        })
    }

    pub fn parse_rule(json: &Json) -> anyhow::Result<pb::Rule> {
        let obj = json.as_object().context("rule must be a JSON object")?;

        let distributions = obj
            .get("distributions")
            .and_then(Json::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        let dist = item
                            .as_object()
                            .context("distribution must be a JSON object")?;

                        Ok(pb::Distribution {
                            variant_key: Self::str_field(dist, "variant_key")?.to_owned(),
                            weight: dist.get("weight").and_then(Json::as_u64).unwrap_or(0) as u32,
                        })
                    })
                    .collect::<anyhow::Result<_>>()
            })
            .transpose()?
            .unwrap_or_default();

        let text = |field: &str| {
            obj.get(field)
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_owned()
        };

        Ok(pb::Rule {
            rank: 0,
            segment_key: text("segment_key"),
            variant_key: text("variant_key"),
            distributions,
            constraint_groups: Self::parse_constraint_groups(obj)?,
            bucket_salt: text("bucket_salt"),
        })
    }

    fn parse_constraint_groups(
        obj: &Map<String, Json>,
    ) -> anyhow::Result<Vec<pb::ConstraintGroup>> {
        if let Some(groups) = obj.get("constraint_groups").and_then(Json::as_array) {
            return groups
                .iter()
                .map(|group| {
                    let constraints = group
                        .as_array()
                        .context("constraint group must be a JSON array")?
                        .iter()
                        .map(Self::parse_constraint)
                        .collect::<anyhow::Result<_>>()?;

                    Ok(pb::ConstraintGroup { constraints })
                })
                .collect();
        }

        let Some(constraints) = obj.get("constraints").and_then(Json::as_array) else {
            return Ok(Vec::new());
        };

        constraints
            .iter()
            .map(|constraint| {
                Ok(pb::ConstraintGroup {
                    constraints: vec![Self::parse_constraint(constraint)?],
                })
            })
            .collect()
    }

    fn str_field<'a>(obj: &'a Map<String, Json>, field: &str) -> anyhow::Result<&'a str> {
        obj.get(field)
            .and_then(Json::as_str)
            .with_context(|| format!("missing string field `{field}`"))
    }

    pub fn flag_to_json(flag: &pb::Flag) -> Json {
        json!({
            "key": flag.key,
            "value_type": pb::ValueType::try_from(flag.value_type)
                .unwrap_or_default()
                .as_str_name(),
            "enabled": flag.enabled,
            "default_variant_key": flag.default_variant_key,
            "archived": flag.archived,
            "variants": flag.variants.iter().map(|variant| json!({
                "key": variant.key,
                "value": variant.value.as_ref().map(Self::value_to_json).unwrap_or(Json::Null),
            })).collect::<Vec<_>>(),
            "rules": flag.rules.iter().map(|rule| json!({
                "rank": rule.rank,
                "segment_key": rule.segment_key,
                "variant_key": rule.variant_key,
                "bucket_salt": rule.bucket_salt,
                "distributions": rule.distributions.iter().map(|dist| json!({
                    "variant_key": dist.variant_key,
                    "weight": dist.weight,
                })).collect::<Vec<_>>(),
                "constraint_groups": rule.constraint_groups.iter().map(|group| {
                    group.constraints.iter().map(Self::constraint_to_json).collect::<Vec<_>>()
                }).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })
    }

    pub fn segment_to_json(segment: &pb::Segment) -> Json {
        json!({
            "key": segment.key,
            "name": segment.name,
            "constraints": segment
                .constraints
                .iter()
                .map(Self::constraint_to_json)
                .collect::<Vec<_>>(),
        })
    }

    fn constraint_to_json(constraint: &pb::Constraint) -> Json {
        json!({
            "attribute": constraint.attribute,
            "operator": pb::ConstraintOperator::try_from(constraint.operator)
                .unwrap_or_default()
                .as_str_name(),
            "values": constraint.values.iter().map(Self::value_to_json).collect::<Vec<_>>(),
        })
    }

    pub fn change_to_json(change: &pb::FlagChange) -> Json {
        json!({
            "id": change.id,
            "version": change.version,
            "actor": change.actor,
            "action": change.action,
            "target_kind": change.target_kind,
            "target_key": change.target_key,
            "detail": Self::parse_json(&change.detail),
            "created_at": change.created_at,
        })
    }

    pub fn evaluated_to_json(flag: &pb::EvaluatedFlag) -> Json {
        let meta = flag.meta.clone().unwrap_or_default();

        json!({
            "flag_key": flag.flag_key,
            "value_type": Self::value_type_name(flag.value_type),
            "value": flag.value.as_ref().map(Self::value_to_json).unwrap_or(Json::Null),
            "variant": meta.variant,
            "reason": Self::reason_name(meta.reason),
            "error_code": meta.error_code,
            "error_message": meta.error_message,
        })
    }

    pub fn value_to_json(value: &prost_types::Value) -> Json {
        match &value.kind {
            Some(Kind::NullValue(_)) | None => Json::Null,
            Some(Kind::BoolValue(b)) => Json::Bool(*b),
            Some(Kind::NumberValue(n)) => json!(n),
            Some(Kind::StringValue(s)) => Json::String(s.clone()),
            Some(Kind::ListValue(list)) => {
                Json::Array(list.values.iter().map(Self::value_to_json).collect())
            }
            Some(Kind::StructValue(fields)) => Self::struct_to_json(fields),
        }
    }

    pub fn struct_to_json(fields: &prost_types::Struct) -> Json {
        Json::Object(
            fields
                .fields
                .iter()
                .map(|(key, value)| (key.clone(), Self::value_to_json(value)))
                .collect(),
        )
    }

    pub fn json_to_value(json: &Json) -> prost_types::Value {
        let kind = match json {
            Json::Null => Kind::NullValue(0),
            Json::Bool(b) => Kind::BoolValue(*b),
            Json::Number(n) => Kind::NumberValue(n.as_f64().unwrap_or(0.0)),
            Json::String(s) => Kind::StringValue(s.clone()),
            Json::Array(items) => Kind::ListValue(prost_types::ListValue {
                values: items.iter().map(Self::json_to_value).collect(),
            }),
            Json::Object(fields) => Kind::StructValue(Self::json_to_struct(fields)),
        };

        prost_types::Value { kind: Some(kind) }
    }

    pub fn json_to_struct(fields: &Map<String, Json>) -> prost_types::Struct {
        prost_types::Struct {
            fields: fields
                .iter()
                .map(|(key, value)| (key.clone(), Self::json_to_value(value)))
                .collect(),
        }
    }
}
