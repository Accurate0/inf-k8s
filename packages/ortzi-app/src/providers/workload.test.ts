import assert from 'node:assert/strict';
import { test } from 'node:test';
import { helmTemplates, planApp } from '../plan.ts';
import { renderFlux } from '../outputs/flux.ts';
import { parse, parseAllDocuments } from 'yaml';
import {
  type App,
  appWorkloads,
  passthroughWarnings,
  containerEnv,
  renderImageValues,
  validate,
  validateResources,
} from '../render.ts';
import {
  versions,
  target,
  platform,
  render,
  base,
  checkRefs,
  releaseValues,
  readValues,
} from '../test-fixtures.ts';

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
  return releaseValues(render(app), 'k8s-workload').workloads[0];
};

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
        gateway: 'public-gateway',
        paths: ['/'],
        hosts: ['foo.anurag.sh'],
        backend: { name: 'web', port: 8080 },
        auth: { mode: 'none', reason: 'public' },
      },
      { name: 'foo', type: 'secret', secretName: 'foo-managed-secrets' },
    ],
    network: { ingress: [] },
  });
  const values = releaseValues(rendered, 'k8s-workload');

  assert.deepEqual(
    values.workloads.map((w: { name: string }) => w.name),
    ['api', 'web', 'sync'],
  );
  assert.equal(values.workloads[2].envFrom, undefined);
  assert.deepEqual(releaseValues(rendered, 'k8s-http-routes').routes[0].rules[0].backendRefs, [
    { group: '', kind: 'Service', name: 'web', port: 8080, weight: 1 },
  ]);
});

test('a Job runs as a hook in its phase', () => {
  const rendered = render({
    name: 'foo',
    workloads: [{ name: 'migrate', image: 'm', kind: 'Job', phase: 'preSync' }],
    network: { ingress: [] },
  });

  const [job] = releaseValues(rendered, 'k8s-workload').workloads;
  assert.equal(job.kind, 'Job');
  assert.equal(job.phase, 'preSync');
});

test('rejects a Job without a phase, or a phase without a Job', () => {
  const app = (workload: Record<string, unknown>) => ({
    name: 'foo',
    workloads: [{ name: 'migrate', image: 'm', ...workload }],
    network: { ingress: [] },
  });

  assert.throws(() => validate(app({ kind: 'Job' }), 'x'), /phase is required for a Job/);
  assert.throws(() => validate(app({ phase: 'preSync' }), 'x'), /phase is only for a Job/);
  assert.throws(
    () =>
      validate(
        app({
          kind: 'Job',
          phase: 'preSync',
          ports: [{ name: 'http', container: 80, service: 80 }],
          service: { name: 'migrate' },
        }),
        'x',
      ),
    /a Job cannot expose ports/,
  );
  assert.throws(() => validate(app({ kind: 'Job', phase: 'during' }), 'x'), /phase/);
  validate(app({ kind: 'Job', phase: 'preSync' }), 'x');
  validate(app({ kind: 'Job', phase: 'postSync' }), 'x');
});

test('rejects cluster addresses written into env', () => {
  const app = (env: Record<string, unknown>, sidecar = false) => ({
    name: 'foo',
    workloads: [
      {
        name: 'api',
        image: 'i',
        ...(sidecar ? { sidecars: [{ name: 's', image: 's', env }] } : { env }),
      },
    ],
    network: { ingress: [] },
  });

  assert.throws(
    () => validate(app({ URL: 'http://api.bar.svc.cluster.local:80' }), 'x'),
    /env.URL: use \{service\} or \{resource\}/,
  );
  assert.throws(
    () => validate(app({ URL: 'api.bar.svc.cluster.local:50051' }, true), 'x'),
    /env.URL/,
  );
  validate(app({ URL: 'https://foo.anurag.sh' }), 'x');
});

test('a sidecar inherits env, envFrom and mounts from its workload', () => {
  const app = {
    name: 'foo',
    network: { ingress: [] },
    workloads: [
      {
        name: 'web',
        image: 'web',
        env: { A: '1', B: '2' },
        envFrom: [{ secret: 'foo' }],
        mounts: { tmp: { path: '/tmp', emptyDir: true } },
        sidecars: [{ name: 'worker', image: 'web', inherit: ['env', 'mounts'], env: { B: '3' } }],
      },
    ],
  };

  const [sidecar] = appWorkloads(app as unknown as App).web.sidecars ?? [];

  assert.deepEqual(sidecar, {
    name: 'worker',
    image: 'web',
    env: { A: '1', B: '3' },
    mounts: { tmp: { path: '/tmp', emptyDir: true } },
  });
});

