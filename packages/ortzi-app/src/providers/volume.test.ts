import assert from 'node:assert/strict';
import { test } from 'node:test';
import { planApp } from '../plan.ts';
import { type App, validate, validateResources } from '../render.ts';
import { versions, target, readValues } from '../test-fixtures.ts';

const volumes = (
  overrides: Partial<Extract<NonNullable<App['resources']>[number], { type: 'volume' }>> = {},
) => ({
  name: 'data',
  type: 'volume' as const,
  claimName: 'foo-data',
  class: 'replicated',
  size: '5Gi',
  access: 'ReadWriteOnce',
  ...overrides,
});

const storage = [
  {
    name: 'replicated',
    type: 'storage',
    storageClass: 'longhorn-replicated',
    backupLabels: { daily: 'enabled' },
  },
  { name: 'nfs', type: 'storage', storageClass: 'nfs' },
];

const withStorage = { ...versions, 'k8s-storage': '0.1.0' };

const volumeWorkload = (mounts: Record<string, unknown>) => ({
  name: 'api',
  image: 'ghcr.io/x/api',
  resources: { memory: '1Mi' },
  mounts,
});

test('volumes render a claim through the storage class export', () => {
  const app = validate(
    {
      name: 'foo',
      network: { ingress: [] },
      resources: [volumes({ backup: 'daily' })],
      workloads: [volumeWorkload({ data: { path: '/data', volume: 'data' } })],
    },
    'x',
  );
  const world = { platform: storage, apps: [app], readValues };
  validateResources(world);
  const sources = planApp(app, withStorage, target, world).sources as {
    chart: string;
    values: { volumes: unknown[]; workloads: { volumes: unknown[] }[] };
  }[];
  const { annotations: _annotations, ...claim } = sources.find((s) => s.chart === 'k8s-storage')
    ?.values.volumes[0] as Record<string, unknown>;

  assert.deepEqual(claim, {
    name: 'foo-data',
    namespace: 'foo',
    storageClass: 'longhorn-replicated',
    size: '5Gi',
    access: 'ReadWriteOnce',
    labels: { daily: 'enabled' },
  });
  assert.deepEqual(sources.find((s) => s.chart === 'k8s-workload')?.values.workloads[0].volumes, [
    { name: 'data', persistentVolumeClaim: { claimName: 'foo-data' } },
  ]);
});

test('volumes are validated', () => {
  const app = (
    volume: ReturnType<typeof volumes>,
    mounts: Record<string, unknown> = { data: { path: '/data', volume: 'data' } },
  ) =>
    validate(
      {
        name: 'foo',
        network: { ingress: [] },
        resources: [volume],
        workloads: [volumeWorkload(mounts)],
      },
      'x',
    );
  const check = (built: App, external = false) =>
    validateResources({
      platform: storage,
      apps: [built],
      ...(external ? { external: new Set(['foo']) } : {}),
    });

  assert.throws(() => check(app(volumes({ class: 'nope' }))), /no storage export named nope/);
  assert.throws(() => check(app(volumes({ class: 'nfs', backup: 'daily' }))), /has no backups/);
  assert.throws(() => check(app(volumes(), {})), /no workload mounts this volume/);
  assert.throws(
    () => check(app(volumes(), { data: { path: '/data', volume: 'other' } }), true),
    /no volume resource named other/,
  );
  check(app(volumes(), {}), true);
});
