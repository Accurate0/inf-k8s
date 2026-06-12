//! Conversions between the domain model and the [`feature_flag_proto`] wire types,
//! expressed as `From`/`TryFrom` impls. The `serde_json` <-> `prost_types::Value`
//! bridging stays as free functions since both sides are foreign types (the orphan
//! rule forbids trait impls), as do the error-metadata constructors.

mod types;

pub use types::ConversionError;

use crate::engine::{EvalContext, Resolution};
use crate::model::{
    Constraint, ConstraintGroup, Distribution, Flag, Rule, Segment, Snapshot, Variant,
};
use feature_flag_proto as pb;
use prost_types::value::Kind;
use serde_json::Value as Json;

pub fn prost_value_to_json(v: &prost_types::Value) -> Json {
    match &v.kind {
        Some(Kind::NullValue(_)) | None => Json::Null,
        Some(Kind::BoolValue(b)) => Json::Bool(*b),
        Some(Kind::NumberValue(n)) => serde_json::Number::from_f64(*n)
            .map(Json::Number)
            .unwrap_or(Json::Null),
        Some(Kind::StringValue(s)) => Json::String(s.clone()),
        Some(Kind::ListValue(l)) => {
            Json::Array(l.values.iter().map(prost_value_to_json).collect())
        }
        Some(Kind::StructValue(s)) => Json::Object(
            s.fields
                .iter()
                .map(|(k, v)| (k.clone(), prost_value_to_json(v)))
                .collect(),
        ),
    }
}

pub fn json_to_prost_value(j: &Json) -> prost_types::Value {
    let kind = match j {
        Json::Null => Kind::NullValue(0),
        Json::Bool(b) => Kind::BoolValue(*b),
        Json::Number(n) => Kind::NumberValue(n.as_f64().unwrap_or(0.0)),
        Json::String(s) => Kind::StringValue(s.clone()),
        Json::Array(a) => Kind::ListValue(prost_types::ListValue {
            values: a.iter().map(json_to_prost_value).collect(),
        }),
        Json::Object(o) => Kind::StructValue(json_object_to_struct(o)),
    };
    prost_types::Value { kind: Some(kind) }
}

fn json_object_to_struct(o: &serde_json::Map<String, Json>) -> prost_types::Struct {
    prost_types::Struct {
        fields: o
            .iter()
            .map(|(k, v)| (k.clone(), json_to_prost_value(v)))
            .collect(),
    }
}

pub fn json_to_struct(j: &Json) -> prost_types::Struct {
    match j {
        Json::Object(o) => json_object_to_struct(o),
        _ => prost_types::Struct::default(),
    }
}

impl From<pb::EvaluationContext> for EvalContext {
    fn from(ctx: pb::EvaluationContext) -> Self {
        let attributes = ctx
            .attributes
            .map(|s| {
                s.fields
                    .iter()
                    .map(|(k, v)| (k.clone(), prost_value_to_json(v)))
                    .collect()
            })
            .unwrap_or_default();
        EvalContext {
            targeting_key: ctx.targeting_key,
            attributes,
        }
    }
}

/// Success metadata for a resolution. Error metadata is built via [`meta_err`] /
/// [`type_mismatch`], which carry codes a [`Resolution`] doesn't.
impl From<&Resolution> for pb::ResolutionMeta {
    fn from(res: &Resolution) -> Self {
        pb::ResolutionMeta {
            variant: res.variant.clone(),
            reason: pb::Reason::from(res.reason) as i32,
            error_code: String::new(),
            error_message: String::new(),
        }
    }
}

pub fn meta_err(code: &str, message: String) -> pb::ResolutionMeta {
    pb::ResolutionMeta {
        variant: String::new(),
        reason: pb::Reason::Error as i32,
        error_code: code.to_string(),
        error_message: message,
    }
}

pub fn type_mismatch(expected: &str) -> pb::ResolutionMeta {
    meta_err("TYPE_MISMATCH", format!("flag is not of type {expected}"))
}

impl From<&Flag> for pb::Flag {
    fn from(flag: &Flag) -> Self {
        pb::Flag {
            key: flag.key.clone(),
            value_type: pb::ValueType::from(flag.value_type) as i32,
            enabled: flag.enabled,
            default_variant_key: flag.default_variant_key.clone(),
            archived: flag.archived,
            variants: flag.variants.iter().map(pb::Variant::from).collect(),
            rules: flag.rules.iter().map(pb::Rule::from).collect(),
        }
    }
}

impl From<&Variant> for pb::Variant {
    fn from(v: &Variant) -> Self {
        pb::Variant {
            key: v.key.clone(),
            value: Some(json_to_prost_value(&v.value)),
        }
    }
}

