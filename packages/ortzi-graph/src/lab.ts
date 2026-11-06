import type { ServeRequest, ServeResponse } from '@ortzi/host';

export const LAB_PORT = 4280;

export interface Upstream {
  /** The router's lab page. */
  page: () => string;

  /** Sends a GraphQL request body to the router with the caller's token. */
  forward: (body: string, accept: string | undefined) => ServeResponse;
}

const locals = (port: number) => [`localhost:${port}`, `127.0.0.1:${port}`];

/**
 * Serves the router's lab page from localhost and forwards its GraphQL requests with a fresh
 * token. Requests for another Host (DNS rebinding) or from another site's Origin are refused, so
 * no other page can query the graph as you.
 */
export function handle(request: ServeRequest, port: number, upstream: Upstream): ServeResponse {
  const allowed = locals(port);
  if (!allowed.includes(request.headers.host ?? '')) {
    return { status: 403, body: 'forbidden host\n' };
  }

  const origin = request.headers.origin;
  if (origin !== undefined && !allowed.some((host) => origin === `http://${host}`)) {
    return { status: 403, body: 'forbidden origin\n' };
  }

  if (request.method === 'GET') {
    return {
      status: 200,
      headers: { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' },
      body: upstream.page(),
    };
  }

  if (request.method === 'POST') {
    return upstream.forward(request.body, request.headers.accept);
  }

  return { status: 405, headers: { allow: 'GET, POST' }, body: 'method not allowed\n' };
}
