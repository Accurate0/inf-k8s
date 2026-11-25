import assert from 'node:assert/strict';
import { test } from 'node:test';
import { AuthError, Authenticator } from '../src/auth.ts';
import { FORGEJO, fetcher, ISSUER, keypair } from './helpers.ts';

const graph = keypair('graph-1');
const forgejo = keypair('forgejo-1');
const forger = keypair('graph-1');

const auth = () =>
  new Authenticator(
    {
      providers: [{ issuer: ISSUER, audience: 'graph' }],
      scopes: ['flux:app:read', 'forgejo:run:read'],
      groups: ['platform_admins'],
      external: [
        {
          name: 'forgejo',
          issuer: FORGEJO,
          jwks: 'https://git.example.test/keys',
          audience: 'graph.example.test',
          grants: [
            {
              scope: 'forgejo:run:read',
              claims: { repository: 'anurag/*', ref: 'refs/heads/main' },
            },
          ],
        },
      ],
    },
    fetcher({
      '/graph/public_key.jwk': { keys: [graph.jwk] },
      '/keys': { keys: [forgejo.jwk] },
      '/graph/userinfo': { groups: ['platform_admins@idm.example.test'] },
    }),
  );

const rejects = async (header: string | undefined, code: string, message: RegExp) => {
  await assert.rejects(auth().authenticate(header), (error: unknown) => {
    assert.ok(error instanceof AuthError);
    assert.equal(error.code, code);
    assert.match(error.message, message);
    return true;
  });
};

test('users are admitted by a scope from the token', async () => {
  const principal = await auth().authenticate(
    `Bearer ${graph.token({ iss: ISSUER, aud: 'graph', sub: 'u', scope: 'openid flux:app:read' })}`,
  );
  assert.equal(principal.kind, 'user');
  assert.ok(principal.scopes.has('flux:app:read'));
});

test('users without a scope fall back to the transition groups from userinfo', async () => {
  const principal = await auth().authenticate(
    `Bearer ${graph.token({ iss: ISSUER, aud: 'graph', sub: 'u', scope: 'openid' })}`,
  );
  assert.equal(principal.scopes.size, 1);
});

test('bad tokens are rejected', async () => {
  await rejects(undefined, 'UNAUTHENTICATED', /missing bearer token/);
  await rejects('Bearer nope', 'UNAUTHENTICATED', /malformed token/);
  await rejects(
    `Bearer ${forger.token({ iss: ISSUER, aud: 'graph', sub: 'u', scope: 'flux:app:read' })}`,
    'UNAUTHENTICATED',
    /bad signature/,
  );
  await rejects(
    `Bearer ${graph.token({ iss: ISSUER, aud: 'other', sub: 'u', scope: 'flux:app:read' })}`,
    'UNAUTHENTICATED',
    /wrong audience/,
  );
  await rejects(
    `Bearer ${graph.token({ iss: ISSUER, aud: 'graph', sub: 'u', exp: 1 })}`,
    'UNAUTHENTICATED',
    /expired/,
  );
  await rejects(
    `Bearer ${graph.token({ iss: 'https://elsewhere', aud: 'graph' })}`,
    'UNAUTHENTICATED',
    /untrusted issuer/,
  );
});

test('forgejo tokens hold the scopes their claims are granted', async () => {
  const main = await auth().authenticate(
    `Bearer ${forgejo.token({ iss: FORGEJO, aud: 'graph.example.test', sub: 'repo', repository: 'anurag/k8s', ref: 'refs/heads/main' })}`,
  );
  assert.equal(main.kind, 'external');
  assert.deepEqual([...main.scopes], ['forgejo:run:read']);

  await rejects(
    `Bearer ${forgejo.token({ iss: FORGEJO, aud: 'graph.example.test', repository: 'anurag/k8s', ref: 'refs/heads/x' })}`,
    'FORBIDDEN',
    /forgejo identity is not allowed here/,
  );
});
