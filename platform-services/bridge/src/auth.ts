import { tracedFetch } from './tracing.ts';
import { createPublicKey, type KeyObject, verify } from 'node:crypto';

export interface Provider {
  issuer: string;
  audience: string;
}

export interface Grant {
  scope: string;
  claims: Record<string, string | string[]>;
}

export interface ExternalAuth {
  name: string;
  issuer: string;
  jwks: string;
  audience: string;
  grants?: Grant[];
}

export interface AuthConfig {
  providers: Provider[];
  scopes: string[];
  groups?: string[];
  external?: ExternalAuth[];
}

export interface Principal {
  kind: 'user' | 'external';
  subject: string;
  issuer: string;
  scopes: Set<string>;
}

export class AuthError extends Error {
  readonly status: number;
  readonly code: string;

  constructor(status: number, code: string, message: string) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

export const unauthorized = (message: string) =>
  new AuthError(401, 'UNAUTHENTICATED', `unauthorized: ${message}`);

export const forbidden = (message: string) =>
  new AuthError(403, 'FORBIDDEN', `forbidden: ${message}`);

const unavailable = (message: string) =>
  new AuthError(503, 'UNAVAILABLE', `jwks error: ${message}`);

export function parseConfig(json: string | undefined): AuthConfig {
  if (!json) {
    throw new Error('SUBGRAPH_AUTH is not set');
  }

  const config = JSON.parse(json) as AuthConfig;
  if (!config.providers?.length || !config.scopes?.length) {
    throw new Error('SUBGRAPH_AUTH needs providers and scopes');
  }
  return config;
}

interface Issuer {
  issuer: string;
  audience: string;
  jwks: string;
  userinfo?: string;
  external?: ExternalAuth;
  keys: Map<string, { key: KeyObject; alg?: string }>;
  refreshedAt?: number;
}

interface Jwt {
  header: { alg?: string; kid?: string };
  claims: Record<string, unknown>;
  signed: string;
  signature: Buffer;
}

const ALGORITHMS: Record<string, { hash: string; ec: boolean }> = {
  ES256: { hash: 'sha256', ec: true },
  ES384: { hash: 'sha384', ec: true },
  RS256: { hash: 'sha256', ec: false },
  RS384: { hash: 'sha384', ec: false },
  RS512: { hash: 'sha512', ec: false },
};

const LEEWAY_SECONDS = 60;
const REFRESH_COOLDOWN_MS = 30_000;
const REFRESH_INTERVAL_MS = 3_600_000;

function decode(token: string): Jwt {
  const parts = token.split('.');
  if (parts.length !== 3) {
    throw unauthorized('malformed token');
  }

  const [header, payload, signature] = parts as [string, string, string];
  try {
    return {
      header: JSON.parse(Buffer.from(header, 'base64url').toString()),
      claims: JSON.parse(Buffer.from(payload, 'base64url').toString()),
      signed: `${header}.${payload}`,
      signature: Buffer.from(signature, 'base64url'),
    };
  } catch {
    throw unauthorized('unreadable token');
  }
}

function glob(pattern: string, value: string): boolean {
  const source = pattern
    .toLowerCase()
    .replace(/[.+^${}()|[\]\\]/g, '\\$&')
    .replace(/\*/g, '.*')
    .replace(/\?/g, '.');
  return new RegExp(`^${source}$`).test(value.toLowerCase());
}

export function matches(claims: Record<string, string | string[]>, token: Record<string, unknown>) {
  return Object.entries(claims).every(([name, allowed]) => {
    const value = token[name];
    return typeof value === 'string' && [allowed].flat().some((pattern) => glob(pattern, value));
  });
}

const groupsMatch = (held: string[], allowed: string[]) =>
  held.some((group) => allowed.some((a) => a === group || a === group.split('@')[0]));

export class Authenticator {
  readonly #config: AuthConfig;
  readonly #issuers: Issuer[];
  readonly #fetch: typeof fetch;

  constructor(config: AuthConfig, fetcher: typeof fetch = fetch) {
    this.#config = config;
    this.#fetch = tracedFetch('auth', fetcher);

    const base = (issuer: string) => issuer.replace(/\/+$/, '');
    this.#issuers = [
      ...config.providers.map((provider) => ({
        issuer: provider.issuer,
        audience: provider.audience,
        jwks: `${base(provider.issuer)}/public_key.jwk`,
        userinfo: `${base(provider.issuer)}/userinfo`,
        keys: new Map(),
      })),
      ...(config.external ?? []).map((external) => ({
        issuer: external.issuer,
        audience: external.audience,
        jwks: external.jwks,
        external,
        keys: new Map(),
      })),
    ];
  }

