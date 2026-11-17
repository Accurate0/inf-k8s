export interface Sidecar {
  name: string;
  image: string;
  inherit?: ('env' | 'envFrom' | 'mounts')[];
  env?: Record<string, unknown>;
  envFrom?: (string | Record<string, unknown>)[];
  mounts?: Record<string, unknown>;
}

export interface Workload {
  image: string;
  kind?: 'Deployment' | 'DaemonSet' | 'CronJob' | 'Job';
  replicas?: number;
  autoscale?: { min: number; max: number; cpu: number };
  schedule?: string;
  phase?: 'preSync' | 'postSync';
  service?: { name: string; trafficDistribution?: string };
  initContainers?: { image: string }[];
  sidecars?: Sidecar[];
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
  gateway: string;
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
  ci: 'github' | 'forgejo';
  context?: string;
  dockerfile?: string;
  args?: Record<string, string>;
  paths?: string[];
  test?: {
    run: string;
    db?: { image: string; databaseName?: string };
    packages?: string[];
    setup?: string;
    toolchain?: { rust: string };
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

export interface Artifact {
  repo: string;
  tag: string;
}

export interface ArtifactSource {
  type: 'artifact';
  repo: string;
  tag: string;
  path?: string;
  name?: string;
}

export type Source = HelmSource | GitSource | ManifestsSource | ArtifactSource;

export interface ArgocdOverrides {
  options?: string[];
  automated?: boolean;
  ignoreDifferences?: Record<string, unknown>[];
}

export interface App {
  name: string;
  repository?: string;
  charts?: Record<string, string>;
  chartRegistry?: string;
  workloads?: NamedWorkload[];
  resources?: Resource[];
  exports?: Export[];
  sources?: Source[];
  artifact?: Artifact;
  overrides?: { argocd?: { application?: ArgocdOverrides } };
  build?: Build;
  network: { ingress: string[]; policy?: boolean };
  namespace?: { name?: string; labels?: Record<string, string> };
}

export type NamedWorkload = Workload & { name: string };

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
  readValues?: (path: string) => Record<string, unknown>;
}

export interface ResourceKind {
  group: 'consumed' | 'exposed';
  ref?: 'value' | 'secret' | 'volume';
}

export const RESOURCE_TYPES: Record<Resource['type'], ResourceKind> = {
  config: { group: 'consumed' },
  secret: { group: 'consumed', ref: 'secret' },
  database: { group: 'consumed', ref: 'value' },
  cache: { group: 'consumed', ref: 'value' },
  bucket: { group: 'consumed', ref: 'value' },
  volume: { group: 'consumed', ref: 'volume' },
  serviceAccount: { group: 'consumed' },
  app: { group: 'consumed', ref: 'value' },
  platform: { group: 'consumed', ref: 'value' },
  supergraph: { group: 'exposed' },
  subgraph: { group: 'exposed' },
  route: { group: 'exposed' },
  oidc: { group: 'exposed' },
  metrics: { group: 'exposed' },
  dashboard: { group: 'exposed' },
};

export const isValueResource = (type: Resource['type']) => RESOURCE_TYPES[type]?.ref === 'value';
