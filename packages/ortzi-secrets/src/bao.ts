import { request } from '@ortzi/kit';
import type { Data } from './env';

export interface Secret {
  data: Data;
  metadata: { version: number; created_time?: string };
}

export interface Version {
  created_time: string;
  deletion_time?: string;
  destroyed?: boolean;
}

export interface Metadata {
  current_version: number;
  versions?: Record<string, Version>;
}

export interface TokenInfo {
  display_name: string;
  policies?: string[];
  ttl: number;
}

interface Body {
  data?: unknown;
  auth?: { client_token: string };
  errors?: string[];
}

const trimSlashes = (path: string) => path.replace(/^\/+|\/+$/g, '');

function failure(status: number, body: string): Error {
  let detail = body;
  try {
    const errors = (JSON.parse(body) as Body).errors;
    if (errors?.length) detail = errors.join('; ');
  } catch {}
  return new Error(`openbao returned ${status}: ${detail.trim()}`);
}

export class Bao {
  private constructor(
    readonly addr: string,
    private readonly mount: string,
    private readonly token: string,
  ) {}

  static fromToken(addr: string, mount: string, token: string): Bao {
    return new Bao(addr.replace(/\/+$/, ''), trimSlashes(mount), token);
  }

  static login(addr: string, mount: string, jwt: string): Bao {
    const base = addr.replace(/\/+$/, '');
    const response = request(`${base}/v1/auth/jwt/login`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ role: 'cli', jwt }),
    });
    if (response.status < 200 || response.status >= 300) {
      throw new Error(
        `openbao jwt login failed: ${failure(response.status, response.body).message}`,
      );
    }

    const token = (JSON.parse(response.body) as Body).auth?.client_token;
    if (!token) throw new Error('openbao jwt login returned no token');
    return Bao.fromToken(base, mount, token);
  }

  private call(method: string, path: string, body?: [string, unknown]): Body | undefined {
    const response = request(`${this.addr}/v1/${path}`, {
      method,
      headers: {
        'X-Vault-Token': this.token,
        ...(body ? { 'Content-Type': body[0] } : {}),
      },
      body: body && JSON.stringify(body[1]),
    });

    if (response.status === 404) return undefined;
    if (response.status === 204) return {};
    if (response.status >= 200 && response.status < 300) return JSON.parse(response.body) as Body;
    throw failure(response.status, response.body);
  }

  private static dataOf<T>(body: Body | undefined, missing: string): T {
    if (!body) throw new Error(missing);
    if (body.data === undefined) throw new Error('openbao response has no data');
    return body.data as T;
  }

  private api(kind: 'data' | 'metadata', path: string): string {
    return `${this.mount}/${kind}/${trimSlashes(path)}`;
  }

  private missing(path: string): string {
    return `no secret at ${this.mount}/${path}`;
  }

  whoami(): TokenInfo {
    return Bao.dataOf(this.call('GET', 'auth/token/lookup-self'), 'token lookup returned nothing');
  }

  list(prefix: string): string[] {
    const body = this.call('LIST', this.api('metadata', prefix).replace(/\/+$/, ''));
    return body ? Bao.dataOf<{ keys: string[] }>(body, '').keys : [];
  }

  get(path: string): Secret | undefined {
    const body = this.call('GET', this.api('data', path));
    return body && Bao.dataOf<Secret>(body, '');
  }

  require(path: string): Secret {
    return Bao.dataOf(this.call('GET', this.api('data', path)), this.missing(path));
  }

  patch(path: string, changes: Record<string, string | null>): number {
    const api = this.api('data', path);
    let written = this.call('PATCH', api, ['application/merge-patch+json', { data: changes }]);
    if (!written) {
      const created = Object.fromEntries(Object.entries(changes).filter(([, v]) => v !== null));
      if (!Object.keys(created).length) throw new Error(this.missing(path));
      written = this.call('POST', api, ['application/json', { data: created }]);
      if (!written) throw new Error('openbao did not create the secret');
    }

    return (written.data as { version?: number } | undefined)?.version ?? 0;
  }

  deleteLatest(path: string): void {
    this.call('DELETE', this.api('data', path));
  }

  metadata(path: string): Metadata {
    return Bao.dataOf(this.call('GET', this.api('metadata', path)), this.missing(path));
  }
}
