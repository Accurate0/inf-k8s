import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parse } from 'yaml';
import { renderAppTerraform } from './terraform.ts';
import { renderApplication as renderArgo } from './outputs/argocd.ts';
import { renderOutputs } from './outputs/index.ts';
import { helmTemplates, planApp } from './plan.ts';
import {
  type App,
  HEADER,
  isCurrent,
  stamped,
  validate,
  validateIngress,
  validateResources,
} from './render.ts';
import {
  versions,
  target,
  renderApplication,
  platform,
  render,
  source,
  base,
  overridden,
} from './test-fixtures.ts';

test('only enables charts for declared blocks', () => {
  const rendered = render({ name: 'foo', network: { ingress: [] } });

  assert.deepEqual(
    rendered.spec.sources.map((s: { chart: string }) => s.chart),
    ['k8s-network'],
  );
  assert.equal(rendered.spec.project, 'projects');
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
        gateway: 'public-gateway',
        paths: ['/'],
        hosts: ['foo.anurag.sh'],
        backend: { name: 'foo', port: 3000 },
        auth: { mode: 'jwt', clients: ['foo'] },
      },
      {
        name: 'foo',
        type: 'oidc',
        displayName: 'foo',
        preferShortUsername: true,
        host: 'foo.anurag.sh',
        secretName: 'kanidm-foo-oidc',
      },
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

test('platform charts come from registry.anurag.sh unless the app overrides the registry', () => {
  const sources = (app: App) =>
    parse(renderApplication(app, versions, target, { platform, apps: [] })).spec.sources;
  const network = (app: App) =>
    sources(app).find((source: { chart?: string }) => source.chart === 'k8s-network');

  assert.equal(
    network({ name: 'foo', network: { ingress: [] } }).repoURL,
    'oci://registry.anurag.sh/charts/k8s-network',
  );
  assert.equal(
    network(
      validate(
        { name: 'foo', chartRegistry: 'oci://ghcr.io/accurate0/charts', network: { ingress: [] } },
        'test',
      ),
    ).repoURL,
    'oci://ghcr.io/accurate0/charts/k8s-network',
  );
});

test('output is stable', () => {
  const app: App = { name: 'foo', network: { ingress: [] } };

  assert.equal(
    renderApplication(app, versions, target, { platform, apps: [] }),
    renderApplication(app, versions, target, { platform, apps: [] }),
  );
});

test('rejects invalid apps', () => {
  assert.throws(() => validate({ name: 'foo' }, 'x'), /requires property "network"/);
  assert.throws(
    () => validate({ name: 'Foo', network: { ingress: [] } }, 'x'),
    /name does not match pattern/,
  );
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
              gateway: 'public-gateway',
              paths: ['/'],
              hosts: ['foo.anurag.sh'],
              backend: { name: 'foo', port: 1 },
            },
          ],
          network: { ingress: [] },
        },
        'x',
      ),
    /resources\[0\] requires property "auth"/,
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

test('an app with nothing for terraform gets no file', () => {
  assert.equal(
    renderAppTerraform({
      ...base,
      resources: [{ name: 't', type: 'platform', export: 'tracing' }],
    }),
    undefined,
  );
});

test('rejects the old routes, oidc and metrics blocks', () => {
  for (const block of ['routes', 'oidc', 'metrics']) {
    assert.throws(
      () => validate({ ...base, [block]: {} }, 'test'),
      new RegExp(`unknown block "${block}"`),
    );
  }
});

test('rejects the removed shorthands', () => {
  const route = (extra: Record<string, unknown>) => ({
    name: 'r',
    type: 'route',
    gateway: 'public-gateway',
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
    () =>
      validate(
        {
          ...base,
          resources: [
            {
              name: 'a',
              type: 'oidc',
              displayName: 'a',
              preferShortUsername: true,
              secretName: 's',
            },
          ],
        },
        'test',
      ),
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
        gateway: 'public-gateway',
        paths: ['/'],
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

test('only rendered outputs add retention annotations', () => {
  const app = validate(
    {
      name: 'foo',
      network: { ingress: [] },
      resources: [
        {
          name: 'foo',
          type: 'oidc',
          displayName: 'foo',
          preferShortUsername: true,
          secretName: 'foo-oidc',
          host: 'foo.example.com',
        },
      ],
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
    'helm.sh/resource-policy': 'keep',
  });
});

test('legacy route and argocd fields are rejected', () => {
  const route = {
    name: 'foo',
    type: 'route',
    gateway: 'public-gateway',
    paths: ['/'],
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

test('routes, oidc clients and builds spell out what used to be defaulted', () => {
  const base = { name: 'foo', network: { ingress: [] } };
  const route = {
    name: 'web',
    type: 'route',
    gateway: 'public-gateway',
    auth: { mode: 'none', reason: 'public' },
    hosts: ['a.sh'],
    backend: { name: 'web', port: 80 },
    paths: ['/'],
  };
  const oidc = {
    name: 'foo',
    type: 'oidc',
    host: 'a.sh',
    secretName: 's',
    displayName: 'Foo',
    preferShortUsername: true,
  };
  const without = (object: Record<string, unknown>, key: string) =>
    Object.fromEntries(Object.entries(object).filter(([k]) => k !== key));

  assert.throws(
    () => validate({ ...base, resources: [without(route, 'gateway')] }, 'x'),
    /resources\[0\] requires property "gateway"/,
  );
  assert.throws(
    () => validate({ ...base, resources: [without(route, 'paths')] }, 'x'),
    /routes\.web\.paths is required with backend/,
  );
  for (const key of ['displayName', 'preferShortUsername']) {
    assert.throws(
      () => validate({ ...base, resources: [without(oidc, key)] }, 'x'),
      new RegExp(`resources\\[0\\] requires property "${key}"`),
    );
  }
  assert.throws(
    () => validate({ ...base, build: { image: 'foo' } }, 'x'),
    /build requires property "ci"/,
  );
});
