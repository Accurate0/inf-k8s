import { type Output, type RunOptions, capture, kubeSend, run } from '@ortzi/host';
import {
  DISCOVERY,
  type DiscoveryList,
  type Resource,
  discovered,
  findResource,
} from './discovery';

const WATCH_SECONDS = 30;

export interface Watched {
  metadata: { resourceVersion?: string };
}

interface Status {
  kind?: string;
  message?: string;
}

export class Kube {
  private constructor(
    readonly context: string | undefined,
    private readonly scope: string | undefined,
    private readonly cache: { resources?: Resource[] },
  ) {}

  static connect(context?: string): Kube {
    return new Kube(context, undefined, {});
  }

  namespace(name: string): Kube {
    return new Kube(this.context, name, this.cache);
  }

  private args(rest: string[]): string[] {
    return [
      ...(this.context ? ['--context', this.context] : []),
      ...(this.scope ? ['-n', this.scope] : []),
      ...rest,
    ];
  }

  run(rest: string[], options: RunOptions = {}): Output {
    return run('kubectl', this.args(rest), options);
  }

  capture(rest: string[], options: RunOptions = {}): string {
    return capture('kubectl', this.args(rest), options);
  }

  request<T>(method: string, path: string, body?: unknown, contentType?: string): T {
    const response = kubeSend({
      context: this.context,
      method,
      path,
      headers: {
        Accept: 'application/json',
        ...(body === undefined ? {} : { 'Content-Type': contentType ?? 'application/json' }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });

    if (response.status >= 400) {
      let message = response.body.trim();
      try {
        message = (JSON.parse(response.body) as Status).message ?? message;
      } catch {}
      throw new Error(`${method} ${path}: ${message || response.status}`);
    }

    return JSON.parse(response.body) as T;
  }

  private resources(): Resource[] {
    if (!this.cache.resources) {
      const fetch = (path: string) => {
        const response = kubeSend({
          context: this.context,
          method: 'GET',
          path,
          headers: { Accept: DISCOVERY },
        });
        if (response.status >= 400) {
          throw new Error(`GET ${path}: ${response.status}`);
        }
        return discovered(JSON.parse(response.body) as DiscoveryList);
      };
      this.cache.resources = [...fetch('/api'), ...fetch('/apis')];
    }

    return this.cache.resources;
  }

  resource(name: string): Resource {
    const found = findResource(this.resources(), name);
    if (!found) {
      throw new Error(`the server doesn't have a resource type "${name}"`);
    }
    return found;
  }

  private path(resource: Resource, name?: string): string {
    if (name && resource.namespaced && !this.scope) {
      throw new Error(`${resource.plural} is namespaced; scope it with namespace()`);
    }

    const base = resource.group
      ? `/apis/${resource.group}/${resource.version}`
      : `/api/${resource.version}`;
    const scoped = resource.namespaced && this.scope ? `/namespaces/${this.scope}` : '';
    return `${base}${scoped}/${resource.plural}${name ? `/${name}` : ''}`;
  }

  get<T>(resource: string, name: string): T {
    return this.request<T>('GET', this.path(this.resource(resource), name));
  }

  // unscoped lists cover every namespace; pass a comma-separated list for several types
  list<T>(resources: string): T[] {
    return resources.split(',').flatMap((name) => {
      const resource = this.resource(name);
      const apiVersion = resource.group
        ? `${resource.group}/${resource.version}`
        : resource.version;
      return this.request<{ items: T[] }>('GET', this.path(resource)).items.map((item) => ({
        apiVersion,
        kind: resource.kind,
        ...item,
      }));
    });
  }

  patch<T>(
    resource: string,
    name: string,
    patch: unknown,
    contentType = 'application/merge-patch+json',
  ): T {
    return this.request<T>('PATCH', this.path(this.resource(resource), name), patch, contentType);
  }

  // blocks on short server-side watches instead of sleeping, the host has no sleep
  waitFor<T extends Watched>(
    resource: string,
    name: string,
    done: (object: T) => boolean,
    minutes: number,
  ): boolean {
    const deadline = Date.now() + minutes * 60_000;
    const path = this.path(this.resource(resource));
    let current = this.get<T>(resource, name);

    while (!done(current)) {
      const remaining = Math.ceil((deadline - Date.now()) / 1000);
      if (remaining <= 0) {
        return false;
      }

      const query = new URLSearchParams({
        watch: 'true',
        fieldSelector: `metadata.name=${name}`,
        resourceVersion: current.metadata.resourceVersion ?? '',
        timeoutSeconds: String(Math.min(remaining, WATCH_SECONDS)),
      });
      const response = kubeSend({
        context: this.context,
        method: 'GET',
        path: `${path}?${query}`,
        headers: { Accept: 'application/json' },
      });
      if (response.status >= 400) {
        throw new Error(`GET ${path}: watch failed with ${response.status}`);
      }

      for (const line of response.body.split('\n').filter(Boolean)) {
        const event = JSON.parse(line) as { type: string; object: T };
        if (event.type === 'ERROR') {
          current = this.get<T>(resource, name);
        } else if (event.type === 'DELETED') {
          throw new Error(`${resource}/${name} was deleted while waiting`);
        } else {
          current = event.object;
        }
      }
    }

    return true;
  }

  create<T>(resource: string, body: unknown): T {
    return this.request<T>('POST', this.path(this.resource(resource)), body);
  }
}
