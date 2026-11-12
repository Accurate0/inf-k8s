import { type Schema, Validator } from 'jsonschema/lib/index.js';
import { stringify } from 'yaml';
import schema from '../app.schema.json' with { type: 'json' };
import {
  declaredPorts,
  describeEnv,
  describeEnvFrom,
  type EnvSource,
  expandWorkload,
  overrideConflicts,
  type Refs,
  SECRET_KINDS,
  type SecretKind,
} from './workload.ts';

export const HEADER_TEXT = 'generated from app.yaml by `ortzi app render`; edit app.yaml instead';
export const HEADER = `# ${HEADER_TEXT}`;

const STAMP = /^# generated at .*\n/m;

export const stamped = (rendered: string, now: Date, header = HEADER) =>
  rendered.replace(`${header}\n`, `${header}\n# generated at ${now.toISOString()}\n`);

export const isCurrent = (existing: string, rendered: string) =>
  STAMP.test(existing) && existing.replace(STAMP, '') === rendered;

export interface Workload {
  image: string;
  kind?: 'Deployment' | 'DaemonSet' | 'CronJob' | 'Job';
  replicas?: number;
  autoscale?: { min: number; max: number; cpu: number };
  schedule?: string;
  phase?: 'preSync' | 'postSync';
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
  | {
      name: string;
      type: 'secret';
      secretName: string;
      secretTemplate?: SecretTemplate;
      generate?: Record<string, GeneratedKey>;
    }
  | { name: string; type: 'cache'; instance?: string }
  | ({ name: string; type: 'route' } & Route)
  | ({ name: string; type: 'oidc' } & OidcClient)
  | ({ name: string; type: 'metrics' } & Metrics)
  | Config
  | { name: string; type: 'dashboard'; configMap: string; file: string }
  | Volume
  | ServiceAccount
  | Subgraph
  | Supergraph;

export interface Subgraph {
  name: string;
  type: 'subgraph';
  workload: string;
  port: string | number;
  path: string;
  websocket?: string;
  prefix: string;
  auth?: SubgraphAuth;
  repository?: string;
  schema: string;
  ref?: string;
}

export interface SubgraphAuth {
  clients: string[];
  groups: string[];
}

export interface Supergraph {
  name: string;
  type: 'supergraph';
  base: string;
}

export interface Config {
  name: string;
  type: 'config';
  configMap: string;
  files: string[];
  secretName?: string;
  values?: Record<string, Record<string, string>>;
}

export interface GeneratedKey {
  length: number;
  digits?: number;
  symbols: number;
  encoding?: 'raw' | 'base64' | 'base64url' | 'base32' | 'hex';
}

export interface Volume {
  name: string;
  type: 'volume';
  claimName: string;
  class: string;
  size: string;
  access: string;
  backup?: 'daily';
}

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
  provider: 'r2' | 's3' | 'garage';
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
  | {
      mode: 'oidc';
      client: string;
      scopes?: string[];
      forwardAccessToken?: boolean;
      logoutPath?: string;
      jwt?: Record<string, unknown>;
    }
  | { mode: 'app' | 'none'; reason: string };

export type RouteDns = { provider: 'cloudflare'; proxied: boolean } | { provider: 'unifi' };

export interface RouteRatelimit {
  requests: number;
  per: string;
  header: string;
}

export interface Route {
  protocol?: 'http' | 'tcp';
  listener?: string;
  hosts: string[];
  gateway?: string;
  auth: RouteAuth;
  backend?: { name: string; port: number };
  paths?: (string | { type: string; value: string })[];
  rules?: {
    paths: (string | { type: string; value: string })[];
    backend: { name: string; port: number };
  }[];
  dns?: RouteDns;
  ratelimit?: RouteRatelimit;
  timeouts?: Record<string, unknown>;
  loadBalancer?: RouteLoadBalancer;
}

export interface RouteLoadBalancer {
  type: 'roundRobin' | 'leastRequest' | 'random' | 'consistentHash';
  headers?: string[];
}

export type OidcClient = { host: string; displayName?: string; secretName: string } & Record<
  string,
  unknown
>;

export interface Build {
  image: string;
  ci?: 'github' | 'forgejo';
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
  overrides?: { argocd?: { application?: ArgocdOverrides } };
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
  url?: string;
  host?: string;
  storageClass?: string;
  backupLabels?: Record<string, string>;
}

export interface World {
  platform: PlatformExport[];
  apps: App[];
  external?: Set<string>;
}

function servicePorts(workload: string, spec: Workload) {
  const ports = declaredPorts(spec)
    .filter((p) => p.service)
    .map((p) => ({ name: p.name as string, port: p.service as number }));
  return { workload, service: spec.service?.name ?? workload, ports };
}

function exportPort(provider: App, exported: Export): number {
  const spec = appWorkloads(provider)[exported.workload];
  const ports = spec ? servicePorts(exported.workload, spec).ports : [];
  const found = ports.find((p) => p.name === exported.port || p.port === exported.port)?.port;
  if (!found)
    throw new Error(
      `${provider.name}: exports.${exported.name}: no matching port on workload ${exported.workload}`,
    );

  return found;
}

const OWN_NAMES = new Set([
  'subgraph',
  'supergraph',
  'secret',
  'route',
  'oidc',
  'metrics',
  'config',
  'dashboard',
  'serviceAccount',
  'volume',
]);

function appExportUrl(app: App, resource: Extract<Resource, { type: 'app' }>, apps: App[]): string {
  const provider = apps.find((a) => a.name === resource.app);
  if (!provider)
    throw new Error(`${app.name}: resources.${resource.name}: no app named ${resource.app}`);

  const exported = provider.exports?.find((e) => e.name === resource.export);
  if (!exported)
    throw new Error(
      `${app.name}: resources.${resource.name}: ${resource.app} has no export named ${resource.export}`,
    );

  const service = appWorkloads(provider)[exported.workload]?.service?.name ?? exported.workload;
  return serviceUrl(service, provider.name, exportPort(provider, exported), resource.path);
}

const serviceUrl = (service: string, namespace: string, port: number, path = '') =>
  `http://${service}.${namespace}.svc.cluster.local${port === 80 ? '' : `:${port}`}${path}`;

function ownServiceUrl(app: App, service: string, port: string | number, path?: string): string {
  const found = appServices(app).find((s) => s.service === service);
  if (!found) throw new Error(`${app.name}: env: no service named ${service}`);

  const exposed = found.ports.find((p) => p.name === port || p.port === port);
  if (!exposed) throw new Error(`${app.name}: env: service ${service} has no port ${port}`);

  return serviceUrl(service, appNamespace(app), exposed.port, path);
}

export type UrlPart = 'url' | 'host' | 'port';

const DEFAULT_PORTS: Record<string, string> = {
  'http:': '80',
  'https:': '443',
  'redis:': '6379',
  'rediss:': '6379',
};

function urlPart(url: string, part: UrlPart, where: string): string {
  if (part === 'url') return url;
  const parsed = new URL(url);
  if (part === 'host') return parsed.hostname;
  const port = parsed.port || DEFAULT_PORTS[parsed.protocol];
  if (!port) throw new Error(`${where}: ${url} has no port`);
  return port;
}

function resourceEnv(
  app: App,
  name: string,
  env: string,
  world: World,
  part: UrlPart = 'url',
): { name: string; value: string } {
  return {
    name: env,
    value: urlPart(resourceUrl(app, name, world), part, `${app.name}: env.${env}`),
  };
}

function resourceUrl(app: App, name: string, { platform, apps }: World): string {
  const resource = app.resources?.find(
    (r) => r.name === name && !OWN_NAMES.has(r.type),
  ) as Resource;
  if (resource.type === 'app') return appExportUrl(app, resource, apps);
  if (resource.type === 'bucket') {
    throw new Error(
      `${app.name}: resources.${name}: read a bucket with {bucket, key}, not {resource}`,
    );
  }
  if (resource.type === 'database') {
    throw new Error(`${app.name}: resources.${name}: list a database's secret in envFrom, not env`);
  }

  const wanted =
    resource.type === 'cache'
      ? { type: 'cache', name: resource.instance ?? 'cache' }
      : { type: 'platform', name: (resource as { export: string }).export };
  const provided = platform.filter((e) => e.type === wanted.type && e.name === wanted.name);
  if (provided.length !== 1) {
    throw new Error(
      `${app.name}: resources.${name}: expected one platform ${wanted.type} export named ${wanted.name}, found ${provided.length}`,
    );
  }

  return provided[0].url as string;
}

const appServices = (app: App) =>
  Object.entries(appWorkloads(app)).map(([workload, spec]) => servicePorts(workload, spec));

const BUCKET_KEYS = [
  'S3_BUCKET',
  'S3_ENDPOINT',
  'S3_REGION',
  'S3_ACCESS_KEY_ID',
  'S3_SECRET_ACCESS_KEY',
];

function containerRefs(spec: Workload): Record<string, unknown>[] {
  const containers = [
    spec,
    ...((spec.initContainers ?? []) as Workload[]),
    ...((spec.sidecars ?? []) as Workload[]),
  ];

  return containers
    .flatMap((c) => [
      ...(c.env && !Array.isArray(c.env) ? Object.values(c.env) : []),
      ...(c.envFrom ?? []),
    ])
    .filter((ref): ref is Record<string, unknown> => typeof ref === 'object' && ref !== null);
}

const mountedVolumes = (app: App) =>
  Object.values(appWorkloads(app)).flatMap((spec) =>
    [
      spec,
      ...((spec.initContainers ?? []) as Workload[]),
      ...((spec.sidecars ?? []) as Workload[]),
    ].flatMap((container) =>
      Object.values((container.mounts ?? {}) as Record<string, { volume?: string }>).flatMap(
        (mount) => mount.volume ?? [],
      ),
    ),
  );

function validateRefs(app: App, external: boolean) {
  const fail = (message: string): never => {
    throw new Error(`${app.name}: ${message}`);
  };

  const refs = [
    ...Object.values(appWorkloads(app)).flatMap(containerRefs),
    ...declared(app, 'config').flatMap((config) => Object.values(config.values ?? {})),
  ];
  for (const resource of app.resources ?? []) {
    if (
      [
        'route',
        'oidc',
        'metrics',
        'config',
        'dashboard',
        'serviceAccount',
        'subgraph',
        'supergraph',
      ].includes(resource.type)
    )
      continue;
    if (resource.type === 'volume') {
      if (!external && !mountedVolumes(app).includes(resource.name))
        fail(`resources.${resource.name}: no workload mounts this volume`);
      continue;
    }
    if (resource.type === 'bucket' && resource.secretTemplate) continue;
    if (external) continue;
    const kinds = ['secret', 'database', 'bucket'].includes(resource.type)
      ? ['resource', resource.type]
      : ['resource'];
    if (!refs.some((ref) => kinds.some((kind) => ref[kind] === resource.name))) {
      fail(`resources.${resource.name}: no workload uses this ${resource.type}`);
    }
  }

  for (const ref of refs) {
    if (ref.bucket && ref.key && !BUCKET_KEYS.includes(ref.key as string)) {
      fail(
        `bucket ${ref.bucket}: no key named ${ref.key}, expected one of ${BUCKET_KEYS.join(', ')}`,
      );
    }
  }

  const services = appServices(app);
  for (const [name, route] of [
    ...Object.entries(appRoutes(app)),
    ...Object.entries(appTcpRoutes(app)),
  ]) {
    for (const backend of [
      ...(route.backend ? [route.backend] : []),
      ...(route.rules ?? []).map((rule) => rule.backend),
    ]) {
      const service = services.find((s) => s.service === backend.name);
      if (!service && external) continue;
      if (!service) fail(`routes.${name}.backend: no service named ${backend.name}`);
      if (!service?.ports.some((p) => p.port === backend.port)) {
        fail(`routes.${name}.backend: service ${backend.name} has no port ${backend.port}`);
      }
    }
  }

  for (const [name, spec] of Object.entries(appWorkloads(app))) {
    if (
      spec.serviceAccount &&
      !app.resources?.some((r) => r.type === 'serviceAccount' && r.name === spec.serviceAccount)
    ) {
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
  name?: string;
  protocol?: string;
  hostname?: string;
  labels?: Record<string, string>;
}

const listenerMatches = (hostname: string | undefined, host: string) =>
  hostname === host || (hostname?.startsWith('*.') && host.endsWith(hostname.slice(1)));

export function validateGatewayAccess(apps: App[], listeners: Listener[]) {
  for (const app of apps) {
    const labels: Record<string, string> = {
      ...app.namespace?.labels,
      'kubernetes.io/metadata.name': appNamespace(app),
    };
    for (const [name, route] of Object.entries(appTcpRoutes(app))) {
      const gateway = route.gateway ?? 'public-gateway';
      const listener = listeners.find((l) => l.gateway === gateway && l.name === route.listener);
      if (!listener?.labels || listener.protocol !== 'TCP')
        throw new Error(
          `${app.name}: routes.${name}: no TCP listener ${route.listener} on ${gateway}`,
        );
      for (const [key, value] of Object.entries(listener.labels)) {
        if (labels[key] !== value) {
          throw new Error(
            `${app.name}: routes.${name}: ${route.listener} needs namespace.labels ${key}: "${value}"`,
          );
        }
      }
    }

    for (const [name, route] of Object.entries(appRoutes(app))) {
      const gateway = route.gateway ?? 'public-gateway';
      for (const host of route.hosts) {
        const matching = listeners.filter(
          (l) => l.gateway === gateway && l.labels && listenerMatches(l.hostname, host),
        );
        if (!matching.length)
          throw new Error(`${app.name}: routes.${name}: no ${gateway} listener accepts ${host}`);
        const listener = matching.find((l) => l.hostname === host) ?? matching[0];
        for (const [key, value] of Object.entries(listener.labels ?? {})) {
          if (labels[key] !== value) {
            throw new Error(
              `${app.name}: routes.${name}: ${host} needs namespace.labels ${key}: "${value}"`,
            );
          }
        }
      }
    }
  }
}

const appSchema = new Validator();

const ALLOWED = new Set([
  'name',
  'repository',
  'charts',
  'workloads',
  'resources',
  'exports',
  'sources',
  'network',
  'namespace',
  'build',
  'overrides',
]);

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
    if (app.network.policy === false && app.network.ingress.length)
      fail('network.ingress needs network.policy: true');
    if (app.namespace.labels) fail('namespace.labels cannot be set with namespace.name');
  }

  if (app.workloads && !Array.isArray(app.workloads)) fail('workloads must be a list');
  const workloadNames = (app.workloads ?? []).map((workload) => workload.name);
  for (const name of workloadNames) {
    if (workloadNames.indexOf(name) !== workloadNames.lastIndexOf(name))
      fail(`workloads: duplicate name ${name}`);
  }

  for (const [name, workload] of Object.entries(appWorkloads(app))) {
    if (!workload.image) fail(`workloads.${name}.image is required`);
    if (workload.kind === 'Job' && !workload.phase)
      fail(`workloads.${name}.phase is required for a Job`);
    if (workload.phase && workload.kind !== 'Job')
      fail(`workloads.${name}.phase is only for a Job`);
    if (workload.kind === 'Job' && declaredPorts(workload).length)
      fail(`workloads.${name}: a Job cannot expose ports`);
    if (declaredPorts(workload).length && !workload.service?.name)
      fail(`workloads.${name}.service.name is required with ports`);
    for (const key of overrideConflicts(workload))
      fail(`workloads.${name}.overrides.${key} is already set on the workload`);
    if (workload.autoscale) {
      const { min, max } = workload.autoscale;
      if ((workload.kind ?? 'Deployment') !== 'Deployment')
        fail(`workloads.${name}.autoscale is only for a Deployment`);
      if (workload.replicas !== undefined)
        fail(`workloads.${name}: set either replicas or autoscale, not both`);
      if (min > max) fail(`workloads.${name}.autoscale.min is more than max`);
      for (const container of [workload, ...((workload.sidecars ?? []) as Workload[])]) {
        if (!container.resources?.cpu)
          fail(`workloads.${name}.autoscale needs resources.cpu on every container`);
      }
    }
    for (const container of [
      workload,
      ...((workload.initContainers ?? []) as Workload[]),
      ...((workload.sidecars ?? []) as Workload[]),
    ]) {
      const targets = ['http', 'grpc', 'exec'].filter((target) => container.probes?.[target]);
      if (targets.length > 1)
        fail(`workloads.${name}.probes: use one of ${targets.join(', ')}, not several`);
      for (const [env, value] of Object.entries(
        !container.env || Array.isArray(container.env) ? {} : container.env,
      )) {
        if (typeof value === 'string' && value.includes('.svc.cluster.local'))
          fail(
            `workloads.${name}.env.${env}: use {service} or {resource} instead of a cluster address`,
          );
      }
    }
    for (const value of Object.values(
      !workload.env || Array.isArray(workload.env) ? {} : workload.env,
    )) {
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
    if (route.listener) fail(`routes.${name}.listener is only for protocol tcp`);
    if (!route.hosts?.length) fail(`routes.${name}.hosts is required`);
    if (!route.backend === !route.rules) fail(`routes.${name} needs either backend or rules`);
    if (route.loadBalancer) {
      const { type, headers } = route.loadBalancer;
      if (type === 'consistentHash' && !headers?.length)
        fail(`routes.${name}.loadBalancer.headers is required for consistentHash`);
      if (type !== 'consistentHash' && headers)
        fail(`routes.${name}.loadBalancer.headers is only for consistentHash`);
    }
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
      if (
        routes.some(
          ([other, r]) =>
            other !== name && r.auth?.mode === 'oidc' && r.auth.client === auth.client,
        )
      ) {
        fail(`routes.${name}.auth: oidc client ${auth.client} is used by another oidc route`);
      }
    } else if (auth.mode === 'app' || auth.mode === 'none') {
      if (!auth.reason?.trim()) fail(`routes.${name}.auth.reason is required for ${auth.mode}`);
    } else {
      fail(`routes.${name}.auth.mode must be jwt, oidc, app or none`);
    }
  }

  const tcpRoutes = Object.entries(validResources || !app.resources ? appTcpRoutes(app) : {});
  for (const [name, route] of tcpRoutes) {
    if (!route.listener) fail(`routes.${name}.listener is required for protocol tcp`);
    if (!route.backend) fail(`routes.${name}.backend is required for protocol tcp`);
    for (const field of HTTP_ONLY) {
      if (route[field] !== undefined) fail(`routes.${name}.${field} is only for protocol http`);
    }
    const auth = route.auth;
    if (auth?.mode !== 'app' && auth?.mode !== 'none')
      fail(`routes.${name}.auth.mode must be app or none for protocol tcp`);
    else if (!auth.reason?.trim()) fail(`routes.${name}.auth.reason is required for ${auth.mode}`);
  }

  for (const resource of validResources ? (app.resources ?? []) : []) {
    if (resource.type === 'secret' && resource.generate) {
      const keys = Object.keys(resource.generate);
      if (!keys.length) fail(`resources.${resource.name}.generate needs at least one key`);
      const objects = keys.map((key) => key.toLowerCase().replaceAll('_', '-'));
      for (const [i, key] of keys.entries()) {
        if (!/^[A-Za-z0-9]([A-Za-z0-9_]*[A-Za-z0-9])?$/.test(key))
          fail(`resources.${resource.name}.generate.${key}: use letters, digits and underscores`);
        if (objects.indexOf(objects[i]) !== i)
          fail(
            `resources.${resource.name}.generate.${key} clashes with ${keys[objects.indexOf(objects[i])]}`,
          );
      }
    }
    if (resource.type === 'config' && !resource.values !== !resource.secretName)
      fail(`resources.${resource.name}: set secretName and values together`);
  }

  if (validResources || !app.resources) {
    const scraped = appMetrics(app).map((metrics) => metrics.workload);
    for (const workload of scraped) {
      if (scraped.indexOf(workload) !== scraped.lastIndexOf(workload))
        fail(`metrics: ${workload} is listed twice`);
    }
  }

  const [invalid] = appSchema.validate(app, schema as unknown as Schema).errors;
  if (invalid) fail(invalid.stack.replace(/^instance\.?/, ''));

  const resourceKeys = (app.resources ?? []).map(
    (r) => `${OWN_NAMES.has(r.type) ? r.type : 'value'}/${r.name}`,
  );
  for (const key of resourceKeys) {
    if (resourceKeys.indexOf(key) !== resourceKeys.lastIndexOf(key))
      fail(`resources: duplicate name ${key.split('/')[1]}`);
  }

  const releases = sourcesOf(app, 'helm').map((release) => release.releaseName ?? release.chart);
  for (const release of releases) {
    if (releases.indexOf(release) !== releases.lastIndexOf(release))
      fail(`sources: duplicate helm release ${release}, set releaseName`);
  }

  const manifests = sourcesOf(app, 'manifests');
  if (manifests.length > 1) fail('sources: only one manifests source is allowed');
  if (manifests[0]?.path.split('/').includes('..'))
    fail('sources: manifests path must stay inside the app directory');
  if (manifests[0] && needsKustomization(app) && manifests[0].path !== '.')
    fail('sources: manifests path must be . with config, dashboard or clusterRules');

  const exportNames = (app.exports ?? []).map((e) => e.name);
  for (const exported of app.exports ?? []) {
    if (exportNames.indexOf(exported.name) !== exportNames.lastIndexOf(exported.name))
      fail(`exports: duplicate name ${exported.name}`);
    if (!appWorkloads(app)[exported.workload])
      fail(`exports.${exported.name}: no workload named ${exported.workload}`);
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
  const found = (app.resources ?? []).find((r) => r.type === kind && r.name === name) as
    { secretName?: string } | undefined;
  if (!found) throw new Error(`${app.name}: no ${kind} resource named ${name}`);
  return found.secretName as string;
}

export function storageClass(app: App, volume: Volume, platform: PlatformExport[]) {
  const found = platform.find((e) => e.type === 'storage' && e.name === volume.class);
  if (!found?.storageClass)
    throw new Error(
      `${app.name}: resources.${volume.name}: no storage export named ${volume.class}`,
    );

  if (volume.backup && !found.backupLabels)
    throw new Error(
      `${app.name}: resources.${volume.name}: storage ${volume.class} has no backups`,
    );

  return {
    storageClass: found.storageClass,
    labels: volume.backup ? found.backupLabels : undefined,
  };
}

export function databaseHost(app: App, spec: DatabaseSpec, platform: PlatformExport[]) {
  const name = spec.enableConnectionPooling === false ? 'direct' : 'pooled';
  const found = platform.find((e) => e.type === 'database' && e.name === name);
  if (!found?.host) throw new Error(`${app.name}: no database export named ${name}`);
  return found.host;
}

export const refsFor = (app: App, world: World): Refs => ({
  resource: (resource, env, part) => resourceEnv(app, resource, env, world, part),
  secret: (kind, name) => secretRef(app, kind, name),
  config: (name) => {
    const found = app.resources?.find((r) => r.type === 'config' && r.name === name) as
      Extract<Resource, { type: 'config' }> | undefined;
    if (!found) throw new Error(`${app.name}: no config resource named ${name}`);
    return found;
  },
  volume: (name) => {
    const found = declared(app, 'volume').find((volume) => volume.name === name);
    if (!found) throw new Error(`${app.name}: no volume resource named ${name}`);
    return found.claimName;
  },
  service: (service, port, path) => ownServiceUrl(app, service, port, path),
});

export function containerEnv(app: App, world: World): { container: string; rows: EnvSource[] }[] {
  const refs = refsFor(app, world);
  return Object.entries(appWorkloads(app)).flatMap(([workload, spec]) =>
    [
      [workload, spec] as const,
      ...((spec.initContainers ?? []) as (Workload & { name: string })[]).map(
        (c) => [`${workload}/${c.name}`, c] as const,
      ),
      ...((spec.sidecars ?? []) as (Workload & { name: string })[]).map(
        (c) => [`${workload}/${c.name}`, c] as const,
      ),
    ].map(([container, c]) => ({
      container,
      rows: [
        ...(c.env && !Array.isArray(c.env) ? describeEnv(c.env, refs) : []),
        ...(c.envFrom ?? []).map((from) => describeEnvFrom(from, refs)),
      ],
    })),
  );
}

export interface ResolvedSubgraph {
  name: string;
  app: string;
  prefix: string;
  url: string;
  websocket?: string;
  auth?: SubgraphAuth;
  schemaUrl?: string;
  schemaPath?: string;
}

export function rawFileUrl(repository: string, ref: string, path: string): string {
  const parsed = new URL(repository);
  const [owner, repo] = parsed.pathname
    .replace(/\.git$/, '')
    .split('/')
    .filter(Boolean);
  if (parsed.protocol !== 'https:' || !owner || !repo) {
    throw new Error(`${repository} is not an https://<host>/<owner>/<repo> url`);
  }
  if (parsed.hostname === 'github.com') {
    return `https://raw.githubusercontent.com/${owner}/${repo}/${ref}/${path}`;
  }
  return `${parsed.origin}/${owner}/${repo}/raw/branch/${ref}/${path}`;
}

function schemaSource(subgraph: Subgraph, where: string) {
  if (subgraph.repository === undefined && subgraph.ref === undefined) {
    if (subgraph.schema.startsWith('/') || subgraph.schema.split('/').includes('..')) {
      throw new Error(`${where}: schema ${subgraph.schema} must stay inside the app directory`);
    }

    return { schemaPath: subgraph.schema };
  }
  if (subgraph.repository === undefined || subgraph.ref === undefined) {
    throw new Error(`${where}: repository and ref go together`);
  }

  return { schemaUrl: rawFileUrl(subgraph.repository, subgraph.ref, subgraph.schema) };
}

export function supergraphApp(apps: App[]): App | undefined {
  const found = apps.filter((app) => declared(app, 'supergraph').length);
  if (found.length > 1) {
    throw new Error(
      `only one app may declare a supergraph, found ${found.map((app) => app.name).join(', ')}`,
    );
  }
  return found[0];
}

export function resolveSubgraphs(apps: App[]): ResolvedSubgraph[] {
  return apps.flatMap((app) =>
    declared(app, 'subgraph').map((subgraph) => {
      const where = `${app.name}: resources.${subgraph.name}`;
      for (const path of [subgraph.path, subgraph.websocket]) {
        if (path !== undefined && !path.startsWith('/')) {
          throw new Error(`${where}: ${path} must start with /`);
        }
      }

      if (!/^[a-z][a-zA-Z0-9]*$/.test(subgraph.prefix)) {
        throw new Error(`${where}: prefix ${subgraph.prefix} must be lowerCamelCase`);
      }

      for (const key of ['clients', 'groups'] as const) {
        const list = subgraph.auth?.[key];
        if (subgraph.auth && (!list?.length || list.some((item) => !item))) {
          throw new Error(`${where}: auth.${key} must list at least one non-empty name`);
        }
      }

      const spec = appWorkloads(app)[subgraph.workload];
      if (!spec) throw new Error(`${where}: no workload named ${subgraph.workload}`);
      const port = exportPort(app, {
        name: subgraph.name,
        workload: subgraph.workload,
        port: subgraph.port,
      });

      return {
        name: subgraph.name,
        app: app.name,
        prefix: subgraph.prefix,
        url: serviceUrl(
          spec.service?.name ?? subgraph.workload,
          appNamespace(app),
          port,
          subgraph.path,
        ),
        ...(subgraph.websocket ? { websocket: subgraph.websocket } : {}),
        ...(subgraph.auth ? { auth: subgraph.auth } : {}),
        ...schemaSource(subgraph, where),
      };
    }),
  );
}

export function validateSubgraphs(apps: App[]) {
  const supergraph = supergraphApp(apps);
  const subgraphs = resolveSubgraphs(apps);
  if (subgraphs.length && !supergraph) {
    throw new Error(
      `${subgraphs.map((s) => s.app).join(', ')}: subgraphs need an app declaring a supergraph`,
    );
  }
  const reserved = subgraphs.find((subgraph) => subgraph.name === 'node');
  if (reserved) {
    throw new Error(`${reserved.app}: subgraph name node is reserved for the router's node plugin`);
  }

  const clients = new Set(apps.flatMap((app) => Object.keys(appOidc(app))));
  for (const subgraph of subgraphs) {
    const missing = subgraph.auth?.clients.filter((client) => !clients.has(client)) ?? [];
    if (missing.length) {
      throw new Error(
        `${subgraph.app}: resources.${subgraph.name}: auth.clients has no oidc resource named ${missing.join(', ')}`,
      );
    }
  }

  for (const key of ['name', 'prefix'] as const) {
    const seen = new Map<string, string>();
    for (const subgraph of subgraphs) {
      const other = seen.get(subgraph[key]);
      if (other) {
        throw new Error(
          `subgraph ${key} ${subgraph[key]} is declared by both ${other} and ${subgraph.app}`,
        );
      }
      seen.set(subgraph[key], subgraph.app);
    }
  }
}

export function validateResources(world: World) {
  validateSubgraphs(world.apps);

  for (const app of world.apps) {
    validateRefs(app, world.external?.has(app.name) ?? false);

    for (const resource of app.resources ?? []) {
      if (resource.type === 'app') appExportUrl(app, resource, world.apps);
    }

    for (const spec of Object.values(appWorkloads(app))) {
      expandWorkload(spec, refsFor(app, world));
    }

    for (const config of declared(app, 'config')) {
      for (const ref of Object.values(config.values ?? {})) {
        const kind = SECRET_KINDS.find((k) => ref[k]) as SecretKind;
        secretRef(app, kind, ref[kind]);
      }
    }

    for (const volume of declared(app, 'volume')) storageClass(app, volume, world.platform);
    for (const spec of Object.values(appDatabases(app))) databaseHost(app, spec, world.platform);
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

export const rbacPath = (app: App, target: Target) =>
  `${target.location}/${app.name}/manifests/rbac.generated.yaml`;

const clusterRoles = (app: App) =>
  declared(app, 'serviceAccount').filter((account) => account.clusterRules);

export function renderRbac(app: App): string | undefined {
  const documents = clusterRoles(app).flatMap((account) => {
    const role = account.clusterRole ?? account.name;
    return [
      {
        apiVersion: 'rbac.authorization.k8s.io/v1',
        kind: 'ClusterRole',
        metadata: { name: role },
        rules: account.clusterRules,
      },
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
  (app.resources ?? []).some(
    (r) =>
      r.type === 'config' ||
      r.type === 'dashboard' ||
      (r.type === 'serviceAccount' && r.clusterRules),
  );

export const kustomizationPath = (app: App, target: Target) =>
  `${target.location}/${app.name}/kustomization.yaml`;

export function renderKustomization(app: App, target: Target): string | undefined {
  if (!needsKustomization(app)) return undefined;

  const generators = [
    ...declared(app, 'config').map(({ configMap, files }) => ({
      name: configMap,
      namespace: appNamespace(app),
      files,
    })),
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
    ...(generators.length
      ? { configMapGenerator: generators, generatorOptions: { disableNameSuffixHash: true } }
      : {}),
  };

  return `${HEADER}\n${stringify(kustomization, { lineWidth: 0, aliasDuplicateObjects: false })}`;
}

export const sourcesOf = <T extends Source['type']>(app: App, type: T) =>
  (app.sources ?? []).filter(
    (source): source is Extract<Source, { type: T }> => source.type === type,
  );

export function validateManifestSources(
  app: App,
  found: { hasManifests: boolean; hasRootKustomization: boolean },
) {
  const declared = sourcesOf(app, 'manifests')[0];
  if (!declared && (found.hasManifests || found.hasRootKustomization || needsKustomization(app))) {
    throw new Error(
      `${app.name}: sources: add a manifests source, this app has a kustomization that would not deploy`,
    );
  }

  if (declared && found.hasRootKustomization && declared.path !== '.') {
    throw new Error(
      `${app.name}: sources: manifests path must be . to deploy the app directory's kustomization`,
    );
  }
}

export const appFiles = (app: App, target: Target) =>
  [
    ...declared(app, 'config').flatMap((config) => config.files),
    ...declared(app, 'dashboard').map((dashboard) => dashboard.file),
    ...sourcesOf(app, 'helm').map((release) => release.values),
    ...sourcesOf(app, 'manifests')
      .filter(() => !needsKustomization(app))
      .map(({ path }) => (path === '.' ? 'kustomization.yaml' : `${path}/kustomization.yaml`)),
  ].map((file) => `${target.location}/${app.name}/${file}`);

export const ciProvider = (app: App) => (app.build as Build | undefined)?.ci ?? 'github';

export const workflowPath = (app: App) => `.${ciProvider(app)}/workflows/app-${app.name}.yaml`;

export function renderWorkflow(app: App, target: Target): string {
  if (ciProvider(app) === 'forgejo') return renderForgejoWorkflow(app, target);

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

const ACTIONS = {
  checkout: 'actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1',
  buildx: 'docker/setup-buildx-action@f87e5991a6d7451dcb8d9637bfbc97413f497069',
  login: 'docker/login-action@81c6f93d4bc991e510b342a00b419b3bb4efc5c6',
  buildPush: 'docker/build-push-action@c3c9e263c25d99ce0380d002d59b67737d91b0dc',
  cache: 'actions/cache@55cc8345863c7cc4c66a329aec7e433d2d1c52a9',
};

const ON_MAIN = "github.ref == 'refs/heads/main'";

const DEPLOY_SCRIPT = String.raw`COMMITTED_AT=$(git show -s --format=%ct "$COMMIT_SHA")
git config --global user.name ci-image-updater
git config --global user.email ci-image-updater@git.anurag.sh
git clone "https://x-access-token:$FORGEJO_TOKEN@git.anurag.sh/anurag/k8s.git" config-repo
cd config-repo
PR_BRANCH="ci/update-$SERVICE_NAME-app-$SERVICE_NAME-$TAG"
TITLE="chore(deploy): update $SERVICE_NAME images to $TAG [skip ci]"
git checkout -b "$PR_BRANCH"
IMAGE="$IMAGE" yq -I2 -P -i '.images[strenv(IMAGE)] = strenv(TAG)' "$VALUES_FILE"
git add "$MANIFESTS_PATH"
git commit -m "$TITLE" || { echo "No changes to commit"; exit 0; }
git push -f origin "$PR_BRANCH"
METADATA=$(jq -cn --arg service "$SERVICE_NAME" --arg tag "$TAG" --arg images "$IMAGE" \
  --arg path "$MANIFESTS_PATH" --arg repo "$REPOSITORY" --arg ref "$REF_NAME" --arg run "$RUN_NUMBER" \
  --arg sha "$COMMIT_SHA" --arg committed_at "$COMMITTED_AT" \
  '{service: $service, tag: $tag, images: $images, path: $path, branch: "main", repo: $repo, ref: $ref, run: $run, sha: $sha, committed_at: $committed_at}')
BODY=$(printf '## Image Update\n\n| Field | Value |\n|-------|-------|\n| **Service** | \x60%s\x60 |\n| **Tag** | \x60%s\x60 |\n| **Manifests Path** | \x60%s\x60 |\n| **Target Branch** | \x60main\x60 |\n\n**Triggered by:** [%s@%s](%s)\n\n<!-- metadata:%s -->\n' \
  "$SERVICE_NAME" "$TAG" "$MANIFESTS_PATH" "$REPOSITORY" "$REF_NAME" "$RUN_URL" "$METADATA")
PAYLOAD=$(jq -n --arg title "$TITLE" --arg head "$PR_BRANCH" --arg body "$BODY" \
  '{title: $title, head: $head, base: "main", body: $body}')
CODE=$(curl -s -o response.json -w '%{http_code}' -X POST \
  -H "Authorization: token $FORGEJO_TOKEN" -H 'Content-Type: application/json' \
  -d "$PAYLOAD" https://git.anurag.sh/api/v1/repos/anurag/k8s/pulls)
case "$CODE" in
  2*) jq -r .html_url response.json ;;
  409) echo "PR already exists for $PR_BRANCH" ;;
  *) echo "failed to create PR (HTTP $CODE)"; cat response.json; exit 1 ;;
esac
`;

function renderForgejoWorkflow(app: App, target: Target): string {
  const build = app.build as Build;
  const dir = `${target.location}/${app.name}`;
  const context = build.context ?? dir;
  const args = Object.entries(build.args ?? {}).map(([k, v]) => `${k}=${v}`);
  const test = build.test;
  const registry = build.image.split('/')[0];
  const cacheRef = `type=registry,ref=${build.image}:buildcache`;

  const buildJob = {
    'runs-on': 'docker',
    steps: [
      { uses: ACTIONS.checkout },
      {
        name: 'Compute commit metadata',
        id: 'commit-meta',
        run: 'echo "committed-at=$(git show -s --format=%ct ${{ github.sha }})" >> "$GITHUB_OUTPUT"',
      },
      { uses: ACTIONS.buildx },
      {
        uses: ACTIONS.login,
        if: ON_MAIN,
        with: {
          registry,
          username: '${{ secrets.REGISTRY_USERNAME }}',
          password: '${{ secrets.REGISTRY_PASSWORD }}',
        },
      },
      {
        uses: ACTIONS.buildPush,
        with: {
          context,
          file: build.dockerfile ?? `${dir}/Dockerfile`,
          push: `\${{ ${ON_MAIN} }}`,
          tags: `${build.image}:latest,${build.image}:\${{ github.sha }}`,
          labels: [
            'net.inf-k8s.commit-sha=${{ github.sha }}',
            'net.inf-k8s.committed-at=${{ steps.commit-meta.outputs.committed-at }}',
          ].join('\n'),
          ...(args.length ? { 'build-args': args.join('\n') } : {}),
          'cache-from': cacheRef,
          'cache-to': `\${{ ${ON_MAIN} && '${cacheRef},mode=max' || '' }}`,
        },
      },
    ],
  };

  const testJob = test && {
    'runs-on': 'docker',
    ...(test.db
      ? {
          services: {
            db: {
              image: test.db,
              env: {
                POSTGRES_USER: 'postgres',
                POSTGRES_PASSWORD: 'postgres',
                POSTGRES_DB: test.dbName ?? 'postgres',
              },
              options:
                '--health-cmd "pg_isready -U postgres" --health-interval 10s --health-timeout 5s --health-retries 5',
            },
          },
        }
      : {}),
    env: {
      ...(test.db
        ? { DATABASE_URL: `postgres://postgres:postgres@db:5432/${test.dbName ?? 'postgres'}` }
        : {}),
      SQLX_OFFLINE: 'true',
    },
    steps: [
      { uses: ACTIONS.checkout },
      ...(test.packages
        ? [{ run: `apt-get update && apt-get install -y ${test.packages.join(' ')}` }]
        : []),
      ...(test.setup ? [{ run: test.setup }] : []),
      {
        uses: ACTIONS.cache,
        with: {
          path: `${dir}/target`,
          key: `\${{ runner.os }}-app-target-\${{ hashFiles('**/Cargo.lock') }}-${app.name}`,
        },
      },
      { run: test.run, 'working-directory': dir },
    ],
  };

  const needs = test ? ['build', 'test'] : ['build'];
  const deployJob = {
    needs,
    if: `\${{ ${ON_MAIN} && !cancelled() && needs.build.result == 'success'${
      test ? " && needs.test.result == 'success'" : ''
    } }}`,
    'runs-on': 'docker',
    steps: [
      { uses: ACTIONS.checkout },
      {
        name: 'Raise image update PR',
        env: {
          FORGEJO_TOKEN: '${{ secrets.CI_IMAGE_UPDATER_TOKEN }}',
          SERVICE_NAME: app.name,
          IMAGE: build.image,
          TAG: '${{ github.sha }}',
          COMMIT_SHA: '${{ github.sha }}',
          MANIFESTS_PATH: dir,
          VALUES_FILE: `${dir}/image.values.yaml`,
          REPOSITORY: '${{ github.repository }}',
          REF_NAME: '${{ github.ref_name }}',
          RUN_NUMBER: '${{ github.run_number }}',
          RUN_URL:
            '${{ github.server_url }}/${{ github.repository }}/actions/runs/${{ github.run_number }}',
        },
        run: DEPLOY_SCRIPT,
      },
    ],
  };

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
    jobs: { build: buildJob, ...(testJob ? { test: testJob } : {}), deploy: deployJob },
  };

  return `${HEADER}\n${stringify(workflow, { lineWidth: 0, aliasDuplicateObjects: false })}`;
}

export const declared = <T extends Resource['type']>(app: App, type: T) =>
  (app.resources ?? []).filter(
    (resource): resource is Extract<Resource, { type: T }> => resource.type === type,
  );

const specs = <S extends { name: string; type: string }>(resources: S[]) =>
  resources.map(({ name, type: _type, ...spec }) => [name, spec] as const);

const HTTP_ONLY = [
  'hosts',
  'paths',
  'rules',
  'dns',
  'ratelimit',
  'timeouts',
  'loadBalancer',
] as const;

export function appRoutes(app: App): Record<string, Route> {
  return Object.fromEntries(
    specs(declared(app, 'route').filter((route) => route.protocol !== 'tcp')),
  ) as Record<string, Route>;
}

export function appTcpRoutes(app: App): Record<string, Route> {
  return Object.fromEntries(
    specs(declared(app, 'route').filter((route) => route.protocol === 'tcp')),
  ) as Record<string, Route>;
}

export function appOidc(app: App): Record<string, OidcClient> {
  return Object.fromEntries(specs(declared(app, 'oidc'))) as Record<string, OidcClient>;
}

export function appMetrics(app: App): Metrics[] {
  return specs(declared(app, 'metrics')).map(([, spec]) => spec as Metrics);
}

export function appSecrets(app: App): {
  name: string;
  secretName: string;
  secretTemplate?: SecretTemplate;
  generate?: Record<string, GeneratedKey>;
}[] {
  return (app.resources ?? []).flatMap((resource) =>
    resource.type === 'secret'
      ? [
          {
            name: resource.name,
            secretName: resource.secretName,
            secretTemplate: resource.secretTemplate,
            generate: resource.generate,
          },
        ]
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

export const databaseRole = (database: string, spec?: Pick<DatabaseSpec, 'role'>) =>
  spec?.role ?? database.replaceAll('-', '_');

export const appRoles = (apps: App[]) =>
  apps.flatMap((app) =>
    Object.entries(appDatabases(app)).map(([database, spec]) => databaseRole(database, spec)),
  );
