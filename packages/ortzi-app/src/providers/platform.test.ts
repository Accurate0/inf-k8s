import assert from 'node:assert/strict';
import { test } from 'node:test';
import { buildGraph } from '../graph.ts';
import { validate, validateResources } from '../render.ts';
import { platform, render, base, releaseValues, readValues } from '../test-fixtures.ts';

test('a platform resource uses a named export', () => {
  const app = (name: string) =>
    validate(
      {
        ...base,
        resources: [{ name: 'traces', type: 'platform', export: name }],
        workloads: [{ name: 'foo', image: 'i', env: { OTEL_TRACING_URL: { resource: 'traces' } } }],
      },
      'test',
    );

  assert.deepEqual(releaseValues(render(app('tracing')), 'k8s-workload').workloads[0].env, [
    { name: 'OTEL_TRACING_URL', value: 'http://tempo/traces' },
  ]);
  assert.throws(() => validateResources({ platform, apps: [app('nope')] }), /export named nope/);
  assert.deepEqual(buildGraph([app('tracing')]).edges, [{ from: 'foo', to: 'platform/tracing' }]);
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
