import { type Schema, Validator } from 'jsonschema/lib/index.js';
import { parse, parseDocument, stringify } from 'yaml';
import schema from '../app.schema.json' with { type: 'json' };
import { bucketName } from './terraform.ts';
import { declaredPorts, expandWorkload, overrideConflicts, type Refs, type SecretKind } from './workload.ts';

const CONFIG_REPO = 'https://github.com/Accurate0/inf-k8s.git';
export const CHART_REGISTRY = 'oci://ghcr.io/accurate0/charts';
const SCHEMA = 'https://k8s-schemas.anurag.sh/argoproj.io/application_v1alpha1.json';

export const HEADER_TEXT = 'generated from app.yaml by `ortzi app render`; edit app.yaml instead';
export const HEADER = `# ${HEADER_TEXT}`;

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
  ports?: unknown[];
  serviceAccount?: string;
}

export type Resource =
  | { name: string; type: 'platform'; export: string }
  | { name: string; type: 'app'; app: string; export: string; path?: string }
  | Bucket
  | ({ name: string; type: 'database' } & DatabaseSpec)
  | { name: string; type: 'secret'; secretName: string }
  | { name: string; type: 'cache'; instance: string }
  | ({ name: string; type: 'route' } & Route)
  | ({ name: string; type: 'oidc' } & OidcClient)
  | ({ name: string; type: 'metrics' } & Metrics)
  | { name: string; type: 'config'; configMap: string; files: string[] }
  | { name: string; type: 'dashboard'; configMap: string; file: string }
  | ServiceAccount;

export interface ServiceAccount {
  name: string;
  type: 'serviceAccount';
  clusterRole?: string;
  clusterRules?: Record<string, unknown>[];
}

export interface Metrics {
  path?: string;
  port?: string;
  interval?: string;
  workload?: string;
}

export interface DatabaseSpec {
  databaseName?: string;
  secretName: string;
  enableConnectionPooling?: boolean;
}

export interface Bucket {
  name: string;
  type: 'bucket';
  provider: 'r2' | 's3';
  bucket?: string;
  user?: string;
  secretName: string;
}

export interface Export {
  name: string;
  workload: string;
  port?: string | number;
}

export type RouteAuth =
  | { mode: 'jwt'; clients: string[] }
  | { mode: 'oidc'; client: string; scopes?: string[]; forwardAccessToken?: boolean; logoutPath?: string; jwt?: Record<string, unknown> }
  | { mode: 'app' | 'none'; reason: string };

export interface Route {
  hosts: string[];
  gateway?: string;
  auth: RouteAuth;
  workload?: string;
  backend?: { name: string; port: number };
  paths?: (string | { type: string; value: string })[];
  rules?: { paths: (string | { type: string; value: string })[]; backend: { name: string; port: number } }[];
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
  resources?: Resource[];
  exports?: Export[];
  sources?: Record<string, unknown>[];
  build?: Build;
  network: { ingress: string[] };
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

function servicePorts(workload: string, spec: Workload) {
  const declared = declaredPorts(spec).filter((p) => p.service).map((p) => ({ name: p.name as string, port: p.service as number }));
  const ports = (declared.length
    ? declared
    : spec.service?.ports ?? (spec.port ? [{ name: spec.service?.portName ?? 'http', port: spec.service?.port ?? spec.port }] : [])) as { name?: string; port: number }[];
  return { workload, service: spec.service?.name ?? workload, ports };
}

function exportPort(provider: App, exported: Export): number {
  const spec = provider.workloads?.[exported.workload];
  const ports = spec ? servicePorts(exported.workload, spec).ports : [];
  const port =
    typeof exported.port === 'number'
      ? exported.port
      : exported.port
        ? ports.find((p) => p.name === exported.port)?.port
        : (spec && servedPort(spec));
  if (!port) throw new Error(`${provider.name}: exports.${exported.name}: no matching port on workload ${exported.workload}`);
  return port;
}

const OWN_NAMES = new Set(['secret', 'route', 'oidc', 'metrics', 'config', 'dashboard', 'serviceAccount']);

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
  const resource = app.resources?.find((r) => r.name === name && !OWN_NAMES.has(r.type)) as Resource;
  if (resource.type === 'app') return { name: env, value: appExportUrl(app, resource, apps) };
  if (resource.type === 'bucket') return { name: env, value: bucketName(app, resource) };
  if (resource.type === 'database') {
    throw new Error(`${app.name}: resources.${name}: list a database's secret in envFrom, not env`);
  }
  const wanted =
    resource.type === 'cache'
      ? { type: 'cache', name: resource.instance }
      : { type: 'platform', name: (resource as { export: string }).export };
  const provided = platform.filter((e) => e.type === wanted.type && e.name === wanted.name);
  if (provided.length !== 1) {
    throw new Error(`${app.name}: resources.${name}: expected one platform ${wanted.type} export named ${wanted.name}, found ${provided.length}`);
  }
  return { name: env, value: provided[0].url };
}

