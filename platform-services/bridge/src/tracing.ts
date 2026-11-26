import {
  type Attributes,
  context,
  propagation,
  SpanKind,
  SpanStatusCode,
  trace,
} from '@opentelemetry/api';
import { AsyncLocalStorageContextManager } from '@opentelemetry/context-async-hooks';
import { W3CTraceContextPropagator } from '@opentelemetry/core';
import { OTLPTraceExporter } from '@opentelemetry/exporter-trace-otlp-http';
import { resourceFromAttributes } from '@opentelemetry/resources';
import {
  BasicTracerProvider,
  BatchSpanProcessor,
  SimpleSpanProcessor,
  type SpanExporter,
} from '@opentelemetry/sdk-trace-base';

export { SpanKind };

// context and propagation always run, so spans join the router's trace even without an exporter
export function startTracing(
  url: string | undefined,
  exporter?: SpanExporter,
): () => Promise<void> {
  context.setGlobalContextManager(new AsyncLocalStorageContextManager().enable());
  propagation.setGlobalPropagator(new W3CTraceContextPropagator());
  if (!url && !exporter) {
    return async () => {};
  }

  const provider = new BasicTracerProvider({
    resource: resourceFromAttributes({ 'service.name': 'bridge' }),
    spanProcessors: [
      exporter
        ? new SimpleSpanProcessor(exporter)
        : new BatchSpanProcessor(new OTLPTraceExporter({ url })),
    ],
  });
  trace.setGlobalTracerProvider(provider);
  return () => provider.shutdown();
}

export function traced<T>(
  name: string,
  attributes: Attributes,
  run: () => Promise<T> | T,
  kind: SpanKind = SpanKind.INTERNAL,
): Promise<T> {
  return trace.getTracer('bridge').startActiveSpan(name, { kind, attributes }, async (span) => {
    try {
      return await run();
    } catch (error) {
      span.recordException(error as Error);
      span.setStatus({ code: SpanStatusCode.ERROR, message: (error as Error).message });
      throw error;
    } finally {
      span.end();
    }
  });
}

export function failed(message: string): void {
  trace.getActiveSpan()?.setStatus({ code: SpanStatusCode.ERROR, message });
}

export function withParent<T>(headers: Record<string, unknown>, run: () => T): T {
  return context.with(propagation.extract(context.active(), headers), run);
}

export type TracedFetch = (
  input: string | URL,
  init?: RequestInit & { template?: string },
) => Promise<Response>;

const REDACTED = '{redacted}';

function placeholder(segment: string): string {
  if (/^\d+$/.test(segment)) {
    return '{id}';
  }
  return segment.length >= 16 && /^[\w-]+$/.test(segment) ? REDACTED : segment;
}

// ids and opaque tokens in paths would make every span name unique
export function redactedPath(url: URL): string {
  const path = url.pathname.split('/').map(placeholder).join('/');
  return url.search ? `${path}?${REDACTED}` : path;
}

// every outbound call is a client span named `<method> <host><template>`, as home-gateway names them
export function tracedFetch(fetcher: typeof fetch = fetch): TracedFetch {
  return (input, init) => {
    const { template: given, ...rest } = init ?? {};
    const url = new URL(String(input));
    const method = rest.method ?? 'GET';
    const template = given ?? redactedPath(url);
    return traced(
      `${method} ${url.host}${template}`,
      {
        'http.request.method': method,
        'server.address': url.hostname,
        'url.template': template,
        'url.full': `${url.origin}${redactedPath(url)}`,
      },
      async () => {
        const response = await fetcher(input, rest);
        trace.getActiveSpan()?.setAttribute('http.response.status_code', response.status);
        if (response.status >= 400) {
          failed(`${response.status}`);
        }
        return response;
      },
      SpanKind.CLIENT,
    );
  };
}
