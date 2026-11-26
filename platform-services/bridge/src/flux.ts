export const APP_ANNOTATION = 'event.toolkit.fluxcd.io/app';
export const COMMIT_ANNOTATION = 'event.toolkit.fluxcd.io/commit';

interface Condition {
  type: string;
  status: string;
  message?: string;
  reason?: string;
}

export interface Metadata {
  name: string;
  namespace?: string;
  generation?: number;
  labels?: Record<string, string>;
  annotations?: Record<string, string>;
}

export interface FluxObject {
  metadata: Metadata;
  spec: {
    suspend?: boolean;
    releaseName?: string;
    targetNamespace?: string;
    image?: string;
    imageRepositoryRef?: { name: string };
  };
  status?: {
    observedGeneration?: number;
    conditions?: Condition[];
    lastAppliedRevision?: string;
    history?: { chartVersion?: string; lastDeployed?: string }[];
    latestRef?: { name?: string; tag?: string };
    lastScanResult?: { scanTime?: string; latestTags?: string[] };
  };
}

export interface ConfigMap {
  metadata: Metadata;
  data?: Record<string, string>;
}

interface PodSpec {
  containers?: { name: string; image: string }[];
  initContainers?: { name: string; image: string }[];
}

export interface Workload {
  kind: string;
  metadata: Metadata;
  spec: {
    replicas?: number;
    selector?: { matchLabels?: Record<string, string> };
    template?: { spec?: PodSpec };
    jobTemplate?: { spec?: { template?: { spec?: PodSpec } } };
  };
  status?: {
    readyReplicas?: number;
    updatedReplicas?: number;
    replicas?: number;
    numberReady?: number;
    desiredNumberScheduled?: number;
    observedGeneration?: number;
  };
}

export interface Pod {
  metadata: Metadata;
  status?: {
    containerStatuses?: {
      restartCount?: number;
      state?: { waiting?: { reason?: string } };
    }[];
  };
}

export interface Image {
  repository: string;
  desiredTag?: string;
  policy?: FluxObject;
  scan?: FluxObject;
}

export interface App {
  name: string;
  kustomization: FluxObject;
  kustomizations: FluxObject[];
  releases: FluxObject[];
  images: Image[];
}

export interface Cluster {
  kustomizations: FluxObject[];
  helmReleases: FluxObject[];
  configMaps: ConfigMap[];
  imagePolicies: FluxObject[];
  imageRepositories: FluxObject[];
}

const belongs = (name: string) => (object: FluxObject) =>
  object.metadata.annotations?.[APP_ANNOTATION] === name;

