import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parse } from 'yaml';
import { buildGraph, toDot, toMermaid } from './graph.ts';
import { renderAppTerraform, renderTerraformIndex } from './terraform.ts';
import { addRoles, type App, appRoles, HEADER, renderDatabase, helmTemplates, isCurrent, renderApplication, renderWorkflow, stamped, validate, validateIngress, validateResources } from './render.ts';

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

const platform = [
  { name: 'tracing', type: 'platform', url: 'http://tempo/traces' },
  { name: 'argocd', type: 'platform', url: 'http://argocd' },
  { name: 'replicated', type: 'cache', url: 'redis://replicated' },
  { name: 'perth', type: 'cache', url: 'redis://perth' },
];

const render = (app: App, t = target, apps: App[] = []) => parse(renderApplication(app, versions, t, { platform, apps }));
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

test('a workload only gets the secrets it lists', () => {
  const rendered = render({
    name: 'foo-bar',
    workloads: {
      'foo-bar': {
        image: 'ghcr.io/accurate0/foo-bar',
        port: 3000,
        envFrom: [{ secret: 'foo-bar' }, { database: 'foo-bar' }],
      },
    },
    resources: [{ name: 'foo-bar', type: 'secret', secretName: 'foo-bar-managed-secrets' }, { name: 'foo-bar', type: 'database', secretName: 'foo-bar-database-secret' }],
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
    resources: [{ name: 'foo', type: 'route', hosts: ['foo.anurag.sh'], auth: { mode: 'jwt', clients: ['foo'] } }, { name: 'foo', type: 'oidc', secretName: 'kanidm-foo-oidc' }, { name: 'scrape', type: 'metrics' }],
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
    resources: [{ name: 'foo', type: 'route', hosts: ['foo.anurag.sh'], workload: 'web', auth: { mode: 'none', reason: 'public' } }, { name: 'foo', type: 'secret', secretName: 'foo-managed-secrets' }],
    network: { ingress: [] },
  });
  const values = source(rendered, 'k8s-workload')?.helm.valuesObject;
  assert.deepEqual(values.workloads.map((w: { name: string }) => w.name), ['api', 'web', 'sync']);
  assert.equal(values.workloads[2].envFrom, undefined);
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
          resources: [{ name: 'foo', type: 'route', hosts: ['foo.anurag.sh'], auth: { mode: 'none', reason: 'public' } }],
          network: { ingress: [] },
        },
        'x',
      ),
    /routes.foo.workload is required/,
  );
});

test('routes to an explicit backend without a workload', () => {
  const rendered = render({
    name: 'foo',
    resources: [{ name: 'foo-route', type: 'route', hosts: ['a.sh', 'b.sh'],
        backend: { name: 'svc', port: 80 },
        paths: ['/v1', { type: 'Exact', value: '/hook' }],
        auth: { mode: 'none', reason: 'public' }, }, { name: 'foo-api', type: 'route', hosts: ['c.sh'], gateway: 'internal-gateway', backend: { name: 'svc', port: 80 } }, { name: 'foo', type: 'oidc', host: 'c.sh', groups: { admins: ['x'] } }, { name: 'foo-cli', type: 'oidc', secretName: 'kanidm-foo-cli-oidc', public: true }],
    sources: [{ repoURL: 'r', name: 'config', path: 'config' }],
    network: { ingress: [] },
  });
  const route = source(rendered, 'k8s-http-routes')?.helm.valuesObject.routes[0];
  assert.equal(route.name, 'foo-route');
  assert.deepEqual(route.hostnames, ['a.sh', 'b.sh']);
  assert.equal(route.rules[0].matches[0].path.value, '/v1');
  assert.deepEqual(route.rules[0].matches[1].path, { type: 'Exact', value: '/hook' });
  assert.equal(source(rendered, 'k8s-http-routes')?.helm.valuesObject.routes[1].name, 'foo-api');
  const client = source(rendered, 'k8s-oidc')?.helm.valuesObject.clients[0];
  assert.equal(client.host, 'c.sh');
  assert.deepEqual(client.groups, { admins: ['x'] });
  const cli = source(rendered, 'k8s-oidc')?.helm.valuesObject.clients[1];
  assert.equal(cli.secretName, 'kanidm-foo-cli-oidc');
  assert.equal(cli.host, 'a.sh');
  assert.equal(rendered.spec.sources.at(-1).name, 'config');
});

test('connection pooling is left to the chart unless set', () => {
  const rendered = render({
    name: 'foo',
    resources: [{ name: 'foo', type: 'database', secretName: 'foo-database-secret' }, { name: 'direct', type: 'database', secretName: 'direct-database-secret', enableConnectionPooling: false }],
    network: { ingress: [] },
  });
  const [pooled, direct] = source(rendered, 'k8s-postgres')?.helm.valuesObject.databases;
  assert.equal('enableConnectionPooling' in pooled, false);
  assert.equal(direct.enableConnectionPooling, false);
});

