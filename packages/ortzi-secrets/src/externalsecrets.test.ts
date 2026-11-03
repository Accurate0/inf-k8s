import assert from 'node:assert/strict';
import { test } from 'node:test';
import { type ExternalSecret, isReady, matching } from './externalsecrets.ts';

const all: ExternalSecret[] = [
  {
    metadata: { name: 'app-managed', namespace: 'app' },
    spec: {
      secretStoreRef: { name: 'openbao-app', kind: 'SecretStore' },
      dataFrom: [{ extract: { key: 'app' } }],
    },
    status: { conditions: [{ type: 'Ready', status: 'True', reason: 'SecretSynced' }] },
  },
  {
    metadata: { name: 'db-managed', namespace: 'app' },
    spec: {
      secretStoreRef: { name: 'openbao-app-db', kind: 'SecretStore' },
      dataFrom: [{ extract: { key: 'app/app-db' } }],
    },
  },
  {
    metadata: { name: 'azure', namespace: 'app' },
    spec: {
      secretStoreRef: { name: 'k8s-shared-vault', kind: 'ClusterSecretStore' },
      dataFrom: [{ extract: { key: 'app' } }],
    },
  },
  {
    metadata: { name: 'other', namespace: 'other' },
    spec: {
      secretStoreRef: { name: 'openbao-other', kind: 'SecretStore' },
      data: [{ remoteRef: { key: 'app/app-db' } }],
    },
  },
];

const names = (found: ExternalSecret[]) => found.map((es) => es.metadata.name);

test('matches exact paths across namespaces', () => {
  assert.deepEqual(names(matching(all, 'app/app-db')), ['db-managed', 'other']);
});

test('a namespace matches its openbao secrets only', () => {
  assert.deepEqual(names(matching(all, 'app')), ['app-managed', 'db-managed']);
});

test('readiness comes from the ready condition', () => {
  assert.ok(isReady(all[0]));
  assert.ok(!isReady(all[1]));
});
