use crate::engine::Reason;
use crate::model::{Operator, ValueType};
use feature_flag_proto as pb;
use feature_flag_proto::ConstraintOperator;

/// Error from a fallible proto-to-domain conversion, carrying a human-readable reason.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct ConversionError(pub String);

impl From<ValueType> for pb::ValueType {
    fn from(t: ValueType) -> Self {
        match t {
            ValueType::Boolean => pb::ValueType::Boolean,
            ValueType::String => pb::ValueType::String,
            ValueType::Integer => pb::ValueType::Integer,
            ValueType::Float => pb::ValueType::Float,
            ValueType::Object => pb::ValueType::Object,
        }
    }
}

impl TryFrom<pb::ValueType> for ValueType {
    type Error = ConversionError;

    fn try_from(t: pb::ValueType) -> Result<Self, Self::Error> {
        Ok(match t {
            pb::ValueType::Unspecified => {
                return Err(ConversionError("value_type is unspecified".into()));
            }
            pb::ValueType::Boolean => ValueType::Boolean,
            pb::ValueType::String => ValueType::String,
            pb::ValueType::Integer => ValueType::Integer,
            pb::ValueType::Float => ValueType::Float,
            pb::ValueType::Object => ValueType::Object,
        })
    }
}

impl From<Reason> for pb::Reason {
    fn from(r: Reason) -> Self {
        match r {
            Reason::Static => pb::Reason::Static,
            Reason::Default => pb::Reason::Default,
            Reason::TargetingMatch => pb::Reason::TargetingMatch,
            Reason::Split => pb::Reason::Split,
            Reason::Disabled => pb::Reason::Disabled,
            Reason::Error => pb::Reason::Error,
        }
    }
}

impl From<Operator> for ConstraintOperator {
    fn from(op: Operator) -> Self {
        match op {
            Operator::Eq => ConstraintOperator::Eq,
            Operator::Neq => ConstraintOperator::Neq,
            Operator::In => ConstraintOperator::In,
            Operator::NotIn => ConstraintOperator::NotIn,
            Operator::Contains => ConstraintOperator::Contains,
            Operator::StartsWith => ConstraintOperator::StartsWith,
            Operator::EndsWith => ConstraintOperator::EndsWith,
            Operator::Gt => ConstraintOperator::Gt,
            Operator::Gte => ConstraintOperator::Gte,
            Operator::Lt => ConstraintOperator::Lt,
            Operator::Lte => ConstraintOperator::Lte,
            Operator::Exists => ConstraintOperator::Exists,
            Operator::Regex => ConstraintOperator::Regex,
        }
    }
}

impl TryFrom<ConstraintOperator> for Operator {
    type Error = ConversionError;

    fn try_from(op: ConstraintOperator) -> Result<Self, Self::Error> {
        Ok(match op {
            ConstraintOperator::Unspecified => {
                return Err(ConversionError("constraint operator is unspecified".into()));
            }
            ConstraintOperator::Eq => Operator::Eq,
            ConstraintOperator::Neq => Operator::Neq,
            ConstraintOperator::In => Operator::In,
            ConstraintOperator::NotIn => Operator::NotIn,
            ConstraintOperator::Contains => Operator::Contains,
            ConstraintOperator::StartsWith => Operator::StartsWith,
            ConstraintOperator::EndsWith => Operator::EndsWith,
            ConstraintOperator::Gt => Operator::Gt,
            ConstraintOperator::Gte => Operator::Gte,
            ConstraintOperator::Lt => Operator::Lt,
            ConstraintOperator::Lte => Operator::Lte,
            ConstraintOperator::Exists => Operator::Exists,
            ConstraintOperator::Regex => Operator::Regex,
        })
    }
}