  async authenticate(header: string | undefined): Promise<Principal> {
    const token = header?.startsWith('Bearer ') ? header.slice(7).trim() : undefined;
    if (!token) {
      throw unauthorized('missing bearer token');
    }

    const jwt = decode(token);
    const issuer = this.#issuers.find((candidate) => candidate.issuer === jwt.claims.iss);
    if (!issuer) {
      throw unauthorized(`untrusted issuer ${String(jwt.claims.iss)}`);
    }

    await this.#verify(issuer, jwt);
    const subject = String(jwt.claims.sub ?? '');

    if (issuer.external) {
      const scopes = new Set(
        (issuer.external.grants ?? [])
          .filter((grant) => matches(grant.claims, jwt.claims))
          .map((grant) => grant.scope),
      );
      if (!scopes.size) {
        throw forbidden(`this ${issuer.external.name} identity is not allowed here`);
      }
      return { kind: 'external', subject, issuer: issuer.issuer, scopes };
    }

    const scopes = new Set(
      typeof jwt.claims.scope === 'string' ? jwt.claims.scope.split(/\s+/).filter(Boolean) : [],
    );
    const scoped = this.#config.scopes.some((scope) => scopes.has(scope));
    const groups = this.#config.groups ?? [];
    if (
      !scoped &&
      !(groups.length && groupsMatch(await this.#groups(issuer, token, jwt), groups))
    ) {
      throw forbidden(
        `${subject} holds none of ${JSON.stringify(this.#config.scopes)} and is in none of ${JSON.stringify(groups)}`,
      );
    }
    return { kind: 'user', subject, issuer: issuer.issuer, scopes };
  }

  async #groups(issuer: Issuer, token: string, jwt: Jwt): Promise<string[]> {
    if (Array.isArray(jwt.claims.groups)) {
      return jwt.claims.groups.map(String);
    }
    if (!issuer.userinfo) {
      return [];
    }

    const response = await this.#fetch(issuer.userinfo, {
      headers: { authorization: `Bearer ${token}` },
    });
    if (!response.ok) {
      throw unauthorized(`userinfo returned ${response.status}`);
    }
    const info = (await response.json()) as { groups?: string[] };
    return info.groups ?? [];
  }

  async #verify(issuer: Issuer, jwt: Jwt): Promise<void> {
    const kid = jwt.header.kid;
    if (!kid) {
      throw unauthorized('token has no kid');
    }

    const now = Date.now();
    const stale =
      issuer.refreshedAt === undefined || now - issuer.refreshedAt > REFRESH_INTERVAL_MS;
    const cooled =
      issuer.refreshedAt === undefined || now - issuer.refreshedAt > REFRESH_COOLDOWN_MS;
    if (stale || (!issuer.keys.has(kid) && cooled)) {
      await this.#refresh(issuer);
    }

    const key = issuer.keys.get(kid);
    if (!key) {
      throw unauthorized(`no key for kid ${kid}`);
    }

    const name = key.alg ?? jwt.header.alg ?? '';
    const algorithm = ALGORITHMS[name];
    if (!algorithm || (jwt.header.alg && jwt.header.alg !== name)) {
      throw unauthorized(`unsupported algorithm ${name}`);
    }

    const valid = verify(
      algorithm.hash,
      Buffer.from(jwt.signed),
      algorithm.ec ? { key: key.key, dsaEncoding: 'ieee-p1363' } : key.key,
      jwt.signature,
    );
    if (!valid) {
      throw unauthorized('token rejected: bad signature');
    }

    const claims = jwt.claims;
    const audiences = [claims.aud].flat();
    if (!audiences.includes(issuer.audience)) {
      throw unauthorized('token rejected: wrong audience');
    }
    if (typeof claims.exp !== 'number' || claims.exp + LEEWAY_SECONDS < now / 1000) {
      throw unauthorized('token rejected: expired');
    }
  }

  async #refresh(issuer: Issuer): Promise<void> {
    issuer.refreshedAt = Date.now();

    let set: { keys?: (JsonWebKey & { kid?: string; alg?: string })[] };
    try {
      const response = await this.#fetch(issuer.jwks);
      if (!response.ok) {
        throw new Error(`${issuer.jwks} returned ${response.status}`);
      }
      set = (await response.json()) as typeof set;
    } catch (error) {
      if (issuer.keys.size) {
        return;
      }
      throw unavailable((error as Error).message);
    }

    const keys = new Map<string, { key: KeyObject; alg?: string }>();
    for (const jwk of set.keys ?? []) {
      if (!jwk.kid || !['EC', 'RSA', 'OKP'].includes(jwk.kty ?? '')) {
        continue;
      }
      try {
        keys.set(jwk.kid, {
          key: createPublicKey({ key: jwk, format: 'jwk' }),
          alg: jwk.alg,
        });
      } catch {
        continue;
      }
    }
    if (keys.size) {
      issuer.keys = keys;
    }
  }
}
