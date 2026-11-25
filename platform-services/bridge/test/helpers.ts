import { generateKeyPairSync, sign } from 'node:crypto';

export const ISSUER = 'https://idm.example.test/oauth2/openid/graph';
export const FORGEJO = 'https://git.example.test/api/actions';

export function keypair(kid: string) {
  const { privateKey, publicKey } = generateKeyPairSync('ec', {
    namedCurve: 'P-256',
  });
  const jwk = {
    ...publicKey.export({ format: 'jwk' }),
    kid,
    alg: 'ES256',
    use: 'sig',
  };
  const token = (claims: Record<string, unknown>) => {
    const header = Buffer.from(JSON.stringify({ alg: 'ES256', kid })).toString('base64url');
    const payload = Buffer.from(
      JSON.stringify({ exp: Math.floor(Date.now() / 1000) + 300, ...claims }),
    ).toString('base64url');
    const signature = sign('sha256', Buffer.from(`${header}.${payload}`), {
      key: privateKey,
      dsaEncoding: 'ieee-p1363',
    }).toString('base64url');
    return `${header}.${payload}.${signature}`;
  };
  return { jwk, token };
}

export type Routes = Record<string, unknown>;

// answers fetch from a path -> json table; strings are sent as text
export function fetcher(routes: Routes): typeof fetch {
  return (async (input: string | URL | Request) => {
    const url = String(input);
    const match = Object.entries(routes).find(([path]) => url.endsWith(path));
    if (!match) {
      return new Response('not found', { status: 404 });
    }
    const [, body] = match;
    return typeof body === 'string'
      ? new Response(body)
      : new Response(JSON.stringify(body), {
          headers: { 'content-type': 'application/json' },
        });
  }) as typeof fetch;
}
