import { ChannelCredentials } from "@grpc/grpc-js";
import {
  StandardResolutionReasons,
  type EvaluationContext,
  type JsonValue,
  type Provider,
  type ResolutionDetails,
} from "@openfeature/server-sdk";
import { FeatureFlagClient } from "./index.js";
import { Reason, ValueType } from "./model.js";

function reasonToOpenFeature(reason: Reason | undefined): string {
  switch (reason) {
    case Reason.REASON_STATIC:
      return StandardResolutionReasons.STATIC;
    case Reason.REASON_DEFAULT:
      return StandardResolutionReasons.DEFAULT;
    case Reason.REASON_TARGETING_MATCH:
      return StandardResolutionReasons.TARGETING_MATCH;
    case Reason.REASON_SPLIT:
      return StandardResolutionReasons.SPLIT;
    case Reason.REASON_DISABLED:
      return StandardResolutionReasons.DISABLED;
    case Reason.REASON_ERROR:
      return StandardResolutionReasons.ERROR;
    default:
      return StandardResolutionReasons.UNKNOWN;
  }
}

/**
 * OpenFeature server provider backed by the feature-flags gRPC service. Each evaluation is
 * a single remote RPC against the typed `resolve` endpoint (no snapshot streaming), with the
 * supplied default returned on any transport or resolution error.
 */
export class FeatureFlagProvider implements Provider {
  readonly runsOn = "server";
  readonly metadata = { name: "feature-flags" } as const;
  readonly hooks = [];

  private readonly client: FeatureFlagClient;

  constructor(
    address: string,
    clientId: string,
    credentials: ChannelCredentials = ChannelCredentials.createInsecure(),
  ) {
    this.client = new FeatureFlagClient(address, clientId, credentials);
  }

  private async resolve<T>(
    flagKey: string,
    valueType: ValueType,
    defaultValue: T,
    context: EvaluationContext,
  ): Promise<ResolutionDetails<T>> {
    const { targetingKey, ...attributes } = context;
    try {
      const result = await this.client.resolve(
        flagKey,
        valueType,
        targetingKey ?? "",
        attributes,
      );

      if (result.value === undefined || result.value === null) {
        return { value: defaultValue, reason: StandardResolutionReasons.DEFAULT };
      }

      return { value: result.value as T, reason: reasonToOpenFeature(result.meta?.reason) };
    } catch (error) {
      return {
        value: defaultValue,
        reason: StandardResolutionReasons.ERROR,
        errorMessage: error instanceof Error ? error.message : String(error),
      };
    }
  }

  resolveBooleanEvaluation(
    flagKey: string,
    defaultValue: boolean,
    context: EvaluationContext,
  ): Promise<ResolutionDetails<boolean>> {
    return this.resolve(flagKey, ValueType.VALUE_TYPE_BOOLEAN, defaultValue, context);
  }

  resolveStringEvaluation(
    flagKey: string,
    defaultValue: string,
    context: EvaluationContext,
  ): Promise<ResolutionDetails<string>> {
    return this.resolve(flagKey, ValueType.VALUE_TYPE_STRING, defaultValue, context);
  }

  resolveNumberEvaluation(
    flagKey: string,
    defaultValue: number,
    context: EvaluationContext,
  ): Promise<ResolutionDetails<number>> {
    return this.resolve(flagKey, ValueType.VALUE_TYPE_FLOAT, defaultValue, context);
  }

  resolveObjectEvaluation<T extends JsonValue>(
    flagKey: string,
    defaultValue: T,
    context: EvaluationContext,
  ): Promise<ResolutionDetails<T>> {
    return this.resolve(flagKey, ValueType.VALUE_TYPE_OBJECT, defaultValue, context);
  }
}