test('a sidecar cannot inherit what its workload does not have', () => {
  const app = {
    name: 'foo',
    network: { ingress: [] },
    workloads: [
      {
        name: 'web',
        image: 'web',
        sidecars: [{ name: 'worker', image: 'web', inherit: ['mounts'] }],
      },
    ],
  };

  assert.throws(
    () => appWorkloads(app as unknown as App),
    /foo: workloads\.web: sidecar worker inherits mounts, but the workload has none/,
  );
});

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

test('rejects unknown workload fields', () => {
  assert.throws(() => short({ uses: ['a'] }), /uses/);
  assert.throws(() => short({ strategy: {} }), /strategy/);
});

test('rejects a build image no workload runs', () => {
  const build = (image: string) => ({
    build: { ci: 'forgejo', image },
    workloads: [{ name: 'foo', image: 'i' }],
  });
  checkRefs(build('i'));

  assert.throws(() => checkRefs(build('j')), /build.image j/);
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
  const workload = releaseValues(render(app), 'k8s-workload').workloads[0];

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

test('workloads are a list with unique names', () => {
  const app = (workloads: unknown) => ({ name: 'foo', network: { ingress: [] }, workloads });
  validate(app([{ name: 'foo', image: 'i' }]), 'x');

  assert.throws(
    () => validate(app({ foo: { image: 'i' } }), 'x'),
    /workloads is not of a type\(s\) array/,
  );
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
    const source = planApp(built, versions, target, { platform, apps: [built], readValues })
      .sources[0] as {
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

test('autoscale passes through and rejects what an hpa cannot use', () => {
  const workload = { resources: { cpu: '50m', memory: '64Mi' } };
  const autoscale = { min: 2, max: 5, cpu: 70 };

  assert.deepEqual(short({ ...workload, autoscale }).autoscale, autoscale);
  assert.throws(() => short({ ...workload, autoscale, replicas: 3 }), /replicas or autoscale/);
  assert.throws(
    () => short({ ...workload, autoscale, kind: 'CronJob', schedule: '* * * * *' }),
    /only for a Deployment/,
  );
  assert.throws(() => short({ ...workload, autoscale: { ...autoscale, min: 6 } }), /more than max/);
  assert.throws(() => short({ autoscale }), /resources.cpu on every container/);
  assert.throws(
    () => short({ ...workload, autoscale, sidecars: [{ name: 's', image: 's' }] }),
    /resources.cpu on every container/,
  );
  assert.throws(() => short({ ...workload, autoscale: { min: 1, max: 2 } }), /cpu/);
});

test('expands exec probes and rejects more than one target', () => {
  const command = ['sh', '-c', 'pgrep -f sidekiq'];
  const out = short({ probes: { exec: { command }, liveness: { every: 30 } } });

  assert.deepEqual(out.livenessProbe, { exec: { command }, periodSeconds: 30 });
  assert.throws(
    () => short({ probes: { exec: { command }, http: { path: '/', port: 80 }, liveness: {} } }),
    /use one of http, exec/,
  );
});

test('describes where each env var comes from', () => {
  const app = validate(
    {
      name: 'foo',
      network: { ingress: [] },
      resources: [
        { name: 'traces', type: 'platform', export: 'tracing' },
        { name: 'foo', type: 'secret', secretName: 'foo-secret' },
      ],
      workloads: [
        {
          name: 'api',
          image: 'i',
          ports: [{ name: 'http', container: 3000, service: 80 }],
          service: { name: 'api' },
          env: {
            A: 'b',
            T: { resource: 'traces' },
            S: { secret: 'foo', key: 'K', optional: true },
            U: { service: 'api', port: 'http', path: '/v1' },
          },
          envFrom: [{ secret: 'foo' }],
        },
      ],
    },
    'x',
  );

  const [{ container, rows }] = containerEnv(app, { platform, apps: [app] });
  assert.equal(container, 'api');
  assert.deepEqual(rows.slice(0, 3), [
    { name: 'A', from: 'literal', value: 'b' },
    { name: 'T', from: 'resource traces', value: 'http://tempo/traces' },
    { name: 'S', from: 'secret foo', secret: { name: 'foo-secret', key: 'K', optional: true } },
  ]);
  assert.equal(rows[3].from, 'service api:http/v1');
  assert.deepEqual(rows[4], {
    name: 'envFrom',
    from: 'secret foo',
    secret: { name: 'foo-secret', optional: false },
  });
});

test('init container and sidecar volumes join the pod', () => {
  const out = short(
    {
      mounts: { data: { path: '/data', emptyDir: true } },
      initContainers: [
        {
          name: 'init',
          image: 'i',
          mounts: {
            template: { path: '/template', config: 'template' },
            data: { path: '/data', emptyDir: true },
          },
        },
      ],
    },
    {
      resources: [{ name: 'template', type: 'config', configMap: 'foo-config', files: ['a.json'] }],
    },
  );

  assert.deepEqual(
    out.volumes.map((v: { name: string }) => v.name),
    ['data', 'template'],
  );
  assert.equal(out.initContainers[0].volumes, undefined);
  assert.deepEqual(out.initContainers[0].volumeMounts, [
    { name: 'template', mountPath: '/template', readOnly: true },
    { name: 'data', mountPath: '/data' },
  ]);
  assert.throws(
    () =>
      short({
        mounts: { data: { path: '/data', emptyDir: true } },
        sidecars: [{ name: 's', image: 's', mounts: { data: { path: '/d', configMap: 'x' } } }],
      }),
    /mounted from different sources/,
  );
});

test('warns about raw env lists, envFrom entries and configMap mounts', () => {
  const app = {
    name: 'foo',
    network: { ingress: [] },
    workloads: [
      {
        name: 'web',
        image: 'web',
        env: [{ name: 'A', value: '1' }],
        envFrom: [{ secretRef: { name: 'raw' } }, { secret: 'declared' }],
        mounts: { cfg: { path: '/etc/cfg', configMap: 'raw-config' } },
      },
    ],
  } as unknown as App;

  assert.deepEqual(passthroughWarnings(app), [
    'foo: workloads.web.env is a raw list; use a map with refs',
    'foo: workloads.web.envFrom has a raw entry; reference a secret, database, bucket or oidc resource',
    'foo: workloads.web.mounts.cfg mounts a raw configMap; declare a config resource',
  ]);
});

test('released images follow their newest main build through a flux image policy', () => {
  const app = (workloads: Record<string, unknown>[]) => ({
    name: 'foo',
    network: { ingress: [] },
    workloads,
  });
  const image = 'registry.anurag.sh/containers/foo';
  const sidecar = 'registry.anurag.sh/containers/foo-sidecar';
  const released = validate(
    app([
      {
        name: 'api',
        image,
        release: 'latest',
        sidecars: [{ name: 'side', image: sidecar, release: 'latest' }],
      },
      { name: 'worker', image, release: 'latest' },
      { name: 'curl', image: 'curlimages/curl' },
    ]),
    'x',
  );

  const plan = planApp(released, versions, target, {
    platform,
    apps: [released],
    readValues: () => ({ images: { 'curlimages/curl': '8.10.1', [image]: 'main-1-abc' } }),
  });
  assert.deepEqual(plan.releases, [image, sidecar]);
  const workload = plan.sources.find((s) => s.type === 'chart' && s.chart === 'k8s-workload');
  const values = workload?.type === 'chart' ? workload.values : {};
  assert.deepEqual(values.images, { 'curlimages/curl': '8.10.1', [image]: 'main-1-abc' });
  assert.equal('release' in values.workloads[0], false);
  assert.equal('release' in values.workloads[0].sidecars[0], false);
  assert.deepEqual(JSON.parse(helmTemplates(plan)[0].values).images, values.images);

  const flux = renderFlux(plan);
  const [repository] = parseAllDocuments(flux.get('kubernetes/foo/app/imagerepository.yaml')!).map(
    (d) => d.toJS(),
  );
  assert.equal(repository.metadata.name, 'foo-foo');
  assert.equal(repository.spec.image, image);
  const [policy, sidecarPolicy] = parseAllDocuments(
    flux.get('kubernetes/foo/app/imagepolicy.yaml')!,
  ).map((d) => d.toJS());
  assert.equal(policy.spec.imageRepositoryRef.name, 'foo-foo');
  assert.equal(policy.spec.filterTags.extract, '$ts');
  assert.deepEqual(policy.spec.policy, { numerical: { order: 'asc' } });
  assert.equal(sidecarPolicy.metadata.name, 'foo-foo-sidecar');
  assert.match(flux.get('kubernetes/foo/app/kustomization.yaml')!, /- imagepolicy.yaml/);

  const values_ = renderImageValues(
    released,
    'images:\n  registry.anurag.sh/containers/foo: main-1-abc\n',
  );
  assert.equal(
    values_,
    [
      'images:',
      '  registry.anurag.sh/containers/foo: main-1-abc # {"$imagepolicy": "flux-system:foo-foo:tag"}',
      '  registry.anurag.sh/containers/foo-sidecar: latest # {"$imagepolicy": "flux-system:foo-foo-sidecar:tag"}',
      '  curlimages/curl: latest',
      '',
    ].join('\n'),
  );
  assert.equal(renderImageValues(released, values_), values_);
  assert.equal(parse(values_).images[image], 'main-1-abc');

  assert.throws(
    () =>
      validate(
        app([
          { name: 'api', image, release: 'latest' },
          { name: 'worker', image },
        ]),
        'x',
      ),
    /released elsewhere; set release here too/,
  );
  assert.throws(
    () => validate(app([{ name: 'api', image: 'ghcr.io/x/foo', release: 'latest' }]), 'x'),
    /only registry\.anurag\.sh\/ images follow their builds/,
  );
});
