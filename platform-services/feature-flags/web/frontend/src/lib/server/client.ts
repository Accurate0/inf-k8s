import { FeatureFlagClient } from "@accurate0/feature-flag-client";
import { env } from "$env/dynamic/private";

export type { Rule, Variant, Constraint, Segment, Flag } from "@accurate0/feature-flag-client";

const address = env.GRPC_ADDR ?? "localhost:50051";

export const client = new FeatureFlagClient(address, "feature-flags-web");
