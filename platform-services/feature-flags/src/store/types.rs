use crate::error::{AppError, AppResult};
use crate::model::{Operator, ValueType};
use serde_json::Value as Json;

pub(super) fn unique_violation(message: String) -> impl FnOnce(sqlx::Error) -> AppError {
    move |e| match &e {
        sqlx::Error::Database(db) if db.is_unique_violation() => AppError::Invalid(message),
        _ => AppError::Sqlx(e),
    }
}

pub(super) fn json_array(value: Json) -> Vec<Json> {
    match value {
        Json::Array(a) => a,
        other => vec![other],
    }
}

pub(super) fn value_type_to_str(t: ValueType) -> &'static str {
    match t {
        ValueType::Boolean => "boolean",
        ValueType::String => "string",
        ValueType::Integer => "integer",
        ValueType::Float => "float",
        ValueType::Object => "object",
    }
}

pub(super) fn value_type_from_str(s: &str) -> AppResult<ValueType> {
    Ok(match s {
        "boolean" => ValueType::Boolean,
        "string" => ValueType::String,
        "integer" => ValueType::Integer,
        "float" => ValueType::Float,
        "object" => ValueType::Object,
        other => return Err(AppError::Invalid(format!("unknown value_type `{other}`"))),
    })
}

pub(super) fn operator_to_str(op: Operator) -> &'static str {
    match op {
        Operator::Eq => "eq",
        Operator::Neq => "neq",
        Operator::In => "in",
        Operator::NotIn => "not_in",
        Operator::Contains => "contains",
        Operator::StartsWith => "starts_with",
        Operator::EndsWith => "ends_with",
        Operator::Gt => "gt",
        Operator::Gte => "gte",
        Operator::Lt => "lt",
        Operator::Lte => "lte",
        Operator::Exists => "exists",
        Operator::Regex => "regex",
        Operator::FlagMatches => "flag_matches",
    }
}

pub(super) fn operator_from_str(s: &str) -> AppResult<Operator> {
    Ok(match s {
        "eq" => Operator::Eq,
        "neq" => Operator::Neq,
        "in" => Operator::In,
        "not_in" => Operator::NotIn,
        "contains" => Operator::Contains,
        "starts_with" => Operator::StartsWith,
        "ends_with" => Operator::EndsWith,
        "gt" => Operator::Gt,
        "gte" => Operator::Gte,
        "lt" => Operator::Lt,
        "lte" => Operator::Lte,
        "exists" => Operator::Exists,
        "regex" => Operator::Regex,
        "flag_matches" => Operator::FlagMatches,
        other => return Err(AppError::Invalid(format!("unknown operator `{other}`"))),
    })
}
