import { stringify } from 'yaml';

const CONFIG_REPO = 'https://github.com/Accurate0/inf-k8s.git';
const CHART_REGISTRY = 'oci://ghcr.io/accurate0/charts';
const SCHEMA = 'https://k8s-schemas.anurag.sh/argoproj.io/application_v1alpha1.json';

export const HEADER = '# generated from app.yaml by `ortzi app render`; edit app.yaml instead';

const STAMP = /^# generated at .*\n/m;

export const stamped = (rendered: string, now: Date) =>
  rendered.replace(`${HEADER}\n`, `${HEADER}\n# generated at ${now.toISOString()}\n`);

export const isCurrent = (existing: string, rendered: string) =>
  STAMP.test(existing) && existing.replace(STAMP, '') === rendered;

export interface Workload {
  image: string;
  port?: number;
  kind?: 'Deployment' | 'DaemonSet' | 'CronJob';
  replicas?: number;
  schedule?: string;
  service?: {
    name?: string;
    port?: number;
    portName?: string;
    ports?: Record<string, unknown>[];
    trafficDistribution?: string;
  };
  initContainers?: { image: string }[];
  sidecars?: { image: string }[];
  health?: string;
  resources?: Record<string, unknown>;
  env?: Record<string, unknown>[];
  envFrom?: Record<string, unknown>[];
  readOnlyRootFilesystem?: boolean;
  secrets?: string[];
  databases?: string[];
}

export type Secret = string | { name: string; secretName: string };

export type RouteAuth =
  | { mode: 'jwt'; clients: string[] }
  | { mode: 'oidc'; client: string }
  | { mode: 'app' | 'none'; reason: string };

export interface Route {
  hosts: string[];
  gateway?: string;
  auth: RouteAuth;
  workload?: string;
  backend?: { name: string; port: number };
  paths?: (string | { type: string; value: string })[];
  annotations?: Record<string, string>;
  ratelimit?: Record<string, unknown>;
  timeouts?: Record<string, unknown>;
}

export type OidcClient = { host?: string; displayName?: string } & Record<string, unknown>;

export interface Build {
  image: string;
  context?: string;
  dockerfile?: string;
  args?: Record<string, string>;
  paths?: string[];
  test?: {
    run: string;
    db?: string;
    dbName?: string;
    packages?: string[];
    setup?: string;
  };
}

export interface App {
  name: string;
  repository?: string;
  charts?: Record<string, string>;
  workloads?: Record<string, Workload>;
  routes?: Record<string, Route>;
  oidc?: Record<string, OidcClient>;
  databases?: Record<string, { databaseName?: string; secretName?: string; enableConnectionPooling?: boolean }>;
  secrets?: Secret[];
  sources?: Record<string, unknown>[];
  build?: Build;
  network: { ingress: string[] };
  metrics?: { path?: string; port?: string; interval?: string; workload?: string };
  namespace?: { labels?: Record<string, string> };
  serviceAccounts?: string[];
}

export type Versions = Record<string, string>;

export interface Target {
  location: string;
  hasManifests: boolean;
  rootKustomization?: boolean;
}

const secretFor = (secret: Secret) =>
  typeof secret === 'string'
    ? { name: secret, secretName: `${secret}-managed-secrets` }
    : secret;

function served(app: App, wanted: string | undefined, block: string): [string, Workload] {
  const workloads = Object.entries(app.workloads ?? {}).filter(([, w]) => w.port || w.service?.ports);
  const found = wanted ? workloads.find(([name]) => name === wanted) : workloads[0];
  if (!found) throw new Error(`${block} needs a workload with a port`);
  if (!wanted && workloads.length > 1) throw new Error(`${block}.workload is required with several ported workloads`);
  return found;
}

const ALLOWED = new Set(['name', 'repository', 'charts', 'workloads', 'routes', 'oidc', 'databases', 'secrets', 'sources', 'network', 'namespace', 'serviceAccounts', 'metrics', 'build', 'lint']);