function served(app: App, wanted: string | undefined, block: string): [string, Workload] {
  const workloads = Object.entries(app.workloads ?? {}).filter(([name, w]) => servicePorts(name, w).ports.length);
  const found = wanted ? workloads.find(([name]) => name === wanted) : workloads[0];
  if (!found) throw new Error(`${block} needs a workload with a port`);
  if (!wanted && workloads.length > 1) throw new Error(`${block}.workload is required with several ported workloads`);
  return found;
}

function servedPort(spec: Workload): number | undefined {
  const { ports } = servicePorts('', spec);
  return spec.service?.port ?? spec.port ?? (ports.length === 1 ? ports[0].port : undefined);
}


const appServices = (app: App) =>
  Object.entries(app.workloads ?? {}).map(([workload, spec]) => servicePorts(workload, spec));

const BUCKET_KEYS = ['S3_BUCKET', 'S3_ENDPOINT', 'S3_REGION', 'S3_ACCESS_KEY_ID', 'S3_SECRET_ACCESS_KEY'];

function containerRefs(spec: Workload): Record<string, unknown>[] {
  const containers = [spec, ...((spec.initContainers ?? []) as Workload[]), ...((spec.sidecars ?? []) as Workload[])];
  return containers.flatMap((c) => [
    ...(c.env && !Array.isArray(c.env) ? Object.values(c.env) : []),
    ...(c.envFrom ?? []),
  ]).filter((ref): ref is Record<string, unknown> => typeof ref === 'object' && ref !== null);
}

function validateRefs(app: App) {
  const fail = (message: string): never => {
    throw new Error(`${app.name}: ${message}`);
  };
  const refs = Object.values(app.workloads ?? {}).flatMap(containerRefs);
  for (const resource of app.resources ?? []) {
    if (['route', 'oidc', 'metrics', 'config', 'dashboard', 'serviceAccount'].includes(resource.type)) continue;
    const kinds = ['secret', 'database', 'bucket'].includes(resource.type) ? ['resource', resource.type] : ['resource'];
    if (!refs.some((ref) => kinds.some((kind) => ref[kind] === resource.name))) {
      fail(`resources.${resource.name}: no workload uses this ${resource.type}`);
    }
  }
  for (const ref of refs) {
    if (ref.bucket && ref.key && !BUCKET_KEYS.includes(ref.key as string)) {
      fail(`bucket ${ref.bucket}: no key named ${ref.key}, expected one of ${BUCKET_KEYS.join(', ')}`);
    }
  }
  const services = appServices(app);
  for (const [name, route] of Object.entries(appRoutes(app))) {
    for (const backend of [...(route.backend ? [route.backend] : []), ...(route.rules ?? []).map((rule) => rule.backend)]) {
      const service = services.find((s) => s.service === backend.name);
      if (!service) fail(`routes.${name}.backend: no service named ${backend.name}`);
      if (!service?.ports.some((p) => p.port === backend.port)) {
        fail(`routes.${name}.backend: service ${backend.name} has no port ${backend.port}`);
      }
    }
  }
  for (const [name, spec] of Object.entries(app.workloads ?? {})) {
    if (spec.serviceAccount && !app.resources?.some((r) => r.type === 'serviceAccount' && r.name === spec.serviceAccount)) {
      fail(`workloads.${name}: no serviceAccount resource named ${spec.serviceAccount}`);
    }
  }
  for (const { workload, port } of appMetrics(app)) {
    const service = services.find((s) => s.workload === workload);
    if (port && service && !service.ports.some((p) => p.name === port)) {
      fail(`metrics: workload ${workload} has no service port named ${port}`);
    }
  }
  if (app.build && !workloadImages(app).includes(app.build.image)) {
    fail(`build.image ${app.build.image} is not used by any workload`);
  }
}

