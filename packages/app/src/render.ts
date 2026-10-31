import { type Schema, Validator } from 'jsonschema/lib/index.js';
import { parse, parseDocument, stringify } from 'yaml';
import schema from '../app.schema.json' with { type: 'json' };
import { expandWorkload, overrideConflicts, type ResolveResource } from './workload.ts';

const CONFIG_REPO = 'https://github.com/Accurate0/inf-k8s.git';
export const CHART_REGISTRY = 'oci://ghcr.io/accurate0/charts';
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
  env?: Record<string, unknown>[] | Record<string, unknown>;
  envFrom?: (string | Record<string, unknown>)[];
  probes?: Record<string, unknown>;
  mounts?: Record<string, unknown>;
  overrides?: Record<string, unknown>;
  readOnlyRootFilesystem?: boolean;
}

export type Resource =
  | { name: string; type: 'tracing' }
  | { name: string; type: 'app'; app: string; export: string; path?: string };

export interface Export {
  name: string;
  workload: string;
  port?: string | number;
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
  resources?: Resource[];
  exports?: Export[];
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

export interface PlatformExport {
  name: string;
  type: string;
  url: string;
}

export interface World {
  platform: PlatformExport[];
  apps: App[];
}

function exportPort(provider: App, exported: Export): number {
  const spec = provider.workloads?.[exported.workload];
  const ports = (spec?.service?.ports ?? []) as { name?: string; port: number }[];
  const port =
    typeof exported.port === 'number'
      ? exported.port
      : exported.port
        ? ports.find((p) => p.name === exported.port)?.port
        : (spec && servedPort(spec));
  if (!port) throw new Error(`${provider.name}: exports.${exported.name}: no matching port on workload ${exported.workload}`);
  return port;
}

function appExportUrl(app: App, resource: Extract<Resource, { type: 'app' }>, apps: App[]): string {
  const provider = apps.find((a) => a.name === resource.app);
  if (!provider) throw new Error(`${app.name}: resources.${resource.name}: no app named ${resource.app}`);
  const exported = provider.exports?.find((e) => e.name === resource.export);
  if (!exported) throw new Error(`${app.name}: resources.${resource.name}: ${resource.app} has no export named ${resource.export}`);
  const service = provider.workloads?.[exported.workload]?.service?.name ?? exported.workload;
  const port = exportPort(provider, exported);
  return `http://${service}.${provider.name}.svc.cluster.local${port === 80 ? '' : `:${port}`}${resource.path ?? ''}`;
}

function resourceEnv(app: App, name: string, env: string, { platform, apps }: World): { name: string; value: string } {
  const resource = app.resources?.find((r) => r.name === name) as Resource;
  if (resource.type === 'app') return { name: env, value: appExportUrl(app, resource, apps) };
  const provided = platform.filter((e) => e.type === resource.type);
  if (provided.length !== 1) {
    throw new Error(`${app.name}: resources.${name}: expected one platform export of type ${resource.type}, found ${provided.length}`);
  }
  return { name: env, value: provided[0].url };
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

function servedPort(spec: Workload): number | undefined {
  const ports = (spec.service?.ports ?? []) as { port: number }[];
  return spec.service?.port ?? spec.port ?? (ports.length === 1 ? ports[0].port : undefined);
}

const appSchema = new Validator();

const ALLOWED = new Set(['name', 'repository', 'charts', 'workloads', 'routes', 'oidc', 'databases', 'secrets', 'resources', 'exports', 'sources', 'network', 'namespace', 'serviceAccounts', 'metrics', 'build', 'lint']);

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
    if (workload.health && !workload.port) fail(`workloads.${name}.health needs port`);
    if (workload.health && workload.probes) fail(`workloads.${name}: use health or probes, not both`);
    for (const key of overrideConflicts(workload)) fail(`workloads.${name}.overrides.${key} is already set on the workload`);
    for (const value of Object.values(!workload.env || Array.isArray(workload.env) ? {} : workload.env)) {
      const used = (value as { resource?: string } | null)?.resource;
      if (used && !app.resources?.some((r) => r.name === used)) fail(`workloads.${name}.env: no resource named ${used}`);
    }
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
  try {
    for (const [name, route] of routes) {
      if (route.backend) continue;
      const [workload, spec] = served(app, route.workload, `routes.${name}`);
      if (!servedPort(spec)) fail(`routes.${name}: workload ${workload} has several service ports, so set backend`);
    }
    if (app.metrics) served(app, app.metrics.workload, 'metrics');
  } catch (e) {
    fail((e as Error).message);
  }
  const [invalid] = appSchema.validate(app, schema as Schema).errors;
  if (invalid) fail(invalid.stack.replace(/^instance\.?/, ''));
  const resourceNames = (app.resources ?? []).map((r) => r.name);
  for (const name of resourceNames) {
    if (resourceNames.indexOf(name) !== resourceNames.lastIndexOf(name)) fail(`resources: duplicate name ${name}`);
  }
  const exportNames = (app.exports ?? []).map((e) => e.name);
  for (const exported of app.exports ?? []) {
    if (exportNames.indexOf(exported.name) !== exportNames.lastIndexOf(exported.name)) fail(`exports: duplicate name ${exported.name}`);
    if (!app.workloads?.[exported.workload]) fail(`exports.${exported.name}: no workload named ${exported.workload}`);
    exportPort(app, exported);
  }
  return app;
}

const SYSTEM_NAMESPACES = new Set(['argocd', 'dragonfly', 'envoy-gateway-system', 'forgejo', 'immich', 'monitoring']);

export function validateIngress(apps: App[]) {
  const known = new Set(apps.map((app) => app.name));
  for (const app of apps) {
    for (const from of app.network.ingress) {
      if (!known.has(from) && !SYSTEM_NAMESPACES.has(from)) {
        throw new Error(`${app.name}: network.ingress: no app or namespace named ${from}`);
      }
    }
  }
}

export function validateResources(world: World) {
  for (const app of world.apps) {
    for (const resource of app.resources ?? []) {
      if (resource.type === 'app') appExportUrl(app, resource, world.apps);
    }
    for (const spec of Object.values(app.workloads ?? {})) {
      expandWorkload(spec, (resource, env) => resourceEnv(app, resource, env, world));
    }
  }
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

export function renderApplication(app: App, versions: Versions, target: Target, world: World): string {
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
            const resolve: ResolveResource = (resource, env) => resourceEnv(app, resource, env, world);
            const { health, ...rest } = expandWorkload(spec, resolve);
            return {
              name: workload,
              ...rest,
              ...(health ? { probes: { path: health } } : {}),
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
    ...world.apps
      .filter((other) => other.name !== name && other.resources?.some((r) => r.type === 'app' && r.app === name))
      .map((other) => other.name)
      .sort(),
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
                port: servedPort(spec) as number,
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
            databaseName: databaseRole(database, spec),
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

const databaseRole = (database: string, spec: { databaseName?: string }) =>
  spec.databaseName ?? database.replaceAll('-', '_');

export const databasePath = (database: string) => `system-components/postgres-cluster/databases/${database}.yaml`;

export function renderDatabase(database: string, spec: { databaseName?: string }): string {
  const name = databaseRole(database, spec);
  const manifest = {
    apiVersion: 'postgresql.cnpg.io/v1',
    kind: 'Database',
    metadata: { name: database, namespace: 'cnpg-system' },
    spec: {
      name,
      owner: name,
      cluster: { name: 'cloudnative-pg-cluster' },
      databaseReclaimPolicy: 'retain',
    },
  };
  return `${HEADER}\n# yaml-language-server: $schema=https://k8s-schemas.anurag.sh/postgresql.cnpg.io/database_v1.json\n${stringify(manifest)}`;
}

export const appRoles = (apps: App[]) =>
  apps.flatMap((app) => Object.entries(app.databases ?? {}).map(([database, spec]) => databaseRole(database, spec)));

export function addRoles(cluster: string, roles: string[]): string {
  const doc = parseDocument(cluster);
  const existing = (doc.toJS().spec.managed.roles as { name: string }[]).map((role) => role.name);
  const missing = roles.filter((role) => !existing.includes(role));
  if (!missing.length) return cluster;
  for (const name of missing) {
    doc.addIn(['spec', 'managed', 'roles'], { name, ensure: 'present', connectionLimit: -1, inherit: true, login: true });
  }
  return doc.toString({ lineWidth: 0 });
}

interface ChartSource {
  chart: string;
  repoURL: string;
  helm?: { valueFiles?: string[]; valuesObject?: unknown };
}

export function helmTemplates(rendered: string): { args: string[]; values: string }[] {
  const application = parse(rendered);
  const name: string = application.metadata.name;
  const sources: ChartSource[] = application.spec.sources;
  return sources
    .filter((source) => source.chart && source.repoURL?.startsWith(CHART_REGISTRY))
    .map((source) => ({
      args: [
        'template',
        name,
        `charts/${source.chart}`,
        '--namespace',
        name,
        ...(source.helm?.valueFiles ?? []).flatMap((file) => ['-f', file.replace(/^\$values\//, '')]),
        '-f',
        '-',
      ],
      values: JSON.stringify(source.helm?.valuesObject ?? {}),
    }));
}