test('jwt routes pass their clients to the chart', () => {
  const rendered = render({
    name: 'foo',
    resources: [{ name: 'api', type: 'route', hosts: ['a.sh'], backend: { name: 'svc', port: 80 }, auth: { mode: 'jwt', clients: ['foo', 'foo-cli'] } }, { name: 'web', type: 'route', hosts: ['a.sh'], backend: { name: 'svc', port: 80 }, auth: { mode: 'app', reason: 'api keys' } }, { name: 'foo', type: 'oidc', secretName: 'kanidm-foo-oidc' }, { name: 'foo-cli', type: 'oidc', secretName: 'kanidm-foo-cli-oidc' }],
    network: { ingress: [] },
  });
  const [api, web] = source(rendered, 'k8s-http-routes')?.helm.valuesObject.routes;
  assert.deepEqual(api.jwtClients, ['foo', 'foo-cli']);
  assert.equal('jwtClients' in web, false);
});

test('rejects invalid route auth', () => {
  const app = (auth: unknown, client: Record<string, unknown> = {}) => ({
    name: 'foo',
    resources: [
      { name: 'foo', type: 'route', hosts: ['a.sh'], backend: { name: 'svc', port: 80 }, auth },
      { name: 'foo', type: 'oidc', secretName: 'kanidm-foo-oidc', ...client },
    ],
    network: { ingress: [] },
  });
  assert.throws(() => validate(app({ mode: 'jwt', clients: [] }), 'x'), /auth.clients is required/);
  assert.throws(() => validate(app({ mode: 'jwt', clients: ['nope'] }), 'x'), /no oidc client named nope/);
  assert.throws(() => validate(app({ mode: 'oidc', client: 'foo' }), 'x'), /securityPolicy must target this route/);
  assert.throws(() => validate(app({ mode: 'none' }), 'x'), /auth.reason is required/);
  assert.throws(() => validate(app({ mode: 'open' }), 'x'), /auth.mode must be/);
  validate(app({ mode: 'oidc', client: 'foo' }, { securityPolicy: { httpRoute: 'foo' } }), 'x');
});

test('rejects ingress from an unknown namespace', () => {
  const app = (name: string, ingress: string[]) => ({ name, network: { ingress } });
  validateIngress([app('foo', ['bar', 'monitoring']), app('bar', [])]);
  assert.throws(() => validateIngress([app('foo', ['baz'])]), /no app or namespace named baz/);
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
    { name: 'foo', resources: [{ name: 'foo', type: 'secret', secretName: 'foo-managed-secrets' }, { name: 'foo', type: 'database', secretName: 'foo-database-secret' }], network: { ingress: [] } },
    versions,
    target,
    { platform, apps: [] },
  );
  assert.doesNotMatch(output, /[&*]a1/);
});

test('the generated time only matters when content changes', () => {
  const rendered = `${HEADER}\nbody: 1\n`;
  const file = stamped(rendered, new Date('2026-10-31T00:00:00Z'));
  assert.match(file, /^# generated at 2026-10-31T00:00:00.000Z$/m);
  assert.ok(isCurrent(file, rendered));
  assert.ok(!isCurrent(file, `${HEADER}\nbody: 2\n`));
  assert.ok(!isCurrent(rendered, rendered));
});

test('uses the app directory when it has its own kustomization', () => {
  const rendered = render(
    { name: 'foo', network: { ingress: [] } },
    { location: 'platform-services', hasManifests: true, rootKustomization: true },
  );
  assert.equal(rendered.spec.sources.at(-1).path, 'platform-services/foo');
});

test('generates a workflow from the build block', () => {
  const app: App = {
    name: 'foo',
    build: {
      image: 'ghcr.io/accurate0/foo',
      context: '.',
      args: { BINARY_NAME: 'foo' },
      paths: ['crates/x/**'],
      test: { run: 'cargo test', db: 'postgres:17', dbName: 'foo', packages: ['a', 'b'] },
    },
    network: { ingress: [] },
  };
  const workflow = parse(renderWorkflow(app, { location: 'projects', hasManifests: false }));
  const paths = workflow.on.push.paths;
  assert.ok(paths.includes('projects/foo/**'));
  assert.ok(paths.includes('!projects/foo/image.values.yaml'));
  assert.ok(paths.includes('crates/x/**'));
  assert.ok(paths.includes('.github/workflows/app-foo.yaml'));
  const inputs = workflow.jobs.app.with;
  assert.equal(inputs.dockerfile, 'projects/foo/Dockerfile');
  assert.equal(inputs.context, '.');
  assert.equal(inputs['build-args'], 'BINARY_NAME=foo');
  assert.equal(inputs['test-packages'], 'a b');
  assert.equal(workflow.jobs.app.uses, './.github/workflows/app-build.yaml');
});

test('output is stable', () => {
  const app: App = { name: 'foo', network: { ingress: [] } };
  assert.equal(renderApplication(app, versions, target, { platform, apps: [] }), renderApplication(app, versions, target, { platform, apps: [] }));
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
          resources: [{ name: 'foo', type: 'route', hosts: ['foo.anurag.sh'] }],
          network: { ingress: [] },
        },
        'x',
      ),
    /routes.foo.auth is required/,
  );
});

