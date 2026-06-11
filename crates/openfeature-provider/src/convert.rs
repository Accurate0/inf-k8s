use feature_flag_client::Context;
use feature_flag_proto::Reason;
use open_feature::{
    EvaluationContext, EvaluationContextFieldValue, EvaluationError, EvaluationErrorCode,
    EvaluationReason, StructValue, Value,
};
use prost_types::value::Kind;

pub fn context_to_client(ctx: &EvaluationContext) -> Context {
    let mut out = Context::new(ctx.targeting_key.clone().unwrap_or_default());
    for (key, value) in &ctx.custom_fields {
        out = match value {
            EvaluationContextFieldValue::Bool(b) => out.bool(key, *b),
            EvaluationContextFieldValue::Int(i) => out.number(key, *i as f64),
            EvaluationContextFieldValue::Float(f) => out.number(key, *f),
            EvaluationContextFieldValue::String(s) => out.string(key, s),
            EvaluationContextFieldValue::DateTime(dt) => out.string(key, dt.to_string()),
            // Opaque struct fields can't be projected onto the wire context.
            EvaluationContextFieldValue::Struct(_) => out,
        };
    }
    out
}

pub fn reason_to_open_feature(reason: Reason) -> EvaluationReason {
    match reason {
        Reason::Static => EvaluationReason::Static,
        Reason::Default => EvaluationReason::Default,
        Reason::TargetingMatch => EvaluationReason::TargetingMatch,
        Reason::Split => EvaluationReason::Split,
        Reason::Disabled => EvaluationReason::Disabled,
        Reason::Error => EvaluationReason::Error,
        Reason::Unspecified => EvaluationReason::Unknown,
    }
}

pub fn error_from_code(code: &str, message: Option<String>) -> EvaluationError {
    let code = match code {
        "FLAG_NOT_FOUND" => EvaluationErrorCode::FlagNotFound,
        "TYPE_MISMATCH" => EvaluationErrorCode::TypeMismatch,
        "PARSE_ERROR" => EvaluationErrorCode::ParseError,
        "TARGETING_KEY_MISSING" => EvaluationErrorCode::TargetingKeyMissing,
        "INVALID_CONTEXT" => EvaluationErrorCode::InvalidContext,
        other => EvaluationErrorCode::General(other.to_string()),
    };
    EvaluationError { code, message }
}

pub fn struct_to_open_feature(s: prost_types::Struct) -> StructValue {
    StructValue {
        fields: s
            .fields
            .into_iter()
            .map(|(k, v)| (k, prost_value_to_open_feature(v)))
            .collect(),
    }
}

fn prost_value_to_open_feature(v: prost_types::Value) -> Value {
    match v.kind {
        Some(Kind::BoolValue(b)) => Value::Bool(b),
        Some(Kind::NumberValue(n)) => Value::Float(n),
        Some(Kind::StringValue(s)) => Value::String(s),
        Some(Kind::ListValue(l)) => {
            Value::Array(l.values.into_iter().map(prost_value_to_open_feature).collect())
        }
        Some(Kind::StructValue(s)) => Value::Struct(struct_to_open_feature(s)),
        Some(Kind::NullValue(_)) | None => Value::String(String::new()),
    }
}
