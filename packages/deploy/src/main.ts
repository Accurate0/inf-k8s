import { args, capture, json, main, parseArgs, run as exec } from '@ortzi/host';
import { age, columns, parseDigest, parseImage, parseKustomizationImages, previousTags, short } from './images';

const ARGOCD = 'argocd';
const REPO = 'Accurate0/inf-k8s';
const SAVED_AUTOMATED = 'ortzi.cloud/saved-automated';
const DEFAULT_AUTOMATED = { prune: true, selfHeal: true };
const WORKLOADS = 'deployments,statefulsets,daemonsets,cronjobs';

const USAGE = `usage: ortzi deploy <command> [flags]

commands:
  status                   what needs attention (--all lists every app)
  status <app>             one app's state and images
  history <app>            previous argo syncs and their image tags
  rollback <app> [tag]     edit images on the cluster, turning auto-sync off first
                           (default tag: the most recent different one in history)
  heal <app> on|off        set self-heal; auto-sync still applies new git commits
  autosync <app> on|off    turn argo auto-sync off, or restore it (returns the app to git)

flags:
  --context CTX    kube context (default: current)
  --all            status: list every app
  --latest         status: also compare against ghcr's latest tag
  --image NAME     rollback: only this image (repeat by running again)
  --yes            apply; writes are a dry run without this`;

interface Automated {
  prune?: boolean;
  selfHeal?: boolean;
}

interface Source {
  repoURL?: string;
  path?: string;
}

interface HistoryEntry {
  deployedAt?: string;
  revision?: string;
  revisions?: string[];
  id?: number;
}

interface Application {
  metadata: { name: string; annotations?: Record<string, string> };
  spec: { source?: Source; sources?: Source[]; syncPolicy?: { automated?: Automated } };
  status?: {
    sync?: { status?: string; revision?: string; revisions?: string[] };
    health?: { status?: string };
    resources?: { kind: string; name: string; namespace?: string }[];
    history?: HistoryEntry[];
  };
}

interface Container {
  name: string;
  image: string;
}

interface PodSpec {
  containers?: Container[];
  initContainers?: Container[];
}

interface Workload {
  kind: string;
  metadata: { name: string; namespace: string };
  spec: {
    template?: { spec?: PodSpec };
    jobTemplate?: { spec?: { template?: { spec?: PodSpec } } };
  };
}

let context: string | undefined;

function kubectl(...rest: string[]): string[] {
  return [...(context ? ['--context', context] : []), ...rest];
}

function getApp(name: string): Application {
  return json<Application>('kubectl', kubectl('-n', ARGOCD, 'get', 'application', name, '-o', 'json'));
}

function listApps(): Application[] {
  return json<{ items: Application[] }>('kubectl', kubectl('-n', ARGOCD, 'get', 'applications', '-o', 'json')).items;
}

function listWorkloads(): Workload[] {
  return json<{ items: Workload[] }>('kubectl', kubectl('get', WORKLOADS, '-A', '-o', 'json')).items;
}

function workloadsFor(app: Application, workloads: Workload[]): Workload[] {
  const owned = new Set(
    (app.status?.resources ?? []).map((r) => `${r.kind}/${r.namespace}/${r.name}`),
  );
  return workloads.filter((w) => owned.has(`${w.kind}/${w.metadata.namespace}/${w.metadata.name}`));
}

function containersOf(workload: Workload): Container[] {
  const pod = workload.spec.template?.spec ?? workload.spec.jobTemplate?.spec?.template?.spec;
  return [...(pod?.containers ?? []), ...(pod?.initContainers ?? [])];
}

function runningTags(workloads: Workload[]): Map<string, string> {
  const running = new Map<string, string>();
  for (const workload of workloads) {
    for (const container of containersOf(workload)) {
      const { repository, tag } = parseImage(container.image);
      running.set(repository, tag);
    }
  }
  return running;
}

function gitSourceIndex(app: Application): number {
  return (app.spec.sources ?? []).findIndex((s) => s.path && s.repoURL?.includes(REPO));
}