test('templates generated charts from the local charts directory', () => {
  const rendered = renderApplication(
    { name: 'foo', workloads: { foo: { image: 'i' } }, network: { ingress: [] } },
    versions,
    { location: 'projects', hasManifests: false },
    { platform, apps: [] },
  );
  const templates = helmTemplates(rendered);
  assert.deepEqual(templates[0].args, [
    'template', 'foo', 'charts/k8s-workload', '--namespace', 'foo',
    '-f', 'projects/foo/image.values.yaml', '-f', '-',
  ]);
  assert.equal(JSON.parse(templates[1].values).namespace, 'foo');
  assert.deepEqual(templates.map((t) => t.args[2]), ['charts/k8s-workload', 'charts/k8s-network']);
});

test('rejects app.yaml fields outside the schema', () => {
  assert.throws(
    () => validate({ name: 'x', network: { ingress: [] }, resources: [{ name: 'x', type: 'database', secretName: 'x-database-secret', nope: true }] }, 'x'),
    /resources/,
  );
});

const provider = (port: unknown = 'http', extra: Record<string, unknown> = {}) =>
  validate({
    name: 'gw',
    network: { ingress: [] },
    exports: [{ name: 'api', workload: 'gw', port }],
    workloads: { gw: { image: 'i', service: { name: 'api', ports: [{ name: 'http', port: 80 }, { name: 'grpc', port: 50051 }] } } },
    ...extra,
  }, 'test');

const consumer = (resource: Record<string, unknown> = {}) =>
  validate({
    name: 'caller',
    network: { ingress: [] },
    resources: [{ name: 'llm', type: 'app', app: 'gw', export: 'api', ...resource }],
    workloads: { caller: { image: 'i', env: { LLM_URL: { resource: 'llm' } } } },
  }, 'test');

const url = (gw: App, caller: App) =>
  source(render(caller, target, [gw, caller]), 'k8s-workload')?.helm.valuesObject.workloads[0].env[0];

test('an exported workload renders to its service url', () => {
  assert.deepEqual(url(provider(), consumer()), { name: 'LLM_URL', value: 'http://api.gw.svc.cluster.local' });
  assert.equal(url(provider('grpc'), consumer()).value, 'http://api.gw.svc.cluster.local:50051');
  assert.equal(url(provider(), consumer({ path: '/v1' })).value, 'http://api.gw.svc.cluster.local/v1');
});

test('using an export opens the provider to the caller', () => {
  const gw = provider('http', { network: { ingress: ['monitoring', 'caller'] } });
  const caller = consumer();
  assert.deepEqual(source(render(gw, target, [gw, caller]), 'k8s-network')?.helm.valuesObject.ingress, ['monitoring', 'caller']);
  const open = provider();
  assert.deepEqual(source(render(open, target, [open, caller]), 'k8s-network')?.helm.valuesObject.ingress, ['caller']);
  assert.deepEqual(source(render(caller, target, [open, caller]), 'k8s-network')?.helm.valuesObject.ingress, []);
});

test('rejects bad exports', () => {
  assert.throws(() => provider('nope'), /no matching port/);
  assert.throws(
    () => validate({ name: 'x', network: { ingress: [] }, exports: [{ name: 'a', workload: 'nope' }] }, 'test'),
    /no workload named nope/,
  );
  assert.throws(() => validateResources({ platform, apps: [consumer()] }), /no app named gw/);
  assert.throws(() => validateResources({ platform, apps: [consumer({ export: 'nope' }), provider()] }), /no export named nope/);
});

test('a route backend comes from a single service port', () => {
  const app = validate({
    name: 'foo',
    resources: [{ name: 'foo', type: 'route', hosts: ['foo.anurag.sh'], auth: { mode: 'none', reason: 'public' } }],
    workloads: { foo: { image: 'i', service: { name: 'api', ports: [{ name: 'http', port: 80 }] } } },
    network: { ingress: [] },
  }, 'test');
  assert.deepEqual(source(render(app), 'k8s-http-routes')?.helm.valuesObject.routes[0].rules[0].backendRefs, [
    { group: '', kind: 'Service', name: 'api', port: 80, weight: 1 },
  ]);
});

