import { EventEmitter } from 'node:events';
import { readFile } from 'node:fs/promises';
import { setTimeout as sleep } from 'node:timers/promises';
import { type TracedFetch, tracedFetch } from './tracing.ts';

const SERVICE_ACCOUNT = '/var/run/secrets/kubernetes.io/serviceaccount/token';
const TIMEOUT_MS = 8_000;
const WATCH_SECONDS = 300;
const MAX_BACKOFF_MS = 30_000;

export interface KubernetesOptions {
  url?: string;
  token?: () => Promise<string>;
  fetcher?: typeof fetch;
  ttlMs?: number;
}

interface Stored {
  metadata?: { uid?: string; resourceVersion?: string; managedFields?: unknown };
}

export interface Change {
  path: string;
  type: 'ADDED' | 'MODIFIED' | 'DELETED';
  object: unknown;
}

class Gone extends Error {}

interface Informer {
  path: string;
  items: Map<string, Stored>;
  resourceVersion?: string;
  synced: Promise<void>;
  markSynced: () => void;
}

const keyOf = (object: Stored) => object.metadata?.uid ?? JSON.stringify(object);

const trim = <T extends Stored>(object: T): T => {
  if (object.metadata?.managedFields) {
    delete object.metadata.managedFields;
  }
  return object;
};

const template = (path: string) =>
  path
    .replace(/\/namespaces\/[^/]+\//, '/namespaces/{namespace}/')
    .replace(/(\/namespaces\/\{namespace\}\/[a-z]+)\/[^/?]+/, '$1/{name}')
    .split('?')[0]!;

// watched paths are served from memory and announce every change; others are fetched and cached briefly
export class Kubernetes {
  readonly changes = new EventEmitter().setMaxListeners(0);
  readonly #url: string;
  readonly #token: () => Promise<string>;
  readonly #fetch: TracedFetch;
  readonly #raw: typeof fetch;
  readonly #ttl: number;
  readonly #cache = new Map<string, { at: number; items: Promise<unknown[]> }>();
  readonly #informers = new Map<string, Informer>();
  readonly #stop = new AbortController();

  constructor(options: KubernetesOptions = {}) {
    this.#url = options.url ?? 'https://kubernetes.default.svc';
    this.#token = options.token ?? (async () => (await readFile(SERVICE_ACCOUNT, 'utf8')).trim());
    this.#raw = options.fetcher ?? fetch;
    this.#fetch = tracedFetch(options.fetcher);
    this.#ttl = options.ttlMs ?? 10_000;
  }

  watch(paths: string[]): Promise<void> {
    for (const path of paths) {
      if (this.#informers.has(path)) {
        continue;
      }

      let markSynced = () => {};
      const synced = new Promise<void>((resolve) => (markSynced = resolve));
      const informer: Informer = { path, items: new Map(), synced, markSynced };
      this.#informers.set(path, informer);
      void this.#run(informer);
    }
    return Promise.all(paths.map((path) => this.#informers.get(path)!.synced)).then(() => {});
  }

  stop(): void {
    this.#stop.abort();
  }

  async list<T>(path: string): Promise<T[]> {
    const informer = this.#informers.get(path);
    if (informer) {
      await informer.synced;
      return [...informer.items.values()] as T[];
    }

    const cached = this.#cache.get(path);
    if (cached && Date.now() - cached.at < this.#ttl) {
      return cached.items as Promise<T[]>;
    }

    const items = this.#list(path).then(({ items }) => items);
    this.#cache.set(path, { at: Date.now(), items });
    items.catch(() => this.#cache.delete(path));
    return items as Promise<T[]>;
  }

  async patch<T>(path: string, body: unknown): Promise<T> {
    const response = await this.#send(path, {
      method: 'PATCH',
      headers: { 'content-type': 'application/merge-patch+json' },
      body: JSON.stringify(body),
    });
    if (!response.ok) {
      throw new Error(`kubernetes PATCH ${path} returned ${response.status}`);
    }

    const object = trim((await response.json()) as Stored);
    for (const informer of this.#informers.values()) {
      if (informer.items.has(keyOf(object))) {
        this.#apply(informer, 'MODIFIED', object);
      }
    }
    return object as T;
  }

  async text(path: string): Promise<string | undefined> {
    const response = await this.#send(path, { headers: { accept: 'text/plain' } });
    // 400 is how the api says there's no previous container to read
    if (response.status === 404 || response.status === 400) {
      return undefined;
    }
    if (!response.ok) {
      throw new Error(`kubernetes ${template(path)} returned ${response.status}`);
    }
    return response.text();
  }

  async #send(
    path: string,
    init: { method?: string; headers?: Record<string, string>; body?: string },
  ) {
    return this.#fetch(`${this.#url}${path}`, {
      ...init,
      template: template(path),
      headers: {
        accept: 'application/json',
        ...init.headers,
        authorization: `Bearer ${await this.#token()}`,
      },
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
  }

  async #list(path: string): Promise<{ items: Stored[]; resourceVersion?: string }> {
    const response = await this.#send(path, {});
    if (!response.ok) {
      throw new Error(`kubernetes ${path} returned ${response.status}`);
    }
    const list = (await response.json()) as {
      items?: Stored[];
      metadata?: { resourceVersion?: string };
    };
    return { items: (list.items ?? []).map(trim), resourceVersion: list.metadata?.resourceVersion };
  }

  async #run(informer: Informer): Promise<void> {
    let backoff = 1_000;
    while (!this.#stop.signal.aborted) {
      try {
        if (!informer.resourceVersion) {
          await this.#relist(informer);
        }
        await this.#stream(informer);
        backoff = 1_000;
      } catch (error) {
        if (this.#stop.signal.aborted) {
          return;
        }
        if (error instanceof Gone) {
          informer.resourceVersion = undefined;
          continue;
        }
        console.error(`watch ${informer.path}: ${(error as Error).message}`);
        await sleep(backoff, undefined, { signal: this.#stop.signal }).catch(() => {});
        backoff = Math.min(backoff * 2, MAX_BACKOFF_MS);
      }
    }
  }

  async #relist(informer: Informer): Promise<void> {
    const { items, resourceVersion } = await this.#list(informer.path);
    const fresh = new Map(items.map((object) => [keyOf(object), object]));

    for (const [key, old] of informer.items) {
      if (!fresh.has(key)) {
        this.#apply(informer, 'DELETED', old);
      }
    }
    for (const [key, object] of fresh) {
      const old = informer.items.get(key);
      if (old?.metadata?.resourceVersion !== object.metadata?.resourceVersion) {
        this.#apply(informer, old ? 'MODIFIED' : 'ADDED', object);
      }
    }

    informer.resourceVersion = resourceVersion;
    informer.markSynced();
  }

  async #stream(informer: Informer): Promise<void> {
    const query = new URLSearchParams({
      watch: '1',
      allowWatchBookmarks: 'true',
      timeoutSeconds: String(WATCH_SECONDS),
      resourceVersion: informer.resourceVersion ?? '',
    });
    const response = await this.#raw(`${this.#url}${informer.path}?${query}`, {
      headers: { authorization: `Bearer ${await this.#token()}`, accept: 'application/json' },
      signal: AbortSignal.any([
        this.#stop.signal,
        AbortSignal.timeout((WATCH_SECONDS + 30) * 1_000),
      ]),
    });
    if (response.status === 410) {
      throw new Gone();
    }
    if (!response.ok || !response.body) {
      throw new Error(`watch returned ${response.status}`);
    }

    let buffer = '';
    for await (const chunk of response.body.pipeThrough(new TextDecoderStream())) {
      buffer += chunk;
      let newline;
      while ((newline = buffer.indexOf('\n')) >= 0) {
        const line = buffer.slice(0, newline).trim();
        buffer = buffer.slice(newline + 1);
        if (line) {
          this.#event(
            informer,
            JSON.parse(line) as {
              type: string;
              object: Stored & { code?: number; message?: string };
            },
          );
        }
      }
    }
  }

  #event(
    informer: Informer,
    event: { type: string; object: Stored & { code?: number; message?: string } },
  ) {
    const { type, object } = event;

    if (type === 'ERROR') {
      if (object.code === 410) {
        throw new Gone();
      }
      throw new Error(object.message ?? 'watch error');
    }

    informer.resourceVersion = object.metadata?.resourceVersion ?? informer.resourceVersion;
    if (type === 'ADDED' || type === 'MODIFIED' || type === 'DELETED') {
      this.#apply(informer, type, trim(object));
    }
  }

  #apply(informer: Informer, type: Change['type'], object: Stored) {
    if (type === 'DELETED') {
      informer.items.delete(keyOf(object));
    } else {
      informer.items.set(keyOf(object), object);
    }
    this.changes.emit('change', { path: informer.path, type, object } satisfies Change);
  }
}

