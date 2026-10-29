import { args, capture, json, main, parseArgs, run as exec } from '@ortzi/host';

const NAMESPACE = 'k8s-schemas';
const CRONJOB = 'k8s-schemas';
const VERSION_URL = 'https://k8s-schemas.anurag.sh/version.json';

const USAGE = `usage: ortzi schemas <command> [flags]

commands:
  sync     run the k8s-schemas job now and follow it
  status   show the schedule, recent runs and the published version
  logs     follow the latest run (or --job NAME)

flags:
  --context CTX        kube context (default: current)
  --detach             sync: start the job and return
  --force              sync: start even if a run is already active
  --timeout MINUTES    sync: give up following after this long (default 20)
  --k8s-version V      sync: override K8S_VERSION for this run, e.g. v1.36.0
  --job NAME           logs: the job to follow`;

interface Condition {
  type: string;
  status: string;
  message?: string;
}

interface Job {
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

interface CronJob {
  metadata: { name: string; uid: string };
  spec: { schedule: string; jobTemplate: { spec: any } };
  status?: { lastScheduleTime?: string; lastSuccessfulTime?: string };
}

let context: string | undefined;

function kubectl(...rest: string[]): string[] {
  return [...(context ? ['--context', context] : []), '-n', NAMESPACE, ...rest];
}

function jobs(): Job[] {
  const list = json<{ items: Job[] }>('kubectl', kubectl('get', 'jobs', '-o', 'json'));
  return list.items
    .filter((j) => j.metadata.ownerReferences?.some((o) => o.kind === 'CronJob' && o.name === CRONJOB))
    .sort((a, b) =>
      (b.metadata.creationTimestamp ?? '').localeCompare(a.metadata.creationTimestamp ?? ''),
    );
}

function result(job: Job): 'complete' | 'failed' | 'running' {
  const conditions = job.status?.conditions ?? [];
  if (conditions.some((c) => c.type === 'Complete' && c.status === 'True')) return 'complete';
  if (conditions.some((c) => c.type === 'Failed' && c.status === 'True')) return 'failed';
  return 'running';
}

function publishedVersion(): string {
  const out = exec('curl', ['-fsS', '-m', '10', VERSION_URL], {
    stdout: 'piped',
    stderr: 'piped',
    check: false,
  });
  if (!out.ok) return 'unknown';
  try {
    const parsed = JSON.parse(out.stdout);
    return String(parsed.version ?? parsed.sha256 ?? out.stdout.trim());
  } catch {
    return out.stdout.trim();
  }
}

function createJob(k8sVersion: string | undefined): string {
  const name = `${CRONJOB}-manual-${Math.floor(Date.now() / 1000)}`;
  if (!k8sVersion) {
    return capture(
      'kubectl',
      kubectl('create', 'job', `--from=cronjob/${CRONJOB}`, name, '-o', 'name'),
    ).trim();
  }

  const cron = json<CronJob>('kubectl', kubectl('get', 'cronjob', CRONJOB, '-o', 'json'));
  const spec = cron.spec.jobTemplate.spec;
  for (const container of spec.template.spec.containers) {
    const env: { name: string; value?: string }[] = container.env ?? [];
    const existing = env.find((e) => e.name === 'K8S_VERSION');
    if (existing) existing.value = k8sVersion;
    else env.push({ name: 'K8S_VERSION', value: k8sVersion });
    container.env = env;
  }
  const job = {
    apiVersion: 'batch/v1',
    kind: 'Job',
    metadata: {
      name,
      namespace: NAMESPACE,
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
  return capture('kubectl', kubectl('create', '-f', '-', '-o', 'name'), {
    stdin: JSON.stringify(job),
  }).trim();
}

function follow(name: string, deadline: number): 'complete' | 'failed' | 'running' {
  while (Date.now() < deadline) {
    exec('kubectl', kubectl('logs', '-f', `job/${name}`, '--pod-running-timeout=5m'), {
      check: false,
    });
    const job = json<Job>('kubectl', kubectl('get', 'job', name, '-o', 'json'));
    const state = result(job);
    if (state !== 'running') return state;
  }
  return 'running';
}

function sync(values: Record<string, string | boolean | undefined>): number {
  const active = jobs().filter((j) => (j.status?.active ?? 0) > 0);
  if (active.length > 0 && !values.force) {
    console.error(
      `${active.map((j) => j.metadata.name).join(', ')} is already running; pass --force to start another`,
    );
    return 1;
  }

  const before = publishedVersion();
  const created = createJob(values['k8s-version'] as string | undefined);
  const name = created.replace(/^job\.batch\//, '');
  console.log(`started ${name}`);
  if (values.detach) return 0;

  const minutes = Number(values.timeout ?? '20');
  const state = follow(name, Date.now() + minutes * 60_000);
  if (state === 'failed') {
    console.error(`${name} failed; see kubectl -n ${NAMESPACE} describe job ${name}`);
    return 1;
  }
  if (state === 'running') {
    console.error(`${name} is still running after ${minutes}m; check with \`ortzi schemas status\``);
    return 1;
  }

  const after = publishedVersion();
  console.log(after === before ? `published version unchanged (${after})` : `published version ${before} -> ${after}`);
  return 0;
}

function status(): number {
  const cron = json<CronJob>('kubectl', kubectl('get', 'cronjob', CRONJOB, '-o', 'json'));
  console.log(`schedule         ${cron.spec.schedule}`);
  console.log(`last scheduled   ${cron.status?.lastScheduleTime ?? '-'}`);
  console.log(`last successful  ${cron.status?.lastSuccessfulTime ?? '-'}`);
  console.log(`published        ${publishedVersion()}`);

  const recent = jobs().slice(0, 6);
  if (recent.length > 0) {
    console.log('');
    for (const job of recent) {
      const manual = job.metadata.annotations?.['cronjob.kubernetes.io/instantiate'] === 'manual';
      console.log(
        [
          job.metadata.name.padEnd(36),
          (manual ? 'manual' : 'scheduled').padEnd(10),
          result(job).padEnd(9),
          job.status?.startTime ?? '-',
          job.status?.completionTime ? `-> ${job.status.completionTime}` : '',
        ].join(' '),
      );
    }
  }
  return 0;
}

function logs(job: string | undefined): number {
  const name = job ?? jobs()[0]?.metadata.name;
  if (!name) {
    console.error('no k8s-schemas jobs found');
    return 1;
  }
  return exec('kubectl', kubectl('logs', '-f', `job/${name}`), { check: false }).code ?? 1;
}

export const run = main(() => {
  const { values, positionals } = parseArgs(args(), {
    help: { type: 'boolean', short: 'h' },
    context: { type: 'string' },
    detach: { type: 'boolean' },
    force: { type: 'boolean' },
    timeout: { type: 'string' },
    'k8s-version': { type: 'string' },
    job: { type: 'string' },
  });
  context = values.context;

  const [command] = positionals;
  if (values.help || !command) {
    console.log(USAGE);
    return command || values.help ? 0 : 2;
  }

  if (!context) {
    context = capture('kubectl', ['config', 'current-context']).trim();
  }
  console.error(`context ${context}`);

  switch (command) {
    case 'sync':
      return sync(values);
    case 'status':
      return status();
    case 'logs':
      return logs(values.job);
    default:
      console.error(`unknown command ${command}\n\n${USAGE}`);
      return 2;
  }
});