test('rejects a route that cannot pick a service port', () => {
  assert.throws(() => validate({
    name: 'foo',
    resources: [{ name: 'foo', type: 'route', hosts: ['foo.anurag.sh'], auth: { mode: 'none', reason: 'public' } }],
    workloads: { foo: { image: 'i', service: { ports: [{ port: 80 }, { port: 81 }] } } },
    network: { ingress: [] },
  }, 'test'), /several service ports/);
});

test('rejects health without a port', () => {
  assert.throws(
    () => validate({ name: 'foo', workloads: { foo: { image: 'i', health: '/health' } }, network: { ingress: [] } }, 'test'),
    /health needs port/,
  );
});

const short = (workload: Record<string, unknown>, extra: Record<string, unknown> = {}) => {
  const app = validate({ name: 'foo', network: { ingress: [] }, ...extra, workloads: { foo: { image: 'i', ...workload } } }, 'test');
  return source(render(app), 'k8s-workload')?.helm.valuesObject.workloads[0];
};

test('expands the env map', () => {
  const { env } = short(
    {
      env: {
        A: 'b',
        N: 300,
        T: { resource: 'traces' },
        S: { secret: 'foo', key: 'K', optional: true },
        F: { field: 'metadata.name' },
      },
    },
    { resources: [{ name: 'traces', type: 'tracing' }, { name: 'foo', type: 'secret', secretName: 'foo-secret' }] },
  );
  assert.deepEqual(env, [
    { name: 'A', value: 'b' },
    { name: 'N', value: '300' },
    { name: 'T', value: 'http://tempo/traces' },
    { name: 'S', valueFrom: { secretKeyRef: { name: 'foo-secret', key: 'K', optional: true } } },
    { name: 'F', valueFrom: { fieldRef: { fieldPath: 'metadata.name' } } },
  ]);
});

test('keeps raw secretRef entries in envFrom', () => {
  const { envFrom } = short({ envFrom: [{ secretRef: { name: 'a', optional: true } }] });
  assert.deepEqual(envFrom, [{ secretRef: { name: 'a', optional: true } }]);
  assert.throws(() => short({ envFrom: ['a'] }), /envFrom/);
});

test('expands probes around a shared target', () => {
  const out = short({
    probes: {
      http: { path: '/health', port: 3000 },
      readiness: { delay: 5, every: 15 },
      startup: { every: 2, timeout: 2, failures: 30 },
    },
  });
  const httpGet = { path: '/health', port: 3000 };
  assert.deepEqual(out.readinessProbe, { httpGet, initialDelaySeconds: 5, periodSeconds: 15 });
  assert.deepEqual(out.startupProbe, { httpGet, periodSeconds: 2, timeoutSeconds: 2, failureThreshold: 30 });
  assert.equal(out.livenessProbe, undefined);
  assert.equal(out.probes, undefined);
});

test('expands mounts to volumes and volume mounts', () => {
  const out = short({
    mounts: {
      tmp: { path: '/tmp', emptyDir: true },
      rules: { path: '/etc/rules', configMap: 'cm', items: ['config.yaml', 'rules/a.yaml'] },
    },
  });
  assert.deepEqual(out.volumes, [
    { name: 'tmp', emptyDir: {} },
    {
      name: 'rules',
      configMap: { name: 'cm', items: [{ key: 'config.yaml', path: 'config.yaml' }, { key: 'a.yaml', path: 'rules/a.yaml' }] },
    },
  ]);
  assert.deepEqual(out.volumeMounts, [
    { name: 'tmp', mountPath: '/tmp' },
    { name: 'rules', mountPath: '/etc/rules', readOnly: true },
  ]);
});

test('expands flat resources, ports and service protocol', () => {
  const out = short({
    resources: { cpu: '30m', memory: '56Mi', memoryLimit: '128Mi' },
    ports: [3000],
    service: { name: 'api', ports: [{ name: 'http', port: 80, targetPort: 3000 }] },
  });
  assert.deepEqual(out.resources, { requests: { cpu: '30m', memory: '56Mi' }, limits: { memory: '128Mi' } });
  assert.deepEqual(out.ports, [{ containerPort: 3000 }]);
  assert.equal(out.service.ports[0].protocol, 'TCP');
});

test('overrides are written directly to the workload', () => {
  const affinity = { podAntiAffinity: { requiredDuringSchedulingIgnoredDuringExecution: [] } };
  const out = short({ overrides: { affinity, minReadySeconds: 15 } });
  assert.deepEqual(out.affinity, affinity);
  assert.equal(out.minReadySeconds, 15);
  assert.equal(out.overrides, undefined);
});

