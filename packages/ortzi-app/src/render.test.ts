import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parse } from 'yaml';
import { buildGraph, toDot, toMermaid } from './graph.ts';
import { renderAppTerraform, renderTerraformIndex } from './terraform.ts';
import { renderApplication as renderArgo } from './outputs/argocd.ts';
import { renderOutputs } from './outputs/index.ts';
import { helmTemplates, planApp } from './plan.ts';
import { addRoles, renderDatabase } from './providers/cnpg.ts';
import {
  validateManifestSources,
  type App,
  appRoles,
  HEADER,
  isCurrent,
  renderKustomization,
  renderRbac,
  renderWorkflow,
  stamped,
  validate,
  validateGatewayAccess,
  validateIngress,
  validateResources,
} from './render.ts';

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

const renderApplication = (
  app: App,
  chartVersions: typeof versions,
  on: typeof target,
  world: Parameters<typeof planApp>[3],
) => renderArgo(planApp(app, chartVersions, on, world), app.overrides?.argocd?.application);

const platform = [
  { name: 'pooled', type: 'database', host: 'pooler.db' },
  { name: 'direct', type: 'database', host: 'direct.db' },
  { name: 'tracing', type: 'platform', url: 'http://tempo/traces' },
  { name: 'argocd', type: 'platform', url: 'http://argocd' },
  { name: 'replicated', type: 'cache', url: 'redis://replicated' },
  { name: 'perth', type: 'cache', url: 'redis://perth' },
];

const render = (app: App, t = target, apps: App[] = []) =>
  parse(renderApplication(app, versions, t, { platform, apps }));
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
    workloads: [
      {
        name: 'foo-bar',
        image: 'ghcr.io/accurate0/foo-bar',
        envFrom: [{ secret: 'foo-bar' }, { database: 'foo-bar' }],
      },
    ],
    resources: [
      { name: 'foo-bar', type: 'secret', secretName: 'foo-bar-managed-secrets' },
      { name: 'foo-bar', type: 'database', secretName: 'foo-bar-database-secret' },
    ],
    network: { ingress: [] },
  });

  assert.deepEqual(source(rendered, 'k8s-workload')?.helm.valuesObject.workloads[0].envFrom, [
    { secretRef: { name: 'foo-bar-managed-secrets' } },
    { secretRef: { name: 'foo-bar-database-secret' } },
  ]);
  assert.deepEqual(source(rendered, 'k8s-workload')?.helm.valueFiles, [
    '$values/projects/foo-bar/image.values.yaml',
  ]);
  assert.equal(
    source(rendered, 'k8s-postgres')?.helm.valuesObject.databases[0].databaseName,
    'foo_bar',
  );
});

test('route and metrics open the network to their callers', () => {
  const rendered = render({
    name: 'foo',
    workloads: [
      {
        name: 'foo',
        image: 'i',
        ports: [{ name: 'http', container: 3000, service: 3000 }],
        service: { name: 'foo' },
      },
    ],
    resources: [
      {
        name: 'foo',
        type: 'route',
        hosts: ['foo.anurag.sh'],
        backend: { name: 'foo', port: 3000 },
        auth: { mode: 'jwt', clients: ['foo'] },
      },
      { name: 'foo', type: 'oidc', host: 'foo.anurag.sh', secretName: 'kanidm-foo-oidc' },
      { name: 'scrape', type: 'metrics', workload: 'foo', port: 'http' },
    ],
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
    workloads: [
      {
        name: 'api',
        image: 'i',
        ports: [{ name: 'http', container: 3000, service: 3000 }],
        service: { name: 'api-svc' },
      },
      {
        name: 'web',
        image: 'j',
        ports: [{ name: 'http', container: 8080, service: 8080 }],
        service: { name: 'web' },
      },
      { name: 'sync', image: 'i', kind: 'CronJob', schedule: '0 1 * * *' },
    ],
    resources: [
      {
        name: 'foo',
        type: 'route',
        hosts: ['foo.anurag.sh'],
        backend: { name: 'web', port: 8080 },
        auth: { mode: 'none', reason: 'public' },
      },
      { name: 'foo', type: 'secret', secretName: 'foo-managed-secrets' },
    ],
    network: { ingress: [] },
  });
  const values = source(rendered, 'k8s-workload')?.helm.valuesObject;

  assert.deepEqual(
    values.workloads.map((w: { name: string }) => w.name),
    ['api', 'web', 'sync'],
  );
  assert.equal(values.workloads[2].envFrom, undefined);
  assert.deepEqual(
    source(rendered, 'k8s-http-routes')?.helm.valuesObject.routes[0].rules[0].backendRefs,
    [{ group: '', kind: 'Service', name: 'web', port: 8080, weight: 1 }],
  );
});

