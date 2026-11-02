import { type Schema, Validator } from 'jsonschema/lib/index.js';
import { stringify } from 'yaml';
import schema from '../app.schema.json' with { type: 'json' };
import { declaredPorts, expandWorkload, overrideConflicts, type Refs, type SecretKind } from './workload.ts';

export const HEADER_TEXT = 'generated from app.yaml by `ortzi app render`; edit app.yaml instead';
export const HEADER = `# ${HEADER_TEXT}`;

const STAMP = /^# generated at .*\n/m;

export const stamped = (rendered: string, now: Date) =>
  rendered.replace(`${HEADER}\n`, `${HEADER}\n# generated at ${now.toISOString()}\n`);

export const isCurrent = (existing: string, rendered: string) =>
  STAMP.test(existing) && existing.replace(STAMP, '') === rendered;

export interface Workload {
  image: string;
  kind?: 'Deployment' | 'DaemonSet' | 'CronJob';
  replicas?: number;
  schedule?: string;
  service?: { name: string; trafficDistribution?: string };
  initContainers?: { image: string }[];
  sidecars?: { image: string }[];
  resources?: Record<string, unknown>;
  env?: Record<string, unknown>[] | Record<string, unknown>;
  envFrom?: (string | Record<string, unknown>)[];
  probes?: Record<string, unknown>;
  mounts?: Record<string, unknown>;
  overrides?: Record<string, unknown>;
  readOnlyRootFilesystem?: boolean;
  ports?: { name: string; container: number; service?: number; appProtocol?: string }[];
  serviceAccount?: string;
}

export type Resource =
  | { name: string; type: 'platform'; export: string }
  | { name: string; type: 'app'; app: string; export: string; path?: string }
  | Bucket
  | ({ name: string; type: 'database' } & DatabaseSpec)
  | { name: string; type: 'secret'; secretName: string; secretTemplate?: SecretTemplate }
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
  port: string;
  interval?: string;
  workload: string;
}

export interface DatabaseSpec {
  secretName: string;
  enableConnectionPooling?: boolean;
  role?: string;
  extensions?: string[];
}

export interface SecretTemplate {
  type?: string;
  includeAllSecrets?: boolean;
  data: Record<string, string>;
}

export interface Bucket {
  name: string;
  type: 'bucket';
  provider: 'r2' | 's3';
  bucket: string;
  user?: string;
  secretName: string;
  secretTemplate?: SecretTemplate;
}

export interface Export {
  name: string;
  workload: string;
  port: string | number;
}

export type RouteAuth =
  | { mode: 'jwt'; clients: string[] }
  | { mode: 'oidc'; client: string; scopes?: string[]; forwardAccessToken?: boolean; logoutPath?: string; jwt?: Record<string, unknown> }
  | { mode: 'app' | 'none'; reason: string };

export interface Route {
  hosts: string[];
  gateway?: string;
  auth: RouteAuth;
  backend?: { name: string; port: number };
  paths?: (string | { type: string; value: string })[];
  rules?: { paths: (string | { type: string; value: string })[]; backend: { name: string; port: number } }[];
  annotations?: Record<string, string>;
  ratelimit?: Record<string, unknown>;
  timeouts?: Record<string, unknown>;
}

export type OidcClient = { host: string; displayName?: string; secretName: string } & Record<string, unknown>;

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

export interface HelmSource {
  type: 'helm';
  chart: string;
  repo: string;
  version: string;
  values: string;
  releaseName?: string;
}

export interface GitSource {
  type: 'git';
  repo: string;
  path: string;
  name?: string;
  revision?: string;
}

export interface ManifestsSource {
  type: 'manifests';
  path: string;
}

export type Source = HelmSource | GitSource | ManifestsSource;

export interface ArgocdOverrides {
  options?: string[];
  automated?: boolean;
  ignoreDifferences?: Record<string, unknown>[];
}

export interface App {
  name: string;
  repository?: string;
  charts?: Record<string, string>;
  workloads?: NamedWorkload[];
  resources?: Resource[];
  exports?: Export[];
  sources?: Source[];
  argocd?: { overrides?: ArgocdOverrides };
  build?: Build;
  network: { ingress: string[]; policy?: boolean };
  namespace?: { name?: string; labels?: Record<string, string> };
}

