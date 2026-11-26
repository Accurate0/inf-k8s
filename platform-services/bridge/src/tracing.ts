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

// every outbound call is a client span, so slow sources show up in the trace
export function tracedFetch(service: string, fetcher: typeof fetch = fetch): typeof fetch {
  return ((input: string | URL | Request, init?: RequestInit) => {
    const url = new URL(input instanceof Request ? input.url : String(input));
    const method = init?.method ?? (input instanceof Request ? input.method : 'GET');
    return traced(
      `${service} ${method}`,
      {
        'http.request.method': method,
        'server.address': url.hostname,
        'url.full': `${url.origin}${url.pathname}`,
        'url.query': url.search.slice(1),
      },
      async () => {
        const response = await fetcher(input, init);
        trace.getActiveSpan()?.setAttribute('http.response.status_code', response.status);
        if (response.status >= 400) {
          failed(`${response.status}`);
        }
        return response;
      },
      SpanKind.CLIENT,
    );
  }) as typeof fetch;
}
