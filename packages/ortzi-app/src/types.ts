import type { Resource, Source } from './providers/provider.ts';

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

export interface Export {
  name: string;
  workload: string;
  port: string | number;
}

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

export interface Artifact {
  repo: string;
  tag: string;
}

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
