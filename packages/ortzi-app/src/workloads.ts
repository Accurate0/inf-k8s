import { type Kube, parseImage } from '@ortzi/kit';
import type { App } from './flux';
import { type RolloutStatus, ROLLED_OUT, rolledOut } from './rollout.ts';

const KINDS = 'deployments,statefulsets,daemonsets,cronjobs,jobs';

export interface Container {
  name: string;
  image: string;
}

interface PodSpec {
  containers?: Container[];
  initContainers?: Container[];
}

export interface Workload {
  kind: string;
  metadata: {
    name: string;
    namespace: string;
    generation?: number;
    resourceVersion?: string;
    labels?: Record<string, string>;
    annotations?: Record<string, string>;
  };
  spec: {
    replicas?: number;
    template?: { spec?: PodSpec };
    jobTemplate?: { spec?: { template?: { spec?: PodSpec } } };
  };
  status?: RolloutStatus;
}

export const listWorkloads = (kube: Kube) => kube.list<Workload>(KINDS);

// Helm stamps its release on what it installs; flux labels what its kustomizations apply.
export function workloadsFor(app: App, workloads: Workload[]): Workload[] {
  const releases = new Set(
    app.releases.map((r) => `${r.spec.targetNamespace}/${r.spec.releaseName ?? r.metadata.name}`),
  );
  const kustomizations = new Set(
    [app.kustomization, ...app.kustomizations].map((ks) => ks.metadata.name),
  );
  return workloads.filter(({ metadata }) => {
    const release = metadata.annotations?.['meta.helm.sh/release-name'];
    const releaseNamespace = metadata.annotations?.['meta.helm.sh/release-namespace'];
    const ks = metadata.labels?.['kustomize.toolkit.fluxcd.io/name'];
    return (
      (release && releases.has(`${releaseNamespace}/${release}`)) || (ks && kustomizations.has(ks))
    );
  });
}

export function containersOf(workload: Workload): Container[] {
  const pod = workload.spec.template?.spec ?? workload.spec.jobTemplate?.spec?.template?.spec;
  return [...(pod?.containers ?? []), ...(pod?.initContainers ?? [])];
}

export function runningTags(workloads: Workload[]): Map<string, string> {
  const running = new Map<string, string>();
  for (const workload of workloads) {
    for (const container of containersOf(workload)) {
      const { repository, tag } = parseImage(container.image);
      running.set(repository, tag);
    }
  }
  return running;
}

export const workloadRef = (workload: Workload) =>
  `${workload.kind.toLowerCase()}/${workload.metadata.name}`;

export function waitForRollout(kube: Kube, workload: Workload, minutes: number): boolean {
  const ref = workloadRef(workload);
  if (workload.kind === 'CronJob') {
    console.log(`${ref} has no rollout; the new image applies on its next run`);
    return true;
  }

  if (!ROLLED_OUT[workload.kind]) {
    console.log(`${ref} has no rollout to wait for`);
    return false;
  }

  return kube
    .namespace(workload.metadata.namespace)
    .waitFor<Workload>(workload.kind.toLowerCase(), workload.metadata.name, rolledOut, minutes);
}

export function waitForRollouts(kube: Kube, workloads: Workload[], minutes: number): boolean {
  return workloads.map((workload) => waitForRollout(kube, workload, minutes)).every(Boolean);
}
