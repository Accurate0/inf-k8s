import type { Kube } from '@ortzi/kit';
import {
  createJob,
  followJob,
  getCronJob,
  jobResult,
  listJobs,
  publishedVersion,
} from './jobs';

export interface SyncOptions {
  detach: boolean;
  force: boolean;
  timeoutMinutes: number;
  k8sVersion?: string;
}

export function sync(kube: Kube, namespace: string, options: SyncOptions): number {
  const active = listJobs(kube).filter((j) => (j.status?.active ?? 0) > 0);
  if (active.length > 0 && !options.force) {
    console.error(
      `${active.map((j) => j.metadata.name).join(', ')} is already running; pass --force to start another`,
    );
    return 1;
  }

  const before = publishedVersion();
  const name = createJob(kube, namespace, options.k8sVersion);
  console.log(`started ${name}`);
  if (options.detach) return 0;

  const state = followJob(kube, name, Date.now() + options.timeoutMinutes * 60_000);
  if (state === 'failed') {
    console.error(`${name} failed; see kubectl -n ${namespace} describe job ${name}`);
    return 1;
  }
  if (state === 'running') {
    console.error(
      `${name} is still running after ${options.timeoutMinutes}m; check with \`ortzi schemas status\``,
    );
    return 1;
  }

  const after = publishedVersion();
  console.log(after === before ? `published version unchanged (${after})` : `published version ${before} -> ${after}`);
  return 0;
}

export function status(kube: Kube): number {
  const cron = getCronJob(kube);
  console.log(`schedule         ${cron.spec.schedule}`);
  console.log(`last scheduled   ${cron.status?.lastScheduleTime ?? '-'}`);
  console.log(`last successful  ${cron.status?.lastSuccessfulTime ?? '-'}`);
  console.log(`published        ${publishedVersion()}`);

  const recent = listJobs(kube).slice(0, 6);
  if (recent.length > 0) {
    console.log('');
    for (const job of recent) {
      const manual = job.metadata.annotations?.['cronjob.kubernetes.io/instantiate'] === 'manual';
      console.log(
        [
          job.metadata.name.padEnd(36),
          (manual ? 'manual' : 'scheduled').padEnd(10),
          jobResult(job).padEnd(9),
          job.status?.startTime ?? '-',
          job.status?.completionTime ? `-> ${job.status.completionTime}` : '',
        ].join(' '),
      );
    }
  }
  return 0;
}

export function logs(kube: Kube, job: string | undefined): number {
  const name = job ?? listJobs(kube)[0]?.metadata.name;
  if (!name) {
    console.error('no k8s-schemas jobs found');
    return 1;
  }
  return kube.run(['logs', '-f', `job/${name}`], { check: false }).code ?? 1;
}
