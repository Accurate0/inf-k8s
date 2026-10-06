import { ChannelCredentials, Metadata, type ClientUnaryCall } from "@grpc/grpc-js";
import {
  AdminClient,
  type CreateFlagRequest,
  type ListChangesResponse,
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
  metadata: Metadata,
  callback: (error: Error | null, response: Res) => void,
) => ClientUnaryCall;

function unary<Req, Res>(
  method: UnaryMethod<Req, Res>,
  request: Req,
  metadata: Metadata = new Metadata(),
): Promise<Res> {
  return new Promise((resolve, reject) => {
    method(request, metadata, (error, response) => {
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
  private readonly clientId: string;

  constructor(
    address: string,
    clientId: string,
    credentials: ChannelCredentials = ChannelCredentials.createInsecure(),
  ) {
    this.admin = new AdminClient(address, credentials);
    this.evaluation = new EvaluationClient(address, credentials);
    this.clientId = clientId;
  }

  /**
   * Metadata carried on every request: `client-id` identifies this service to the backend
   * (required on the evaluation path), and an optional `actor` names the acting user for
   * the audit log.
   */
  private meta(actor?: string): Metadata {
    const metadata = new Metadata();
    metadata.set("client-id", this.clientId);
    if (actor) metadata.set("actor", actor);
    return metadata;
  }

  listFlags(includeArchived = false): Promise<ListFlagsResponse> {
    return unary(this.admin.listFlags.bind(this.admin), { includeArchived }, this.meta());
  }

  getFlag(key: string): Promise<Flag> {
    return unary(this.admin.getFlag.bind(this.admin), { key }, this.meta());
  }

  createFlag(request: CreateFlagRequest, actor?: string): Promise<Flag> {
    return unary(this.admin.createFlag.bind(this.admin), request, this.meta(actor));
  }

  updateFlag(
    key: string,
    enabled: boolean,
    defaultVariantKey: string,
    actor?: string,
  ): Promise<Flag> {
    return unary(
      this.admin.updateFlag.bind(this.admin),
      { key, enabled, defaultVariantKey },
      this.meta(actor),
    );
  }

  archiveFlag(key: string, archived: boolean, actor?: string): Promise<Flag> {
    return unary(this.admin.archiveFlag.bind(this.admin), { key, archived }, this.meta(actor));
  }

  deleteFlag(key: string, actor?: string): Promise<void> {
    return unary(this.admin.deleteFlag.bind(this.admin), { key }, this.meta(actor)).then(
      () => undefined,
    );
  }

  upsertVariant(flagKey: string, variant: Variant, actor?: string): Promise<Flag> {
    return unary(
      this.admin.upsertVariant.bind(this.admin),
      { flagKey, variant },
      this.meta(actor),
    );
  }

  deleteVariant(flagKey: string, variantKey: string, actor?: string): Promise<Flag> {
    return unary(
      this.admin.deleteVariant.bind(this.admin),
      { flagKey, variantKey },
      this.meta(actor),
    );
  }

  setFlagRules(flagKey: string, rules: Rule[], actor?: string): Promise<Flag> {
    return unary(
      this.admin.setFlagRules.bind(this.admin),
      { flagKey, rules },
      this.meta(actor),
    );
  }

  listSegments(): Promise<ListSegmentsResponse> {
    return unary(this.admin.listSegments.bind(this.admin), {}, this.meta());
  }

  upsertSegment(segment: Segment, actor?: string): Promise<Segment> {
    return unary(this.admin.updateSegment.bind(this.admin), { segment }, this.meta(actor));
  }

  deleteSegment(key: string, actor?: string): Promise<void> {
    return unary(this.admin.deleteSegment.bind(this.admin), { key }, this.meta(actor)).then(
      () => undefined,
    );
  }

  /**
   * Read the audit log newest-first. Pass `targetKind`/`targetKey` to scope it to a
   * single flag or segment; `limit` caps the row count (clamped server-side).
   */
  listChanges(
    targetKind = "",
    targetKey = "",
    limit = 0,
  ): Promise<ListChangesResponse> {
    return unary(
      this.admin.listChanges.bind(this.admin),
      { targetKind, targetKey, limit },
      this.meta(),
    );
  }

  getSnapshot(): Promise<SnapshotResponse> {
    return unary(this.evaluation.getSnapshot.bind(this.evaluation), {}, this.meta());
  }

  resolveAll(targetingKey: string, attributes: Record<string, unknown>): Promise<ResolveAllResponse> {
    return unary(
      this.evaluation.resolveAll.bind(this.evaluation),
      { context: { targetingKey, attributes } },
      this.meta(),
    );
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
    const metadata = this.meta();
    const into = (p: Promise<{ value?: unknown; meta?: ResolutionMeta }>): Promise<EvaluatedFlag> =>
      p.then((r) => ({ flagKey, valueType, value: r.value, meta: r.meta }));

    switch (valueType) {
      case ValueType.VALUE_TYPE_BOOLEAN:
        return into(unary(evaluation.resolveBoolean.bind(evaluation), request, metadata));
      case ValueType.VALUE_TYPE_STRING:
        return into(unary(evaluation.resolveString.bind(evaluation), request, metadata));
      case ValueType.VALUE_TYPE_INTEGER:
        return into(unary(evaluation.resolveInteger.bind(evaluation), request, metadata));
      case ValueType.VALUE_TYPE_FLOAT:
        return into(unary(evaluation.resolveFloat.bind(evaluation), request, metadata));
      case ValueType.VALUE_TYPE_OBJECT:
        return into(unary(evaluation.resolveObject.bind(evaluation), request, metadata));
      default:
        return Promise.reject(new Error(`unsupported value type: ${valueType}`));
    }
  }

  /**
   * The current snapshot followed by a fresh one on every config change. Pass an
   * `AbortSignal` to cancel the underlying server stream when the consumer goes away.
   */
  async *streamSnapshot(signal?: AbortSignal): AsyncIterable<SnapshotResponse> {
    const stream = this.evaluation.streamSnapshot({}, this.meta());
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
