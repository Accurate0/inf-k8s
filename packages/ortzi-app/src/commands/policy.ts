import { applying, type Kube, parseImage, short } from '@ortzi/kit';
import {
  type App,
  assertChangeable,
  desiredTags,
  driftCorrected,
  getApp,
  requestReconcile,
  setDriftCorrection,
  setSuspended,
  suspended,
  waitForReady,
} from '../flux';
import {
  type Workload,
  containersOf,
  listWorkloads,
  runningTags,
  waitForRollouts,
  workloadsFor,
} from '../workloads';

export interface WaitOptions {
  wait: boolean;
  timeoutMinutes: number;
}

export function heal(kube: Kube, name: string, state: 'on' | 'off', yes: boolean): number {
  assertChangeable(name);
  const app = getApp(kube, name);
  if (state === 'on' && driftCorrected(app) && !suspended(app)) {
    console.log(`self-heal is already on for ${name}`);
    return 0;
  }

  console.log(`${yes ? 'setting' : 'would set'} self-heal ${state} for ${name}`);
  if (state === 'off') {
    console.log('flux only reports drift now, and new git commits wait until self-heal is back on');
  }

  if (applying(yes)) {
    setDriftCorrection(kube, app, state === 'on');
  }
  return 0;
}

function warnAboutDrift(app: App, workloads: Workload[]): void {
  const running = runningTags(workloads);
  for (const [image, tag] of desiredTags(app)) {
    const now = running.get(image);
    if (now && now !== tag) {
      console.log(`warning: this replaces ${image}:${short(now)} with git's ${short(tag)}`);
    }
  }
}

function driftedWorkloads(app: App, workloads: Workload[]): Workload[] {
  const git = desiredTags(app);
  return workloads.filter((workload) =>
    containersOf(workload).some((container) => {
      const { repository, tag } = parseImage(container.image);
      const wanted = git.get(repository);
      return wanted !== undefined && wanted !== tag;
    }),
  );
}

function waitForGit(kube: Kube, app: App, drifted: Workload[], minutes: number): boolean {
  if (!waitForReady(kube, app, minutes) || !waitForRollouts(kube, drifted, minutes)) {
    return false;
  }
  const current = getApp(kube, app.name);
  return driftedWorkloads(current, workloadsFor(current, listWorkloads(kube))).length === 0;
}

function autosyncOn(kube: Kube, app: App, yes: boolean, options: WaitOptions): number {
  if (!suspended(app)) {
    console.log(`auto-sync is already on for ${app.name}`);
    return 0;
  }

  const workloads = workloadsFor(app, listWorkloads(kube));
  warnAboutDrift(app, workloads);

  console.log(`${yes ? 'resuming' : 'would resume'} ${app.name}`);
  if (!applying(yes)) {
    return 0;
  }

  setSuspended(kube, app, false);
  requestReconcile(kube, app);
  if (!options.wait) {
    return 0;
  }
  if (!waitForGit(kube, app, driftedWorkloads(app, workloads), options.timeoutMinutes)) {
    console.error(`${app.name} did not return to git; check \`ortzi app status ${app.name}\``);
    return 1;
  }

  console.log(`${app.name} is back on what git says`);
  return 0;
}

export function autosync(
  kube: Kube,
  name: string,
  state: 'on' | 'off',
  yes: boolean,
  options: WaitOptions,
): number {
  assertChangeable(name);
  const app = getApp(kube, name);

  if (state === 'on') {
    return autosyncOn(kube, app, yes, options);
  }
  console.log(`${yes ? 'suspending' : 'would suspend'} ${name}`);
  if (applying(yes)) {
    setSuspended(kube, app, true);
  }
  return 0;
}
