import { type TracedFetch, tracedFetch } from './tracing.ts';

export interface ForgejoRunJson {
  id: number;
  index_in_repo: number;
  workflow_id: string;
  commit_sha: string;
  status: string;
  title: string;
  started?: string | null;
  stopped?: string | null;
  html_url: string;
  prettyref?: string;
  event?: string;
}

export interface ForgejoJobJson {
  id: number;
  name: string;
  status: string;
  html_url: string;
}

export interface ForgejoCommitJson {
  sha: string;
  html_url?: string;
  commit: { message: string; author?: { name?: string; date?: string } };
}

const TIMEOUT_MS = 8_000;
const TTL_MS = 15_000;

// one query asks for the same run lists from many resolvers, and they're large
export class Forgejo {
  readonly #url: string;
  readonly #token: string;
  readonly #fetch: TracedFetch;
  readonly #publicUrl: string;
  readonly #cache = new Map<string, { at: number; value: Promise<unknown> }>();

  constructor(url: string, token: string, publicUrl: string, fetcher: typeof fetch = fetch) {
    this.#url = url.replace(/\/+$/, '');
    this.#token = token;
    this.#publicUrl = publicUrl.replace(/\/+$/, '');
    this.#fetch = tracedFetch(fetcher);
  }

  get publicUrl(): string {
    return this.#publicUrl;
  }

  async #request(path: string, template: string, method = 'GET'): Promise<Response> {
    const response = await this.#fetch(`${this.#url}/api/v1${path}`, {
      method,
      template: `/api/v1${template}`,
      headers: {
        authorization: `token ${this.#token}`,
        accept: 'application/json',
      },
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
    if (!response.ok && response.status !== 404) {
      throw new Error(`forgejo ${path} returned ${response.status}`);
    }
    return response;
  }

  #json<T>(path: string, template: string): Promise<T | undefined> {
    const cached = this.#cache.get(path);
    if (cached && Date.now() - cached.at < TTL_MS) {
      return cached.value as Promise<T | undefined>;
    }

    if (this.#cache.size > 500) {
      for (const [key, entry] of this.#cache) {
        if (Date.now() - entry.at >= TTL_MS) {
          this.#cache.delete(key);
        }
      }
    }

    const value = this.#request(path, template).then((response) =>
      response.status === 404 ? undefined : (response.json() as Promise<T>),
    );
    this.#cache.set(path, { at: Date.now(), value });
    value.catch(() => this.#cache.delete(path));
    return value;
  }

  commit(repository: string, sha: string): Promise<ForgejoCommitJson | undefined> {
    return this.#json(
      `/repos/${repository}/git/commits/${sha}`,
      '/repos/{owner}/{repo}/git/commits/{sha}',
    );
  }

  async runs(repository: string, limit: number): Promise<ForgejoRunJson[]> {
    const page = await this.#json<{ workflow_runs?: ForgejoRunJson[] }>(
      `/repos/${repository}/actions/runs?page=1&limit=${limit}`,
      '/repos/{owner}/{repo}/actions/runs',
    );
    return page?.workflow_runs ?? [];
  }

  run(repository: string, id: number): Promise<ForgejoRunJson | undefined> {
    return this.#json(
      `/repos/${repository}/actions/runs/${id}`,
      '/repos/{owner}/{repo}/actions/runs/{run}',
    );
  }

  async jobs(repository: string, run: number): Promise<ForgejoJobJson[]> {
    return (
      (await this.#json<ForgejoJobJson[]>(
        `/repos/${repository}/actions/runs/${run}/jobs`,
        '/repos/{owner}/{repo}/actions/runs/{run}/jobs',
      )) ?? []
    );
  }

  async act(repository: string, run: number, action: 'rerun' | 'cancel'): Promise<void> {
    const response = await this.#request(
      `/repos/${repository}/actions/runs/${run}/${action}`,
      `/repos/{owner}/{repo}/actions/runs/{run}/${action}`,
      'POST',
    );
    if (!response.ok) {
      throw new Error(`forgejo ${action} of run ${run} returned ${response.status}`);
    }

    for (const key of this.#cache.keys()) {
      if (key.startsWith(`/repos/${repository}/actions/`)) {
        this.#cache.delete(key);
      }
    }
  }

  async logTail(repository: string, job: number, lines: number): Promise<string | undefined> {
    const response = await this.#request(
      `/repos/${repository}/actions/jobs/${job}/logs`,
      '/repos/{owner}/{repo}/actions/jobs/{job}/logs',
    );
    if (response.status === 404) {
      return undefined;
    }
    const all = (await response.text()).replace(/\s+$/, '').split('\n');
    return all.slice(-Math.min(Math.max(lines, 1), 500)).join('\n');
  }
}
