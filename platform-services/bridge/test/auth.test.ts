import assert from 'node:assert/strict';
import { test } from 'node:test';
import { AuthError, Authenticator, parseConfig } from '../src/auth.ts';
import { fetcher, JWKS, keypair, passport } from './helpers.ts';

const router = keypair('router-1');
const forger = keypair('router-1');

const auth = () =>
  new Authenticator(
    { audience: 'bridge', jwks: JWKS },
    fetcher({ '/.well-known/passport.jwks': { keys: [router.jwk] } }),
  );

const rejects = async (token: string | undefined, message: RegExp) => {
  await assert.rejects(auth().authenticate(token), (error: unknown) => {
    assert.ok(error instanceof AuthError);
    assert.equal(error.code, 'UNAUTHENTICATED');
    assert.match(error.message, message);
    return true;
  });
};

test('users carry the scopes the router resolved', async () => {
  const principal = await auth().authenticate(router.token(passport(['flux:app:read'])));
  assert.equal(principal.kind, 'user');
  assert.equal(principal.issuer, 'kanidm');
  assert.equal(principal.subject, 'u');
  assert.ok(principal.scopes.has('flux:app:read'));
});

test('external identities carry the scopes their claims were granted', async () => {
  const principal = await auth().authenticate(
    router.token(
      passport([], {
        identity: {
          issuer: 'forgejo',
          kind: 'external',
          token_issuer: 'https://git.example.test/api/actions',
          subject: 'repo:anurag/k8s:ref:refs/heads/main',
          scopes: ['forgejo:run:read'],
          claims: { repository: 'anurag/k8s' },
        },
      }),
    ),
  );
  assert.equal(principal.kind, 'external');
  assert.equal(principal.issuer, 'forgejo');
  assert.deepEqual([...principal.scopes], ['forgejo:run:read']);
});

test('anything but a current passport from the router for this subgraph is rejected', async () => {
  await rejects(undefined, /missing graph passport/);
  await rejects('nope', /malformed passport/);
  await rejects(forger.token(passport(['flux:app:read'])), /bad signature/);
  await rejects(router.token(passport([], { aud: 'registry' })), /wrong audience/);
  await rejects(
    router.token(passport([], { iss: 'https://idm.example.test' })),
    /not from the router/,
  );
  await rejects(router.token(passport([], { exp: 1 })), /expired/);
  await rejects(router.token(passport([], { identity: undefined })), /has no identity/);
});

test('SUBGRAPH_AUTH needs an audience and the router keys', () => {
  assert.deepEqual(parseConfig(JSON.stringify({ audience: 'bridge', jwks: JWKS, grants: {} })), {
    audience: 'bridge',
    jwks: JWKS,
    grants: {},
  });
  assert.throws(() => parseConfig(undefined), /SUBGRAPH_AUTH is not set/);
  assert.throws(
    () => parseConfig(JSON.stringify({ providers: [], scopes: [] })),
    /needs an audience and the router's jwks/,
  );
});
