import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parseKustomizationImages, parseValuesImages, previousTags } from './images.ts';

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

test('finds the most recent different tags', () => {
  const running = new Map([['api', 'c']]);
  const history = [new Map([['api', 'c']]), new Map([['api', 'b']]), new Map([['api', 'a']])];
  assert.equal(previousTags(running, history)?.get('api'), 'b');
  assert.equal(previousTags(running, [new Map([['api', 'c']])]), undefined);
});

test('reads images from an image values file', () => {
  const text = 'other: 1\nimages:\n  ghcr.io/accurate0/api: abc\n  ghcr.io/accurate0/web: "def"\nlater:\n  nope: x\n';
  assert.deepEqual(
    [...parseValuesImages(text)],
    [
      ['ghcr.io/accurate0/api', 'abc'],
      ['ghcr.io/accurate0/web', 'def'],
    ],
  );
});