export type NamedWorkload = Workload & { name: string };

export function appWorkloads(app: App): Record<string, Workload> {
  return Object.fromEntries((app.workloads ?? []).map(({ name, ...spec }) => [name, spec]));
}

export const appNamespace = (app: App) => app.namespace?.name ?? app.name;

export type Versions = Record<string, string>;

export interface Target {
  location: string;
  hasManifests: boolean;
}

export interface PlatformExport {
  name: string;
  type: string;
  url: string;
}

export interface World {
  platform: PlatformExport[];
  apps: App[];
  external?: Set<string>;
}

function servicePorts(workload: string, spec: Workload) {
  const ports = declaredPorts(spec).filter((p) => p.service).map((p) => ({ name: p.name as string, port: p.service as number }));
  return { workload, service: spec.service?.name ?? workload, ports };
}

function exportPort(provider: App, exported: Export): number {
  const spec = appWorkloads(provider)[exported.workload];
  const ports = spec ? servicePorts(exported.workload, spec).ports : [];
  const found = ports.find((p) => p.name === exported.port || p.port === exported.port)?.port;
  if (!found) throw new Error(`${provider.name}: exports.${exported.name}: no matching port on workload ${exported.workload}`);
  return found;
}

const OWN_NAMES = new Set(['secret', 'route', 'oidc', 'metrics', 'config', 'dashboard', 'serviceAccount']);

function appExportUrl(app: App, resource: Extract<Resource, { type: 'app' }>, apps: App[]): string {
  const provider = apps.find((a) => a.name === resource.app);
  if (!provider) throw new Error(`${app.name}: resources.${resource.name}: no app named ${resource.app}`);
  const exported = provider.exports?.find((e) => e.name === resource.export);
  if (!exported) throw new Error(`${app.name}: resources.${resource.name}: ${resource.app} has no export named ${resource.export}`);
  const service = appWorkloads(provider)[exported.workload]?.service?.name ?? exported.workload;
  const port = exportPort(provider, exported);
  return `http://${service}.${provider.name}.svc.cluster.local${port === 80 ? '' : `:${port}`}${resource.path ?? ''}`;
}

