import { FeatureFlagClient } from "@accurate0/feature-flag-client";
import { env } from "$env/dynamic/private";

export type { Rule, Variant, Constraint, Segment, Flag } from "@accurate0/feature-flag-client";

let instance: FeatureFlagClient | undefined;

function getClient(): FeatureFlagClient {
  if (!instance) {
    const address = env.GRPC_ADDR ?? "localhost:50051";
    instance = new FeatureFlagClient(address, "feature-flags-web");
  }
  return instance;
}

// Lazy proxy so importing this module never constructs the gRPC client (and never
// loads @grpc/grpc-js); the channel is only created on first actual use at runtime.
export const client = new Proxy({} as FeatureFlagClient, {
  get(_target, prop, receiver) {
    const target = getClient();
    const value = Reflect.get(target, prop, receiver);
    return typeof value === "function" ? value.bind(target) : value;
  },
});
