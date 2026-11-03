import assert from 'node:assert/strict';
import { test } from 'node:test';
import { discovered, findResource } from './discovery.ts';

const resources = discovered({
  items: [
    {
      metadata: { name: '' },
      versions: [
        {
          version: 'v1',
          resources: [
            {
              resource: 'pods',
              singularResource: 'pod',
              shortNames: ['po'],
              scope: 'Namespaced',
              responseKind: { kind: 'Pod' },
            },
          ],
        },
      ],
    },
    {
      metadata: { name: 'external-secrets.io' },
      versions: [
        {
          version: 'v1',
          resources: [
            {
              resource: 'externalsecrets',
              singularResource: 'externalsecret',
              shortNames: ['es'],
              scope: 'Namespaced',
              responseKind: { kind: 'ExternalSecret' },
            },
          ],
        },
        { version: 'v1beta1', resources: [] },
      ],
    },
    {
      metadata: { name: 'apiextensions.k8s.io' },
      versions: [
        {
          version: 'v1',
          resources: [
            {
              resource: 'customresourcedefinitions',
              singularResource: 'customresourcedefinition',
              scope: 'Cluster',
              responseKind: { kind: 'CustomResourceDefinition' },
            },
          ],
        },
      ],
    },
  ],
});

test('resolves plurals, singulars, kinds and short names', () => {
  for (const name of ['pods', 'pod', 'Pod', 'po']) {
    assert.equal(findResource(resources, name)?.plural, 'pods');
  }
  assert.equal(findResource(resources, 'es')?.group, 'external-secrets.io');
});

test('a group suffix picks the group and the preferred version', () => {
  const es = findResource(resources, 'externalsecrets.external-secrets.io');
  assert.deepEqual([es?.group, es?.version, es?.namespaced], ['external-secrets.io', 'v1', true]);
  assert.equal(findResource(resources, 'externalsecrets.example.com'), undefined);
});

test('cluster scoped resources are marked', () => {
  assert.equal(findResource(resources, 'customresourcedefinition')?.namespaced, false);
});
