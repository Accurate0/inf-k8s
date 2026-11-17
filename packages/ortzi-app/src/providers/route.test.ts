import assert from 'node:assert/strict';
import { test } from 'node:test';
import { planApp } from '../plan.ts';
import { type App, validate, validateGatewayAccess, validateResources } from '../render.ts';
import { versions, target, platform, render, source, base, checkRefs } from '../test-fixtures.ts';

test('routes to an explicit backend without a workload', () => {
  const rendered = render({
    name: 'foo',
    resources: [
      {
        name: 'foo-route',
        type: 'route',
        gateway: 'public-gateway',
        hosts: ['a.sh', 'b.sh'],
        backend: { name: 'svc', port: 80 },
        paths: ['/v1', { type: 'Exact', value: '/hook' }],
        auth: { mode: 'none', reason: 'public' },
      },
      {
        name: 'foo-api',
        type: 'route',
        paths: ['/'],
        hosts: ['c.sh'],
        gateway: 'internal-gateway',
        backend: { name: 'svc', port: 80 },
      },
      {
        name: 'foo',
        type: 'oidc',
        displayName: 'foo',
        preferShortUsername: true,
        host: 'c.sh',
        groups: { admins: ['x'] },
      },
      {
        name: 'foo-cli',
        type: 'oidc',
        displayName: 'foo-cli',
        preferShortUsername: true,
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

test('jwt routes pass their clients to the chart', () => {
  const rendered = render({
    name: 'foo',
    resources: [
      {
        name: 'api',
        type: 'route',
        gateway: 'public-gateway',
        paths: ['/'],
        hosts: ['a.sh'],
        backend: { name: 'svc', port: 80 },
        auth: { mode: 'jwt', clients: ['foo', 'foo-cli'] },
      },
      {
        name: 'web',
        type: 'route',
        gateway: 'public-gateway',
        paths: ['/'],
        hosts: ['a.sh'],
        backend: { name: 'svc', port: 80 },
        auth: { mode: 'app', reason: 'api keys' },
      },
      {
        name: 'foo',
        type: 'oidc',
        displayName: 'foo',
        preferShortUsername: true,
        host: 'a.sh',
        secretName: 'kanidm-foo-oidc',
      },
      {
        name: 'foo-cli',
        type: 'oidc',
        displayName: 'foo-cli',
        preferShortUsername: true,
        host: 'a.sh',
        secretName: 'kanidm-foo-cli-oidc',
      },
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
      {
        name: 'foo',
        type: 'route',
        gateway: 'public-gateway',
        paths: ['/'],
        hosts: ['a.sh'],
        backend: { name: 'svc', port: 80 },
        auth,
      },
      {
        name: 'foo',
        type: 'oidc',
        displayName: 'foo',
        preferShortUsername: true,
        host: 'a.sh',
        secretName: 'kanidm-foo-oidc',
        ...client,
      },
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
  assert.throws(() => validate(app({ mode: 'open' }), 'x'), /auth.mode is not one of enum values/);
  validate(app({ mode: 'oidc', client: 'foo' }), 'x');

  assert.throws(
    () => validate(app({ mode: 'oidc', client: 'foo' }, { securityPolicy: {} }), 'x'),
    /resources/,
  );
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
          resources: [
            {
              name: 'r',
              type: 'route',
              gateway: 'public-gateway',
              paths: ['/'],
              hosts: ['a'],
              backend: { name: 'x', port: 1 },
            },
          ],
        },
        'test',
      ),
    /resources\[0\] requires property "auth"/,
  );
});

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
        gateway: 'public-gateway',
        paths: ['/'],
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
      {
        name: 'r',
        type: 'route',
        gateway: 'public-gateway',
        hosts: [host],
        auth: { mode: 'none', reason: 'public' },
      },
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

test('route dns becomes external-dns annotations', () => {
  const route = (dns: unknown) => ({
    name: 'foo',
    type: 'route',
    gateway: 'public-gateway',
    paths: ['/'],
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
          gateway: 'public-gateway',
          paths: ['/'],
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

test('route loadBalancer renders to envoy and checks headers', () => {
  const app = (loadBalancer: Record<string, unknown>) => ({
    name: 'foo',
    resources: [
      {
        name: 'web',
        type: 'route',
        gateway: 'public-gateway',
        paths: ['/'],
        hosts: ['a.sh'],
        backend: { name: 'svc', port: 80 },
        auth: { mode: 'app', reason: 'sessions' },
        loadBalancer,
      },
    ],
    network: { ingress: [] },
  });

  const [route] = source(
    render(validate(app({ type: 'consistentHash', headers: ['Cookie', 'User-Agent'] }), 'x')),
    'k8s-http-routes',
  )?.helm.valuesObject.routes;
  assert.deepEqual(route.loadBalancer, {
    type: 'ConsistentHash',
    consistentHash: { type: 'Headers', headers: [{ name: 'Cookie' }, { name: 'User-Agent' }] },
  });

  const [plain] = source(render(validate(app({ type: 'leastRequest' }), 'x')), 'k8s-http-routes')
    ?.helm.valuesObject.routes;
  assert.deepEqual(plain.loadBalancer, { type: 'LeastRequest' });

  assert.throws(() => validate(app({ type: 'consistentHash' }), 'x'), /headers is required/);
  assert.throws(
    () => validate(app({ type: 'random', headers: ['Cookie'] }), 'x'),
    /only for consistentHash/,
  );
});

test('a tcp route attaches to a gateway listener', () => {
  const tcp = (extra: Record<string, unknown> = {}) =>
    validate(
      {
        ...base,
        resources: [
          {
            name: 'ssh',
            type: 'route',
            gateway: 'public-gateway',
            protocol: 'tcp',
            listener: 'git-ssh',
            auth: { mode: 'app', reason: 'ssh keys' },
            backend: { name: 'foo', port: 22 },
            ...extra,
          },
        ],
        workloads: [
          {
            name: 'foo',
            image: 'i',
            ports: [{ name: 'ssh', container: 2222, service: 22 }],
            service: { name: 'foo' },
          },
        ],
      },
      'test',
    );
  const app = tcp();
  validateResources({ platform, apps: [app] });
  const rendered = render(app);

  assert.deepEqual(source(rendered, 'k8s-http-routes')?.helm.valuesObject, {
    tcpRoutes: [
      {
        name: 'ssh',
        gateway: { name: 'public-gateway', namespace: 'envoy-gateway-system' },
        listener: 'git-ssh',
        backend: { name: 'foo', port: 22 },
      },
    ],
    routes: [],
  });
  assert.deepEqual(source(rendered, 'k8s-network')?.helm.valuesObject.ingress, [
    'envoy-gateway-system',
  ]);

  assert.throws(() => tcp({ listener: undefined }), /listener is required for protocol tcp/);
  assert.throws(() => tcp({ hosts: ['a.sh'] }), /hosts is only for protocol http/);
  assert.throws(() => tcp({ dns: { provider: 'unifi' } }), /dns is only for protocol http/);
  assert.throws(
    () => tcp({ auth: { mode: 'jwt', clients: ['x'] } }),
    /auth.mode must be app or none for protocol tcp/,
  );
  assert.throws(() => tcp({ auth: { mode: 'none', reason: ' ' } }), /auth.reason is required/);
  assert.throws(
    () =>
      validate(
        {
          ...base,
          resources: [
            {
              name: 'web',
              type: 'route',
              gateway: 'public-gateway',
              paths: ['/'],
              listener: 'git-ssh',
              hosts: ['a.sh'],
              auth: { mode: 'none', reason: 'public' },
              backend: { name: 'foo', port: 80 },
            },
          ],
        },
        'test',
      ),
    /listener is only for protocol tcp/,
  );
  assert.throws(
    () => validateResources({ platform, apps: [tcp({ backend: { name: 'foo', port: 23 } })] }),
    /service foo has no port 23/,
  );

  const listeners = [
    {
      gateway: 'public-gateway',
      name: 'git-ssh',
      protocol: 'TCP',
      labels: { 'kubernetes.io/metadata.name': 'foo' },
    },
    { gateway: 'public-gateway', name: 'web', protocol: 'HTTPS', hostname: 'a.sh', labels: {} },
  ];
  validateGatewayAccess([app], listeners);
  assert.throws(
    () => validateGatewayAccess([tcp({ listener: 'web' })], listeners),
    /no TCP listener web on public-gateway/,
  );
  assert.throws(
    () =>
      validateGatewayAccess([{ ...app, name: 'bar' }], [{ ...listeners[0], labels: { x: 'y' } }]),
    /git-ssh needs namespace.labels x/,
  );
});
