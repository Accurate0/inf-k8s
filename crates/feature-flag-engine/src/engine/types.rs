use serde_json::Value;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Static,
    Default,
    TargetingMatch,
    Split,
    Disabled,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    FlagNotFound,
    ParseError,
    General,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::FlagNotFound => "FLAG_NOT_FOUND",
            ErrorCode::ParseError => "PARSE_ERROR",
            ErrorCode::General => "GENERAL",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Resolution {
    pub value: Value,
    pub variant: String,
    pub reason: Reason,
}

#[derive(Clone, Debug, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct EvalError {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Clone, Debug, Default)]
pub struct EvalContext {
    pub targeting_key: String,
    pub attributes: HashMap<String, Value>,
}