const FLUX = 'flux-system';

export const paths = {
  kustomizations: `/apis/kustomize.toolkit.fluxcd.io/v1/namespaces/${FLUX}/kustomizations`,
  helmReleases: `/apis/helm.toolkit.fluxcd.io/v2/namespaces/${FLUX}/helmreleases`,
  configMaps: `/api/v1/namespaces/${FLUX}/configmaps`,
  imagePolicies: `/apis/image.toolkit.fluxcd.io/v1/namespaces/${FLUX}/imagepolicies`,
  imageRepositories: `/apis/image.toolkit.fluxcd.io/v1/namespaces/${FLUX}/imagerepositories`,
  workloads: [
    { kind: 'Deployment', path: '/apis/apps/v1/deployments', resource: 'deployments' },
    { kind: 'StatefulSet', path: '/apis/apps/v1/statefulsets', resource: 'statefulsets' },
    { kind: 'DaemonSet', path: '/apis/apps/v1/daemonsets', resource: 'daemonsets' },
    { kind: 'CronJob', path: '/apis/batch/v1/cronjobs', resource: 'cronjobs' },
    { kind: 'Job', path: '/apis/batch/v1/jobs', resource: 'jobs' },
  ],
  pods: '/api/v1/pods',
  replicaSets: (namespace: string) => `/apis/apps/v1/namespaces/${namespace}/replicasets`,
  events: (namespace: string) => `/api/v1/namespaces/${namespace}/events`,
  log: (namespace: string, pod: string) => `/api/v1/namespaces/${namespace}/pods/${pod}/log`,
  fluxObject: (resource: 'kustomizations' | 'helmreleases', name: string) =>
    `${resource === 'kustomizations' ? paths.kustomizations : paths.helmReleases}/${name}`,
  workload: (namespace: string, kind: string, name: string) => {
    const found = paths.workloads.find((workload) => workload.kind === kind);
    if (!found) {
      return undefined;
    }
    const group = found.path.slice(0, found.path.lastIndexOf('/'));
    return `${group}/namespaces/${namespace}/${found.resource}/${name}`;
  },
};

export const watched = [
  paths.kustomizations,
  paths.helmReleases,
  paths.configMaps,
  paths.imagePolicies,
  paths.imageRepositories,
  ...paths.workloads.map((workload) => workload.path),
  paths.pods,
];
