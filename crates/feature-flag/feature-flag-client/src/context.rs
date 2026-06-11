use feature_flag_proto::EvaluationContext;
use prost_types::value::Kind;
use prost_types::{Struct, Value};
use std::collections::BTreeMap;

/// Builder for an [`EvaluationContext`]: a targeting key plus typed attributes that
/// the backend matches segment rules against.
#[derive(Clone, Debug, Default)]
pub struct Context {
    targeting_key: String,
    attributes: BTreeMap<String, Value>,
}

impl Context {
    pub fn new(targeting_key: impl Into<String>) -> Self {
        Self {
            targeting_key: targeting_key.into(),
            attributes: BTreeMap::new(),
        }
    }

    pub fn string(self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attr(key, Kind::StringValue(value.into()))
    }

    pub fn bool(self, key: impl Into<String>, value: bool) -> Self {
        self.attr(key, Kind::BoolValue(value))
    }

    pub fn number(self, key: impl Into<String>, value: f64) -> Self {
        self.attr(key, Kind::NumberValue(value))
    }

    /// Set an attribute from any JSON value (arrays/objects supported).
    pub fn json(self, key: impl Into<String>, value: &serde_json::Value) -> Self {
        self.attr_value(key, json_to_value(value))
    }

    fn attr(self, key: impl Into<String>, kind: Kind) -> Self {
        self.attr_value(key, Value { kind: Some(kind) })
    }

    fn attr_value(mut self, key: impl Into<String>, value: Value) -> Self {
        self.attributes.insert(key.into(), value);
        self
    }
}

impl From<Context> for EvaluationContext {
    fn from(ctx: Context) -> Self {
        EvaluationContext {
            targeting_key: ctx.targeting_key,
            attributes: Some(Struct {
                fields: ctx.attributes.into_iter().collect(),
            }),
        }
    }
}

fn json_to_value(j: &serde_json::Value) -> Value {
    let kind = match j {
        serde_json::Value::Null => Kind::NullValue(0),
        serde_json::Value::Bool(b) => Kind::BoolValue(*b),
        serde_json::Value::Number(n) => Kind::NumberValue(n.as_f64().unwrap_or(0.0)),
        serde_json::Value::String(s) => Kind::StringValue(s.clone()),
        serde_json::Value::Array(a) => Kind::ListValue(prost_types::ListValue {
            values: a.iter().map(json_to_value).collect(),
        }),
        serde_json::Value::Object(o) => Kind::StructValue(Struct {
            fields: o.iter().map(|(k, v)| (k.clone(), json_to_value(v))).collect(),
        }),
    };
    Value { kind: Some(kind) }
}
