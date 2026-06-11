//! The pure feature-flag domain: the data model, the side-effect-free evaluation
//! [`Engine`], and conversions to/from the [`feature_flag_proto`] wire types. Shared
//! by the backend service (authoritative evaluation) and the client crate (optional
//! in-process local evaluation against a server-provided snapshot).

pub mod convert;
pub mod engine;
pub mod model;

pub use engine::{Engine, ErrorCode, EvalContext, EvalError, Reason, Resolution};
pub use model::{
    Constraint, Distribution, Flag, Operator, Segment, Snapshot, ValueType, Variant,
};
