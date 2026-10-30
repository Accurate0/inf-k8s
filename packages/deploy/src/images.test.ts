import assert from 'node:assert/strict';
import { test } from 'node:test';
import { age, columns, parseDigest, parseImage, parseKustomizationImages, previousTags, short } from './images.ts';

test('reads images from a kustomization', () => {
  const text = `resources:
  - deployment.yaml
commonAnnotations:
  newTag: nope
images:
  - name: ghcr.io/accurate0/api
    newTag: abc
  - name: ghcr.io/accurate0/web
    newTag: "def"
patches:
  - name: ignored
    newTag: nope
`;
  assert.deepEqual(
    [...parseKustomizationImages(text)],
    [
      ['ghcr.io/accurate0/api', 'abc'],
      ['ghcr.io/accurate0/web', 'def'],
    ],
  );
  assert.equal(parseKustomizationImages('resources: []\n').size, 0);
});

test('splits image references', () => {
  assert.deepEqual(parseImage('ghcr.io/a/b:v1'), { repository: 'ghcr.io/a/b', tag: 'v1' });
  assert.deepEqual(parseImage('localhost:5000/a'), { repository: 'localhost:5000/a', tag: 'latest' });
  assert.deepEqual(parseImage('ghcr.io/a/b:v1@sha256:abc'), { repository: 'ghcr.io/a/b', tag: 'v1' });
});

test('reads the digest header case-insensitively', () => {
  assert.equal(parseDigest('HTTP/2 200\r\nDocker-Content-Digest: sha256:abc\r\n'), 'sha256:abc');
  assert.equal(parseDigest('HTTP/2 404\r\n'), undefined);
});

test('shortens only full shas', () => {
  assert.equal(short('a'.repeat(40)), 'a'.repeat(8));
  assert.equal(short('v1.2.3'), 'v1.2.3');
});

test('finds the most recent different tags', () => {
  const running = new Map([['api', 'c']]);
  const history = [new Map([['api', 'c']]), new Map([['api', 'b']]), new Map([['api', 'a']])];
  assert.equal(previousTags(running, history)?.get('api'), 'b');
  assert.equal(previousTags(running, [new Map([['api', 'c']])]), undefined);
});

test('aligns columns', () => {
  assert.equal(columns(['NAME', 'AGE'], [['api', '5h'], ['home-gateway', '3d']]), 'NAME          AGE\napi           5h\nhome-gateway  3d');
});

test('formats ages', () => {
  const start = Date.parse('2026-01-01T00:00:00Z');
  assert.equal(age('2026-01-01T00:00:00Z', start + 5 * 60_000), '5m');
  assert.equal(age('2026-01-01T00:00:00Z', start + 5 * 3_600_000), '5h');
  assert.equal(age('2026-01-01T00:00:00Z', start + 48 * 3_600_000), '2d');
});
