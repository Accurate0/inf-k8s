import { ConstraintOperator, Reason, ValueType } from "@accurate0/feature-flag-client/model";

export const valueTypeLabels: Record<number, string> = {
  [ValueType.VALUE_TYPE_BOOLEAN]: "boolean",
  [ValueType.VALUE_TYPE_STRING]: "string",
  [ValueType.VALUE_TYPE_INTEGER]: "integer",
  [ValueType.VALUE_TYPE_FLOAT]: "float",
  [ValueType.VALUE_TYPE_OBJECT]: "object",
};

export const valueTypeOptions = [
  { value: ValueType.VALUE_TYPE_BOOLEAN, label: "boolean" },
  { value: ValueType.VALUE_TYPE_STRING, label: "string" },
  { value: ValueType.VALUE_TYPE_INTEGER, label: "integer" },
  { value: ValueType.VALUE_TYPE_FLOAT, label: "float" },
  { value: ValueType.VALUE_TYPE_OBJECT, label: "object" },
];

export const operatorLabels: Record<number, string> = {
  [ConstraintOperator.CONSTRAINT_OPERATOR_EQ]: "eq",
  [ConstraintOperator.CONSTRAINT_OPERATOR_NEQ]: "neq",
  [ConstraintOperator.CONSTRAINT_OPERATOR_IN]: "in",
  [ConstraintOperator.CONSTRAINT_OPERATOR_NOT_IN]: "not_in",
  [ConstraintOperator.CONSTRAINT_OPERATOR_CONTAINS]: "contains",
  [ConstraintOperator.CONSTRAINT_OPERATOR_STARTS_WITH]: "starts_with",
  [ConstraintOperator.CONSTRAINT_OPERATOR_ENDS_WITH]: "ends_with",
  [ConstraintOperator.CONSTRAINT_OPERATOR_GT]: "gt",
  [ConstraintOperator.CONSTRAINT_OPERATOR_GTE]: "gte",
  [ConstraintOperator.CONSTRAINT_OPERATOR_LT]: "lt",
  [ConstraintOperator.CONSTRAINT_OPERATOR_LTE]: "lte",
  [ConstraintOperator.CONSTRAINT_OPERATOR_EXISTS]: "exists",
  [ConstraintOperator.CONSTRAINT_OPERATOR_REGEX]: "regex",
};

export const operatorOptions = Object.entries(operatorLabels).map(([value, label]) => ({
  value: Number(value),
  label,
}));

export const reasonLabels: Record<number, string> = {
  [Reason.REASON_STATIC]: "STATIC",
  [Reason.REASON_DEFAULT]: "DEFAULT",
  [Reason.REASON_TARGETING_MATCH]: "TARGETING_MATCH",
  [Reason.REASON_SPLIT]: "SPLIT",
  [Reason.REASON_DISABLED]: "DISABLED",
  [Reason.REASON_ERROR]: "ERROR",
};
