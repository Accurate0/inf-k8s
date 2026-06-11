// Browser-safe entrypoint: generated message types and enums only, with no dependency on
// @grpc/grpc-js. Import this from UI code; import the package root (which pulls in the gRPC
// transport) only from server-side code.

export {
  ValueType,
  Reason,
  ConstraintOperator,
  type Flag,
  type Variant,
  type Constraint,
  type Segment,
  type Distribution,
  type Rule,
  type EvaluationContext,
} from "./gen/featureflag/v1/common.js";
export type {
  CreateFlagRequest,
  ListFlagsResponse,
  ListSegmentsResponse,
} from "./gen/featureflag/v1/admin.js";
export type {
  SnapshotResponse,
  ResolveAllResponse,
  EvaluatedFlag,
  ResolutionMeta,
} from "./gen/featureflag/v1/evaluation.js";
