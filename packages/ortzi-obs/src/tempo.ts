export interface AnyValue {
  stringValue?: string;
  intValue?: string | number;
  doubleValue?: number;
  boolValue?: boolean;
}

export interface Attribute {
  key: string;
  value: AnyValue;
}

export interface OtlpSpan {
  spanId: string;
  parentSpanId?: string;
  name: string;
  startTimeUnixNano: string;
  endTimeUnixNano: string;
  attributes?: Attribute[];
  status?: { code?: string | number; message?: string };
}

interface ResourceSpans {
  resource?: { attributes?: Attribute[] };
  scopeSpans?: { spans?: OtlpSpan[] }[];
  instrumentationLibrarySpans?: { spans?: OtlpSpan[] }[];
}

export interface TraceResponse {
  batches?: ResourceSpans[];
  resourceSpans?: ResourceSpans[];
  trace?: { resourceSpans?: ResourceSpans[] };
}

export interface SearchResponse {
  traces?: {
    traceID: string;
    rootServiceName?: string;
    rootTraceName?: string;
    startTimeUnixNano?: string;
    durationMs?: number;
  }[];
}

export interface Span {
  id: string;
  parent?: string;
  service: string;
  name: string;
  start: bigint;
  end: bigint;
  error?: string;
  attributes: Record<string, string>;
}

export const isTraceId = (value: string) => /^[0-9a-fA-F]{16,32}$/.test(value);

const display = (value: AnyValue) =>
  String(value.stringValue ?? value.intValue ?? value.doubleValue ?? value.boolValue ?? '');

const record = (attributes: Attribute[] = []) =>
  Object.fromEntries(attributes.map(({ key, value }) => [key, display(value ?? {})]));

const isError = (code: string | number | undefined) => code === 2 || code === 'STATUS_CODE_ERROR';

export function spans(response: TraceResponse): Span[] {
  const batches = response.trace?.resourceSpans ?? response.resourceSpans ?? response.batches ?? [];

  return batches
    .flatMap((batch) => {
      const service = record(batch.resource?.attributes)['service.name'] ?? '-';
      const scopes = batch.scopeSpans ?? batch.instrumentationLibrarySpans ?? [];
      return scopes.flatMap((scope) =>
        (scope.spans ?? []).map((span) => ({
          id: span.spanId,
          ...(span.parentSpanId ? { parent: span.parentSpanId } : {}),
          service,
          name: span.name,
          start: BigInt(span.startTimeUnixNano),
          end: BigInt(span.endTimeUnixNano),
          ...(isError(span.status?.code) ? { error: span.status?.message || 'error' } : {}),
          attributes: record(span.attributes),
        })),
      );
    })
    .sort((a, b) => (a.start < b.start ? -1 : a.start > b.start ? 1 : 0));
}

export function ms(ns: bigint): string {
  const value = Number(ns) / 1e6;
  return value >= 1000 ? `${(value / 1000).toFixed(2)}s` : `${value.toFixed(value < 10 ? 2 : 1)}ms`;
}

/** Spans depth-first, children under their parent; spans whose parent is missing are roots. */
export function ordered(all: Span[]): { span: Span; depth: number }[] {
  const ids = new Set(all.map((span) => span.id));
  const children = new Map<string | undefined, Span[]>();
  for (const span of all) {
    const parent = span.parent && ids.has(span.parent) ? span.parent : undefined;
    children.set(parent, [...(children.get(parent) ?? []), span]);
  }

  const out: { span: Span; depth: number }[] = [];
  const walk = (parent: string | undefined, depth: number) => {
    for (const span of children.get(parent) ?? []) {
      out.push({ span, depth });
      walk(span.id, depth + 1);
    }
  };
  walk(undefined, 0);
  return out;
}

export function tree(all: Span[]): string[] {
  const origin = all[0]?.start ?? 0n;

  return ordered(all).map(({ span, depth }) => {
    const at = `+${ms(span.start - origin)}`.padStart(10);
    const took = ms(span.end - span.start).padStart(9);
    const error = span.error ? `  ERROR ${span.error}` : '';
    return `${at} ${took}  ${'  '.repeat(depth)}${span.service}: ${span.name}${error}`;
  });
}
