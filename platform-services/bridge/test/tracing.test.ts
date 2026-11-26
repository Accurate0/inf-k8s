import assert from 'node:assert/strict';
import { test } from 'node:test';
import { redactedPath } from '../src/tracing.ts';

test('paths without a template hide ids, opaque tokens and queries', () => {
  assert.equal(
    redactedPath(new URL('https://git/api/v1/repos/anurag/k8s/actions/runs/3705/jobs')),
    '/api/v1/repos/anurag/k8s/actions/runs/{id}/jobs',
  );
  assert.equal(
    redactedPath(new URL('https://git/api/v1/repos/anurag/k8s/git/commits/dc816721ab08025a834c')),
    '/api/v1/repos/anurag/k8s/git/commits/{redacted}',
  );
  assert.equal(
    redactedPath(new URL('https://idm/oauth2/openid/graph/userinfo?x=1')),
    '/oauth2/openid/graph/userinfo?{redacted}',
  );
});
