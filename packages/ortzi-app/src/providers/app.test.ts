import assert from 'node:assert/strict';
import { test } from 'node:test';
import { type App, validate, validateResources } from '../render.ts';
import { target, platform, render, provider, releaseValues, readValues } from '../test-fixtures.ts';

const consumer = (resource: Record<string, unknown> = {}) =>
  validate(
    {
      name: 'caller',
      network: { ingress: [] },
      resources: [{ name: 'llm', type: 'app', app: 'gw', export: 'api', ...resource }],
      workloads: [{ name: 'caller', image: 'i', env: { LLM_URL: { resource: 'llm' } } }],
    },
    'test',
  );

const url = (gw: App, caller: App) =>
  releaseValues(render(caller, target, [gw, caller]), 'k8s-workload').workloads[0].env[0];

test('an exported workload renders to its service url', () => {
  assert.deepEqual(url(provider(), consumer()), {
    name: 'LLM_URL',
    value: 'http://api.gw.svc.cluster.local',
  });
  assert.equal(url(provider('grpc'), consumer()).value, 'http://api.gw.svc.cluster.local:50051');
  assert.equal(
    url(provider(), consumer({ path: '/v1' })).value,
    'http://api.gw.svc.cluster.local/v1',
  );
});

test('using an export opens the provider to the caller', () => {
  const gw = provider('http', { network: { ingress: ['monitoring', 'caller'] } });
  const caller = consumer();

  assert.deepEqual(releaseValues(render(gw, target, [gw, caller]), 'k8s-network').ingress, [
    'monitoring',
    'caller',
  ]);
  const open = provider();

  assert.deepEqual(releaseValues(render(open, target, [open, caller]), 'k8s-network').ingress, [
    'caller',
  ]);
  assert.deepEqual(
    releaseValues(render(caller, target, [open, caller]), 'k8s-network').ingress,
    [],
  );
});

test('rejects bad exports', () => {
  assert.throws(() => provider('nope'), /no matching port/);
  assert.throws(
    () =>
      validate(
        {
          name: 'x',
          network: { ingress: [] },
          exports: [{ name: 'a', workload: 'nope', port: 'http' }],
        },
        'test',
      ),
    /no workload named nope/,
  );
  assert.throws(() => validateResources({ platform, apps: [consumer()] }), /no app named gw/);
  assert.throws(
    () => validateResources({ platform, apps: [consumer({ export: 'nope' }), provider()] }),
    /no export named nope/,
  );
});
