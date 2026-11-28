import { type TracedFetch, tracedFetch } from './tracing.ts';
import { createPublicKey, type KeyObject, verify } from 'node:crypto';

export const PASSPORT_HEADER = 'x-graph-passport';
export const PASSPORT_ISSUER = 'graph-router';

export interface AuthConfig {
  audience: string;
  jwks: string;
}

export interface Principal {
  kind: 'user' | 'external';
  subject: string;
  issuer: string;
  scopes: Set<string>;
}

interface Identity {
  issuer: string;
  kind: 'user' | 'external';
  subject: string;
  scopes?: string[];
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

const unavailable = (message: string) =>
  new AuthError(503, 'UNAVAILABLE', `jwks error: ${message}`);

export function parseConfig(json: string | undefined): AuthConfig {
  if (!json) {
    throw new Error('SUBGRAPH_AUTH is not set');
  }

  const config = JSON.parse(json) as AuthConfig;
  if (!config.audience || !config.jwks) {
    throw new Error("SUBGRAPH_AUTH needs an audience and the router's jwks");
  }
  return config;
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

const LEEWAY_SECONDS = 5;
const REFRESH_COOLDOWN_MS = 30_000;
const REFRESH_INTERVAL_MS = 3_600_000;

function decode(token: string): Jwt {
  const parts = token.split('.');
  if (parts.length !== 3) {
    throw unauthorized('malformed passport');
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
    throw unauthorized('unreadable passport');
  }
}

export class Authenticator {
  readonly #config: AuthConfig;
  readonly #fetch: TracedFetch;
  #keys = new Map<string, { key: KeyObject; alg?: string }>();
  #refreshedAt?: number;

  constructor(config: AuthConfig, fetcher: typeof fetch = fetch) {
    this.#config = config;
    this.#fetch = tracedFetch(fetcher);
  }

  async authenticate(passport: string | undefined): Promise<Principal> {
    if (!passport) {
      throw unauthorized('missing graph passport');
    }

    const jwt = decode(passport);
    await this.#verify(jwt);

    const identity = jwt.claims.identity as Identity | undefined;
    if (!identity || (identity.kind !== 'user' && identity.kind !== 'external')) {
      throw unauthorized('passport has no identity');
    }
    return {
      kind: identity.kind,
      subject: identity.subject,
      issuer: identity.issuer,
      scopes: new Set(identity.scopes ?? []),
    };
  }

  async #verify(jwt: Jwt): Promise<void> {
    const kid = jwt.header.kid;
    if (!kid) {
      throw unauthorized('passport has no kid');
    }

    const now = Date.now();
    const stale = this.#refreshedAt === undefined || now - this.#refreshedAt > REFRESH_INTERVAL_MS;
    const cooled = this.#refreshedAt === undefined || now - this.#refreshedAt > REFRESH_COOLDOWN_MS;
    if (stale || (!this.#keys.has(kid) && cooled)) {
      await this.#refresh();
    }

    const key = this.#keys.get(kid);
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
      throw unauthorized('passport rejected: bad signature');
    }

    const claims = jwt.claims;
    if (claims.iss !== PASSPORT_ISSUER) {
      throw unauthorized('passport rejected: not from the router');
    }
    if (![claims.aud].flat().includes(this.#config.audience)) {
      throw unauthorized('passport rejected: wrong audience');
    }
    if (typeof claims.exp !== 'number' || claims.exp + LEEWAY_SECONDS < now / 1000) {
      throw unauthorized('passport rejected: expired');
    }
  }

  async #refresh(): Promise<void> {
    this.#refreshedAt = Date.now();
    const jwks = this.#config.jwks;

    let set: { keys?: (JsonWebKey & { kid?: string; alg?: string })[] };
    try {
      const response = await this.#fetch(jwks);
      if (!response.ok) {
        throw new Error(`${jwks} returned ${response.status}`);
      }
      set = (await response.json()) as typeof set;
    } catch (error) {
      if (this.#keys.size) {
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
      this.#keys = keys;
    }
  }
}
