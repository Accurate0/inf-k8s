import assert from 'node:assert/strict';
import { test } from 'node:test';
import { buildArgs, localTag } from './docker.ts';

test('local builds use the commit sha, or mark uncommitted changes with the time', () => {
  const now = new Date('2026-11-19T15:42:07.123Z');
  assert.equal(localTag('abc', false, now), 'abc');
  assert.equal(localTag('abc', true, now), 'abc-dirty-20261119154207');
});

test('local builds label the image the way ci does and push it with the build cache', () => {
  const args = buildArgs(
    {
      ci: 'forgejo',
      image: 'registry.anurag.sh/containers/foo',
      args: { BINARY_NAME: 'foo' },
    },
    'platform-services/foo',
    'abc',
    1700000000,
    'linux/amd64',
  );

  assert.deepEqual(args.slice(0, 6), [
    'buildx',
    'build',
    '--platform',
    'linux/amd64',
    '--file',
    'platform-services/foo/Dockerfile',
  ]);
  assert.ok(args.includes('registry.anurag.sh/containers/foo:abc'));
  assert.ok(args.includes('net.inf-k8s.commit-sha=abc'));
  assert.ok(args.includes('net.inf-k8s.committed-at=1700000000'));
  assert.ok(args.includes('BINARY_NAME=foo'));
  assert.ok(args.includes('type=registry,ref=registry.anurag.sh/containers/foo:buildcache'));
  assert.equal(args.at(-2), '--push');
  assert.equal(args.at(-1), 'platform-services/foo');
});
