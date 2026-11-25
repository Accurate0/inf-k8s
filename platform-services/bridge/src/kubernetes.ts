import { readFile } from 'node:fs/promises';

const SERVICE_ACCOUNT = '/var/run/secrets/kubernetes.io/serviceaccount/token';

export interface KubernetesOptions {
  url?: string;
  token?: () => Promise<string>;
  fetcher?: typeof fetch;
  ttlMs?: number;
}

// one query reads the same lists from several resolvers, so they're cached briefly
export class Kubernetes {
  readonly #url: string;
  readonly #token: () => Promise<string>;
  readonly #fetch: typeof fetch;
  readonly #ttl: number;
  readonly #cache = new Map<string, { at: number; items: Promise<unknown[]> }>();

  constructor(options: KubernetesOptions = {}) {
    this.#url = options.url ?? 'https://kubernetes.default.svc';
    this.#token = options.token ?? (async () => (await readFile(SERVICE_ACCOUNT, 'utf8')).trim());
    this.#fetch = options.fetcher ?? fetch;
    this.#ttl = options.ttlMs ?? 10_000;
  }

  list<T>(path: string): Promise<T[]> {
    const cached = this.#cache.get(path);
    if (cached && Date.now() - cached.at < this.#ttl) {
      return cached.items as Promise<T[]>;
    }

    const items = this.#get(path);
    this.#cache.set(path, { at: Date.now(), items });
    items.catch(() => this.#cache.delete(path));
    return items as Promise<T[]>;
  }

  async #get(path: string): Promise<unknown[]> {
    const response = await this.#fetch(`${this.#url}${path}`, {
      headers: {
        authorization: `Bearer ${await this.#token()}`,
        accept: 'application/json',
      },
    });
    if (!response.ok) {
      throw new Error(`kubernetes ${path} returned ${response.status}`);
    }
    return ((await response.json()) as { items?: unknown[] }).items ?? [];
  }
}

const FLUX = 'flux-system';

export const paths = {
  kustomizations: `/apis/kustomize.toolkit.fluxcd.io/v1/namespaces/${FLUX}/kustomizations`,
  helmReleases: `/apis/helm.toolkit.fluxcd.io/v2/namespaces/${FLUX}/helmreleases`,
  configMaps: `/api/v1/namespaces/${FLUX}/configmaps`,
  imagePolicies: `/apis/image.toolkit.fluxcd.io/v1/namespaces/${FLUX}/imagepolicies`,
  imageRepositories: `/apis/image.toolkit.fluxcd.io/v1/namespaces/${FLUX}/imagerepositories`,
  workloads: (namespace?: string) => {
    const scope = namespace ? `/namespaces/${namespace}` : '';
    return [
      { kind: 'Deployment', path: `/apis/apps/v1${scope}/deployments` },
      { kind: 'StatefulSet', path: `/apis/apps/v1${scope}/statefulsets` },
      { kind: 'DaemonSet', path: `/apis/apps/v1${scope}/daemonsets` },
      { kind: 'CronJob', path: `/apis/batch/v1${scope}/cronjobs` },
    ];
  },
  pods: (namespace: string) => `/api/v1/namespaces/${namespace}/pods`,
};
