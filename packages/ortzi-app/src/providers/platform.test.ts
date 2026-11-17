import assert from 'node:assert/strict';
import { test } from 'node:test';
import { buildGraph } from '../graph.ts';
import { validate, validateResources } from '../render.ts';
import { platform, render, source, base } from '../test-fixtures.ts';

test('a platform resource uses a named export', () => {
  const app = (name: string) =>
    validate(
      {
        ...base,
        resources: [{ name: 'argo', type: 'platform', export: name }],
        workloads: [{ name: 'foo', image: 'i', env: { ARGOCD_SERVER: { resource: 'argo' } } }],
      },
      'test',
    );

  assert.deepEqual(
    source(render(app('argocd')), 'k8s-workload')?.helm.valuesObject.workloads[0].env,
    [{ name: 'ARGOCD_SERVER', value: 'http://argocd' }],
  );
  assert.throws(() => validateResources({ platform, apps: [app('nope')] }), /export named nope/);
  assert.deepEqual(buildGraph([app('argocd')]).edges, [{ from: 'foo', to: 'platform/argocd' }]);
});

test('a platform resource needs an export', () => {
  assert.throws(
    () => validate({ ...base, resources: [{ name: 'a', type: 'platform' }] }, 'test'),
    /resources/,
  );
});

test('rejects the old tracing resource type', () => {
  assert.throws(
    () => validate({ ...base, resources: [{ name: 't', type: 'tracing' }] }, 'test'),
    /resources/,
  );
});
