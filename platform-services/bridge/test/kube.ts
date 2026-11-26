interface Meta {
  metadata: {
    name: string;
    namespace?: string;
    uid?: string;
    resourceVersion?: string;
    [key: string]: unknown;
  };
  [key: string]: unknown;
}

export interface Recorded {
  method: string;
  path: string;
  search: string;
  contentType?: string;
  body?: unknown;
}

const encoder = new TextEncoder();

const merge = (target: any, patch: any): any => {
  if (typeof patch !== 'object' || patch === null || Array.isArray(patch)) {
    return patch;
  }
  const out = { ...(typeof target === 'object' && target !== null ? target : {}) };
  for (const [key, value] of Object.entries(patch)) {
    out[key] = value === null ? undefined : merge(out[key], value);
  }
  return out;
};

const collectionOf = (path: string) => path.replace(/\/namespaces\/[^/]+\//, '/');

// an api server: lists, streamed watches the test pushes events into, merge patches and text endpoints
export class FakeKube {
  readonly requests: Recorded[] = [];
  readonly #lists = new Map<string, Meta[]>();
  readonly #texts: Record<string, string>;
  readonly #streams = new Map<string, ReadableStreamDefaultController<Uint8Array>[]>();
  readonly #watching = new Map<string, (() => void)[]>();
  #version = 100;

  constructor(lists: Record<string, Meta[]>, texts: Record<string, string> = {}) {
    for (const [path, items] of Object.entries(lists)) {
      this.#lists.set(
        path,
        items.map((item) => this.#stamp(path, structuredClone(item))),
      );
    }
    this.#texts = texts;
  }

  #stamp(path: string, item: Meta): Meta {
    item.metadata.uid ??= `${path}/${item.metadata.namespace ?? ''}/${item.metadata.name}`;
    item.metadata.resourceVersion ??= String(++this.#version);
    return item;
  }

  watching(path: string): Promise<void> {
    if (this.#streams.get(path)?.length) {
      return Promise.resolve();
    }
    return new Promise((resolve) =>
      this.#watching.set(path, [...(this.#watching.get(path) ?? []), resolve]),
    );
  }

  push(path: string, type: 'ADDED' | 'MODIFIED' | 'DELETED', item: Meta): void {
    const items = this.#lists.get(path) ?? [];
    const stamped = this.#stamp(path, structuredClone(item));
    stamped.metadata.resourceVersion = String(++this.#version);
    const rest = items.filter((existing) => existing.metadata.uid !== stamped.metadata.uid);
    this.#lists.set(path, type === 'DELETED' ? rest : [...rest, stamped]);
    this.#send(path, { type, object: stamped });
  }

  expire(path: string): void {
    this.#send(path, { type: 'ERROR', object: { kind: 'Status', code: 410, message: 'too old' } });
  }

  #send(path: string, event: unknown) {
    for (const controller of this.#streams.get(path) ?? []) {
      controller.enqueue(encoder.encode(`${JSON.stringify(event)}\n`));
    }
  }

  readonly fetcher = (async (input: string | URL | Request, init?: RequestInit) => {
    const url = new URL(String(input));
    const method = init?.method ?? 'GET';
    const headers = new Headers(init?.headers);
    this.requests.push({
      method,
      path: url.pathname,
      search: url.search,
      contentType: headers.get('content-type') ?? undefined,
      body: typeof init?.body === 'string' ? JSON.parse(init.body) : undefined,
    });

    if (url.searchParams.get('watch')) {
      const path = url.pathname;
      let opened: ReadableStreamDefaultController<Uint8Array>;
      const drop = () =>
        this.#streams.set(
          path,
          (this.#streams.get(path) ?? []).filter((c) => c !== opened),
        );
      const stream = new ReadableStream<Uint8Array>({
        cancel: () => {
          drop();
        },
        start: (controller: ReadableStreamDefaultController<Uint8Array>) => {
          opened = controller;
          this.#streams.set(path, [...(this.#streams.get(path) ?? []), controller]);
          for (const resolve of this.#watching.get(path) ?? []) {
            resolve();
          }
          this.#watching.delete(path);
          init?.signal?.addEventListener('abort', () => {
            drop();
            try {
              controller.close();
            } catch {}
          });
        },
      });
      return new Response(stream);
    }

    if (method === 'PATCH') {
      const slash = url.pathname.lastIndexOf('/');
      const name = url.pathname.slice(slash + 1);
      const collection = url.pathname.slice(0, slash);
      const namespace = /\/namespaces\/([^/]+)\//.exec(url.pathname)?.[1];
      for (const [path, items] of this.#lists) {
        if (path !== collection && path !== collectionOf(collection)) {
          continue;
        }
        const index = items.findIndex(
          (item) =>
            item.metadata.name === name &&
            (!namespace || (item.metadata.namespace ?? namespace) === namespace),
        );
        if (index < 0) {
          continue;
        }
        const patched = merge(items[index], JSON.parse(String(init?.body)));
        patched.metadata.resourceVersion = String(++this.#version);
        items[index] = patched;
        return Response.json(patched);
      }
      return new Response('{}', { status: 404 });
    }

    const text = this.#texts[`${url.pathname}${url.search}`];
    if (text !== undefined) {
      return new Response(text);
    }
    const items = this.#lists.get(url.pathname);
    if (items) {
      return Response.json({ metadata: { resourceVersion: String(this.#version) }, items });
    }
    return new Response('{}', { status: 404 });
  }) as typeof fetch;
}