function resourceEnv(app: App, name: string, env: string, { platform, apps }: World): { name: string; value: string } {
  const resource = app.resources?.find((r) => r.name === name && !OWN_NAMES.has(r.type)) as Resource;
  if (resource.type === 'app') return { name: env, value: appExportUrl(app, resource, apps) };
  if (resource.type === 'bucket') {
    throw new Error(`${app.name}: resources.${name}: read a bucket with {bucket, key}, not {resource}`);
  }
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

const appServices = (app: App) =>
  Object.entries(appWorkloads(app)).map(([workload, spec]) => servicePorts(workload, spec));

const BUCKET_KEYS = ['S3_BUCKET', 'S3_ENDPOINT', 'S3_REGION', 'S3_ACCESS_KEY_ID', 'S3_SECRET_ACCESS_KEY'];

function containerRefs(spec: Workload): Record<string, unknown>[] {
  const containers = [spec, ...((spec.initContainers ?? []) as Workload[]), ...((spec.sidecars ?? []) as Workload[])];
  return containers.flatMap((c) => [
    ...(c.env && !Array.isArray(c.env) ? Object.values(c.env) : []),
    ...(c.envFrom ?? []),
  ]).filter((ref): ref is Record<string, unknown> => typeof ref === 'object' && ref !== null);
}

function validateRefs(app: App, external: boolean) {
  const fail = (message: string): never => {
    throw new Error(`${app.name}: ${message}`);
  };
  const refs = Object.values(appWorkloads(app)).flatMap(containerRefs);
  for (const resource of app.resources ?? []) {
    if (['route', 'oidc', 'metrics', 'config', 'dashboard', 'serviceAccount'].includes(resource.type)) continue;
    if (resource.type === 'bucket' && resource.secretTemplate) continue;
    if (external) continue;
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
      if (!service && external) continue;
      if (!service) fail(`routes.${name}.backend: no service named ${backend.name}`);
      if (!service?.ports.some((p) => p.port === backend.port)) {
        fail(`routes.${name}.backend: service ${backend.name} has no port ${backend.port}`);
      }
    }
  }
  for (const [name, spec] of Object.entries(appWorkloads(app))) {
    if (spec.serviceAccount && !app.resources?.some((r) => r.type === 'serviceAccount' && r.name === spec.serviceAccount)) {
      fail(`workloads.${name}: no serviceAccount resource named ${spec.serviceAccount}`);
    }
  }
  for (const { workload, port } of appMetrics(app)) {
    const service = services.find((s) => s.workload === workload);
    if (!service?.ports.length) fail(`metrics: workload ${workload} has no service ports`);
    if (!service?.ports.some((p) => p.name === port)) {
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
    const labels: Record<string, string> = { ...app.namespace?.labels, 'kubernetes.io/metadata.name': appNamespace(app) };
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

const ALLOWED = new Set(['name', 'repository', 'charts', 'workloads', 'resources', 'exports', 'sources', 'network', 'namespace', 'build', 'argocd']);

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
  if (app.namespace?.name) {
    if (app.network.policy === undefined) fail('namespace.name needs an explicit network.policy');
    if (app.network.policy === false && app.network.ingress.length) fail('network.ingress needs network.policy: true');
    if (app.namespace.labels) fail('namespace.labels cannot be set with namespace.name');
  }
  if (app.workloads && !Array.isArray(app.workloads)) fail('workloads must be a list');
  const workloadNames = (app.workloads ?? []).map((workload) => workload.name);
  for (const name of workloadNames) {
    if (workloadNames.indexOf(name) !== workloadNames.lastIndexOf(name)) fail(`workloads: duplicate name ${name}`);
  }
  for (const [name, workload] of Object.entries(appWorkloads(app))) {
    if (!workload.image) fail(`workloads.${name}.image is required`);
    if (declaredPorts(workload).length && !workload.service?.name) fail(`workloads.${name}.service.name is required with ports`);
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
  for (const [name, route] of routes) {
    if (!route.hosts?.length) fail(`routes.${name}.hosts is required`);
    if (!route.backend === !route.rules) fail(`routes.${name} needs either backend or rules`);
    const auth = route.auth;
    if (!auth) {
      fail(`routes.${name}.auth is required`);
    } else if (auth.mode === 'jwt') {
      if (!auth.clients?.length) fail(`routes.${name}.auth.clients is required for jwt`);
      for (const client of auth.clients ?? []) {
        if (!oidc[client]) fail(`routes.${name}.auth: no oidc client named ${client}`);
      }
    } else if (auth.mode === 'oidc') {
      if (!oidc[auth.client]) fail(`routes.${name}.auth: no oidc client named ${auth.client}`);
      if (routes.some(([other, r]) => other !== name && r.auth?.mode === 'oidc' && r.auth.client === auth.client)) {
        fail(`routes.${name}.auth: oidc client ${auth.client} is used by another oidc route`);
      }
    } else if (auth.mode === 'app' || auth.mode === 'none') {
      if (!auth.reason?.trim()) fail(`routes.${name}.auth.reason is required for ${auth.mode}`);
    } else {
      fail(`routes.${name}.auth.mode must be jwt, oidc, app or none`);
    }
  }
  if (validResources || !app.resources) {
    const scraped = appMetrics(app).map((metrics) => metrics.workload);
    for (const workload of scraped) {
      if (scraped.indexOf(workload) !== scraped.lastIndexOf(workload)) fail(`metrics: ${workload} is listed twice`);
    }
  }
  const [invalid] = appSchema.validate(app, schema as unknown as Schema).errors;
  if (invalid) fail(invalid.stack.replace(/^instance\.?/, ''));
  const resourceKeys = (app.resources ?? []).map((r) => `${OWN_NAMES.has(r.type) ? r.type : 'value'}/${r.name}`);
  for (const key of resourceKeys) {
    if (resourceKeys.indexOf(key) !== resourceKeys.lastIndexOf(key)) fail(`resources: duplicate name ${key.split('/')[1]}`);
  }
  const releases = sourcesOf(app, 'helm').map((release) => release.releaseName ?? release.chart);
  for (const release of releases) {
    if (releases.indexOf(release) !== releases.lastIndexOf(release)) fail(`sources: duplicate helm release ${release}, set releaseName`);
  }
  const manifests = sourcesOf(app, 'manifests');
  if (manifests.length > 1) fail('sources: only one manifests source is allowed');
  if (manifests[0]?.path.split('/').includes('..')) fail('sources: manifests path must stay inside the app directory');
  if (manifests[0] && needsKustomization(app) && manifests[0].path !== '.') fail('sources: manifests path must be . with config, dashboard or clusterRules');
  const exportNames = (app.exports ?? []).map((e) => e.name);
  for (const exported of app.exports ?? []) {
    if (exportNames.indexOf(exported.name) !== exportNames.lastIndexOf(exported.name)) fail(`exports: duplicate name ${exported.name}`);
    if (!appWorkloads(app)[exported.workload]) fail(`exports.${exported.name}: no workload named ${exported.workload}`);
    exportPort(app, exported);
  }
  return app;
}

const SYSTEM_NAMESPACES = new Set(['argocd', 'dragonfly', 'envoy-gateway-system', 'monitoring']);

export function validateIngress(apps: App[]) {
  const known = new Set(apps.map(appNamespace));
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

export const refsFor = (app: App, world: World): Refs => ({
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
    validateRefs(app, world.external?.has(app.name) ?? false);
    for (const resource of app.resources ?? []) {
      if (resource.type === 'app') appExportUrl(app, resource, world.apps);
    }
    for (const spec of Object.values(appWorkloads(app))) {
      expandWorkload(spec, refsFor(app, world));
    }
  }
}

export const workloadImages = (app: App) =>
  Object.values(appWorkloads(app)).flatMap((w) => [
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
        subjects: [{ kind: 'ServiceAccount', name: account.name, namespace: appNamespace(app) }],
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
    ...declared(app, 'config').map(({ configMap, files }) => ({ name: configMap, namespace: appNamespace(app), files })),
    ...declared(app, 'dashboard').map(({ configMap, file }) => ({
      name: configMap,
      namespace: appNamespace(app),
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

export const sourcesOf = <T extends Source['type']>(app: App, type: T) =>
  (app.sources ?? []).filter((source): source is Extract<Source, { type: T }> => source.type === type);

export function validateManifestSources(app: App, found: { hasManifests: boolean; hasRootKustomization: boolean }) {
  const declared = sourcesOf(app, 'manifests')[0];
  if (!declared && (found.hasManifests || found.hasRootKustomization || needsKustomization(app))) {
    throw new Error(`${app.name}: sources: add a manifests source, this app has a kustomization that would not deploy`);
  }
  if (declared && found.hasRootKustomization && declared.path !== '.') {
    throw new Error(`${app.name}: sources: manifests path must be . to deploy the app directory's kustomization`);
  }
}

export const appFiles = (app: App, target: Target) =>
  [
    ...declared(app, 'config').flatMap((config) => config.files),
    ...declared(app, 'dashboard').map((dashboard) => dashboard.file),
    ...sourcesOf(app, 'helm').map((release) => release.values),
    ...sourcesOf(app, 'manifests').filter(() => !needsKustomization(app)).map(({ path }) => (path === '.' ? 'kustomization.yaml' : `${path}/kustomization.yaml`)),
  ].map(
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

export const declared = <T extends Resource['type']>(app: App, type: T) =>
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

export function appSecrets(app: App): { name: string; secretName: string; secretTemplate?: SecretTemplate }[] {
  return (app.resources ?? []).flatMap((resource) =>
    resource.type === 'secret'
      ? [{ name: resource.name, secretName: resource.secretName, secretTemplate: resource.secretTemplate }]
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

export const databaseRole = (database: string, spec?: Pick<DatabaseSpec, 'role'>) => spec?.role ?? database.replaceAll('-', '_');

export const appRoles = (apps: App[]) =>
  apps.flatMap((app) => Object.entries(appDatabases(app)).map(([database, spec]) => databaseRole(database, spec)));