test('rejects overrides that clash with the workload', () => {
  assert.throws(() => short({ replicas: 2, overrides: { replicas: 3 } }), /overrides.replicas is already set/);
  assert.throws(
    () => short({ probes: { http: { path: '/', port: 1 }, liveness: {} }, overrides: { livenessProbe: {} } }),
    /overrides.livenessProbe is already set/,
  );
  assert.throws(() => short({ port: 1, health: '/', probes: {} }), /health or probes/);
  assert.throws(() => short({ env: { A: { resource: 'nope' } } }), /no resource named nope/);
});

const graphApps = [
  validate({
    name: 'gw',
    network: { ingress: ['caller', 'monitoring', 'other'] },
    exports: [{ name: 'api', workload: 'gw', port: 80 }],
    workloads: { gw: { image: 'i', port: 80 } },
  }, 'test'),
  validate({
    name: 'caller',
    network: { ingress: [] },
    resources: [
      { name: 'llm', type: 'app', app: 'gw', export: 'api' },
      { name: 'traces', type: 'tracing' },
      { name: 'foo-bar', type: 'database', secretName: 'foo-bar-database-secret' },
    ],
  }, 'test'),
  validate({ name: 'other', network: { ingress: [] } }, 'test'),
];

test('graph shows resources, databases and hand written ingress', () => {
  const { edges } = buildGraph(graphApps);
  assert.deepEqual(edges, [
    { from: 'caller', to: 'gw', label: 'api' },
    { from: 'caller', to: 'platform/tracing' },
    { from: 'caller', to: 'database/foo_bar' },
    { from: 'monitoring', to: 'gw', label: 'ingress', dashed: true },
    { from: 'other', to: 'gw', label: 'ingress', dashed: true },
  ]);
});

test('graph can focus on one app', () => {
  const { nodes, edges } = buildGraph(graphApps, 'other');
  assert.deepEqual(nodes.map((n) => n.id).sort(), ['gw', 'other']);
  assert.equal(edges.length, 1);
});

test('graph renders mermaid and dot', () => {
  const graph = buildGraph(graphApps);
  const mermaid = toMermaid(graph);
  assert.match(mermaid, /^graph LR/);
  assert.match(mermaid, /caller -->\|api\| gw/);
  assert.match(mermaid, /other -\.->\|ingress\| gw/);
  assert.match(mermaid, /database_foo_bar\[\("foo_bar"\)\]/);
  const dot = toDot(graph);
  assert.match(dot, /"caller" -> "gw" \[label="api"\];/);
  assert.match(dot, /"other" -> "gw" \[label="ingress", style=dashed\];/);
});

test('rejects bad resources', () => {
  const app = { name: 'foo', network: { ingress: [] }, resources: [{ name: 'a', type: 'tracing' }] };
  assert.throws(() => validate({ ...app, resources: [app.resources[0], app.resources[0]] }, 'test'), /duplicate name a/);
  assert.throws(() => validate({ ...app, resources: [{ name: 'a', type: 'nope' }] }, 'test'), /resources/);
  const traced = validate({ ...app, workloads: { foo: { image: 'i', env: { T: { resource: 'a' } } } } }, 'test');
  assert.throws(() => validateResources({ platform: [], apps: [traced] }), /platform platform export named tracing/);
});

test('rejects unknown workload fields', () => {
  assert.throws(() => short({ uses: ['a'] }), /uses/);
  assert.throws(() => short({ strategy: {} }), /strategy/);
});

test('renders a database owned by its role', () => {
  assert.deepEqual(parse(renderDatabase('foo-bar', {})), {
    apiVersion: 'postgresql.cnpg.io/v1',
    kind: 'Database',
    metadata: { name: 'foo-bar', namespace: 'cnpg-system' },
    spec: {
      name: 'foo_bar',
      owner: 'foo_bar',
      cluster: { name: 'cloudnative-pg-cluster' },
      databaseReclaimPolicy: 'retain',
    },
  });
  assert.equal(parse(renderDatabase('foo', { databaseName: 'bar' })).spec.owner, 'bar');
});