// image.values.yaml is `images:` then one `<repository>: <tag> # {"$imagepolicy": ...}` per line
export function imageValues(values: string | undefined): Map<string, string> {
  const tags = new Map<string, string>();
  for (const line of values?.split('\n') ?? []) {
    const match = /^\s+([^\s:#][^\s]*):\s+([^\s#]+)/.exec(line);
    if (match) {
      tags.set(match[1]!, match[2]!);
    }
  }
  return tags;
}

export function assemble(name: string, cluster: Cluster): App | undefined {
  const own = cluster.kustomizations.filter(belongs(name));
  const kustomization = own.find((ks) => ks.metadata.name === name);
  if (!kustomization) {
    return undefined;
  }

  const desired = imageValues(
    cluster.configMaps.find((cm) => cm.metadata.name === `${name}-image-values`)?.data?.[
      'values.yaml'
    ],
  );
  const scans = cluster.imageRepositories.filter(belongs(name));
  const policies = cluster.imagePolicies.filter(belongs(name));
  const repositories = new Set([
    ...desired.keys(),
    ...scans.map((scan) => scan.spec.image).filter((image): image is string => Boolean(image)),
  ]);

  return {
    name,
    kustomization,
    kustomizations: own.filter((ks) => ks !== kustomization),
    releases: cluster.helmReleases.filter(belongs(name)),
    images: [...repositories].sort().map((repository) => {
      const scan = scans.find((candidate) => candidate.spec.image === repository);
      return {
        repository,
        desiredTag: desired.get(repository),
        scan,
        policy: policies.find(
          (policy) =>
            policy.spec.imageRepositoryRef?.name === scan?.metadata.name ||
            policy.status?.latestRef?.name === repository,
        ),
      };
    }),
  };
}

export const appNames = (cluster: Cluster) =>
  cluster.kustomizations
    .filter((ks) => ks.metadata.annotations?.[APP_ANNOTATION] === ks.metadata.name)
    .map((ks) => ks.metadata.name)
    .sort();

export function ready(object: FluxObject): boolean {
  const condition = object.status?.conditions?.find((c) => c.type === 'Ready');
  return (
    object.status?.observedGeneration === object.metadata.generation && condition?.status === 'True'
  );
}

export const readyMessage = (object: FluxObject) =>
  object.status?.conditions?.find((c) => c.type === 'Ready')?.message;

export const objectsOf = (app: App) => [app.kustomization, ...app.kustomizations, ...app.releases];

export const revision = (app: App) =>
  app.kustomization.metadata.annotations?.[COMMIT_ANNOTATION] ??
  app.kustomization.status?.lastAppliedRevision;

export const lastDeployed = (app: App) =>
  app.releases
    .map((release) => release.status?.history?.[0]?.lastDeployed)
    .filter((at): at is string => Boolean(at))
    .sort()
    .at(-1);

// helm stamps its release on what it installs; flux labels what its kustomizations apply
export function workloadsFor(app: App, workloads: Workload[]): Workload[] {
  const releases = new Set(
    app.releases.map((r) => `${r.spec.targetNamespace}/${r.spec.releaseName ?? r.metadata.name}`),
  );
  const kustomizations = new Set(
    [app.kustomization, ...app.kustomizations].map((ks) => ks.metadata.name),
  );
  return workloads.filter(({ metadata }) => {
    const release = metadata.annotations?.['meta.helm.sh/release-name'];
    const namespace = metadata.annotations?.['meta.helm.sh/release-namespace'];
    const ks = metadata.labels?.['kustomize.toolkit.fluxcd.io/name'];
    return (release && releases.has(`${namespace}/${release}`)) || (ks && kustomizations.has(ks));
  });
}

export function containerImages(workload: Workload): string[] {
  const pod = workload.spec.template?.spec ?? workload.spec.jobTemplate?.spec?.template?.spec;
  return [...(pod?.containers ?? []), ...(pod?.initContainers ?? [])].map((c) => c.image);
}

export function splitImage(image: string): {
  repository: string;
  tag?: string;
} {
  const at = image.split('@')[0]!;
  const colon = at.lastIndexOf(':');
  return colon > at.lastIndexOf('/')
    ? { repository: at.slice(0, colon), tag: at.slice(colon + 1) }
    : { repository: at };
}

export function replicas(workload: Workload): {
  ready: number;
  desired: number;
} {
  const status = workload.status ?? {};
  if (workload.kind === 'DaemonSet') {
    return {
      ready: status.numberReady ?? 0,
      desired: status.desiredNumberScheduled ?? 0,
    };
  }
  if (workload.kind === 'CronJob' || workload.kind === 'Job') {
    return { ready: 0, desired: 0 };
  }
  return {
    ready: status.readyReplicas ?? 0,
    desired: workload.spec.replicas ?? 1,
  };
}

const selects = (selector: Record<string, string> | undefined, pod: Pod) =>
  selector !== undefined &&
  Object.entries(selector).every(([key, value]) => pod.metadata.labels?.[key] === value);

export function podsOf(workload: Workload, pods: Pod[]): Pod[] {
  return pods.filter((pod) => selects(workload.spec.selector?.matchLabels, pod));
}

export function health(pods: Pod[]): {
  restarts: number;
  waitingReason?: string;
} {
  const statuses = pods.flatMap((pod) => pod.status?.containerStatuses ?? []);
  return {
    restarts: statuses.reduce((sum, status) => sum + (status.restartCount ?? 0), 0),
    waitingReason: statuses.map((status) => status.state?.waiting?.reason).find(Boolean),
  };
}

// v1.0.<n>-<sha7>, as `ortzi app version` tags releases
export const tagSha = (tag: string | undefined) => /-([0-9a-f]{7})$/.exec(tag ?? '')?.[1];
