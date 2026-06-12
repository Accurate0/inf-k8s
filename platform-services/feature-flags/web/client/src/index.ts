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
  type Prerequisite,
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

/** Build gRPC metadata carrying the acting user, recorded by the backend audit log. */
function actorMetadata(actor?: string): Metadata {
  const metadata = new Metadata();
  if (actor) metadata.set("actor", actor);
  return metadata;
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

  createFlag(request: CreateFlagRequest, actor?: string): Promise<Flag> {
    return unary(this.admin.createFlag.bind(this.admin), request, actorMetadata(actor));
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
      actorMetadata(actor),
    );
  }

  archiveFlag(key: string, archived: boolean, actor?: string): Promise<Flag> {
    return unary(this.admin.archiveFlag.bind(this.admin), { key, archived }, actorMetadata(actor));
  }

  deleteFlag(key: string, actor?: string): Promise<void> {
    return unary(this.admin.deleteFlag.bind(this.admin), { key }, actorMetadata(actor)).then(
      () => undefined,
    );
  }

  upsertVariant(flagKey: string, variant: Variant, actor?: string): Promise<Flag> {
    return unary(
      this.admin.upsertVariant.bind(this.admin),
      { flagKey, variant },
      actorMetadata(actor),
    );
  }

  deleteVariant(flagKey: string, variantKey: string, actor?: string): Promise<Flag> {
    return unary(
      this.admin.deleteVariant.bind(this.admin),
      { flagKey, variantKey },
      actorMetadata(actor),
    );
  }

  setFlagRules(flagKey: string, rules: Rule[], actor?: string): Promise<Flag> {
    return unary(
      this.admin.setFlagRules.bind(this.admin),
      { flagKey, rules },
      actorMetadata(actor),
    );
  }

  setFlagPrerequisites(
    flagKey: string,
    prerequisites: Prerequisite[],
    actor?: string,
  ): Promise<Flag> {
    return unary(
      this.admin.setFlagPrerequisites.bind(this.admin),
      { flagKey, prerequisites },
      actorMetadata(actor),
    );
  }

  listSegments(): Promise<ListSegmentsResponse> {
    return unary(this.admin.listSegments.bind(this.admin), {});
  }

  upsertSegment(segment: Segment, actor?: string): Promise<Segment> {
    return unary(this.admin.updateSegment.bind(this.admin), { segment }, actorMetadata(actor));
  }

  deleteSegment(key: string, actor?: string): Promise<void> {
    return unary(this.admin.deleteSegment.bind(this.admin), { key }, actorMetadata(actor)).then(
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
    return unary(this.admin.listChanges.bind(this.admin), { targetKind, targetKey, limit });
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
