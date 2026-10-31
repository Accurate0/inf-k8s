import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parse } from 'yaml';
import { type App, renderApplication, validate } from './render.ts';

const versions = {
  'k8s-workload': '0.1.0',
  'k8s-network': '0.1.0',
  'k8s-observability': '0.1.0',
  'k8s-http-routes': '2.3.1',
  'k8s-oidc': '0.4.0',
  'k8s-postgres': '1.0.1',
  'k8s-secrets': '0.6.0',
};
const target = { location: 'projects', hasManifests: false };

const render = (app: App, t = target) => parse(renderApplication(app, versions, t));
const source = (rendered: { spec: { sources: Record<string, any>[] } }, chart: string) =>
  rendered.spec.sources.find((s) => s.chart === chart);

test('only enables charts for declared blocks', () => {
  const rendered = render({ name: 'foo', network: { callers: [] } });
  assert.deepEqual(
    rendered.spec.sources.map((s: { chart: string }) => s.chart),
    ['k8s-network'],
  );
  assert.equal(rendered.spec.project, 'projects');
});

test('wires database and secrets into the workload', () => {
  const rendered = render({
    name: 'foo-bar',
    workload: { image: 'ghcr.io/accurate0/foo-bar', port: 3000 },
    database: {},
    secrets: ['foo-bar'],
    network: { callers: [] },
  });
  assert.deepEqual(source(rendered, 'k8s-workload')?.helm.valuesObject.envFrom, [
    { secretRef: { name: 'foo-bar-managed-secrets' } },
    { secretRef: { name: 'foo-bar-database-secret' } },
  ]);
  assert.deepEqual(source(rendered, 'k8s-workload')?.helm.valueFiles, [
    '$values/projects/foo-bar/image.values.yaml',
  ]);
  assert.equal(source(rendered, 'k8s-postgres')?.helm.valuesObject.databases[0].databaseName, 'foo_bar');
});

test('route and metrics open the network to their callers', () => {
  const rendered = render({
    name: 'foo',
    workload: { image: 'i', port: 3000 },
    route: { host: 'foo.anurag.sh' },
    oidc: {},
    metrics: {},
    network: { callers: ['ai-gateway', 'monitoring'] },
  });
  assert.deepEqual(source(rendered, 'k8s-network')?.helm.valuesObject.callers, [
    'envoy-gateway-system',
    'monitoring',
    'ai-gateway',
  ]);
});

test('an app can pin a chart version', () => {
  const rendered = render({
    name: 'foo',
    charts: { 'k8s-network': '0.0.9' },
    network: { callers: [] },
  });
  assert.equal(source(rendered, 'k8s-network')?.targetRevision, '0.0.9');
});

test('keeps manifests as an optional source', () => {
  const rendered = render(
    { name: 'foo', network: { callers: [] } },
    { location: 'platform-services', hasManifests: true },
  );
  assert.equal(rendered.spec.sources.at(-1).path, 'platform-services/foo/manifests');
});

test('does not emit yaml anchors', () => {
  const output = renderApplication(
    { name: 'foo', database: {}, secrets: ['foo'], network: { callers: [] } },
    versions,
    target,
  );
  assert.doesNotMatch(output, /[&*]a1/);
});

test('output is stable', () => {
  const app: App = { name: 'foo', network: { callers: [] } };
  assert.equal(renderApplication(app, versions, target), renderApplication(app, versions, target));
});

test('rejects invalid apps', () => {
  assert.throws(() => validate({ name: 'foo' }, 'x'), /network.callers/);
  assert.throws(() => validate({ name: 'Foo', network: { callers: [] } }, 'x'), /kebab-case/);
  assert.throws(() => validate({ name: 'foo', nope: 1, network: { callers: [] } }, 'x'), /unknown block/);
  assert.throws(
    () =>
      validate(
        {
          name: 'foo',
          workload: { image: 'i', port: 1 },
          route: { host: 'foo.anurag.sh' },
          network: { callers: [] },
        },
        'x',
      ),
    /every route authenticates/,
  );
});
