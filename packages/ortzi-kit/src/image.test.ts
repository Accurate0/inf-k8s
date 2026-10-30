import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parseDigest, parseImage } from './image.ts';

test('splits image references', () => {
  assert.deepEqual(parseImage('ghcr.io/a/b:v1'), { repository: 'ghcr.io/a/b', tag: 'v1' });
  assert.deepEqual(parseImage('localhost:5000/a'), { repository: 'localhost:5000/a', tag: 'latest' });
  assert.deepEqual(parseImage('ghcr.io/a/b:v1@sha256:abc'), { repository: 'ghcr.io/a/b', tag: 'v1' });
});

test('reads the digest header case-insensitively', () => {
  assert.equal(parseDigest('HTTP/2 200\r\nDocker-Content-Digest: sha256:abc\r\n'), 'sha256:abc');
  assert.equal(parseDigest('HTTP/2 404\r\n'), undefined);
});
