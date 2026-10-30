import { type Kube, fetchText } from '@ortzi/kit';

export const CRONJOB = 'k8s-schemas';
const VERSION_URL = 'https://k8s-schemas.anurag.sh/version.json';

interface Condition {
  type: string;
  status: string;
  message?: string;
}

export interface Job {
  metadata: {
    name: string;
    creationTimestamp?: string;
    annotations?: Record<string, string>;
    ownerReferences?: { kind: string; name: string }[];
  };
  status?: {
    active?: number;
    succeeded?: number;
    failed?: number;
    startTime?: string;
    completionTime?: string;
    conditions?: Condition[];
  };
}

export interface CronJob {
  metadata: { name: string; uid: string };
  spec: { schedule: string; jobTemplate: { spec: any } };
  status?: { lastScheduleTime?: string; lastSuccessfulTime?: string };
}

export type JobResult = 'complete' | 'failed' | 'running';

export const getCronJob = (kube: Kube) => kube.get<CronJob>('cronjob', CRONJOB);

export function listJobs(kube: Kube): Job[] {
  return kube
    .list<Job>('jobs')
    .filter((j) => j.metadata.ownerReferences?.some((o) => o.kind === 'CronJob' && o.name === CRONJOB))
    .sort((a, b) =>
      (b.metadata.creationTimestamp ?? '').localeCompare(a.metadata.creationTimestamp ?? ''),
    );
}

export function jobResult(job: Job): JobResult {
  const conditions = job.status?.conditions ?? [];
  if (conditions.some((c) => c.type === 'Complete' && c.status === 'True')) return 'complete';
  if (conditions.some((c) => c.type === 'Failed' && c.status === 'True')) return 'failed';
  return 'running';
}

export function publishedVersion(): string {
  const text = fetchText(VERSION_URL, { timeoutSeconds: 10 });
  if (text === undefined) return 'unknown';
  try {
    const parsed = JSON.parse(text);
    return String(parsed.version ?? parsed.sha256 ?? text.trim());
  } catch {
    return text.trim();
  }
}

function jobWithVersion(cron: CronJob, name: string, namespace: string, k8sVersion: string) {
  const spec = cron.spec.jobTemplate.spec;
  for (const container of spec.template.spec.containers) {
    const env: { name: string; value?: string }[] = container.env ?? [];
    const existing = env.find((e) => e.name === 'K8S_VERSION');
    if (existing) existing.value = k8sVersion;
    else env.push({ name: 'K8S_VERSION', value: k8sVersion });
    container.env = env;
  }
  return {
    apiVersion: 'batch/v1',
    kind: 'Job',
    metadata: {
      name,
      namespace,
      annotations: { 'cronjob.kubernetes.io/instantiate': 'manual' },
      ownerReferences: [
        {
          apiVersion: 'batch/v1',
          kind: 'CronJob',
          name: CRONJOB,
          uid: cron.metadata.uid,
          controller: true,
        },
      ],
    },
    spec,
  };
}

export function createJob(kube: Kube, namespace: string, k8sVersion: string | undefined): string {
  const name = `${CRONJOB}-manual-${Math.floor(Date.now() / 1000)}`;
  const created = k8sVersion
    ? kube.capture(['create', '-f', '-', '-o', 'name'], {
        stdin: JSON.stringify(jobWithVersion(getCronJob(kube), name, namespace, k8sVersion)),
      })
    : kube.capture(['create', 'job', `--from=cronjob/${CRONJOB}`, name, '-o', 'name']);
  return created.trim().replace(/^job\.batch\//, '');
}

export function followJob(kube: Kube, name: string, deadline: number): JobResult {
  while (Date.now() < deadline) {
    kube.run(['logs', '-f', `job/${name}`, '--pod-running-timeout=5m'], { check: false });
    const state = jobResult(kube.get<Job>('job', name));
    if (state !== 'running') return state;
  }
  return 'running';
}