function gitPath(app: Application): string | undefined {
  const index = gitSourceIndex(app);
  if (index >= 0) return app.spec.sources?.[index].path;
  return app.spec.source?.repoURL?.includes(REPO) ? app.spec.source.path : undefined;
}

function tagsAt(app: Application, revision: string | undefined): Map<string, string> {
  const path = gitPath(app);
  if (!path || !revision) return new Map();
  for (const dir of [path, `${path}/manifests`]) {
    const url = `https://raw.githubusercontent.com/${REPO}/${revision}/${dir}/kustomization.yaml`;
    const out = exec('curl', ['-fsS', '-m', '15', url], { stdout: 'piped', stderr: 'piped', check: false });
    const images = out.ok ? parseKustomizationImages(out.stdout) : new Map<string, string>();
    if (images.size > 0) return images;
  }
  return new Map();
}

function gitRevision(
  app: Application,
  entry: { revision?: string; revisions?: string[] } | undefined,
): string | undefined {
  const index = gitSourceIndex(app);
  return (index >= 0 ? entry?.revisions?.[index] : undefined) ?? entry?.revision;
}

function ghcrDigest(repository: string, tag: string): string | undefined {
  const path = repository.replace(/^ghcr\.io\//, '');
  if (path === repository) return undefined;
  const token = exec('curl', ['-fsS', '-m', '15', `https://ghcr.io/token?scope=repository:${path}:pull`], {
    stdout: 'piped',
    stderr: 'piped',
    check: false,
  });
  if (!token.ok) return undefined;
  const headers = exec(
    'curl',
    [
      '-fsSI',
      '-m',
      '15',
      '-H',
      `Authorization: Bearer ${JSON.parse(token.stdout).token}`,
      '-H',
      'Accept: application/vnd.oci.image.index.v1+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.docker.distribution.manifest.v2+json',
      `https://ghcr.io/v2/${path}/manifests/${tag}`,
    ],
    { stdout: 'piped', stderr: 'piped', check: false },
  );
  return headers.ok ? parseDigest(headers.stdout) : undefined;
}

function isEmergency(app: Application): boolean {
  return app.metadata.annotations?.[SAVED_AUTOMATED] !== undefined;
}

interface Problem {
  severe: boolean;
  text: string;
}

interface Report {
  app: Application;
  images: { image: string; running?: string; git: string; latest?: string }[];
  problems: Problem[];
}

function buildReport(app: Application, workloads: Workload[], latest: boolean): Report {
  const running = runningTags(workloadsFor(app, workloads));
  const images = [...tagsAt(app, gitRevision(app, app.status?.sync))].map(([image, git]) => {
    const now = running.get(image);
    let state: string | undefined;
    if (latest) {
      const current = now ? ghcrDigest(image, now) : undefined;
      const newest = ghcrDigest(image, 'latest');
      state = !current || !newest ? '-' : current === newest ? 'yes' : 'behind';
    }
    return { image, running: now, git, latest: state };
  });

  const problems: Problem[] = [];
  const emergency = isEmergency(app);
  const automated = app.spec.syncPolicy?.automated;
  const health = app.status?.health?.status ?? 'Unknown';
  for (const { image, running: now, git, latest: state } of images) {
    const name = displayImage(image);
    if (!now) problems.push({ severe: false, text: `${name} is not running` });
    else if (now !== git) {
      const how = emergency ? 'rolled back by hand: ' : '';
      problems.push({ severe: true, text: `${name} ${how}running ${short(now)}, git says ${short(git)}` });
    }
    if (state === 'behind') problems.push({ severe: false, text: `${name} is behind latest` });
  }
  if (emergency) {
    problems.push({ severe: true, text: `auto-sync turned off; undo: ortzi deploy autosync ${app.metadata.name} on` });
  } else if (!automated) {
    problems.push({ severe: false, text: 'auto-sync is off' });
  } else if (!automated.selfHeal) {
    problems.push({ severe: false, text: 'self-heal is off' });
  }
  if ((app.status?.sync?.status ?? 'Unknown') !== 'Synced') {
    problems.push({ severe: false, text: `sync is ${app.status?.sync?.status ?? 'Unknown'}` });
  }
  if (health !== 'Healthy') {
    problems.push({ severe: health !== 'Progressing', text: `health is ${health}` });
  }
  return { app, images, problems };
}

function displayImage(image: string): string {
  return image.replace(/^ghcr\.io\/accurate0\//, '');
}

function symbol(report: Report): string {
  if (report.problems.length === 0) return '\u2713';
  return report.problems.some((p) => p.severe) ? '\u2717' : '!';
}

function lastDeployed(app: Application): string {
  const at = app.status?.history?.at(-1)?.deployedAt;
  return at ? age(at, Date.now()) : '-';
}

function summary(reports: Report[]): number {
  const bad = reports.filter((r) => r.problems.length > 0);
  if (bad.length === 0) {
    console.log(`\u2713 All ${reports.length} apps are running what git says.`);
    return 0;
  }
  console.log(`${symbol({ ...bad[0], problems: bad.flatMap((r) => r.problems) })} ${bad.length} of ${reports.length} apps need attention\n`);
  for (const report of bad) {
    console.log(`${symbol(report)} ${report.app.metadata.name}`);
    for (const problem of report.problems) console.log(`    ${problem.text}`);
    console.log('');
  }
  console.log(`\u2713 ${reports.length - bad.length} apps ok`);
  return 0;
}

function listAll(reports: Report[], latest: boolean): number {
  const rows = reports.map(({ app, images }) => [
    app.metadata.name,
    images.length > 0 ? `${images.filter((i) => i.running === i.git).length}/${images.length}` : '-',
    app.status?.sync?.status ?? '-',
    app.status?.health?.status ?? '-',
    app.spec.syncPolicy?.automated ? 'on' : 'OFF',
    ...(latest ? [`${images.filter((i) => i.latest === 'behind').length} behind`] : []),
    lastDeployed(app),
  ]);
  const header = ['NAME', 'READY', 'SYNC', 'HEALTH', 'AUTOSYNC', ...(latest ? ['LATEST'] : []), 'AGE'];
  console.log(columns(header, rows));
  return 0;
}

function detail(report: Report): number {
  const { app, images, problems } = report;
  const automated = app.spec.syncPolicy?.automated;
  console.log(`${symbol(report)} ${app.metadata.name}`);
  console.log(`  sync       ${app.status?.sync?.status ?? '-'}`);
  console.log(`  health     ${app.status?.health?.status ?? '-'}`);
  console.log(`  auto-sync  ${automated ? 'on' : 'off'}`);
  console.log(`  self-heal  ${automated?.selfHeal ? 'on' : 'off'}`);
  console.log(`  deployed   ${lastDeployed(app)} ago`);
  if (problems.length > 0) {
    console.log('');
    for (const problem of problems) console.log(`  ${problem.text}`);
  }
  if (images.length > 0) {
    console.log('');
    console.log(
      columns(
        ['IMAGE', 'RUNNING', 'GIT', 'LATEST'],
        images.map((i) => [displayImage(i.image), short(i.running ?? '-'), short(i.git), i.latest ?? '-']),
      ),
    );
  }
  return 0;
}

function status(name: string | undefined, all: boolean, latest: boolean): number {
  const workloads = listWorkloads();
  if (name) return detail(buildReport(getApp(name), workloads, true));
  const reports = listApps().map((app) => buildReport(app, workloads, latest));
  return all ? listAll(reports, latest) : summary(reports);
}

function historyEntries(app: Application): HistoryEntry[] {
  return [...(app.status?.history ?? [])].reverse();
}

function history(name: string): number {
  const app = getApp(name);
  const entries = historyEntries(app);
  if (entries.length === 0) {
    console.log(`${name} has no sync history`);
    return 0;
  }
  const rows = entries.map((entry) => {
    const revision = gitRevision(app, entry);
    const tags = [...tagsAt(app, revision)];
    const unique = new Set(tags.map(([, tag]) => tag));
    const images =
      unique.size === 1
        ? short(tags[0][1])
        : tags.map(([image, tag]) => `${image.split('/').pop()}=${short(tag)}`).join(' ') || '-';
    return [(entry.deployedAt ?? '-').replace('T', ' ').replace(/:\d\dZ$/, ''), short(revision ?? '-'), images];
  });
  console.log(columns(['DEPLOYED (UTC)', 'REVISION', 'IMAGES'], rows));
  return 0;
}

function assertAllowed(name: string): void {
  if (name === ARGOCD || name.endsWith('-apps')) {
    throw new Error(`refusing to change ${name}: it manages other applications`);
  }
}

function patchApp(name: string, patch: unknown): void {
  capture(
    'kubectl',
    kubectl('-n', ARGOCD, 'patch', 'application', name, '--type', 'merge', '-p', JSON.stringify(patch)),
  );
}

function turnAutosyncOff(app: Application): void {
  const automated = app.spec.syncPolicy?.automated;
  if (!automated) return;
  const annotations = isEmergency(app) ? {} : { [SAVED_AUTOMATED]: JSON.stringify(automated) };
  patchApp(app.metadata.name, {
    metadata: { annotations },
    spec: { syncPolicy: { automated: null } },
  });
}

function rollback(
  name: string,
  tag: string | undefined,
  only: string | undefined,
  yes: boolean,
): number {
  assertAllowed(name);
  const app = getApp(name);
  const workloads = workloadsFor(app, listWorkloads());
  const running = runningTags(workloads);

  let targets: Map<string, string>;
  if (tag) {
    targets = new Map(
      [...tagsAt(app, gitRevision(app, app.status?.sync)).keys()].map((image) => [image, tag]),
    );
  } else {
    const previous = previousTags(
      running,
      historyEntries(app).map((entry) => tagsAt(app, gitRevision(app, entry))),
    );
    if (!previous) {
      console.error(`no different tags in ${name}'s history; pass a tag`);
      return 1;
    }
    targets = previous;
  }
  if (only) {
    const match = [...targets.keys()].filter((image) => image === only || image.endsWith(`/${only}`));
    if (match.length === 0) {
      console.error(`${name} has no image ${only}; it has ${[...targets.keys()].join(', ')}`);
      return 1;
    }
    targets = new Map(match.map((image) => [image, targets.get(image) as string]));
  }

  const missing = [...targets].filter(
    ([image, target]) => image.startsWith('ghcr.io/') && !ghcrDigest(image, target),
  );
  if (missing.length > 0) {
    for (const [image, target] of missing) console.error(`${image}:${target} does not exist in ghcr`);
    console.error('nothing changed; use --image to limit the rollback');
    return 1;
  }

  const edits: { workload: Workload; container: string; image: string; from: string }[] = [];
  for (const workload of workloads) {
    for (const container of containersOf(workload)) {
      const { repository, tag: current } = parseImage(container.image);
      const target = targets.get(repository);
      if (target && target !== current) {
        edits.push({ workload, container: container.name, image: `${repository}:${target}`, from: current });
      }
    }
  }
  if (edits.length === 0) {
    console.log(`${name} is already running those tags`);
    return 0;
  }

  console.log(app.spec.syncPolicy?.automated ? `would turn auto-sync off for ${name}` : `auto-sync is already off for ${name}`);
  for (const { workload, container, image, from } of edits) {
    console.log(
      `${workload.kind.toLowerCase()}/${workload.metadata.name} ${container}: ${short(from)} -> ${image}`,
    );
  }
  if (!yes) {
    console.log('dry run; pass --yes to apply');
    return 0;
  }

  turnAutosyncOff(app);
  for (const { workload, container, image } of edits) {
    capture(
      'kubectl',
      kubectl(
        '-n',
        workload.metadata.namespace,
        'set',
        'image',
        `${workload.kind.toLowerCase()}/${workload.metadata.name}`,
        `${container}=${image}`,
      ),
    );
  }
  console.log(`rolled back ${name}; auto-sync stays off until \`ortzi deploy autosync ${name} on\`, which returns it to git`);
  return 0;
}

function heal(name: string, state: string, yes: boolean): number {
  assertAllowed(name);
  const app = getApp(name);
  if (!app.spec.syncPolicy?.automated) {
    console.error(`auto-sync is off for ${name}, so self-heal has no effect; see \`ortzi deploy autosync\``);
    return 1;
  }
  const enabled = state === 'on';
  console.log(`${yes ? 'setting' : 'would set'} self-heal ${state} for ${name}`);
  if (!enabled) console.log('auto-sync still applies new git commits; use `deploy autosync off` to stop that too');
  if (!yes) {
    console.log('dry run; pass --yes to apply');
    return 0;
  }
  patchApp(name, { spec: { syncPolicy: { automated: { selfHeal: enabled } } } });
  return 0;
}

function autosync(name: string, state: string, yes: boolean): number {
  assertAllowed(name);
  const app = getApp(name);
  if (state === 'off') {
    console.log(`${yes ? 'turning' : 'would turn'} auto-sync off for ${name}`);
    if (!yes) {
      console.log('dry run; pass --yes to apply');
      return 0;
    }
    turnAutosyncOff(app);
    return 0;
  }

  if (app.spec.syncPolicy?.automated) {
    console.log(`auto-sync is already on for ${name}`);
    return 0;
  }
  const saved = app.metadata.annotations?.[SAVED_AUTOMATED];
  const automated: Automated = saved ? JSON.parse(saved) : DEFAULT_AUTOMATED;
  if (!saved) console.log(`no saved policy for ${name}; using ${JSON.stringify(automated)}`);

  const git = tagsAt(app, gitRevision(app, app.status?.sync));
  const running = runningTags(workloadsFor(app, listWorkloads()));
  for (const [image, tag] of git) {
    const now = running.get(image);
    if (now && now !== tag) {
      console.log(`warning: this replaces ${image}:${short(now)} with git's ${short(tag)}`);
    }
  }
  console.log(`${yes ? 'restoring' : 'would restore'} auto-sync for ${name}`);
  if (!yes) {
    console.log('dry run; pass --yes to apply');
    return 0;
  }
  patchApp(name, {
    metadata: { annotations: { [SAVED_AUTOMATED]: null } },
    spec: { syncPolicy: { automated } },
  });
  return 0;
}

function onOff(command: string, name: string | undefined, state: string | undefined): name is string {
  if (name && (state === 'on' || state === 'off')) return true;
  console.error(`usage: ortzi deploy ${command} <app> on|off`);
  return false;
}

export const run = main(() => {
  const { values, positionals } = parseArgs(args(), {
    help: { type: 'boolean', short: 'h' },
    context: { type: 'string' },
    all: { type: 'boolean' },
    latest: { type: 'boolean' },
    image: { type: 'string' },
    yes: { type: 'boolean' },
  });
  context = values.context;

  const [command, name, extra] = positionals;
  if (values.help || !command) {
    console.log(USAGE);
    return command || values.help ? 0 : 2;
  }

  if (!context) {
    context = capture('kubectl', ['config', 'current-context']).trim();
  }
  console.error(`context ${context}`);

  switch (command) {
    case 'status':
      return status(name, values.all, values.latest);
    case 'history':
      if (!name) break;
      return history(name);
    case 'rollback':
      if (!name) break;
      return rollback(name, extra, values.image, values.yes);
    case 'heal':
      return onOff(command, name, extra) ? heal(name, extra, values.yes) : 2;
    case 'autosync':
      return onOff(command, name, extra) ? autosync(name, extra, values.yes) : 2;
    default:
      console.error(`unknown command ${command}\n\n${USAGE}`);
      return 2;
  }
  console.error(USAGE);
  return 2;
});
