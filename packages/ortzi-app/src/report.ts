import { ghcrDigest, short } from '@ortzi/kit';
import {
  type App,
  desiredTags,
  isManualSync,
  objectsOf,
  ready,
  readyMessage,
  suspended,
} from './flux';
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
  app: App;
  images: ImageState[];
  problems: Problem[];
}

export const displayImage = (image: string) => image.replace(/^ghcr\.io\/accurate0\//, '');

function latestState(image: string, running: string | undefined): ImageState['latest'] {
  const current = running ? ghcrDigest(image, running) : undefined;
  const newest = ghcrDigest(image, 'latest');
  if (!current || !newest) {
    return 'unknown';
  }
  return current === newest ? 'yes' : 'behind';
}

function imageProblems({ image, running, git, latest }: ImageState, emergency: boolean): Problem[] {
  const name = displayImage(image);
  const problems: Problem[] = [];

  if (!running) {
    problems.push({ severe: false, text: `${name} is not running` });
  } else if (running !== git) {
    const how = emergency ? 'rolled back by hand: ' : '';
    problems.push({
      severe: true,
      text: `${name} ${how}running ${short(running)}, git says ${short(git)}`,
    });
  }

  if (latest === 'behind') {
    problems.push({ severe: false, text: `${name} is behind latest` });
  }
  return problems;
}

function appProblems(app: App): Problem[] {
  const problems: Problem[] = [];
  if (isManualSync(app)) {
    return problems;
  }
  if (suspended(app)) {
    problems.push({
      severe: true,
      text: `auto-sync turned off; undo: ortzi app autosync ${app.name} on --yes`,
    });
  }

  for (const object of objectsOf(app).filter((o) => !ready(o))) {
    problems.push({
      severe: !suspended(app),
      text: `${object.metadata.name}: ${readyMessage(object)}`,
    });
  }
  return problems;
}

export function buildReport(app: App, workloads: Workload[], latest: boolean): Report {
  const running = runningTags(workloadsFor(app, workloads));
  const images = [...desiredTags(app)].map(([image, wanted]): ImageState => {
    // pinned tags read tag@digest; containers report the tag alone
    const git = wanted.split('@')[0];
    const now = running.get(image);
    return { image, running: now, git, latest: latest ? latestState(image, now) : undefined };
  });

  const emergency = suspended(app) && !isManualSync(app);
  return {
    app,
    images,
    problems: [...images.flatMap((i) => imageProblems(i, emergency)), ...appProblems(app)],
  };
}

export function symbol(problems: Problem[]): string {
  if (problems.length === 0) {
    return '✓';
  }
  return problems.some((p) => p.severe) ? '✗' : '!';
}