test('only adds missing roles and never removes one', () => {
  const cluster = '# keep\nspec:\n  managed:\n    roles:\n      - name: old\n        login: true\n';
  assert.equal(addRoles(cluster, ['old']), cluster);
  const added = addRoles(cluster, ['new']);
  assert.match(added, /# keep/);
  assert.deepEqual(parse(added).spec.managed.roles, [
    { name: 'old', login: true },
    { name: 'new', ensure: 'present', connectionLimit: -1, inherit: true, login: true },
  ]);
  assert.deepEqual(
    appRoles([{ name: 'a', network: { ingress: [] }, resources: [{ name: 'foo-bar', type: 'database', secretName: 'foo-bar-database-secret' }, { name: 'baz', type: 'database', secretName: 'baz-database-secret', databaseName: 'qux' }] }]),
    ['foo_bar', 'qux'],
  );
});

const base = { name: 'foo', network: { ingress: [] } };

test('terraform covers an app database', () => {
  const { resource } = JSON.parse(renderAppTerraform({ ...base, resources: [{ name: 'foo-bar', type: 'database', secretName: 'foo-bar-database-secret' }] }) as string);
  assert.deepEqual(resource.vault_database_secret_backend_static_role['foo-bar'].username, 'foo_bar');
  assert.equal(resource.vault_policy['foo-bar'].name, 'db-foo-bar');
  assert.deepEqual(resource.vault_kubernetes_auth_backend_role['foo-bar'].bound_service_account_namespaces, ['foo']);
});

test('terraform covers an r2 bucket with a token for that bucket only', () => {
  const { resource } = JSON.parse(
    renderAppTerraform({ ...base, resources: [{ name: 'images', type: 'bucket', secretName: 'images-bucket-secrets', provider: 'r2' }] }) as string,
  );
  const id = 'foo-images';
  assert.equal(resource.cloudflare_r2_bucket[id].name, 'foo-images');
  assert.equal(resource.cloudflare_r2_bucket[id].lifecycle.prevent_destroy, true);
  assert.match(resource.cloudflare_account_token[id].policies[0].resources, /r2\.bucket\..*_default_/);
  assert.equal(resource.vault_kv_secret_v2[id].name, 'foo/resource-images');
  assert.match(resource.vault_kv_secret_v2[id].data_json, /sha256\(cloudflare_account_token\.foo-images\.value\)/);
  assert.equal(resource.aws_s3_bucket, undefined);
});

test('terraform covers an s3 bucket with its own user', () => {
  const { resource } = JSON.parse(
    renderAppTerraform({
      ...base,
      resources: [{ name: 'images', type: 'bucket', secretName: 'images-bucket-secrets', provider: 's3', bucket: 'named' }],
    }) as string,
  );
  const id = 'foo-images';
  assert.equal(resource.aws_s3_bucket[id].bucket, 'named');
  assert.equal(resource.aws_s3_bucket[id].lifecycle.prevent_destroy, true);
  assert.equal(resource.aws_s3_bucket_public_access_block[id].block_public_policy, true);
  assert.match(resource.aws_iam_policy[id].policy, /aws_s3_bucket\.foo-images\.arn/);
  assert.equal(resource.vault_kv_secret_v2[id].name, 'foo/resource-images');
  assert.equal(resource.cloudflare_r2_bucket, undefined);
});

test('an app with nothing for terraform gets no file', () => {
  assert.equal(renderAppTerraform({ ...base, resources: [{ name: 't', type: 'tracing' }] }), undefined);
});

test('the terraform index lists database names sorted', () => {
  const index = JSON.parse(
    renderTerraformIndex([
      { ...base, resources: [{ name: 'z', type: 'database', secretName: 'z-database-secret' }] },
      { ...base, name: 'a', resources: [{ name: 'b', type: 'database', secretName: 'b-database-secret' }] },
    ]),
  );
  assert.deepEqual(index.output.database_names.value, ['b', 'z']);
});

test('a bucket resource wires its name and managed secret', () => {
  const app = validate(
    {
      ...base,
      resources: [{ name: 'images', type: 'bucket', secretName: 'images-bucket-secrets', provider: 'r2' }],
      workloads: { foo: { image: 'i', env: { BUCKET: { resource: 'images' } }, envFrom: [{ bucket: 'images' }] } },
    },
    'test',
  );
  const rendered = render(app);
  assert.deepEqual(source(rendered, 'k8s-workload')?.helm.valuesObject.workloads[0].env, [{ name: 'BUCKET', value: 'foo-images' }]);
  assert.deepEqual(source(rendered, 'k8s-secrets')?.helm.valuesObject.secrets, [
    { name: 'resource-images', managedSecrets: [{ secretName: 'images-bucket-secrets' }] },
  ]);
});

test('an s3 bucket can keep an existing iam user', () => {
  const app: App = { ...base, resources: [{ name: 'storage', type: 'bucket', secretName: 'storage-bucket-secrets', provider: 's3', bucket: 'named', user: 'existing' }] };
  const { resource } = JSON.parse(renderAppTerraform(app) as string);
  assert.equal(resource.aws_iam_user['foo-storage'].name, 'existing');
  assert.equal(resource.aws_iam_policy['foo-storage'].name, 'existing-access');
  assert.equal(resource.aws_iam_policy['foo-storage'].description, 'Access policy for foo application');
  const index = JSON.parse(renderTerraformIndex([app]));
  assert.equal(index.output.s3_access_keys.value['foo-storage'].id, '${aws_iam_access_key.foo-storage.id}');
  assert.equal(index.output.s3_access_keys.sensitive, true);
});

test('the terraform index has no key output without s3 buckets', () => {
  assert.equal(JSON.parse(renderTerraformIndex([base])).output.s3_access_keys, undefined);
});

test('rejects a database resource used in env', () => {
  const app = validate(
    {
      ...base,
      resources: [{ name: 'db', type: 'database', secretName: 'db-database-secret' }],
      workloads: { foo: { image: 'i', env: { DB: { resource: 'db' } } } },
    },
    'test',
  );
  assert.throws(() => validateResources({ platform, apps: [app] }), /in envFrom, not env/);
});

test('rejects the old databases block', () => {
  assert.throws(() => validate({ ...base, databases: { foo: {} } }, 'test'), /unknown block "databases"/);
});

test('a secret can share a name with a database', () => {
  const app = validate(
    {
      ...base,
      resources: [
        { name: 'foo', type: 'secret', secretName: 'foo-managed-secrets' },
        { name: 'foo', type: 'database', secretName: 'foo-database-secret' },
      ],
    },
    'test',
  );
  assert.equal(source(render(app), 'k8s-secrets')?.helm.valuesObject.secrets[0].name, 'foo');
  assert.equal(source(render(app), 'k8s-postgres')?.helm.valuesObject.databases[0].name, 'foo');
});

test('rejects duplicate secrets and a secret used in env', () => {
  const secret = { name: 'foo', type: 'secret', secretName: 'foo-managed-secrets' };
  assert.throws(() => validate({ ...base, resources: [secret, secret] }, 'test'), /duplicate name foo/);
  assert.throws(
    () => validate({ ...base, resources: [secret], workloads: { foo: { image: 'i', env: { A: { resource: 'foo' } } } } }, 'test'),
    /list a secret in envFrom/,
  );
});

test('a cache resource picks its instance', () => {
  const cached = (instance: string) =>
    validate(
      {
        ...base,
        resources: [{ name: 'sessions', type: 'cache', instance }],
        workloads: { foo: { image: 'i', env: { REDIS_URL: { resource: 'sessions' } } } },
      },
      'test',
    );
  assert.deepEqual(source(render(cached('perth')), 'k8s-workload')?.helm.valuesObject.workloads[0].env, [
    { name: 'REDIS_URL', value: 'redis://perth' },
  ]);
  assert.throws(() => validateResources({ platform, apps: [cached('nope')] }), /platform cache export named nope/);
  assert.deepEqual(buildGraph([cached('perth')]).edges, [{ from: 'foo', to: 'cache/perth' }]);
});

test('rejects a cache resource without an instance', () => {
  assert.throws(() => validate({ ...base, resources: [{ name: 'c', type: 'cache' }] }, 'test'), /resources/);
});

test('secret resources default their kubernetes secret name', () => {
  const app = validate(
    {
      ...base,
      resources: [
        { name: 'foo', type: 'secret', secretName: 'foo-managed-secrets' },
        { name: 'other', type: 'secret', secretName: 'custom' },
      ],
    },
    'test',
  );
  assert.deepEqual(source(render(app), 'k8s-secrets')?.helm.valuesObject.secrets.map((s: { managedSecrets: unknown }) => s.managedSecrets), [
    [{ secretName: 'foo-managed-secrets' }],
    [{ secretName: 'custom' }],
  ]);
});

test('rejects the old secrets block', () => {
  assert.throws(() => validate({ ...base, secrets: ['foo'] }, 'test'), /unknown block "secrets"/);
});

test('an app can scrape several workloads', () => {
  const app = validate(
    {
      ...base,
      resources: [
        { name: 'api', type: 'metrics', workload: 'api', path: '/metrics' },
        { name: 'web', type: 'metrics', workload: 'web', interval: '10s' },
      ],
      workloads: { api: { image: 'i', port: 1 }, web: { image: 'i', port: 2 } },
    },
    'test',
  );
  const scrapes = render(app).spec.sources.filter((s: { chart: string }) => s.chart === 'k8s-observability');
  assert.deepEqual(scrapes.map((s: any) => s.helm.valuesObject), [
    { name: 'api', namespace: 'foo', path: '/metrics' },
    { name: 'web', namespace: 'foo', interval: '10s' },
  ]);
  assert.deepEqual(source(render(app), 'k8s-network')?.helm.valuesObject.ingress, ['monitoring']);
});

test('rejects bad route and metrics resources', () => {
  const workloads = { api: { image: 'i', port: 1 } };
  assert.throws(
    () =>
      validate(
        {
          ...base,
          workloads,
          resources: [
            { name: 'a', type: 'metrics', workload: 'api' },
            { name: 'b', type: 'metrics', workload: 'api' },
          ],
        },
        'test',
      ),
    /api is listed twice/,
  );
  assert.throws(
    () => validate({ ...base, resources: [{ name: 'r', type: 'route', hosts: ['a'], backend: { name: 'x', port: 1 } }] }, 'test'),
    /auth is required/,
  );
});

test('workloads reference secrets by resource name', () => {
  const app = validate(
    {
      ...base,
      resources: [
        { name: 'foo', type: 'secret', secretName: 'foo-secret' },
        { name: 'foo', type: 'database', secretName: 'foo-db' },
        { name: 'images', type: 'bucket', provider: 'r2', secretName: 'images-secret' },
      ],
      workloads: {
        foo: {
          image: 'i',
          env: {
            A: { secret: 'foo', key: 'K' },
            B: { bucket: 'images', key: 'S3_ENDPOINT' },
          },
          envFrom: [{ secret: 'foo', optional: true }, { database: 'foo' }, { bucket: 'images', prefix: 'X_' }],
        },
      },
    },
    'test',
  );
  const workload = source(render(app), 'k8s-workload')?.helm.valuesObject.workloads[0];
  assert.deepEqual(workload.envFrom, [
    { secretRef: { name: 'foo-secret', optional: true } },
    { secretRef: { name: 'foo-db' } },
    { prefix: 'X_', secretRef: { name: 'images-secret' } },
  ]);
  assert.deepEqual(workload.env, [
    { name: 'A', valueFrom: { secretKeyRef: { name: 'foo-secret', key: 'K' } } },
    { name: 'B', valueFrom: { secretKeyRef: { name: 'images-secret', key: 'S3_ENDPOINT' } } },
  ]);
  assert.deepEqual(source(render(app), 'k8s-secrets')?.helm.valuesObject.secrets[1].managedSecrets, [{ secretName: 'images-secret' }]);
});

test('rejects references to missing database and bucket resources', () => {
  const missing = (envFrom: unknown[]) =>
    validateResources({ platform, apps: [validate({ ...base, workloads: { foo: { image: 'i', envFrom } } }, 'test')] });
  assert.throws(() => missing([{ database: 'nope' }]), /no database resource named nope/);
  assert.throws(() => missing([{ bucket: 'nope' }]), /no bucket resource named nope/);
});

test('rejects the old routes, oidc and metrics blocks', () => {
  for (const block of ['routes', 'oidc', 'metrics']) {
    assert.throws(() => validate({ ...base, [block]: {} }, 'test'), new RegExp(`unknown block "${block}"`));
  }
});

test('an oidc client secret can be referenced by resource name', () => {
  const app = validate(
    {
      ...base,
      resources: [{ name: 'foo', type: 'oidc', host: 'a.sh', secretName: 'foo-oidc' }],
      workloads: { foo: { image: 'i', env: { AUD: { oidc: 'foo', key: 'client-id', optional: true } } } },
    },
    'test',
  );
  const rendered = render(app);
  assert.deepEqual(source(rendered, 'k8s-workload')?.helm.valuesObject.workloads[0].env, [
    { name: 'AUD', valueFrom: { secretKeyRef: { name: 'foo-oidc', key: 'client-id', optional: true } } },
  ]);
  assert.equal(source(rendered, 'k8s-oidc')?.helm.valuesObject.clients[0].secretName, 'foo-oidc');
});

test('rejects resources without a secret name and unknown references', () => {
  for (const type of ['secret', 'database']) {
    assert.throws(() => validate({ ...base, resources: [{ name: 'a', type }] }, 'test'), /resources/);
  }
  assert.throws(() => validate({ ...base, resources: [{ name: 'a', type: 'oidc', host: 'a.sh' }] }, 'test'), /resources/);
  assert.throws(() => validate({ ...base, resources: [{ name: 'a', type: 'bucket', provider: 'r2' }] }, 'test'), /resources/);
  assert.throws(
    () => validateResources({ platform, apps: [validate({ ...base, workloads: { foo: { image: 'i', envFrom: [{ secret: 'nope' }] } } }, 'test')] }),
    /no secret resource named nope/,
  );
});

test('a platform resource uses a named export', () => {
  const app = (name: string) =>
    validate(
      {
        ...base,
        resources: [{ name: 'argo', type: 'platform', export: name }],
        workloads: { foo: { image: 'i', env: { ARGOCD_SERVER: { resource: 'argo' } } } },
      },
      'test',
    );
  assert.deepEqual(source(render(app('argocd')), 'k8s-workload')?.helm.valuesObject.workloads[0].env, [
    { name: 'ARGOCD_SERVER', value: 'http://argocd' },
  ]);
  assert.throws(() => validateResources({ platform, apps: [app('nope')] }), /export named nope/);
  assert.deepEqual(buildGraph([app('argocd')]).edges, [{ from: 'foo', to: 'platform/argocd' }]);
});

test('a platform resource needs an export', () => {
  assert.throws(() => validate({ ...base, resources: [{ name: 'a', type: 'platform' }] }, 'test'), /resources/);
});