export function validate(raw: unknown, where: string): App {
  const app = raw as App;
  const fail = (message: string): never => {
    throw new Error(`${where}: ${message}`);
  };
  if (typeof app !== 'object' || app === null) fail('expected a mapping');
  for (const key of Object.keys(app)) {
    if (!ALLOWED.has(key)) fail(`unknown block "${key}"`);
  }
  if (!/^[a-z0-9]([a-z0-9-]*[a-z0-9])?$/.test(app.name ?? '')) fail('name must be kebab-case');
  if (!Array.isArray(app.network?.ingress)) {
    fail('network.ingress is required (use [] for none)');
  }
  for (const [name, workload] of Object.entries(app.workloads ?? {})) {
    if (!workload.image) fail(`workloads.${name}.image is required`);
  }
  const routes = Object.entries(app.routes ?? {});
  const clients = Object.entries(app.oidc ?? {});
  for (const [name, route] of routes) {
    if (!route.hosts?.length) fail(`routes.${name}.hosts is required`);
    const auth = route.auth;
    if (!auth) {
      fail(`routes.${name}.auth is required`);
    } else if (auth.mode === 'jwt') {
      if (!auth.clients?.length) fail(`routes.${name}.auth.clients is required for jwt`);
      for (const client of auth.clients ?? []) {
        if (!app.oidc?.[client]) fail(`routes.${name}.auth: no oidc client named ${client}`);
      }
    } else if (auth.mode === 'oidc') {
      const client = app.oidc?.[auth.client];
      if (!client) fail(`routes.${name}.auth: no oidc client named ${auth.client}`);
      const policy = client?.securityPolicy as { httpRoute?: string } | undefined;
      if (!policy || (policy.httpRoute ?? auth.client) !== name) {
        fail(`routes.${name}.auth: oidc.${auth.client}.securityPolicy must target this route`);
      }
    } else if (auth.mode === 'app' || auth.mode === 'none') {
      if (!auth.reason?.trim()) fail(`routes.${name}.auth.reason is required for ${auth.mode}`);
    } else {
      fail(`routes.${name}.auth.mode must be jwt, oidc, app or none`);
    }
  }
  for (const [name, client] of clients) {
    if (!client.host && !routes.length) fail(`oidc.${name} needs a route or a host`);
  }
  const secretNames = (app.secrets ?? []).map(secretFor).map((s) => s.name);
  for (const [name, workload] of Object.entries(app.workloads ?? {})) {
    for (const secret of workload.secrets ?? []) {
      if (!secretNames.includes(secret)) fail(`workloads.${name}.secrets: no secret named ${secret}`);
    }
    for (const database of workload.databases ?? []) {
      if (!app.databases?.[database]) fail(`workloads.${name}.databases: no database named ${database}`);
    }
  }
  try {
    for (const [name, route] of routes) {
      if (!route.backend) served(app, route.workload, `routes.${name}`);
    }
    if (app.metrics) served(app, app.metrics.workload, 'metrics');
  } catch (e) {
    fail((e as Error).message);
  }
  return app;
}

function chart(app: App, versions: Versions, name: string, helm: Record<string, unknown>) {
  const targetRevision = app.charts?.[name] ?? versions[name];
  if (!targetRevision) throw new Error(`${app.name}: no version for chart ${name}`);
  return { chart: name, repoURL: `${CHART_REGISTRY}/${name}`, targetRevision, helm };
}

export const workloadImages = (app: App) =>
  Object.values(app.workloads ?? {}).flatMap((w) => [
    w.image,
    ...(w.initContainers ?? []).map((c) => c.image),
    ...(w.sidecars ?? []).map((c) => c.image),
  ]);

export const imageValuesPath = (app: App, target: Target) =>
  `${target.location}/${app.name}/image.values.yaml`;

export const workflowPath = (app: App) => `.github/workflows/app-${app.name}.yaml`;

