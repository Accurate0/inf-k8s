import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parseAllDocuments } from 'yaml';
import { renderOutputs } from './index.ts';
import { renderFlux } from './flux.ts';
import { planApp } from '../plan.ts';
import type { App } from '../render.ts';
import { platform, target, versions } from '../test-fixtures.ts';

const artifact = { repo: 'oci://registry.anurag.sh/manifests/foo', tag: 'main' };

const world = {
  platform,
  apps: [],
  readValues: () => ({ replicas: 2 }),
};

const flux = (app: App) => {
  const files = renderFlux(planApp(app, versions, target, world), app.overrides?.flux);
  return (path: string) => {
    const content = files.get(`kubernetes/${app.name}/${path}`);
    assert.ok(content, `${path} not rendered`);
    return parseAllDocuments(content).map((document) => document.toJS());
  };
};

const workload: App = {
  name: 'foo',
  workloads: [{ name: 'foo', image: 'ghcr.io/accurate0/foo' }],
  network: { ingress: [] },
  artifact,
};

test('flux renders an app kustomization over the cluster artifact', () => {
  const [ks] = flux(workload)('ks.yaml');

  assert.equal(ks.metadata.name, 'foo');
  assert.equal(ks.metadata.namespace, 'flux-system');
  assert.equal(ks.metadata.annotations['kustomize.toolkit.fluxcd.io/prune'], 'disabled');
  assert.deepEqual(ks.spec.sourceRef, {
    kind: 'OCIRepository',
    name: 'flux-system',
    namespace: 'flux-system',
  });
  assert.equal(ks.spec.path, './foo/app');
  assert.equal(ks.spec.prune, true);
  assert.equal(ks.spec.wait, true);
});

test('flux renders a helm release per platform chart into the app namespace', () => {
  const files = flux(workload);
  const releases = files('app/helmrelease.yaml');
  const repositories = files('app/ocirepository.yaml');

  assert.deepEqual(
    releases.map((release) => release.metadata.name),
    ['foo-k8s-workload', 'foo-k8s-network'],
  );

  const [release] = releases;
  assert.equal(release.spec.releaseName, 'foo-k8s-workload');
  assert.equal(release.spec.targetNamespace, 'foo');
  assert.equal(release.spec.storageNamespace, 'foo');
  assert.equal(release.spec.install.disableTakeOwnership, undefined);
  assert.equal(release.spec.values.createNamespace, false);
  assert.equal(release.metadata.annotations['event.toolkit.fluxcd.io/app'], 'foo');

  assert.equal(repositories[0].spec.url, 'oci://registry.anurag.sh/charts/k8s-workload');
  assert.deepEqual(repositories[0].spec.ref, { tag: versions['k8s-workload'] });
  assert.deepEqual(repositories[0].spec.secretRef, { name: 'harbor' });
});

test('apps in a shared namespace render beneath it', () => {
  const app = {
    ...workload,
    namespace: { name: 'shared' },
    network: { ingress: [], policy: true },
  };
  const files = renderFlux(planApp(app, versions, target, world));
  const [ks] = parseAllDocuments(files.get('kubernetes/shared/foo/ks.yaml')!).map((d) => d.toJS());

  assert.equal(ks.spec.path, './shared/foo/app');
  assert.ok(files.has('kubernetes/shared/foo/app/helmrelease.yaml'));
});

test('flux lists every rendered file in the app kustomization', () => {
  const [kustomization] = flux(workload)('app/kustomization.yaml');

  assert.deepEqual(kustomization.resources, ['helmrelease.yaml', 'ocirepository.yaml']);
});

test('flux keeps the release name of external charts', () => {
  const files = flux({
    ...workload,
    sources: [
      {
        type: 'helm',
        chart: 'bar',
        repo: 'https://charts.example.com',
        version: '1.0.0',
        values: 'values.yaml',
      },
      {
        type: 'helm',
        chart: 'baz',
        repo: 'oci://ghcr.io/example/charts',
        version: '2.0.0',
        values: 'values.yaml',
        releaseName: 'qux',
      },
    ],
  });
  const releases = files('app/helmrelease.yaml');
  const bar = releases.find((release) => release.metadata.name === 'foo-bar');
  const qux = releases.find((release) => release.metadata.name === 'foo-qux');

  assert.equal(bar.spec.releaseName, 'foo');
  assert.equal(bar.spec.chart.spec.sourceRef.kind, 'HelmRepository');
  assert.deepEqual(bar.spec.values, { replicas: 2 });
  assert.equal(qux.spec.releaseName, 'qux');
  assert.deepEqual(qux.spec.chartRef, {
    kind: 'OCIRepository',
    name: 'foo-qux',
    namespace: 'flux-system',
  });

  const [helmRepository] = files('app/helmrepository.yaml');
  assert.equal(helmRepository.spec.url, 'https://charts.example.com');
  const oci = files('app/ocirepository.yaml').find((r) => r.metadata.name === 'foo-qux');
  assert.equal(oci.spec.url, 'oci://ghcr.io/example/charts/baz');
  assert.equal(oci.spec.secretRef, undefined);
});

test('flux applies manifests from the signed app artifact', () => {
  const files = flux({ ...workload, sources: [{ type: 'manifests', path: '.' }] });
  const repository = files('app/ocirepository.yaml').find(
    (r) => r.metadata.name === 'foo-manifests',
  );
  const [ks] = files('app/ks.yaml');

  assert.equal(repository.spec.url, artifact.repo);
  assert.equal(repository.spec.layerSelector.operation, 'extract');
  assert.equal(repository.spec.verify.provider, 'cosign');
  assert.equal(ks.metadata.name, 'foo-manifests');
  assert.equal(ks.spec.targetNamespace, 'foo');
  assert.equal(ks.spec.sourceRef.name, 'foo-manifests');
});

test('flux overrides add dependencies, timeout and drift ignores', () => {
  const ignore = [{ paths: ['/spec/replicas'], target: { kind: 'Deployment' } }];
  const files = flux({
    ...workload,
    overrides: {
      flux: {
        kustomization: { dependsOn: ['bar'], timeout: '10m' },
        helmRelease: { driftIgnore: ignore },
      },
    },
  });
  const [ks] = files('ks.yaml');

  assert.deepEqual(ks.spec.dependsOn, [{ name: 'bar' }]);
  assert.equal(ks.spec.timeout, '10m');
  for (const release of files('app/helmrelease.yaml')) {
    assert.deepEqual(release.spec.driftDetection, { mode: 'enabled', ignore });
  }
});

test('apps without an artifact render no flux output', () => {
  const app = { ...workload, artifact: undefined };
  const paths = renderOutputs(planApp(app, versions, target, world), app).map(([path]) => path);

  assert.deepEqual(paths, ['projects/foo/application.yaml']);
});
