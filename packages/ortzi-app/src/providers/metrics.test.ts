import assert from 'node:assert/strict';
import { test } from 'node:test';
import { validate } from '../render.ts';
import { render, base, checkRefs, releaseValues } from '../test-fixtures.ts';

test('an app can scrape several workloads', () => {
  const app = validate(
    {
      ...base,
      resources: [
        { name: 'api', type: 'metrics', workload: 'api', port: 'http', path: '/metrics' },
        { name: 'web', type: 'metrics', workload: 'web', port: 'http', interval: '10s' },
      ],
      workloads: [
        {
          name: 'api',
          image: 'i',
          ports: [{ name: 'http', container: 1, service: 1 }],
          service: { name: 'api' },
        },
        {
          name: 'web',
          image: 'i',
          ports: [{ name: 'http', container: 2, service: 2 }],
          service: { name: 'web' },
        },
      ],
    },
    'test',
  );
  const scrapes = render(app).releases.filter((r) =>
    r.metadata.name.startsWith('foo-k8s-observability'),
  );

  assert.deepEqual(
    scrapes.map((r) => r.metadata.name),
    ['foo-k8s-observability-api', 'foo-k8s-observability-web'],
  );
  assert.deepEqual(
    scrapes.map((r) => r.spec.values),
    [
      { name: 'api', namespace: 'foo', port: 'http', path: '/metrics' },
      { name: 'web', namespace: 'foo', port: 'http', interval: '10s' },
    ],
  );
  assert.deepEqual(releaseValues(render(app), 'k8s-network').ingress, ['monitoring']);
});

test('rejects metrics on a port the service does not have', () => {
  const scrape = (port: string) => ({
    workloads: [
      {
        name: 'foo',
        image: 'i',
        service: { name: 'api' },
        ports: [{ name: 'http', container: 3000, service: 80 }],
      },
    ],
    resources: [{ name: 'm', type: 'metrics', workload: 'foo', port }],
  });
  checkRefs(scrape('http'));

  assert.throws(() => checkRefs(scrape('nope')), /no service port named nope/);
});