export function renderWorkflow(app: App, target: Target): string {
  const build = app.build as Build;
  const dir = `${target.location}/${app.name}`;
  const context = build.context ?? dir;
  const args = Object.entries(build.args ?? {}).map(([k, v]) => `${k}=${v}`);
  const test = build.test;
  const workflow = {
    name: `app ${app.name}`,
    on: {
      push: {
        paths: [
          `${dir}/**`,
          `!${dir}/manifests/**`,
          `!${dir}/application.yaml`,
          `!${dir}/app.yaml`,
          `!${dir}/image.values.yaml`,
          ...(build.paths ?? []),
          workflowPath(app),
        ],
      },
      workflow_dispatch: null,
    },
    jobs: {
      app: {
        permissions: { contents: 'read', packages: 'write' },
        uses: './.github/workflows/app-build.yaml',
        with: {
          service: app.name,
          directory: dir,
          image: build.image,
          context,
          dockerfile: build.dockerfile ?? `${dir}/Dockerfile`,
          ...(args.length ? { 'build-args': args.join('\n') } : {}),
          ...(test
            ? {
                'test-run': test.run,
                ...(test.db ? { 'test-db-image': test.db } : {}),
                ...(test.dbName ? { 'test-db-name': test.dbName } : {}),
                ...(test.packages ? { 'test-packages': test.packages.join(' ') } : {}),
                ...(test.setup ? { 'test-setup': test.setup } : {}),
              }
            : {}),
          deploy: "${{ github.ref == 'refs/heads/main' }}",
        },
        secrets: 'inherit',
      },
    },
  };
  return `${HEADER}\n${stringify(workflow, { lineWidth: 0, aliasDuplicateObjects: false })}`;
}

