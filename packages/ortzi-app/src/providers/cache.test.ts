import assert from 'node:assert/strict';
import { test } from 'node:test';
import { buildGraph } from '../graph.ts';
import { type App, containerEnv, validate, validateResources } from '../render.ts';
import { target, platform, render, base, releaseValues, readValues } from '../test-fixtures.ts';

test('a cache resource picks its instance', () => {
  const cached = (instance: string) =>
    validate(
      {
        ...base,
        resources: [{ name: 'sessions', type: 'cache', instance }],
        workloads: [{ name: 'foo', image: 'i', env: { REDIS_URL: { resource: 'sessions' } } }],
      },
      'test',
    );

  assert.deepEqual(releaseValues(render(cached('perth')), 'k8s-workload').workloads[0].env, [
    { name: 'REDIS_URL', value: 'redis://perth' },
  ]);
  assert.throws(
    () => validateResources({ platform, apps: [cached('nope')] }),
    /platform cache export named nope/,
  );
  assert.deepEqual(buildGraph([cached('perth')]).edges, [{ from: 'foo', to: 'cache/perth' }]);
});

test('a cache resource defaults to the cache instance', () => {
  const app = validate(
    {
      ...base,
      resources: [{ name: 'sessions', type: 'cache' }],
      workloads: [{ name: 'foo', image: 'i', env: { REDIS_URL: { resource: 'sessions' } } }],
    },
    'test',
  );

  assert.deepEqual(releaseValues(render(app), 'k8s-workload').workloads[0].env, [
    { name: 'REDIS_URL', value: 'redis://cache' },
  ]);
  assert.deepEqual(buildGraph([app]).edges, [{ from: 'foo', to: 'cache/cache' }]);
});

test('a resource env can take the host or port of its url', () => {
  const provider: App = {
    name: 'gw',
    network: { ingress: [] },
    workloads: [
      {
        name: 'gw',
        image: 'i',
        ports: [{ name: 'api', container: 8080, service: 8080 }],
        service: { name: 'gw' },
      },
    ],
    exports: [{ name: 'api', workload: 'gw', port: 'api' }],
  };
  const app = validate(
    {
      ...base,
      resources: [
        { name: 'cache', type: 'cache' },
        { name: 'gw', type: 'app', app: 'gw', export: 'api', path: '/v1' },
        { name: 'traces', type: 'platform', export: 'tracing' },
      ],
      workloads: [
        {
          name: 'foo',
          image: 'i',
          env: {
            REDIS_HOST: { resource: 'cache', part: 'host' },
            REDIS_PORT: { resource: 'cache', part: 'port' },
            REDIS_URL: { resource: 'cache', part: 'url' },
            GW_HOST: { resource: 'gw', part: 'host' },
            GW_PORT: { resource: 'gw', part: 'port' },
            TRACES_PORT: { resource: 'traces', part: 'port' },
          },
        },
      ],
    },
    'test',
  );

  assert.deepEqual(
    releaseValues(render(app, target, [app, provider]), 'k8s-workload').workloads[0].env,
    [
      { name: 'REDIS_HOST', value: 'cache' },
      { name: 'REDIS_PORT', value: '6379' },
      { name: 'REDIS_URL', value: 'redis://cache' },
      { name: 'GW_HOST', value: 'gw.gw.svc.cluster.local' },
      { name: 'GW_PORT', value: '8080' },
      { name: 'TRACES_PORT', value: '80' },
    ],
  );
  assert.equal(
    containerEnv(app, { platform, apps: [app, provider] })[0].rows[0].from,
    'resource cache host',
  );
  assert.throws(
    () =>
      validate(
        {
          ...base,
          resources: [{ name: 'cache', type: 'cache' }],
          workloads: [{ name: 'foo', image: 'i', env: { X: { resource: 'cache', part: 'path' } } }],
        },
        'test',
      ),
    /workloads\[0\].env is not exactly one/,
  );
});
