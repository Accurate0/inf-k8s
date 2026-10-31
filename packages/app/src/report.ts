import { ghcrDigest, short } from '@ortzi/kit';
import { type Application, isEmergency, isManualSync } from './argo';
import { syncedTags } from './git';
import { type Workload, runningTags, workloadsFor } from './workloads';

export interface Problem {
  severe: boolean;
  text: string;
}

export interface ImageState {
  image: string;
  running?: string;
  git: string;
  latest?: 'yes' | 'behind' | 'unknown';
}

export interface Report {
  app: Application;
  images: ImageState[];
  problems: Problem[];
}

export const displayImage = (image: string) => image.replace(/^ghcr\.io\/accurate0\//, '');

function latestState(image: string, running: string | undefined): ImageState['latest'] {
  const current = running ? ghcrDigest(image, running) : undefined;
  const newest = ghcrDigest(image, 'latest');
  if (!current || !newest) return 'unknown';
  return current === newest ? 'yes' : 'behind';
}

function imageProblems({ image, running, git, latest }: ImageState, emergency: boolean): Problem[] {
  const name = displayImage(image);
  const problems: Problem[] = [];
  if (!running) {
    problems.push({ severe: false, text: `${name} is not running` });
  } else if (running !== git) {
    const how = emergency ? 'rolled back by hand: ' : '';
    problems.push({ severe: true, text: `${name} ${how}running ${short(running)}, git says ${short(git)}` });
  }
  if (latest === 'behind') problems.push({ severe: false, text: `${name} is behind latest` });
  return problems;
}

function appProblems(app: Application): Problem[] {
  const problems: Problem[] = [];
  const automated = app.spec.syncPolicy?.automated;
  if (isEmergency(app)) {
    problems.push({
      severe: true,
      text: `auto-sync turned off; undo: ortzi app autosync ${app.metadata.name} on --yes`,
    });
  } else if (!automated && !isManualSync(app)) {
    problems.push({ severe: false, text: 'auto-sync is off' });
  } else if (automated && !automated.selfHeal) {
    problems.push({ severe: false, text: 'self-heal is off' });
  }

  const sync = app.status?.sync?.status ?? 'Unknown';
  if (sync !== 'Synced') problems.push({ severe: false, text: `sync is ${sync}` });
  const health = app.status?.health?.status ?? 'Unknown';
  if (health !== 'Healthy') {
    problems.push({ severe: health !== 'Progressing', text: `health is ${health}` });
  }
  return problems;
}

export function buildReport(app: Application, workloads: Workload[], latest: boolean): Report {
  const running = runningTags(workloadsFor(app, workloads));
  const images = [...syncedTags(app)].map(([image, git]): ImageState => {
    const now = running.get(image);
    return { image, running: now, git, latest: latest ? latestState(image, now) : undefined };
  });
  const emergency = isEmergency(app);
  return {
    app,
    images,
    problems: [...images.flatMap((i) => imageProblems(i, emergency)), ...appProblems(app)],
  };
}

export function symbol(problems: Problem[]): string {
  if (problems.length === 0) return '✓';
  return problems.some((p) => p.severe) ? '✗' : '!';
}