export function renderApplication(app: App, versions: Versions, target: Target): string {
  const { name } = app;
  const annotations = {
    'inf-k8s.net/app': name,
    'inf-k8s.net/repository': app.repository ?? 'https://github.com/Accurate0/inf-k8s',
  };
  const databaseSecret = (database: string) =>
    app.databases?.[database].secretName ?? `${database}-database-secret`;
  const dir = `${target.location}/${name}`;
  const routes = Object.entries(app.routes ?? {});
  const sources: Record<string, unknown>[] = [];

  if (app.workloads && Object.keys(app.workloads).length) {
    const secrets = (app.secrets ?? []).map(secretFor);
    sources.push({ repoURL: CONFIG_REPO, ref: 'values' });
    sources.push(
      chart(app, versions, 'k8s-workload', {
        valueFiles: [`$values/${imageValuesPath(app, target)}`],
        valuesObject: {
          namespace: name,
          ...(routes.length || app.namespace?.labels
            ? {
                namespaceLabels:
                  app.namespace?.labels ??
                  Object.fromEntries(
                    routes.flatMap(([, route]) =>
                      (route.gateway ?? 'public-gateway') === 'internal-gateway'
                        ? [['internal-gateway-access', 'true']]
                        : route.hosts.map((host) => [
                            `gateway.inf-k8s.net/${host.replaceAll('.', '-')}`,
                            'true',
                          ]),
                    ),
                  ),
              }
            : {}),
          ...(app.serviceAccounts ? { serviceAccounts: app.serviceAccounts } : {}),
          workloads: Object.entries(app.workloads).map(([workload, spec]) => {
            const { health, envFrom, secrets: only, databases: dbs, ...rest } = spec;
            const chosen = dbs ?? Object.keys(app.databases ?? {});
            const from = [
              ...(envFrom ?? []),
              ...secrets
                .filter((s) => !only || only.includes(s.name))
                .map((s) => ({ secretRef: { name: s.secretName } })),
              ...chosen.map((database) => ({
                ...(chosen.length > 1
                  ? { prefix: `${database.toUpperCase().replaceAll('-', '_')}_` }
                  : {}),
                secretRef: { name: databaseSecret(database) },
              })),
            ];
            return {
              name: workload,
              ...rest,
              ...(health ? { probes: { path: health } } : {}),
              ...(from.length ? { envFrom: from } : {}),
            };
          }),
        },
      }),
    );
  }

  const ingress = [
    ...(routes.length ? ['envoy-gateway-system'] : []),
    ...(app.metrics ? ['monitoring'] : []),
    ...app.network.ingress,
  ];
  sources.push(
    chart(app, versions, 'k8s-network', {
      valuesObject: { namespace: name, ingress: [...new Set(ingress)] },
    }),
  );

  if (routes.length) {
    sources.push(
      chart(app, versions, 'k8s-http-routes', {
        valuesObject: {
          routes: routes.map(([routeName, route]) => {
            let backend = route.backend;
            if (!backend) {
              const [workload, spec] = served(app, route.workload, `routes.${routeName}`);
              backend = {
                name: spec.service?.name ?? workload,
                port: spec.service?.port ?? (spec.port as number),
              };
            }
            return {
              name: routeName,
              ...(route.auth?.mode === 'jwt' ? { jwtClients: route.auth.clients } : {}),
              gateway: {
                name: route.gateway ?? 'public-gateway',
                namespace: 'envoy-gateway-system',
              },
              ...(route.annotations ? { annotations: route.annotations } : {}),
              hostnames: route.hosts,
              ...(route.ratelimit ? { ratelimit: route.ratelimit } : {}),
              rules: [
                {
                  backendRefs: [{ group: '', kind: 'Service', ...backend, weight: 1 }],
                  matches: (route.paths ?? ['/']).map((match) => ({
                    path: typeof match === 'string' ? { type: 'PathPrefix', value: match } : match,
                  })),
                  ...(route.timeouts ? { timeouts: route.timeouts } : {}),
                },
              ],
            };
          }),
        },
      }),
    );
  }

  if (app.oidc && Object.keys(app.oidc).length) {
    sources.push(
      chart(app, versions, 'k8s-oidc', {
        valuesObject: {
          clients: Object.entries(app.oidc).map(([client, spec]) => ({
            name: client,
            namespace: name,
            displayName: client,
            host: routes[0]?.[1].hosts[0],
            preferShortUsername: true,
            ...spec,
            secretName: `kanidm-${client}-oidc`,
            secretNamespace: name,
            annotations,
          })),
        },
      }),
    );
  }

  if (app.databases && Object.keys(app.databases).length) {
    sources.push(
      chart(app, versions, 'k8s-postgres', {
        valuesObject: {
          databases: Object.entries(app.databases).map(([database, spec]) => ({
            name: database,
            namespace: name,
            databaseName: spec.databaseName ?? database.replaceAll('-', '_'),
            secretName: databaseSecret(database),
            ...(spec.enableConnectionPooling === undefined ? {} : { enableConnectionPooling: spec.enableConnectionPooling }),
            annotations,
          })),
        },
      }),
    );
  }

  if (app.secrets?.length) {
    sources.push(
      chart(app, versions, 'k8s-secrets', {
        valuesObject: {
          defaults: { namespace: name, provider: 'openbao', resyncInterval: 900, annotations },
          secrets: app.secrets.map(secretFor).map((s) => ({
            name: s.name,
            managedSecrets: [{ secretName: s.secretName }],
          })),
        },
      }),
    );
  }

  if (app.metrics) {
    const { workload: _workload, ...metrics } = app.metrics;
    const [workload, spec] = served(app, app.metrics.workload, 'metrics');
    sources.push(
      chart(app, versions, 'k8s-observability', {
        valuesObject: {
          name: workload,
          namespace: name,
          ...(spec.service?.portName ? { port: spec.service.portName } : {}),
          ...metrics,
        },
      }),
    );
  }

  if (target.hasManifests || target.rootKustomization) {
    sources.push({ repoURL: CONFIG_REPO, name: 'application', path: target.rootKustomization ? dir : `${dir}/manifests` });
  }
  sources.push(...(app.sources ?? []));

  const application = {
    apiVersion: 'argoproj.io/v1alpha1',
    kind: 'Application',
    metadata: { name, namespace: 'argocd' },
    spec: {
      syncPolicy: {
        syncOptions: ['CreateNamespace=true', 'ServerSideApply=true'],
        automated: { prune: true, selfHeal: true },
      },
      project: target.location,
      sources,
      destination: { server: 'https://kubernetes.default.svc', namespace: name },
    },
    operation: {
      initiatedBy: { username: 'github-actions' },
      sync: { prune: true, syncStrategy: { hook: {} } },
    },
  };

  return `${HEADER}\n# yaml-language-server: $schema=${SCHEMA}\n${stringify(application, { lineWidth: 0, aliasDuplicateObjects: false })}`;
}
