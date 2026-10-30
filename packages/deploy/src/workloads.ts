import { type Kube, parseImage } from '@ortzi/kit';
import type { Application } from './argo';

const KINDS = 'deployments,statefulsets,daemonsets,cronjobs';

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
  metadata: { name: string; namespace: string };
  spec: {
    template?: { spec?: PodSpec };
    jobTemplate?: { spec?: { template?: { spec?: PodSpec } } };
  };
}

export const listWorkloads = (kube: Kube) => kube.list<Workload>(KINDS, '-A');

export function workloadsFor(app: Application, workloads: Workload[]): Workload[] {
  const owned = new Set(
    (app.status?.resources ?? []).map((r) => `${r.kind}/${r.namespace}/${r.name}`),
  );
  return workloads.filter((w) => owned.has(`${w.kind}/${w.metadata.namespace}/${w.metadata.name}`));
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
  const output = kube
    .namespace(workload.metadata.namespace)
    .run(['rollout', 'status', ref, `--timeout=${minutes * 60}s`], { check: false });
  return output.ok;
}

export function waitForRollouts(kube: Kube, workloads: Workload[], minutes: number): boolean {
  return workloads.map((workload) => waitForRollout(kube, workload, minutes)).every(Boolean);
}