fn domain_constraint_to_pb(c: &Constraint) -> pb::Constraint {
    pb::Constraint {
        attribute: c.attribute.clone(),
        operator: pb::ConstraintOperator::from(c.operator) as i32,
        values: c.values.iter().map(json_to_prost_value).collect(),
    }
}

impl From<&Rule> for pb::Rule {
    fn from(r: &Rule) -> Self {
        pb::Rule {
            rank: r.rank,
            segment_key: r.segment_key.clone().unwrap_or_default(),
            variant_key: r.variant_key.clone().unwrap_or_default(),
            distributions: r
                .distributions
                .iter()
                .map(|d| pb::Distribution {
                    variant_key: d.variant_key.clone(),
                    weight: d.weight,
                })
                .collect(),
            constraint_groups: r
                .constraint_groups
                .iter()
                .map(|g| pb::ConstraintGroup {
                    constraints: g.constraints.iter().map(domain_constraint_to_pb).collect(),
                })
                .collect(),
            bucket_salt: r.bucket_salt.clone(),
        }
    }
}

impl From<&Segment> for pb::Segment {
    fn from(s: &Segment) -> Self {
        pb::Segment {
            key: s.key.clone(),
            name: s.name.clone(),
            constraints: s.constraints.iter().map(domain_constraint_to_pb).collect(),
        }
    }
}

impl From<&pb::Variant> for Variant {
    fn from(v: &pb::Variant) -> Self {
        Variant {
            key: v.key.clone(),
            value: v.value.as_ref().map(prost_value_to_json).unwrap_or(Json::Null),
        }
    }
}

/// Convert a list of proto constraints to the domain model, failing if any operator
/// is unspecified. Shared by segment and inline rule constraint conversion.
fn pb_constraints_to_domain(constraints: &[pb::Constraint]) -> Result<Vec<Constraint>, ConversionError> {
    let mut out = Vec::with_capacity(constraints.len());
    for c in constraints {
        out.push(Constraint {
            attribute: c.attribute.clone(),
            operator: c.operator().try_into()?,
            values: c.values.iter().map(prost_value_to_json).collect(),
        });
    }
    Ok(out)
}

impl TryFrom<&pb::Rule> for Rule {
    type Error = ConversionError;

    fn try_from(r: &pb::Rule) -> Result<Self, Self::Error> {
        Ok(Rule {
            rank: r.rank,
            segment_key: (!r.segment_key.is_empty()).then(|| r.segment_key.clone()),
            variant_key: (!r.variant_key.is_empty()).then(|| r.variant_key.clone()),
            distributions: r
                .distributions
                .iter()
                .map(|d| Distribution {
                    variant_key: d.variant_key.clone(),
                    weight: d.weight,
                })
                .collect(),
            constraint_groups: r
                .constraint_groups
                .iter()
                .map(|g| {
                    Ok(ConstraintGroup {
                        constraints: pb_constraints_to_domain(&g.constraints)?,
                    })
                })
                .collect::<Result<_, ConversionError>>()?,
            bucket_salt: r.bucket_salt.clone(),
        })
    }
}

impl TryFrom<&pb::Segment> for Segment {
    type Error = ConversionError;

    fn try_from(s: &pb::Segment) -> Result<Self, Self::Error> {
        Ok(Segment {
            key: s.key.clone(),
            name: s.name.clone(),
            constraints: pb_constraints_to_domain(&s.constraints)?,
        })
    }
}

impl TryFrom<&pb::Flag> for Flag {
    type Error = ConversionError;

    fn try_from(f: &pb::Flag) -> Result<Self, Self::Error> {
        Ok(Flag {
            key: f.key.clone(),
            value_type: f.value_type().try_into()?,
            enabled: f.enabled,
            default_variant_key: f.default_variant_key.clone(),
            archived: f.archived,
            variants: f.variants.iter().map(Variant::from).collect(),
            rules: f
                .rules
                .iter()
                .map(Rule::try_from)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl TryFrom<pb::SnapshotResponse> for Snapshot {
    type Error = ConversionError;

    fn try_from(resp: pb::SnapshotResponse) -> Result<Self, Self::Error> {
        let mut snapshot = Snapshot {
            version: resp.version,
            ..Default::default()
        };
        for f in &resp.flags {
            let flag = Flag::try_from(f)?;
            snapshot.flags.insert(flag.key.clone(), flag);
        }
        for s in &resp.segments {
            let segment = Segment::try_from(s)?;
            snapshot.segments.insert(segment.key.clone(), segment);
        }
        Ok(snapshot)
    }
}