export interface Listener {
  gateway: string;
  hostname?: string;
  labels?: Record<string, string>;
}

const listenerMatches = (hostname: string | undefined, host: string) =>
  hostname === host || (hostname?.startsWith('*.') && host.endsWith(hostname.slice(1)));

export function validateGatewayAccess(apps: App[], listeners: Listener[]) {
  for (const app of apps) {
    const labels: Record<string, string> = { ...app.namespace?.labels, 'kubernetes.io/metadata.name': app.name };
    for (const [name, route] of Object.entries(appRoutes(app))) {
      const gateway = route.gateway ?? 'public-gateway';
      for (const host of route.hosts) {
        const matching = listeners.filter((l) => l.gateway === gateway && l.labels && listenerMatches(l.hostname, host));
        if (!matching.length) throw new Error(`${app.name}: routes.${name}: no ${gateway} listener accepts ${host}`);
        const listener = matching.find((l) => l.hostname === host) ?? matching[0];
        for (const [key, value] of Object.entries(listener.labels ?? {})) {
          if (labels[key] !== value) {
            throw new Error(`${app.name}: routes.${name}: ${host} needs namespace.labels ${key}: "${value}"`);
          }
        }
      }
    }
  }
}

const appSchema = new Validator();

const ALLOWED = new Set(['name', 'repository', 'charts', 'workloads', 'resources', 'exports', 'sources', 'network', 'namespace', 'serviceAccounts', 'build']);

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
      if (used && !app.resources?.some((r) => r.name === used && !OWN_NAMES.has(r.type))) {
        fail(`workloads.${name}.env: no resource named ${used}; list a secret in envFrom`);
      }
    }
  }
  const validResources = Array.isArray(app.resources);
  const routes = Object.entries(validResources || !app.resources ? appRoutes(app) : {});
  const oidc = validResources || !app.resources ? appOidc(app) : {};
  const clients = Object.entries(oidc);
  for (const [name, route] of routes) {
    if (!route.hosts?.length) fail(`routes.${name}.hosts is required`);
    const auth = route.auth;
    if (!auth) {
      fail(`routes.${name}.auth is required`);
    } else if (auth.mode === 'jwt') {
      if (!auth.clients?.length) fail(`routes.${name}.auth.clients is required for jwt`);
      for (const client of auth.clients ?? []) {
        if (!oidc[client]) fail(`routes.${name}.auth: no oidc client named ${client}`);
      }
    } else if (auth.mode === 'oidc') {
      const client = oidc[auth.client];
      if (!client) fail(`routes.${name}.auth: no oidc client named ${auth.client}`);
      const policy = client?.securityPolicy as { httpRoute?: string } | undefined;
      if (policy && (policy.httpRoute ?? auth.client) !== name) {
        fail(`routes.${name}.auth: oidc.${auth.client}.securityPolicy must target this route`);
      }
      if (routes.some(([other, r]) => other !== name && r.auth?.mode === 'oidc' && r.auth.client === auth.client)) {
        fail(`routes.${name}.auth: oidc client ${auth.client} is used by another oidc route`);
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
      if (route.backend || route.rules) continue;
      const [workload, spec] = served(app, route.workload, `routes.${name}`);
      if (!servedPort(spec)) fail(`routes.${name}: workload ${workload} has several service ports, so set backend`);
    }
    if (validResources || !app.resources) {
      const scraped = appMetrics(app).map((metrics) => served(app, metrics.workload, 'metrics')[0]);
      for (const workload of scraped) {
        if (scraped.indexOf(workload) !== scraped.lastIndexOf(workload)) fail(`metrics: ${workload} is listed twice`);
      }
    }
  } catch (e) {
    fail((e as Error).message);
  }
  const [invalid] = appSchema.validate(app, schema as unknown as Schema).errors;
  if (invalid) fail(invalid.stack.replace(/^instance\.?/, ''));
  const resourceKeys = (app.resources ?? []).map((r) => `${OWN_NAMES.has(r.type) ? r.type : 'value'}/${r.name}`);
  for (const key of resourceKeys) {
    if (resourceKeys.indexOf(key) !== resourceKeys.lastIndexOf(key)) fail(`resources: duplicate name ${key.split('/')[1]}`);
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

function secretRef(app: App, kind: SecretKind, name: string): string {
  const found = (app.resources ?? []).find((r) => r.type === kind && r.name === name) as { secretName?: string } | undefined;
  if (!found) throw new Error(`${app.name}: no ${kind} resource named ${name}`);
  return found.secretName as string;
}

const refsFor = (app: App, world: World): Refs => ({
  resource: (resource, env) => resourceEnv(app, resource, env, world),
  secret: (kind, name) => secretRef(app, kind, name),
  config: (name) => {
    const found = app.resources?.find((r) => r.type === 'config' && r.name === name) as Extract<Resource, { type: 'config' }> | undefined;
    if (!found) throw new Error(`${app.name}: no config resource named ${name}`);
    return found;
  },
});

export function validateResources(world: World) {
  for (const app of world.apps) {
    validateRefs(app);
    for (const resource of app.resources ?? []) {
      if (resource.type === 'app') appExportUrl(app, resource, world.apps);
    }
    for (const spec of Object.values(app.workloads ?? {})) {
      expandWorkload(spec, refsFor(app, world));
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

export const rbacPath = (app: App, target: Target) => `${target.location}/${app.name}/manifests/rbac.generated.yaml`;

const clusterRoles = (app: App) => declared(app, 'serviceAccount').filter((account) => account.clusterRules);

export function renderRbac(app: App): string | undefined {
  const documents = clusterRoles(app).flatMap((account) => {
    const role = account.clusterRole ?? account.name;
    return [
      { apiVersion: 'rbac.authorization.k8s.io/v1', kind: 'ClusterRole', metadata: { name: role }, rules: account.clusterRules },
      {
        apiVersion: 'rbac.authorization.k8s.io/v1',
        kind: 'ClusterRoleBinding',
        metadata: { name: account.name },
        subjects: [{ kind: 'ServiceAccount', name: account.name, namespace: app.name }],
        roleRef: { apiGroup: 'rbac.authorization.k8s.io', kind: 'ClusterRole', name: role },
      },
    ];
  });
  if (!documents.length) return undefined;
  return `${HEADER}\n${documents.map((doc) => stringify(doc, { lineWidth: 0, aliasDuplicateObjects: false })).join('---\n')}`;
}

export const needsKustomization = (app: App) =>
  (app.resources ?? []).some((r) => r.type === 'config' || r.type === 'dashboard' || (r.type === 'serviceAccount' && r.clusterRules));

export const kustomizationPath = (app: App, target: Target) => `${target.location}/${app.name}/kustomization.yaml`;

export function renderKustomization(app: App, target: Target): string | undefined {
  if (!needsKustomization(app)) return undefined;
  const generators = [
    ...declared(app, 'config').map(({ configMap, files }) => ({ name: configMap, namespace: app.name, files })),
    ...declared(app, 'dashboard').map(({ configMap, file }) => ({
      name: configMap,
      namespace: app.name,
      files: [file],
      options: { disableNameSuffixHash: true, labels: { 'inf-k8s.net/grafana-dashboard': 'true' } },
    })),
  ];
  const resources = [
    ...(target.hasManifests ? ['manifests'] : []),
    ...(clusterRoles(app).length ? ['manifests/rbac.generated.yaml'] : []),
  ];
  const kustomization = {
    apiVersion: 'kustomize.config.k8s.io/v1beta1',
    kind: 'Kustomization',
    ...(resources.length ? { resources } : {}),
    ...(generators.length ? { configMapGenerator: generators, generatorOptions: { disableNameSuffixHash: true } } : {}),
  };
  return `${HEADER}\n${stringify(kustomization, { lineWidth: 0, aliasDuplicateObjects: false })}`;
}

export const appFiles = (app: App, target: Target) =>
  [...declared(app, 'config').flatMap((config) => config.files), ...declared(app, 'dashboard').map((dashboard) => dashboard.file)].map(
    (file) => `${target.location}/${app.name}/${file}`,
  );

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
    appDatabases(app)[database].secretName;
  const dir = `${target.location}/${name}`;
  const routes = Object.entries(appRoutes(app));
  const sources: Record<string, unknown>[] = [];
  const serviceAccounts = [...(app.serviceAccounts ?? []), ...declared(app, 'serviceAccount').map((account) => account.name)];
  const loginPolicy = (client: string) => {
    const route = routes.find(([, r]) => r.auth?.mode === 'oidc' && r.auth.client === client);
    if (!route) return {};
    const { mode: _mode, client: _client, ...policy } = route[1].auth as Extract<RouteAuth, { mode: 'oidc' }>;
    return { securityPolicy: { httpRoute: route[0], ...policy } };
  };

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
          ...(serviceAccounts.length ? { serviceAccounts } : {}),
          workloads: Object.entries(app.workloads).map(([workload, spec]) => {
            const { health, ...rest } = expandWorkload(spec, refsFor(app, world));
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
    ...(appMetrics(app).length ? ['monitoring'] : []),
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
            if (!backend && !route.rules) {
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
              rules: (route.rules ?? [{ paths: route.paths ?? ['/'], backend: backend as { name: string; port: number } }]).map((rule) => ({
                backendRefs: [{ group: '', kind: 'Service', ...rule.backend, weight: 1 }],
                matches: rule.paths.map((match) => ({
                  path: typeof match === 'string' ? { type: 'PathPrefix', value: match } : match,
                })),
                ...(route.timeouts ? { timeouts: route.timeouts } : {}),
              })),
            };
          }),
        },
      }),
    );
  }

  if (Object.keys(appOidc(app)).length) {
    sources.push(
      chart(app, versions, 'k8s-oidc', {
        valuesObject: {
          clients: Object.entries(appOidc(app)).map(([client, spec]) => ({
            name: client,
            namespace: name,
            displayName: client,
            host: routes[0]?.[1].hosts[0],
            preferShortUsername: true,
            ...spec,
            ...loginPolicy(client),
            secretName: spec.secretName,
            secretNamespace: name,
            annotations,
          })),
        },
      }),
    );
  }

  if (Object.keys(appDatabases(app)).length) {
    sources.push(
      chart(app, versions, 'k8s-postgres', {
        valuesObject: {
          databases: Object.entries(appDatabases(app)).map(([database, spec]) => ({
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

  const bucketSecrets = (app.resources ?? []).flatMap((resource) =>
    resource.type === 'bucket' ? [{ name: `resource-${resource.name}`, secretName: resource.secretName }] : [],
  );
  if (appSecrets(app).length || bucketSecrets.length) {
    sources.push(
      chart(app, versions, 'k8s-secrets', {
        valuesObject: {
          defaults: { namespace: name, provider: 'openbao', resyncInterval: 900, annotations },
          secrets: [...appSecrets(app), ...bucketSecrets].map((s) => ({
            name: s.name,
            managedSecrets: [{ secretName: s.secretName }],
          })),
        },
      }),
    );
  }

  for (const scrape of appMetrics(app)) {
    const { workload: _workload, ...metrics } = scrape;
    const [workload, spec] = served(app, scrape.workload, 'metrics');
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

const declared = <T extends Resource['type']>(app: App, type: T) =>
  (app.resources ?? []).filter((resource): resource is Extract<Resource, { type: T }> => resource.type === type);

const specs = <S extends { name: string; type: string }>(resources: S[]) =>
  resources.map(({ name, type: _type, ...spec }) => [name, spec] as const);

export function appRoutes(app: App): Record<string, Route> {
  return Object.fromEntries(specs(declared(app, 'route'))) as Record<string, Route>;
}

export function appOidc(app: App): Record<string, OidcClient> {
  return Object.fromEntries(specs(declared(app, 'oidc'))) as Record<string, OidcClient>;
}

export function appMetrics(app: App): Metrics[] {
  return specs(declared(app, 'metrics')).map(([, spec]) => spec as Metrics);
}

export function appSecrets(app: App): { name: string; secretName: string }[] {
  return (app.resources ?? []).flatMap((resource) =>
    resource.type === 'secret'
      ? [{ name: resource.name, secretName: resource.secretName }]
      : [],
  );
}

export function appDatabases(app: App): Record<string, DatabaseSpec> {
  const declared = (app.resources ?? []).flatMap((resource) => {
    if (resource.type !== 'database') return [];
    const { name, type: _type, ...spec } = resource;
    return [[name, spec] as const];
  });
  return Object.fromEntries(declared);
}

export const databaseRole = (database: string, spec: { databaseName?: string }) =>
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
  apps.flatMap((app) => Object.entries(appDatabases(app)).map(([database, spec]) => databaseRole(database, spec)));

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
