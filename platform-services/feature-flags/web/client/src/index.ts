import { ChannelCredentials, type ClientUnaryCall } from "@grpc/grpc-js";
import {
  AdminClient,
  type CreateFlagRequest,
  type ListFlagsResponse,
  type ListSegmentsResponse,
} from "./gen/featureflag/v1/admin.js";
import {
  EvaluationClient,
  type EvaluatedFlag,
  type ResolveAllResponse,
  type ResolveRequest,
  type ResolutionMeta,
  type SnapshotResponse,
} from "./gen/featureflag/v1/evaluation.js";
import {
  type Flag,
  type Rule,
  type Segment,
  ValueType,
  type Variant,
} from "./gen/featureflag/v1/common.js";

export * from "./model.js";

type UnaryMethod<Req, Res> = (
  request: Req,
  callback: (error: Error | null, response: Res) => void,
) => ClientUnaryCall;

function unary<Req, Res>(method: UnaryMethod<Req, Res>, request: Req): Promise<Res> {
  return new Promise((resolve, reject) => {
    method(request, (error, response) => {
      if (error) reject(error);
      else resolve(response);
    });
  });
}

/**
 * Server-side gRPC client for the feature-flags service. Wraps the generated callback
 * clients in promises and exposes the snapshot stream as an async iterable. Intended to
 * run in the SvelteKit Node process, never in the browser.
 */
export class FeatureFlagClient {
  private readonly admin: AdminClient;
  private readonly evaluation: EvaluationClient;

  constructor(address: string, credentials: ChannelCredentials = ChannelCredentials.createInsecure()) {
    this.admin = new AdminClient(address, credentials);
    this.evaluation = new EvaluationClient(address, credentials);
  }

  listFlags(includeArchived = false): Promise<ListFlagsResponse> {
    return unary(this.admin.listFlags.bind(this.admin), { includeArchived });
  }

  getFlag(key: string): Promise<Flag> {
    return unary(this.admin.getFlag.bind(this.admin), { key });
  }

  createFlag(request: CreateFlagRequest): Promise<Flag> {
    return unary(this.admin.createFlag.bind(this.admin), request);
  }

  updateFlag(key: string, enabled: boolean, defaultVariantKey: string): Promise<Flag> {
    return unary(this.admin.updateFlag.bind(this.admin), { key, enabled, defaultVariantKey });
  }

  archiveFlag(key: string, archived: boolean): Promise<Flag> {
    return unary(this.admin.archiveFlag.bind(this.admin), { key, archived });
  }

  deleteFlag(key: string): Promise<void> {
    return unary(this.admin.deleteFlag.bind(this.admin), { key }).then(() => undefined);
  }

  upsertVariant(flagKey: string, variant: Variant): Promise<Flag> {
    return unary(this.admin.upsertVariant.bind(this.admin), { flagKey, variant });
  }

  deleteVariant(flagKey: string, variantKey: string): Promise<Flag> {
    return unary(this.admin.deleteVariant.bind(this.admin), { flagKey, variantKey });
  }

  setFlagRules(flagKey: string, rules: Rule[]): Promise<Flag> {
    return unary(this.admin.setFlagRules.bind(this.admin), { flagKey, rules });
  }

  listSegments(): Promise<ListSegmentsResponse> {
    return unary(this.admin.listSegments.bind(this.admin), {});
  }

  upsertSegment(segment: Segment): Promise<Segment> {
    return unary(this.admin.updateSegment.bind(this.admin), { segment });
  }

  deleteSegment(key: string): Promise<void> {
    return unary(this.admin.deleteSegment.bind(this.admin), { key }).then(() => undefined);
  }

  getSnapshot(): Promise<SnapshotResponse> {
    return unary(this.evaluation.getSnapshot.bind(this.evaluation), {});
  }

  resolveAll(targetingKey: string, attributes: Record<string, unknown>): Promise<ResolveAllResponse> {
    return unary(this.evaluation.resolveAll.bind(this.evaluation), {
      context: { targetingKey, attributes },
    });
  }

  /**
   * Resolve a single flag through the typed RPC matching its value type, normalised into
   * the same {@link EvaluatedFlag} shape `resolveAll` returns so callers can render either
   * uniformly. The full {@link ResolutionMeta} (including `errorMessage`) is preserved.
   */
  resolve(
    flagKey: string,
    valueType: ValueType,
    targetingKey: string,
    attributes: Record<string, unknown>,
  ): Promise<EvaluatedFlag> {
    const request: ResolveRequest = { flagKey, context: { targetingKey, attributes } };
    const evaluation = this.evaluation;
    const into = (p: Promise<{ value?: unknown; meta?: ResolutionMeta }>): Promise<EvaluatedFlag> =>
      p.then((r) => ({ flagKey, valueType, value: r.value, meta: r.meta }));

    switch (valueType) {
      case ValueType.VALUE_TYPE_BOOLEAN:
        return into(unary(evaluation.resolveBoolean.bind(evaluation), request));
      case ValueType.VALUE_TYPE_STRING:
        return into(unary(evaluation.resolveString.bind(evaluation), request));
      case ValueType.VALUE_TYPE_INTEGER:
        return into(unary(evaluation.resolveInteger.bind(evaluation), request));
      case ValueType.VALUE_TYPE_FLOAT:
        return into(unary(evaluation.resolveFloat.bind(evaluation), request));
      case ValueType.VALUE_TYPE_OBJECT:
        return into(unary(evaluation.resolveObject.bind(evaluation), request));
      default:
        return Promise.reject(new Error(`unsupported value type: ${valueType}`));
    }
  }

  /**
   * The current snapshot followed by a fresh one on every config change. Pass an
   * `AbortSignal` to cancel the underlying server stream when the consumer goes away.
   */
  async *streamSnapshot(signal?: AbortSignal): AsyncIterable<SnapshotResponse> {
    const stream = this.evaluation.streamSnapshot({});
    if (signal) {
      signal.addEventListener("abort", () => stream.cancel(), { once: true });
    }
    try {
      for await (const snapshot of stream) {
        yield snapshot as SnapshotResponse;
      }
    } catch (error) {
      if (signal?.aborted) return;
      throw error;
    }
  }
}
