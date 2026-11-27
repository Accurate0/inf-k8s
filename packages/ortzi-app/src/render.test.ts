import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parse } from 'yaml';
import { renderAppTerraform } from './terraform.ts';
import { renderOutputs } from './outputs/index.ts';
import { renderFluxIndex } from './outputs/flux-index.ts';
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
  platform,
  render,
  base,
  overridden,
  releaseValues,
  readValues,
} from './test-fixtures.ts';

const repository = (rendered: ReturnType<typeof render>, name: string) =>
  rendered.objects.find((o) => o.kind === 'OCIRepository' && o.metadata.name === name);

test('only enables charts for declared blocks', () => {
  const rendered = render({ name: 'foo', network: { ingress: [] } });

  assert.deepEqual(
    rendered.releases.map((r) => r.metadata.name),
    ['foo-k8s-network'],
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
        gateways: [{ gateway: 'public-gateway', hosts: ['foo.anurag.sh'] }],
        paths: ['/'],
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

  assert.deepEqual(releaseValues(rendered, 'k8s-network').ingress, [
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

  assert.deepEqual(repository(rendered, 'foo-k8s-network').spec.ref, { tag: '0.0.9' });
});

test('does not emit yaml anchors', () => {
  const { files } = render({
    name: 'foo',
    resources: [
      { name: 'foo', type: 'secret', secretName: 'foo-managed-secrets' },
      { name: 'foo', type: 'database', secretName: 'foo-database-secret' },
    ],
    network: { ingress: [] },
  });

  assert.doesNotMatch([...files.values()].join('\n'), /[&*]a1/);
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
  const network = (app: App) => repository(render(app), 'foo-k8s-network').spec;

  assert.equal(
    network({ name: 'foo', network: { ingress: [] } }).url,
    'oci://registry.anurag.sh/charts/k8s-network',
  );
  assert.equal(
    network(
      validate(
        { name: 'foo', chartRegistry: 'oci://ghcr.io/accurate0/charts', network: { ingress: [] } },
        'test',
      ),
    ).url,
    'oci://ghcr.io/accurate0/charts/k8s-network',
  );
});

test('output is stable', () => {
  const app: App = { name: 'foo', network: { ingress: [] } };

  assert.deepEqual([...render(app).files], [...render(app).files]);
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
              gateways: [{ gateway: 'public-gateway', hosts: ['foo.anurag.sh'] }],
              paths: ['/'],
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
    { platform, apps: [], readValues },
  );
  const templates = helmTemplates(plan);

  assert.deepEqual(templates[0].args, [
    'template',
    'foo',
    'charts/k8s-workload',
    '--namespace',
    'foo',
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
    gateways: [{ gateway: 'public-gateway', hosts: ['a.sh'] }],
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

  const app = rendered.objects.find(
    (o) => o.kind === 'Kustomization' && o.metadata?.name === 'foo',
  );
  assert.equal(app.spec.path, './kube-system/foo/app');
  assert.ok(rendered.releases.every((r) => r.spec.targetNamespace === 'kube-system'));
  assert.equal(releaseValues(rendered, 'k8s-secrets').defaults.namespace, 'kube-system');
  const { resource } = JSON.parse(renderAppTerraform(overridden) as string);

  assert.equal(resource.vault_kv_secret_v2['foo-images'].name, 'kube-system/resource-images');
});

test('network.policy false skips the network chart', () => {
  assert.equal(releaseValues(render(overridden), 'k8s-network'), undefined);
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

test('namespace labels reach the namespace and the workload chart', () => {
  const labels = { 'internal-gateway-access': 'true' };
  const app = (extra: Partial<App>): App => ({
    name: 'foo',
    network: { ingress: [] },
    namespace: { labels },
    ...extra,
  });
  const plan = planApp(app({}), versions, target, { platform, apps: [], readValues });
  const index = renderFluxIndex([plan], { namespaces: [], components: [] });

  assert.deepEqual(parse(index.get('kubernetes/foo/namespace.yaml')!).metadata.labels, labels);
  const workload = render(app({ workloads: [{ name: 'foo', image: 'a' }] }));

  assert.deepEqual(releaseValues(workload, 'k8s-workload').namespaceLabels, labels);
  assert.equal(releaseValues(workload, 'k8s-workload').createNamespace, false);
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

  assert.equal(releaseValues(rendered, 'k8s-network').namespace, 'bar');
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
        gateways: [{ gateway: 'public-gateway', hosts: ['foo.example.com'] }],
        paths: ['/'],
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
    artifact: { repo: 'oci://registry.anurag.sh/manifests/foo', tag: 'main' },
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
  const sources = rendered.objects.filter(
    (o) =>
      ['OCIRepository', 'GitRepository'].includes(o.kind) && !o.metadata.name.includes('-k8s-'),
  );

  assert.deepEqual(
    sources.map((o) => [o.kind, o.metadata.name, o.spec.url]),
    [
      ['OCIRepository', 'foo-manifests', 'oci://registry.anurag.sh/manifests/foo'],
      ['GitRepository', 'foo-config', 'https://example.com/r.git'],
    ],
  );
  assert.deepEqual(sources[1].spec.ref, { branch: 'main' });
});

test('raw sources are rejected', () => {
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

test('overrides reject unknown outputs and fields', () => {
  assert.throws(
    () => validate({ name: 'foo', network: { ingress: [] }, overrides: { helm: {} } }, 'x'),
    /overrides/,
  );
  assert.throws(
    () =>
      validate(
        {
          name: 'foo',
          network: { ingress: [] },
          overrides: { flux: { kustomization: { nope: 1 } } },
        },
        'x',
      ),
    /overrides/,
  );
  assert.throws(
    () =>
      validate(
        {
          name: 'foo',
          network: { ingress: [] },
          overrides: { argocd: { application: {} } },
        },
        'x',
      ),
    /overrides/,
  );
});

test('retained resources are kept when their release goes', () => {
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
  const plan = planApp(app, versions, target, { platform, apps: [], readValues });
  const oidc = plan.sources.find((s) => s.type === 'chart' && s.chart === 'k8s-oidc') as {
    values: Record<string, unknown>;
    retain?: boolean;
  };

  assert.equal(oidc.retain, true);
  assert.equal('retainAnnotations' in oidc.values, false);
  assert.deepEqual(releaseValues(render(app), 'k8s-oidc').retainAnnotations, {
    'helm.sh/resource-policy': 'keep',
  });
});

test('legacy route and argocd fields are rejected', () => {
  const route = {
    name: 'foo',
    type: 'route',
    gateways: [{ gateway: 'public-gateway', hosts: ['foo.example.com'] }],
    paths: ['/'],
    auth: { mode: 'none', reason: 'public' },
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
    gateways: [{ gateway: 'public-gateway', hosts: ['a.sh'] }],
    auth: { mode: 'none', reason: 'public' },
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
    () => validate({ ...base, resources: [without(route, 'gateways')] }, 'x'),
    /resources\[0\] requires property "gateways"/,
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
