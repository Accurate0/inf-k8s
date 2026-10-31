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
  const rendered = render({ name: 'foo', network: { ingress: [] } });
  assert.deepEqual(
    rendered.spec.sources.map((s: { chart: string }) => s.chart),
    ['k8s-network'],
  );
  assert.equal(rendered.spec.project, 'projects');
});

test('wires database and secrets into the workload', () => {
  const rendered = render({
    name: 'foo-bar',
    workloads: { 'foo-bar': { image: 'ghcr.io/accurate0/foo-bar', port: 3000 } },
    database: {},
    secrets: ['foo-bar'],
    network: { ingress: [] },
  });
  assert.deepEqual(source(rendered, 'k8s-workload')?.helm.valuesObject.workloads[0].envFrom, [
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
    workloads: { foo: { image: 'i', port: 3000 } },
    route: { hosts: ['foo.anurag.sh'] },
    oidc: {},
    metrics: {},
    network: { ingress: ['ai-gateway', 'monitoring'] },
  });
  assert.deepEqual(source(rendered, 'k8s-network')?.helm.valuesObject.ingress, [
    'envoy-gateway-system',
    'monitoring',
    'ai-gateway',
  ]);
});

test('renders several workloads and routes to the named one', () => {
  const rendered = render({
    name: 'foo',
    workloads: {
      api: { image: 'i', port: 3000, service: { name: 'api-svc' } },
      web: { image: 'j', port: 8080 },
      sync: { image: 'i', kind: 'CronJob', schedule: '0 1 * * *' },
    },
    route: { hosts: ['foo.anurag.sh'], workload: 'web', unauthenticated: true },
    secrets: ['foo'],
    network: { ingress: [] },
  });
  const values = source(rendered, 'k8s-workload')?.helm.valuesObject;
  assert.deepEqual(values.workloads.map((w: { name: string }) => w.name), ['api', 'web', 'sync']);
  assert.equal(values.workloads[2].envFrom.length, 1);
  assert.deepEqual(source(rendered, 'k8s-http-routes')?.helm.valuesObject.routes[0].rules[0].backendRefs, [
    { group: '', kind: 'Service', name: 'web', port: 8080, weight: 1 },
  ]);
});

test('requires route.workload when several workloads have ports', () => {
  assert.throws(
    () =>
      validate(
        {
          name: 'foo',
          workloads: { a: { image: 'i', port: 1 }, b: { image: 'j', port: 2 } },
          route: { hosts: ['foo.anurag.sh'], unauthenticated: true },
          network: { ingress: [] },
        },
        'x',
      ),
    /route.workload is required/,
  );
});

test('a workload can limit which secrets it receives', () => {
  const rendered = render({
    name: 'foo',
    workloads: {
      api: { image: 'i', secrets: ['api'] },
      web: { image: 'j', secrets: [] },
      all: { image: 'k' },
    },
    secrets: ['api', { name: 'web', secretName: 'custom' }],
    network: { ingress: [] },
  });
  const [api, web, all] = source(rendered, 'k8s-workload')?.helm.valuesObject.workloads;
  assert.deepEqual(api.envFrom, [{ secretRef: { name: 'api-managed-secrets' } }]);
  assert.equal(web.envFrom, undefined);
  assert.equal(all.envFrom.length, 2);
  assert.equal(source(rendered, 'k8s-secrets')?.helm.valuesObject.secrets[1].managedSecrets[0].secretName, 'custom');
});

test('routes to an explicit backend without a workload', () => {
  const rendered = render({
    name: 'foo',
    route: {
      hosts: ['a.sh', 'b.sh'],
      name: 'foo-route',
      backend: { name: 'svc', port: 80 },
      paths: ['/v1'],
      unauthenticated: true,
    },
    oidc: { host: 'c.sh', groups: { admins: ['x'] } },
    sources: [{ repoURL: 'r', name: 'config', path: 'config' }],
    network: { ingress: [] },
  });
  const route = source(rendered, 'k8s-http-routes')?.helm.valuesObject.routes[0];
  assert.equal(route.name, 'foo-route');
  assert.deepEqual(route.hostnames, ['a.sh', 'b.sh']);
  assert.equal(route.rules[0].matches[0].path.value, '/v1');
  const client = source(rendered, 'k8s-oidc')?.helm.valuesObject.clients[0];
  assert.equal(client.host, 'c.sh');
  assert.deepEqual(client.groups, { admins: ['x'] });
  assert.equal(rendered.spec.sources.at(-1).name, 'config');
});

test('an app can pin a chart version', () => {
  const rendered = render({
    name: 'foo',
    charts: { 'k8s-network': '0.0.9' },
    network: { ingress: [] },
  });
  assert.equal(source(rendered, 'k8s-network')?.targetRevision, '0.0.9');
});

test('keeps manifests as an optional source', () => {
  const rendered = render(
    { name: 'foo', network: { ingress: [] } },
    { location: 'platform-services', hasManifests: true },
  );
  assert.equal(rendered.spec.sources.at(-1).path, 'platform-services/foo/manifests');
});

test('does not emit yaml anchors', () => {
  const output = renderApplication(
    { name: 'foo', database: {}, secrets: ['foo'], network: { ingress: [] } },
    versions,
    target,
  );
  assert.doesNotMatch(output, /[&*]a1/);
});

test('output is stable', () => {
  const app: App = { name: 'foo', network: { ingress: [] } };
  assert.equal(renderApplication(app, versions, target), renderApplication(app, versions, target));
});

test('rejects invalid apps', () => {
  assert.throws(() => validate({ name: 'foo' }, 'x'), /network.ingress/);
  assert.throws(() => validate({ name: 'Foo', network: { ingress: [] } }, 'x'), /kebab-case/);
  assert.throws(() => validate({ name: 'foo', nope: 1, network: { ingress: [] } }, 'x'), /unknown block/);
  assert.throws(
    () =>
      validate(
        {
          name: 'foo',
          workloads: { foo: { image: 'i', port: 1 } },
          route: { hosts: ['foo.anurag.sh'] },
          network: { ingress: [] },
        },
        'x',
      ),
    /every route authenticates/,
  );
});