test('routes to an explicit backend without a workload', () => {
  const rendered = render({
    name: 'foo',
    resources: [
      {
        name: 'foo-route',
        type: 'route',
        hosts: ['a.sh', 'b.sh'],
        backend: { name: 'svc', port: 80 },
        paths: ['/v1', { type: 'Exact', value: '/hook' }],
        auth: { mode: 'none', reason: 'public' },
      },
      {
        name: 'foo-api',
        type: 'route',
        hosts: ['c.sh'],
        gateway: 'internal-gateway',
        backend: { name: 'svc', port: 80 },
      },
      { name: 'foo', type: 'oidc', host: 'c.sh', groups: { admins: ['x'] } },
      {
        name: 'foo-cli',
        type: 'oidc',
        host: 'a.sh',
        secretName: 'kanidm-foo-cli-oidc',
        public: true,
      },
    ],
    sources: [{ type: 'git', repo: 'r', name: 'config', path: 'config' }],
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

test('database hosts come from platform exports', () => {
  const rendered = render({
    name: 'foo',
    resources: [
      { name: 'foo', type: 'database', secretName: 'foo-database-secret' },
      {
        name: 'direct',
        type: 'database',
        secretName: 'direct-database-secret',
        enableConnectionPooling: false,
      },
    ],
    network: { ingress: [] },
  });
  const [pooled, direct] = source(rendered, 'k8s-postgres')?.helm.valuesObject.databases;

  assert.equal(pooled.host, 'pooler.db');
  assert.equal(direct.host, 'direct.db');
});

test('jwt routes pass their clients to the chart', () => {
  const rendered = render({
    name: 'foo',
    resources: [
      {
        name: 'api',
        type: 'route',
        hosts: ['a.sh'],
        backend: { name: 'svc', port: 80 },
        auth: { mode: 'jwt', clients: ['foo', 'foo-cli'] },
      },
      {
        name: 'web',
        type: 'route',
        hosts: ['a.sh'],
        backend: { name: 'svc', port: 80 },
        auth: { mode: 'app', reason: 'api keys' },
      },
      { name: 'foo', type: 'oidc', host: 'a.sh', secretName: 'kanidm-foo-oidc' },
      { name: 'foo-cli', type: 'oidc', host: 'a.sh', secretName: 'kanidm-foo-cli-oidc' },
    ],
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
      { name: 'foo', type: 'oidc', host: 'a.sh', secretName: 'kanidm-foo-oidc', ...client },
    ],
    network: { ingress: [] },
  });

  assert.throws(() => validate(app({ mode: 'jwt', clients: [] }), 'x'), /auth.clients is required/);
  assert.throws(
    () => validate(app({ mode: 'jwt', clients: ['nope'] }), 'x'),
    /no oidc client named nope/,
  );
  assert.throws(
    () => validate(app({ mode: 'oidc', client: 'nope' }), 'x'),
    /no oidc client named nope/,
  );
  assert.throws(() => validate(app({ mode: 'none' }), 'x'), /auth.reason is required/);
  assert.throws(() => validate(app({ mode: 'open' }), 'x'), /auth.mode must be/);
  validate(app({ mode: 'oidc', client: 'foo' }), 'x');

  assert.throws(
    () => validate(app({ mode: 'oidc', client: 'foo' }, { securityPolicy: {} }), 'x'),
    /resources/,
  );
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
    { name: 'foo', network: { ingress: [] }, sources: [{ type: 'manifests', path: 'manifests' }] },
    { location: 'platform-services', hasManifests: true },
  );

  assert.equal(rendered.spec.sources.at(-1).path, 'platform-services/foo/manifests');
});

test('does not emit yaml anchors', () => {
  const output = renderApplication(
    {
      name: 'foo',
      resources: [
        { name: 'foo', type: 'secret', secretName: 'foo-managed-secrets' },
        { name: 'foo', type: 'database', secretName: 'foo-database-secret' },
      ],
      network: { ingress: [] },
    },
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
    { name: 'foo', network: { ingress: [] }, sources: [{ type: 'manifests', path: '.' }] },
    { location: 'platform-services', hasManifests: true },
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

  assert.equal(
    renderApplication(app, versions, target, { platform, apps: [] }),
    renderApplication(app, versions, target, { platform, apps: [] }),
  );
});

test('rejects invalid apps', () => {
  assert.throws(() => validate({ name: 'foo' }, 'x'), /network.ingress/);
  assert.throws(() => validate({ name: 'Foo', network: { ingress: [] } }, 'x'), /kebab-case/);
  assert.throws(
    () => validate({ name: 'foo', nope: 1, network: { ingress: [] } }, 'x'),
    /unknown block/,
  );
  assert.throws(
    () =>
      validate(
        {
          name: 'foo',
          workloads: [
            {
              name: 'foo',
              image: 'i',
              ports: [{ name: 'http', container: 1, service: 1 }],
              service: { name: 'foo' },
            },
          ],
          resources: [
            {
              name: 'foo',
              type: 'route',
              hosts: ['foo.anurag.sh'],
              backend: { name: 'foo', port: 1 },
            },
          ],
          network: { ingress: [] },
        },
        'x',
      ),
    /routes.foo.auth is required/,
  );
});

test('templates generated charts from the local charts directory', () => {
  const plan = planApp(
    { name: 'foo', workloads: [{ name: 'foo', image: 'i' }], network: { ingress: [] } },
    versions,
    { location: 'projects', hasManifests: false },
    { platform, apps: [] },
  );
  const templates = helmTemplates(plan);

  assert.deepEqual(templates[0].args, [
    'template',
    'foo',
    'charts/k8s-workload',
    '--namespace',
    'foo',
    '-f',
    'projects/foo/image.values.yaml',
    '-f',
    '-',
  ]);
  assert.equal(JSON.parse(templates[1].values).namespace, 'foo');
  assert.deepEqual(
    templates.map((t) => t.args[2]),
    ['charts/k8s-workload', 'charts/k8s-network'],
  );
});

test('rejects app.yaml fields outside the schema', () => {
  assert.throws(
    () =>
      validate(
        {
          name: 'x',
          network: { ingress: [] },
          resources: [{ name: 'x', type: 'database', secretName: 'x-database-secret', nope: true }],
        },
        'x',
      ),
    /resources/,
  );
});

const provider = (port: unknown = 'http', extra: Record<string, unknown> = {}) =>
  validate(
    {
      name: 'gw',
      network: { ingress: [] },
      exports: [{ name: 'api', workload: 'gw', port }],
      workloads: [
        {
          name: 'gw',
          image: 'i',
          service: { name: 'api' },
          ports: [
            { name: 'http', container: 3000, service: 80 },
            { name: 'grpc', container: 50051, service: 50051 },
          ],
        },
      ],
      ...extra,
    },
    'test',
  );

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
  source(render(caller, target, [gw, caller]), 'k8s-workload')?.helm.valuesObject.workloads[0]
    .env[0];

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

  assert.deepEqual(
    source(render(gw, target, [gw, caller]), 'k8s-network')?.helm.valuesObject.ingress,
    ['monitoring', 'caller'],
  );
  const open = provider();

  assert.deepEqual(
    source(render(open, target, [open, caller]), 'k8s-network')?.helm.valuesObject.ingress,
    ['caller'],
  );
  assert.deepEqual(
    source(render(caller, target, [open, caller]), 'k8s-network')?.helm.valuesObject.ingress,
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

const short = (workload: Record<string, unknown>, extra: Record<string, unknown> = {}) => {
  const app = validate(
    {
      name: 'foo',
      network: { ingress: [] },
      ...extra,
      workloads: [{ name: 'foo', image: 'i', ...workload }],
    },
    'test',
  );
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
    {
      resources: [
        { name: 'traces', type: 'platform', export: 'tracing' },
        { name: 'foo', type: 'secret', secretName: 'foo-secret' },
      ],
    },
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
  assert.deepEqual(out.startupProbe, {
    httpGet,
    periodSeconds: 2,
    timeoutSeconds: 2,
    failureThreshold: 30,
  });
  assert.equal(out.livenessProbe, undefined);
  assert.equal(out.probes, undefined);
});

test('expands mounts to volumes and volume mounts', () => {
  const out = short({
    mounts: {
      tmp: { path: '/tmp', emptyDir: true },
      rules: { path: '/etc/rules', configMap: 'cm' },
    },
  });

  assert.deepEqual(out.volumes, [
    { name: 'tmp', emptyDir: {} },
    {
      name: 'rules',
      configMap: { name: 'cm' },
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
    ports: [{ name: 'http', container: 3000, service: 80 }],
    service: { name: 'api' },
  });

  assert.deepEqual(out.resources, {
    requests: { cpu: '30m', memory: '56Mi' },
    limits: { memory: '128Mi' },
  });
  assert.deepEqual(out.ports, [{ name: 'http', containerPort: 3000 }]);
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
  assert.throws(
    () => short({ replicas: 2, overrides: { replicas: 3 } }),
    /overrides.replicas is already set/,
  );
  assert.throws(
    () =>
      short({
        probes: { http: { path: '/', port: 1 }, liveness: {} },
        overrides: { livenessProbe: {} },
      }),
    /overrides.livenessProbe is already set/,
  );
  assert.throws(() => short({ health: '/' }), /health/);
  assert.throws(() => short({ env: { A: { resource: 'nope' } } }), /no resource named nope/);
  assert.throws(
    () => short({ ports: [{ name: 'http', container: 1 }] }),
    /service.name is required with ports/,
  );
  assert.throws(() => short({ ports: [3000], service: { name: 'a' } }), /ports/);
});

const graphApps = [
  validate(
    {
      name: 'gw',
      network: { ingress: ['caller', 'monitoring', 'other'] },
      exports: [{ name: 'api', workload: 'gw', port: 80 }],
      workloads: [
        {
          name: 'gw',
          image: 'i',
          ports: [{ name: 'http', container: 80, service: 80 }],
          service: { name: 'gw' },
        },
      ],
    },
    'test',
  ),
  validate(
    {
      name: 'caller',
      network: { ingress: [] },
      resources: [
        { name: 'llm', type: 'app', app: 'gw', export: 'api' },
        { name: 'traces', type: 'platform', export: 'tracing' },
        { name: 'foo-bar', type: 'database', secretName: 'foo-bar-database-secret' },
      ],
    },
    'test',
  ),
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
  const app = {
    name: 'foo',
    network: { ingress: [] },
    resources: [{ name: 'a', type: 'platform', export: 'tracing' }],
  };

  assert.throws(
    () => validate({ ...app, resources: [app.resources[0], app.resources[0]] }, 'test'),
    /duplicate name a/,
  );
  assert.throws(
    () => validate({ ...app, resources: [{ name: 'a', type: 'nope' }] }, 'test'),
    /resources/,
  );
  const traced = validate(
    { ...app, workloads: [{ name: 'foo', image: 'i', env: { T: { resource: 'a' } } }] },
    'test',
  );

  assert.throws(
    () => validateResources({ platform: [], apps: [traced] }),
    /platform platform export named tracing/,
  );
});

test('rejects unknown workload fields', () => {
  assert.throws(() => short({ uses: ['a'] }), /uses/);
  assert.throws(() => short({ strategy: {} }), /strategy/);
});

test('renders a database owned by its role', () => {
  assert.deepEqual(parse(renderDatabase('foo-bar')), {
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
    appRoles([
      {
        name: 'a',
        network: { ingress: [] },
        resources: [
          { name: 'foo-bar', type: 'database', secretName: 'foo-bar-database-secret' },
          { name: 'baz', type: 'database', secretName: 'baz-database-secret' },
        ],
      },
    ]),
    ['foo_bar', 'baz'],
  );
});

const base = { name: 'foo', network: { ingress: [] } };

test('terraform covers an app database', () => {
  const { resource } = JSON.parse(
    renderAppTerraform({
      ...base,
      resources: [{ name: 'foo-bar', type: 'database', secretName: 'foo-bar-database-secret' }],
    }) as string,
  );

  assert.deepEqual(
    resource.vault_database_secret_backend_static_role['foo-bar'].username,
    'foo_bar',
  );
  assert.equal(resource.vault_policy['foo-bar'].name, 'db-foo-bar');
  assert.deepEqual(
    resource.vault_kubernetes_auth_backend_role['foo-bar'].bound_service_account_namespaces,
    ['foo'],
  );
});

test('terraform covers an r2 bucket with a token for that bucket only', () => {
  const { resource } = JSON.parse(
    renderAppTerraform({
      ...base,
      resources: [
        {
          name: 'images',
          type: 'bucket',
          bucket: 'foo-images',
          secretName: 'images-bucket-secrets',
          provider: 'r2',
        },
      ],
    }) as string,
  );
  const id = 'foo-images';

  assert.equal(resource.cloudflare_r2_bucket[id].name, 'foo-images');
  assert.equal(resource.cloudflare_r2_bucket[id].lifecycle.prevent_destroy, true);
  assert.match(
    resource.cloudflare_account_token[id].policies[0].resources,
    /r2\.bucket\..*_default_/,
  );
  assert.equal(resource.vault_kv_secret_v2[id].name, 'foo/resource-images');
  assert.match(
    resource.vault_kv_secret_v2[id].data_json,
    /sha256\(cloudflare_account_token\.foo-images\.value\)/,
  );
  assert.equal(resource.aws_s3_bucket, undefined);
});

test('terraform covers an s3 bucket with its own user', () => {
  const { resource } = JSON.parse(
    renderAppTerraform({
      ...base,
      resources: [
        {
          name: 'images',
          type: 'bucket',
          secretName: 'images-bucket-secrets',
          provider: 's3',
          bucket: 'named',
        },
      ],
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
  assert.equal(
    renderAppTerraform({
      ...base,
      resources: [{ name: 't', type: 'platform', export: 'tracing' }],
    }),
    undefined,
  );
});

test('the terraform index lists database names sorted', () => {
  const index = JSON.parse(
    renderTerraformIndex([
      { ...base, resources: [{ name: 'z', type: 'database', secretName: 'z-database-secret' }] },
      {
        ...base,
        name: 'a',
        resources: [{ name: 'b', type: 'database', secretName: 'b-database-secret' }],
      },
    ]),
  );

  assert.deepEqual(index.output.database_names.value, ['b', 'z']);
});

test('a bucket resource wires its name and managed secret', () => {
  const app = validate(
    {
      ...base,
      resources: [
        {
          name: 'images',
          type: 'bucket',
          bucket: 'foo-images',
          secretName: 'images-bucket-secrets',
          provider: 'r2',
        },
      ],
      workloads: [
        {
          name: 'foo',
          image: 'i',
          env: { BUCKET: { bucket: 'images', key: 'S3_BUCKET' } },
          envFrom: [{ bucket: 'images' }],
        },
      ],
    },
    'test',
  );
  const rendered = render(app);

  assert.deepEqual(source(rendered, 'k8s-workload')?.helm.valuesObject.workloads[0].env, [
    {
      name: 'BUCKET',
      valueFrom: { secretKeyRef: { name: 'images-bucket-secrets', key: 'S3_BUCKET' } },
    },
  ]);
  assert.deepEqual(source(rendered, 'k8s-secrets')?.helm.valuesObject.secrets, [
    { name: 'resource-images', managedSecrets: [{ secretName: 'images-bucket-secrets' }] },
  ]);
});

test('an s3 bucket can keep an existing iam user', () => {
  const app: App = {
    ...base,
    resources: [
      {
        name: 'storage',
        type: 'bucket',
        secretName: 'storage-bucket-secrets',
        provider: 's3',
        bucket: 'named',
        user: 'existing',
      },
    ],
  };
  const { resource } = JSON.parse(renderAppTerraform(app) as string);

  assert.equal(resource.aws_iam_user['foo-storage'].name, 'existing');
  assert.equal(resource.aws_iam_policy['foo-storage'].name, 'existing-access');
  assert.equal(
    resource.aws_iam_policy['foo-storage'].description,
    'Access policy for foo application',
  );
  const index = JSON.parse(renderTerraformIndex([app]));

  assert.equal(
    index.output.s3_access_keys.value['foo-storage'].id,
    '${aws_iam_access_key.foo-storage.id}',
  );
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
      workloads: [{ name: 'foo', image: 'i', env: { DB: { resource: 'db' } } }],
    },
    'test',
  );

  assert.throws(() => validateResources({ platform, apps: [app] }), /in envFrom, not env/);
});

test('rejects the old databases block', () => {
  assert.throws(
    () => validate({ ...base, databases: { foo: {} } }, 'test'),
    /unknown block "databases"/,
  );
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

  assert.throws(
    () => validate({ ...base, resources: [secret, secret] }, 'test'),
    /duplicate name foo/,
  );
  assert.throws(
    () =>
      validate(
        {
          ...base,
          resources: [secret],
          workloads: [{ name: 'foo', image: 'i', env: { A: { resource: 'foo' } } }],
        },
        'test',
      ),
    /list a secret in envFrom/,
  );
});

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

  assert.deepEqual(
    source(render(cached('perth')), 'k8s-workload')?.helm.valuesObject.workloads[0].env,
    [{ name: 'REDIS_URL', value: 'redis://perth' }],
  );
  assert.throws(
    () => validateResources({ platform, apps: [cached('nope')] }),
    /platform cache export named nope/,
  );
  assert.deepEqual(buildGraph([cached('perth')]).edges, [{ from: 'foo', to: 'cache/perth' }]);
});

test('rejects a cache resource without an instance', () => {
  assert.throws(
    () => validate({ ...base, resources: [{ name: 'c', type: 'cache' }] }, 'test'),
    /resources/,
  );
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

  assert.deepEqual(
    source(render(app), 'k8s-secrets')?.helm.valuesObject.secrets.map(
      (s: { managedSecrets: unknown }) => s.managedSecrets,
    ),
    [[{ secretName: 'foo-managed-secrets' }], [{ secretName: 'custom' }]],
  );
});

test('rejects the old secrets block', () => {
  assert.throws(() => validate({ ...base, secrets: ['foo'] }, 'test'), /unknown block "secrets"/);
});

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
  const scrapes = render(app).spec.sources.filter(
    (s: { chart: string }) => s.chart === 'k8s-observability',
  );

  assert.deepEqual(
    scrapes.map((s: any) => s.helm.valuesObject),
    [
      { name: 'api', namespace: 'foo', port: 'http', path: '/metrics' },
      { name: 'web', namespace: 'foo', port: 'http', interval: '10s' },
    ],
  );
  assert.deepEqual(source(render(app), 'k8s-network')?.helm.valuesObject.ingress, ['monitoring']);
});

test('rejects bad route and metrics resources', () => {
  const workloads = [
    {
      name: 'api',
      image: 'i',
      ports: [{ name: 'http', container: 1, service: 1 }],
      service: { name: 'api' },
    },
  ];

  assert.throws(
    () =>
      validate(
        {
          ...base,
          workloads,
          resources: [
            { name: 'a', type: 'metrics', workload: 'api', port: 'http' },
            { name: 'b', type: 'metrics', workload: 'api', port: 'http' },
          ],
        },
        'test',
      ),
    /api is listed twice/,
  );
  assert.throws(
    () =>
      validate(
        {
          ...base,
          resources: [{ name: 'r', type: 'route', hosts: ['a'], backend: { name: 'x', port: 1 } }],
        },
        'test',
      ),
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
        {
          name: 'images',
          type: 'bucket',
          bucket: 'b',
          provider: 'r2',
          secretName: 'images-secret',
        },
      ],
      workloads: [
        {
          name: 'foo',
          image: 'i',
          env: {
            A: { secret: 'foo', key: 'K' },
            B: { bucket: 'images', key: 'S3_ENDPOINT' },
          },
          envFrom: [
            { secret: 'foo', optional: true },
            { database: 'foo' },
            { bucket: 'images', prefix: 'X_' },
          ],
        },
      ],
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
  assert.deepEqual(
    source(render(app), 'k8s-secrets')?.helm.valuesObject.secrets[1].managedSecrets,
    [{ secretName: 'images-secret' }],
  );
});

test('rejects references to missing database and bucket resources', () => {
  const missing = (envFrom: unknown[]) =>
    validateResources({
      platform,
      apps: [validate({ ...base, workloads: [{ name: 'foo', image: 'i', envFrom }] }, 'test')],
    });

  assert.throws(() => missing([{ database: 'nope' }]), /no database resource named nope/);
  assert.throws(() => missing([{ bucket: 'nope' }]), /no bucket resource named nope/);
});

test('rejects the old routes, oidc and metrics blocks', () => {
  for (const block of ['routes', 'oidc', 'metrics']) {
    assert.throws(
      () => validate({ ...base, [block]: {} }, 'test'),
      new RegExp(`unknown block "${block}"`),
    );
  }
});

test('an oidc client secret can be referenced by resource name', () => {
  const app = validate(
    {
      ...base,
      resources: [{ name: 'foo', type: 'oidc', host: 'a.sh', secretName: 'foo-oidc' }],
      workloads: [
        {
          name: 'foo',
          image: 'i',
          env: { AUD: { oidc: 'foo', key: 'client-id', optional: true } },
        },
      ],
    },
    'test',
  );
  const rendered = render(app);

  assert.deepEqual(source(rendered, 'k8s-workload')?.helm.valuesObject.workloads[0].env, [
    {
      name: 'AUD',
      valueFrom: { secretKeyRef: { name: 'foo-oidc', key: 'client-id', optional: true } },
    },
  ]);
  assert.equal(source(rendered, 'k8s-oidc')?.helm.valuesObject.clients[0].secretName, 'foo-oidc');
});

test('rejects resources without a secret name and unknown references', () => {
  for (const type of ['secret', 'database']) {
    assert.throws(
      () => validate({ ...base, resources: [{ name: 'a', type }] }, 'test'),
      /resources/,
    );
  }
  assert.throws(
    () => validate({ ...base, resources: [{ name: 'a', type: 'oidc', host: 'a.sh' }] }, 'test'),
    /resources/,
  );
  assert.throws(
    () => validate({ ...base, resources: [{ name: 'a', type: 'bucket', provider: 'r2' }] }, 'test'),
    /resources/,
  );
  assert.throws(
    () =>
      validateResources({
        platform,
        apps: [
          validate(
            { ...base, workloads: [{ name: 'foo', image: 'i', envFrom: [{ secret: 'nope' }] }] },
            'test',
          ),
        ],
      }),
    /no secret resource named nope/,
  );
});

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

const checkRefs = (app: Record<string, unknown>) =>
  validateResources({ platform, apps: [validate({ ...base, ...app }, 'test')] });

test('rejects a route backend that is not a service in the app', () => {
  const route = (backend: unknown) => ({
    workloads: [
      {
        name: 'foo',
        image: 'i',
        service: { name: 'api' },
        ports: [{ name: 'http', container: 3000, service: 80 }],
      },
    ],
    resources: [
      {
        name: 'r',
        type: 'route',
        hosts: ['a.sh'],
        backend,
        auth: { mode: 'none', reason: 'public' },
      },
    ],
  });
  checkRefs(route({ name: 'api', port: 80 }));

  assert.throws(() => checkRefs(route({ name: 'nope', port: 80 })), /no service named nope/);
  assert.throws(() => checkRefs(route({ name: 'api', port: 81 })), /has no port 81/);
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

test('rejects a bucket key terraform does not write', () => {
  const bucket = (key: string) => ({
    resources: [{ name: 'b', type: 'bucket', bucket: 'b', provider: 'r2', secretName: 's' }],
    workloads: [{ name: 'foo', image: 'i', env: { A: { bucket: 'b', key } } }],
  });
  checkRefs(bucket('S3_ENDPOINT'));

  assert.throws(() => checkRefs(bucket('S3_NOPE')), /no key named S3_NOPE/);
});

test('rejects a resource no workload uses', () => {
  const unused = (resource: Record<string, unknown>) => ({
    resources: [resource],
    workloads: [{ name: 'foo', image: 'i' }],
  });

  assert.throws(
    () => checkRefs(unused({ name: 'a', type: 'platform', export: 'tracing' })),
    /no workload uses this platform/,
  );
  assert.throws(
    () => checkRefs(unused({ name: 'a', type: 'secret', secretName: 's' })),
    /no workload uses this secret/,
  );
  checkRefs({
    resources: [{ name: 'a', type: 'secret', secretName: 's' }],
    workloads: [
      { name: 'foo', image: 'i', initContainers: [{ image: 'j', envFrom: [{ secret: 'a' }] }] },
    ],
  });
});

test('rejects a build image no workload runs', () => {
  const build = (image: string) => ({ build: { image }, workloads: [{ name: 'foo', image: 'i' }] });
  checkRefs(build('i'));

  assert.throws(() => checkRefs(build('j')), /build.image j/);
});

test('rejects a route whose namespace lacks the listener label', () => {
  const listeners = [
    { gateway: 'public-gateway', hostname: '*.anurag.sh', labels: { wild: 'true' } },
    { gateway: 'public-gateway', hostname: 'git.anurag.sh', labels: { exact: 'true' } },
    { gateway: 'public-gateway', hostname: '*.same.sh' },
  ];
  const app = (host: string, labels?: Record<string, string>): App => ({
    ...base,
    namespace: { labels },
    resources: [
      { name: 'r', type: 'route', hosts: [host], auth: { mode: 'none', reason: 'public' } },
    ],
  });
  validateGatewayAccess([app('a.anurag.sh', { wild: 'true' })], listeners);
  validateGatewayAccess([app('git.anurag.sh', { exact: 'true' })], listeners);

  assert.throws(
    () => validateGatewayAccess([app('a.anurag.sh')], listeners),
    /needs namespace.labels wild/,
  );
  assert.throws(
    () => validateGatewayAccess([app('git.anurag.sh', { wild: 'true' })], listeners),
    /needs namespace.labels exact/,
  );
  assert.throws(
    () => validateGatewayAccess([app('a.same.sh')], listeners),
    /no public-gateway listener accepts a.same.sh/,
  );
  assert.throws(
    () => validateGatewayAccess([app('nope.sh')], listeners),
    /no public-gateway listener/,
  );
});

test('rejects unknown oidc fields and envFrom keys', () => {
  assert.throws(
    () =>
      validate(
        {
          ...base,
          resources: [{ name: 'a', type: 'oidc', secretName: 's', host: 'a.sh', nope: 1 }],
        },
        'test',
      ),
    /resources/,
  );
  assert.throws(
    () =>
      validate(
        { ...base, workloads: [{ name: 'foo', image: 'i', envFrom: [{ secrets: 'a' }] }] },
        'test',
      ),
    /envFrom/,
  );
});

test('one ports list fills the container and the service', () => {
  const app = validate(
    {
      ...base,
      workloads: [
        {
          name: 'foo',
          image: 'i',
          ports: [
            { name: 'grpc', container: 50051, service: 80, appProtocol: 'grpc' },
            { name: 'admin', container: 9000 },
          ],
          service: { name: 'api' },
          sidecars: [
            { image: 'j', name: 's', ports: [{ name: 'side', container: 8004, service: 8004 }] },
          ],
        },
      ],
      exports: [{ name: 'side', workload: 'foo', port: 'side' }],
    },
    'test',
  );
  const workload = source(render(app), 'k8s-workload')?.helm.valuesObject.workloads[0];

  assert.deepEqual(workload.ports, [
    { name: 'grpc', containerPort: 50051 },
    { name: 'admin', containerPort: 9000 },
  ]);
  assert.deepEqual(workload.sidecars[0].ports, [{ name: 'side', containerPort: 8004 }]);
  assert.deepEqual(workload.service, {
    name: 'api',
    ports: [
      { name: 'grpc', port: 80, targetPort: 50051, appProtocol: 'grpc', protocol: 'TCP' },
      { name: 'side', port: 8004, targetPort: 8004, protocol: 'TCP' },
    ],
  });
  validateResources({ platform, apps: [app] });
});

test('route rules send paths to different backends', () => {
  const app = validate(
    {
      ...base,
      workloads: [
        {
          name: 'a',
          image: 'i',
          ports: [{ name: 'http', container: 1, service: 80 }],
          service: { name: 'a' },
        },
        {
          name: 'b',
          image: 'j',
          ports: [{ name: 'http', container: 2, service: 81 }],
          service: { name: 'b' },
        },
      ],
      resources: [
        {
          name: 'r',
          type: 'route',
          hosts: ['a.sh'],
          gateway: 'lan-gateway',
          auth: { mode: 'app', reason: 'r' },
          rules: [
            { paths: ['/v1'], backend: { name: 'a', port: 80 } },
            { paths: ['/'], backend: { name: 'b', port: 81 } },
          ],
        },
      ],
    },
    'test',
  );
  validateResources({ platform, apps: [app] });
  const { rules, gateway } = source(render(app), 'k8s-http-routes')?.helm.valuesObject.routes[0];

  assert.equal(gateway.name, 'lan-gateway');
  assert.deepEqual(
    rules.map((r: any) => [r.matches[0].path.value, r.backendRefs[0].name, r.backendRefs[0].port]),
    [
      ['/v1', 'a', 80],
      ['/', 'b', 81],
    ],
  );
});

test('the login policy on a route reaches its oidc client', () => {
  const app = validate(
    {
      ...base,
      resources: [
        {
          name: 'web',
          type: 'route',
          hosts: ['a.sh'],
          backend: { name: 's', port: 1 },
          auth: { mode: 'oidc', client: 'c', scopes: ['openid'], forwardAccessToken: true },
        },
        { name: 'c', type: 'oidc', host: 'a.sh', secretName: 's' },
      ],
    },
    'test',
  );
  const client = source(render(app), 'k8s-oidc')?.helm.valuesObject.clients[0];

  assert.deepEqual(client.securityPolicy, {
    httpRoute: 'web',
    scopes: ['openid'],
    forwardAccessToken: true,
  });
  const bare = validate(
    {
      ...base,
      resources: [
        {
          name: 'web',
          type: 'route',
          hosts: ['a.sh'],
          backend: { name: 's', port: 1 },
          auth: { mode: 'oidc', client: 'c' },
        },
        { name: 'c', type: 'oidc', host: 'a.sh', secretName: 's' },
      ],
    },
    'test',
  );

  assert.deepEqual(source(render(bare), 'k8s-oidc')?.helm.valuesObject.clients[0].securityPolicy, {
    httpRoute: 'web',
  });
  assert.throws(
    () =>
      validate(
        {
          ...base,
          resources: [
            {
              name: 'a',
              type: 'route',
              hosts: ['a.sh'],
              backend: { name: 's', port: 1 },
              auth: { mode: 'oidc', client: 'c' },
            },
            {
              name: 'b',
              type: 'route',
              hosts: ['a.sh'],
              backend: { name: 's', port: 1 },
              auth: { mode: 'oidc', client: 'c' },
            },
            { name: 'c', type: 'oidc', host: 'a.sh', secretName: 's' },
          ],
        },
        'test',
      ),
    /used by another oidc route/,
  );
});

test('a config resource generates its configmap and mount', () => {
  const app = validate(
    {
      ...base,
      resources: [
        {
          name: 'rules',
          type: 'config',
          configMap: 'foo-rules',
          files: ['config.yaml', 'rules/a.yaml'],
        },
      ],
      workloads: [
        { name: 'foo', image: 'i', mounts: { rules: { path: '/etc/rules', config: 'rules' } } },
      ],
    },
    'test',
  );
  const workload = source(render(app), 'k8s-workload')?.helm.valuesObject.workloads[0];

  assert.deepEqual(workload.volumes, [
    {
      name: 'rules',
      configMap: {
        name: 'foo-rules',
        items: [
          { key: 'config.yaml', path: 'config.yaml' },
          { key: 'a.yaml', path: 'rules/a.yaml' },
        ],
      },
    },
  ]);
  assert.deepEqual(parse(renderKustomization(app, target) as string).configMapGenerator, [
    { name: 'foo-rules', namespace: 'foo', files: ['config.yaml', 'rules/a.yaml'] },
  ]);
  assert.throws(
    () =>
      validateResources({
        platform,
        apps: [
          validate(
            {
              ...base,
              workloads: [
                { name: 'foo', image: 'i', mounts: { r: { path: '/r', config: 'nope' } } },
              ],
            },
            'test',
          ),
        ],
      }),
    /no config resource named nope/,
  );
});

test('a dashboard resource generates a labelled configmap', () => {
  const app = validate(
    {
      ...base,
      resources: [
        { name: 'd', type: 'dashboard', configMap: 'foo-dashboard', file: 'dashboards/foo.json' },
      ],
    },
    'test',
  );
  const { configMapGenerator, resources } = parse(
    renderKustomization(app, { ...target, hasManifests: true }) as string,
  );

  assert.equal(configMapGenerator[0].options.labels['inf-k8s.net/grafana-dashboard'], 'true');
  assert.deepEqual(configMapGenerator[0].files, ['dashboards/foo.json']);
  assert.deepEqual(resources, ['manifests']);
  validateResources({ platform, apps: [app] });
});

test('a serviceAccount resource generates its account and cluster role', () => {
  const rules = [{ apiGroups: [''], resources: ['secrets'], verbs: ['get'] }];
  const app = validate(
    {
      ...base,
      resources: [
        { name: 'sa', type: 'serviceAccount', clusterRole: 'foo-access', clusterRules: rules },
      ],
      workloads: [{ name: 'foo', image: 'i', serviceAccount: 'sa' }],
    },
    'test',
  );
  const values = source(render(app), 'k8s-workload')?.helm.valuesObject;

  assert.deepEqual(values.serviceAccounts, ['sa']);
  assert.equal(values.workloads[0].serviceAccountName, 'sa');
  assert.equal(values.workloads[0].serviceAccount, undefined);
  const rbac = renderRbac(app) as string;

  assert.match(rbac, /kind: ClusterRole\nmetadata:\n  name: foo-access/);
  assert.match(rbac, /kind: ClusterRoleBinding\nmetadata:\n  name: sa/);
  assert.deepEqual(parse(renderKustomization(app, target) as string).resources, [
    'manifests/rbac.generated.yaml',
  ]);
  assert.throws(
    () => checkRefs({ workloads: [{ name: 'foo', image: 'i', serviceAccount: 'nope' }] }),
    /no serviceAccount resource named nope/,
  );
});

test('rejects the removed shorthands', () => {
  const route = (extra: Record<string, unknown>) => ({
    name: 'r',
    type: 'route',
    hosts: ['a.sh'],
    auth: { mode: 'none', reason: 'public' },
    ...extra,
  });
  const rules = [{ paths: ['/'], backend: { name: 's', port: 1 } }];

  assert.throws(
    () => validate({ ...base, resources: [route({})] }, 'test'),
    /needs either backend or rules/,
  );
  assert.throws(
    () =>
      validate({ ...base, resources: [route({ rules, backend: { name: 's', port: 1 } })] }, 'test'),
    /needs either backend or rules/,
  );
  assert.throws(
    () =>
      validate(
        { ...base, resources: [{ name: 'a', type: 'bucket', provider: 'r2', secretName: 's' }] },
        'test',
      ),
    /resources/,
  );
  assert.throws(
    () => validate({ ...base, resources: [{ name: 'a', type: 'oidc', secretName: 's' }] }, 'test'),
    /resources/,
  );
  assert.throws(
    () =>
      validate({ ...base, resources: [{ name: 'a', type: 'metrics', workload: 'foo' }] }, 'test'),
    /resources/,
  );
  assert.throws(() => validate({ ...base, serviceAccounts: ['a'] }, 'test'), /unknown block/);
  assert.throws(
    () =>
      validate(
        { ...base, workloads: [{ name: 'foo', image: 'i', service: { name: 'a', port: 80 } }] },
        'test',
      ),
    /port/,
  );
  const reads = validate(
    {
      ...base,
      resources: [{ name: 'b', type: 'bucket', bucket: 'b', provider: 'r2', secretName: 's' }],
      workloads: [{ name: 'foo', image: 'i', env: { B: { resource: 'b' } } }],
    },
    'test',
  );

  assert.throws(
    () => validateResources({ platform, apps: [reads] }),
    /read a bucket with \{bucket, key\}/,
  );
});

const overridden: App = {
  name: 'foo',
  namespace: { name: 'kube-system' },
  network: { ingress: [], policy: false },
  resources: [
    {
      name: 'images',
      type: 'bucket',
      provider: 'r2',
      bucket: 'foo-images',
      secretName: 'images-bucket-secrets',
      secretTemplate: { type: 'example.io/config', data: { bucket: '{{ .S3_BUCKET }}' } },
    },
  ],
};

test('a namespace override reaches the destination, charts and terraform path', () => {
  const rendered = render(overridden);

  assert.equal(rendered.metadata.name, 'foo');
  assert.equal(rendered.spec.destination.namespace, 'kube-system');
  assert.equal(
    source(rendered, 'k8s-secrets')?.helm.valuesObject.defaults.namespace,
    'kube-system',
  );
  const { resource } = JSON.parse(renderAppTerraform(overridden) as string);

  assert.equal(resource.vault_kv_secret_v2['foo-images'].name, 'kube-system/resource-images');
});

test('network.policy false skips the network chart', () => {
  assert.equal(source(render(overridden), 'k8s-network'), undefined);
});

test('a namespace override must state its network policy', () => {
  const namespace = { name: 'kube-system' };

  assert.throws(
    () => validate({ name: 'foo', namespace, network: { ingress: [] } }, 'foo'),
    /explicit network\.policy/,
  );
  assert.throws(
    () => validate({ name: 'foo', namespace, network: { ingress: ['bar'], policy: false } }, 'foo'),
    /network\.policy: true/,
  );
  assert.throws(
    () =>
      validate(
        {
          name: 'foo',
          namespace: { ...namespace, labels: { a: 'b' } },
          network: { ingress: [], policy: false },
        },
        'foo',
      ),
    /namespace\.labels/,
  );
});

test('a bucket secret template sets the secret type and data', () => {
  assert.deepEqual(source(render(overridden), 'k8s-secrets')?.helm.valuesObject.secrets, [
    {
      name: 'resource-images',
      managedSecrets: [
        {
          secretName: 'images-bucket-secrets',
          type: 'example.io/config',
          template: { data: { bucket: '{{ .S3_BUCKET }}' } },
        },
      ],
    },
  ]);
});

test('a bucket with a secret template needs no workload', () => {
  assert.doesNotThrow(() => validateResources({ platform, apps: [overridden] }));
});

const release = {
  type: 'helm' as const,
  chart: 'forgejo',
  repo: 'oci://code.forgejo.org/forgejo-helm/forgejo',
  version: '1.0.0',
  values: 'forgejo.values.yaml',
};

test('helm releases read values from a file beside app.yaml', () => {
  const rendered = render(
    { name: 'foo', network: { ingress: [] }, sources: [release] },
    { location: 'selfhosted', hasManifests: false },
  );

  assert.equal(rendered.spec.project, 'selfhosted');
  assert.equal(rendered.spec.sources[0].ref, 'values');
  assert.deepEqual(source(rendered, 'forgejo'), {
    chart: 'forgejo',
    repoURL: release.repo,
    targetRevision: '1.0.0',
    helm: { valueFiles: ['$values/selfhosted/foo/forgejo.values.yaml'] },
  });
});

test('a chart listed twice needs distinct release names', () => {
  const app = (sources: App['sources']) => ({ name: 'foo', network: { ingress: [] }, sources });

  assert.throws(() => validate(app([release, release]), 'x'), /duplicate helm release forgejo/);
  validate(app([release, { ...release, releaseName: 'other' }]), 'x');
});

test('helm templates pull upstream charts', () => {
  const plan = planApp(
    {
      name: 'foo',
      network: { ingress: [] },
      sources: [
        release,
        {
          type: 'helm',
          chart: 'reloader',
          repo: 'https://example.com/charts',
          version: '2.0.0',
          values: 'r.yaml',
          releaseName: 'rel',
        },
      ],
    },
    versions,
    target,
    { platform, apps: [] },
  );
  const [, oci, https] = helmTemplates(plan).map(({ args }) => args);

  assert.deepEqual(oci.slice(1, 5), ['foo', release.repo, '--version', '1.0.0']);
  assert.deepEqual(https.slice(1, 7), [
    'rel',
    'reloader',
    '--repo',
    'https://example.com/charts',
    '--version',
    '2.0.0',
  ]);
  assert.ok(oci.includes('projects/foo/forgejo.values.yaml'));
});

test('argocd overrides replace the default policy', () => {
  const ignoreDifferences = [{ group: '', kind: 'Secret', jsonPointers: ['/data'] }];
  const rendered = render({
    name: 'foo',
    network: { ingress: [] },
    overrides: {
      argocd: {
        application: { options: ['ServerSideApply=true'], automated: false, ignoreDifferences },
      },
    },
  });

  assert.deepEqual(rendered.spec.syncPolicy, { syncOptions: ['ServerSideApply=true'] });
  assert.deepEqual(rendered.spec.ignoreDifferences, ignoreDifferences);
  validate(
    {
      name: 'foo',
      network: { ingress: [] },
      overrides: { argocd: { application: { automated: false } } },
    },
    'x',
  );
});

test('namespace labels use managed metadata without workloads', () => {
  const labels = { 'internal-gateway-access': 'true' };
  const bare = render({ name: 'foo', network: { ingress: [] }, namespace: { labels } });

  assert.deepEqual(bare.spec.syncPolicy.managedNamespaceMetadata, { labels });
  const workload = render({
    name: 'foo',
    network: { ingress: [] },
    namespace: { labels },
    workloads: [{ name: 'foo', image: 'a' }],
  });

  assert.equal(workload.spec.syncPolicy.managedNamespaceMetadata, undefined);
  assert.deepEqual(source(workload, 'k8s-workload')?.helm.valuesObject.namespaceLabels, labels);
});

test('namespace.name needs an explicit network policy', () => {
  const app = (network: App['network']) => ({ name: 'foo', network, namespace: { name: 'bar' } });

  assert.throws(() => validate(app({ ingress: [] }), 'x'), /explicit network.policy/);
  assert.throws(
    () => validate(app({ ingress: ['a'], policy: false }), 'x'),
    /needs network.policy: true/,
  );
  validate(app({ ingress: [], policy: false }), 'x');
  const rendered = render(app({ ingress: ['a'], policy: true }));

  assert.equal(source(rendered, 'k8s-network')?.helm.valuesObject.namespace, 'bar');
});

test('consumers outside workloads relax only the checks they can satisfy', () => {
  const app: App = {
    name: 'foo',
    network: { ingress: [] },
    resources: [
      { name: 'foo', type: 'secret', secretName: 'foo-secrets' },
      { name: 'foo', type: 'database', secretName: 'foo-db' },
      {
        name: 'web',
        type: 'route',
        hosts: ['foo.example.com'],
        auth: { mode: 'app', reason: 'r' },
        backend: { name: 'chart-service', port: 80 },
      },
    ],
  };

  assert.throws(() => validateResources({ platform, apps: [app] }), /no workload uses/);
  validateResources({ platform, apps: [app], external: new Set(['foo']) });
  const wrongPort: App = {
    ...app,
    workloads: [
      {
        name: 'foo',
        image: 'a',
        service: { name: 'chart-service' },
        ports: [{ name: 'http', container: 8080, service: 8080 }],
      },
    ],
  };

  assert.throws(
    () => validateResources({ platform, apps: [wrongPort], external: new Set(['foo']) }),
    /has no port 80/,
  );
});

test('database role and extensions flow through every output', () => {
  const app: App = {
    name: 'foo',
    network: { ingress: [] },
    resources: [
      {
        name: 'foo',
        type: 'database',
        secretName: 'foo-db',
        role: 'foo_2',
        extensions: ['vector'],
      },
    ],
  };

  assert.deepEqual(appRoles([app]), ['foo_2']);
  assert.equal(
    source(render(app), 'k8s-postgres')?.helm.valuesObject.databases[0].databaseName,
    'foo_2',
  );
  const database = parse(renderDatabase('foo', { role: 'foo_2', extensions: ['vector'] }));

  assert.equal(database.metadata.name, 'foo');
  assert.equal(database.spec.name, 'foo_2');
  assert.equal(database.spec.owner, 'foo_2');
  assert.deepEqual(database.spec.extensions, [{ name: 'vector', ensure: 'present' }]);
  const terraform = JSON.parse(renderAppTerraform(app) as string);

  assert.equal(terraform.resource.vault_database_secret_backend_static_role.foo.username, 'foo_2');
});

test('secret templates can exclude other keys', () => {
  const rendered = render({
    name: 'foo',
    network: { ingress: [] },
    resources: [
      {
        name: 'foo',
        type: 'secret',
        secretName: 's',
        secretTemplate: { includeAllSecrets: false, data: { a: 'b' } },
      },
    ],
  });

  assert.deepEqual(source(rendered, 'k8s-secrets')?.helm.valuesObject.secrets[0].managedSecrets, [
    { secretName: 's', template: { includeAllSecrets: false, data: { a: 'b' } } },
  ]);
  validate(
    {
      name: 'foo',
      network: { ingress: [] },
      resources: [
        {
          name: 'foo',
          type: 'secret',
          secretName: 's',
          secretTemplate: { includeAllSecrets: false, data: { a: 'b' } },
        },
      ],
    },
    'x',
  );
});

test('typed sources render in the order listed', () => {
  const rendered = render({
    name: 'foo',
    network: { ingress: [] },
    sources: [
      { type: 'manifests', path: '.' },
      {
        type: 'git',
        repo: 'https://example.com/r.git',
        path: 'config',
        name: 'config',
        revision: 'main',
      },
    ] as App['sources'],
  });

  assert.deepEqual(rendered.spec.sources.slice(1), [
    {
      repoURL: 'https://github.com/Accurate0/inf-k8s.git',
      name: 'application',
      path: 'projects/foo',
    },
    {
      repoURL: 'https://example.com/r.git',
      name: 'config',
      path: 'config',
      targetRevision: 'main',
    },
  ]);
});

test('manifests sources are limited and stay in the app directory', () => {
  const app = (sources: unknown[], extra = {}) => ({
    name: 'foo',
    network: { ingress: [] },
    sources,
    ...extra,
  });

  assert.throws(
    () =>
      validate(
        app([
          { type: 'manifests', path: '.' },
          { type: 'manifests', path: 'x' },
        ]),
        'x',
      ),
    /only one manifests/,
  );
  assert.throws(
    () => validate(app([{ type: 'manifests', path: '../other' }]), 'x'),
    /inside the app directory/,
  );
  const config = { resources: [{ name: 'c', type: 'config', configMap: 'c', files: ['a'] }] };

  assert.throws(
    () => validate(app([{ type: 'manifests', path: 'manifests' }], config), 'x'),
    /path must be \./,
  );
  assert.throws(() => validate(app([{ type: 'bogus', path: 'x' }]), 'x'));
  assert.throws(() => validate(app([{ type: 'helm', chart: 'a', repo: 'b', version: '1' }]), 'x'));
});

test('kustomizations on disk must be declared as a source', () => {
  const app = (sources: App['sources'], resources?: App['resources']): App => ({
    name: 'foo',
    network: { ingress: [] },
    sources,
    resources,
  });
  const none = { hasManifests: false, hasRootKustomization: false };
  validateManifestSources(app(undefined), none);

  assert.throws(
    () => validateManifestSources(app(undefined), { ...none, hasManifests: true }),
    /add a manifests source/,
  );
  assert.throws(
    () => validateManifestSources(app(undefined), { ...none, hasRootKustomization: true }),
    /add a manifests source/,
  );
  assert.throws(
    () =>
      validateManifestSources(
        app(undefined, [{ name: 'c', type: 'config', configMap: 'c', files: ['a'] }]),
        none,
      ),
    /add a manifests source/,
  );
  assert.throws(
    () =>
      validateManifestSources(app([{ type: 'manifests', path: 'manifests' }]), {
        hasManifests: true,
        hasRootKustomization: true,
      }),
    /must be \./,
  );
  validateManifestSources(app([{ type: 'manifests', path: '.' }]), {
    hasManifests: true,
    hasRootKustomization: true,
  });
});

test('raw argo sources are rejected', () => {
  assert.throws(() =>
    validate(
      { name: 'foo', network: { ingress: [] }, sources: [{ repoURL: 'r', path: 'p' }] },
      'x',
    ),
  );
});

test('the old sync block is rejected', () => {
  assert.throws(
    () => validate({ name: 'foo', network: { ingress: [] }, sync: { automated: false } }, 'x'),
    /unknown block "sync"/,
  );
});

test('workloads are a list with unique names', () => {
  const app = (workloads: unknown) => ({ name: 'foo', network: { ingress: [] }, workloads });
  validate(app([{ name: 'foo', image: 'i' }]), 'x');

  assert.throws(() => validate(app({ foo: { image: 'i' } }), 'x'), /workloads must be a list/);
  assert.throws(
    () =>
      validate(
        app([
          { name: 'a', image: 'i' },
          { name: 'a', image: 'i' },
        ]),
        'x',
      ),
    /duplicate name a/,
  );
  assert.throws(() => validate(app([{ image: 'i' }]), 'x'));
});

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

test('overrides replace the default application policy', () => {
  const ignoreDifferences = [{ group: '', kind: 'Secret', jsonPointers: ['/data'] }];
  const app = validate(
    {
      name: 'foo',
      network: { ingress: [] },
      overrides: {
        argocd: {
          application: { options: ['ServerSideApply=true'], automated: false, ignoreDifferences },
        },
      },
    },
    'x',
  );
  const [[, rendered]] = renderOutputs(planApp(app, versions, target, { platform, apps: [] }), app);

  assert.deepEqual(parse(rendered).spec.syncPolicy, { syncOptions: ['ServerSideApply=true'] });
  assert.deepEqual(parse(rendered).spec.ignoreDifferences, ignoreDifferences);
});

test('overrides reject unknown outputs and fields', () => {
  assert.throws(
    () => validate({ name: 'foo', network: { ingress: [] }, overrides: { flux: {} } }, 'x'),
    /overrides/,
  );
  assert.throws(
    () =>
      validate(
        {
          name: 'foo',
          network: { ingress: [] },
          overrides: { argocd: { application: { nope: true } } },
        },
        'x',
      ),
    /overrides/,
  );
});

test('route dns becomes external-dns annotations', () => {
  const route = (dns: unknown) => ({
    name: 'foo',
    type: 'route',
    auth: { mode: 'none', reason: 'public' },
    hosts: ['foo.example.com'],
    backend: { name: 'foo', port: 80 },
    dns,
  });
  const annotations = (dns: unknown) => {
    const app = validate({ name: 'foo', network: { ingress: [] }, resources: [route(dns)] }, 'x');
    const source = planApp(app, versions, target, { platform, apps: [] }).sources.find(
      (s) => s.type === 'chart' && s.chart === 'k8s-http-routes',
    );
    return (source as { values: { routes: { annotations: unknown }[] } }).values.routes[0]
      .annotations;
  };

  assert.deepEqual(annotations({ provider: 'cloudflare', proxied: false }), {
    'external-dns.kubernetes.io/managed': 'true',
    'external-dns.kubernetes.io/cloudflare-proxied': 'false',
  });
  assert.deepEqual(annotations({ provider: 'unifi' }), {
    'external-dns.kubernetes.io/managed-unifi': 'true',
  });
  assert.throws(() => annotations({ provider: 'cloudflare' }), /dns|resources/);
});

test('ratelimit renders a distinct header rule', () => {
  const app = validate(
    {
      name: 'foo',
      network: { ingress: [] },
      resources: [
        {
          name: 'foo',
          type: 'route',
          auth: { mode: 'none', reason: 'public' },
          hosts: ['foo.example.com'],
          backend: { name: 'foo', port: 80 },
          ratelimit: { requests: 5, per: 'Second', header: 'x-real-ip' },
        },
      ],
    },
    'x',
  );
  const source = planApp(app, versions, target, { platform, apps: [] }).sources.find(
    (s) => s.type === 'chart' && s.chart === 'k8s-http-routes',
  );

  assert.deepEqual(
    (source as { values: { routes: { ratelimit: unknown }[] } }).values.routes[0].ratelimit,
    {
      rules: [
        {
          clientSelectors: [{ headers: [{ name: 'x-real-ip', invert: false, type: 'Distinct' }] }],
          limit: { requests: 5, unit: 'Second' },
        },
      ],
    },
  );
});

test("service env resolves against the app's own services", () => {
  const app = (env: Record<string, unknown>) =>
    validate(
      {
        name: 'foo',
        network: { ingress: [] },
        workloads: [
          {
            name: 'api',
            image: 'ghcr.io/x/api',
            resources: { memory: '1Mi' },
            service: { name: 'api' },
            ports: [
              { name: 'http', container: 3000, service: 8000 },
              { name: 'web', container: 3001, service: 80 },
            ],
            env,
          },
        ],
      },
      'x',
    );
  const value = (env: Record<string, unknown>) => {
    const built = app({ URL: env });
    const source = planApp(built, versions, target, { platform, apps: [built] }).sources[0] as {
      values: { workloads: { env: { value: string }[] }[] };
    };
    return source.values.workloads[0].env[0].value;
  };

  assert.equal(
    value({ service: 'api', port: 'http', path: '/v1' }),
    'http://api.foo.svc.cluster.local:8000/v1',
  );
  assert.equal(value({ service: 'api', port: 80 }), 'http://api.foo.svc.cluster.local');
  assert.throws(() => value({ service: 'nope', port: 80 }), /no service named nope/);
  assert.throws(() => value({ service: 'api', port: 'nope' }), /no port nope/);
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
  const world = { platform: storage, apps: [app] };
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

test('a missing database export is an error', () => {
  const app = validate(
    {
      name: 'foo',
      network: { ingress: [] },
      resources: [{ name: 'foo', type: 'database', secretName: 'foo-secret' }],
      workloads: [
        {
          name: 'api',
          image: 'ghcr.io/x/api',
          resources: { memory: '1Mi' },
          envFrom: [{ database: 'foo' }],
        },
      ],
    },
    'x',
  );

  assert.throws(
    () => validateResources({ platform: [], apps: [app] }),
    /no database export named pooled/,
  );
});

test('only the argocd output adds retention annotations', () => {
  const app = validate(
    {
      name: 'foo',
      network: { ingress: [] },
      resources: [{ name: 'foo', type: 'oidc', secretName: 'foo-oidc', host: 'foo.example.com' }],
    },
    'x',
  );
  const plan = planApp(app, versions, target, { platform, apps: [] });
  const oidc = plan.sources.find((s) => s.type === 'chart' && s.chart === 'k8s-oidc') as {
    values: Record<string, unknown>;
    retain?: boolean;
  };

  assert.equal(oidc.retain, true);
  assert.equal('retainAnnotations' in oidc.values, false);
  const source = parse(renderArgo(plan)).spec.sources.find(
    (s: { chart?: string }) => s.chart === 'k8s-oidc',
  );

  assert.deepEqual(source.helm.valuesObject.retainAnnotations, {
    'argocd.argoproj.io/sync-options': 'Prune=false,Delete=false',
  });
});

test('legacy route and argocd fields are rejected', () => {
  const route = {
    name: 'foo',
    type: 'route',
    auth: { mode: 'none', reason: 'public' },
    hosts: ['foo.example.com'],
    backend: { name: 'foo', port: 80 },
  };

  assert.throws(
    () => validate({ name: 'foo', network: { ingress: [] }, argocd: {} }, 'x'),
    /unknown block "argocd"/,
  );
  assert.throws(
    () =>
      validate(
        { name: 'foo', network: { ingress: [] }, resources: [{ ...route, annotations: {} }] },
        'x',
      ),
    /resources/,
  );
  assert.throws(
    () =>
      validate(
        {
          name: 'foo',
          network: { ingress: [] },
          resources: [{ ...route, ratelimit: { rules: [] } }],
        },
        'x',
      ),
    /resources/,
  );
});
