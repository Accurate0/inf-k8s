import { applying, type Kube, parseImage, short } from '@ortzi/kit';
import {
  type Application,
  assertChangeable,
  getApp,
  restoreAutosync,
  savedAutomated,
  setSelfHeal,
  turnAutosyncOff,
  waitForSync,
} from '../argo';
import { syncedTags } from '../git';
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
  if (!getApp(kube, name).spec.syncPolicy?.automated) {
    console.error(`auto-sync is off for ${name}, so self-heal has no effect; see \`ortzi app autosync\``);
    return 1;
  }
  console.log(`${yes ? 'setting' : 'would set'} self-heal ${state} for ${name}`);
  if (state === 'off') {
    console.log('auto-sync still applies new git commits; use `deploy autosync off` to stop that too');
  }
  if (applying(yes)) setSelfHeal(kube, name, state === 'on');
  return 0;
}

function warnAboutDrift(app: Application, workloads: Workload[]): void {
  const running = runningTags(workloads);
  for (const [image, tag] of syncedTags(app)) {
    const now = running.get(image);
    if (now && now !== tag) {
      console.log(`warning: this replaces ${image}:${short(now)} with git's ${short(tag)}`);
    }
  }
}

function driftedWorkloads(app: Application, workloads: Workload[]): Workload[] {
  const git = syncedTags(app);
  return workloads.filter((workload) =>
    containersOf(workload).some((container) => {
      const { repository, tag } = parseImage(container.image);
      const wanted = git.get(repository);
      return wanted !== undefined && wanted !== tag;
    }),
  );
}

function waitForGit(
  kube: Kube,
  app: Application,
  drifted: Workload[],
  minutes: number,
): boolean {
  const name = app.metadata.name;
  if (!waitForSync(kube, name, minutes) || !waitForRollouts(kube, drifted, minutes)) return false;
  const current = getApp(kube, name);
  return driftedWorkloads(current, workloadsFor(current, listWorkloads(kube))).length === 0;
}

function autosyncOn(kube: Kube, app: Application, yes: boolean, options: WaitOptions): number {
  const name = app.metadata.name;
  if (app.spec.syncPolicy?.automated) {
    console.log(`auto-sync is already on for ${name}`);
    return 0;
  }
  const { automated, saved } = savedAutomated(app);
  if (!saved) console.log(`no saved policy for ${name}; using ${JSON.stringify(automated)}`);
  const workloads = workloadsFor(app, listWorkloads(kube));
  warnAboutDrift(app, workloads);
  console.log(`${yes ? 'restoring' : 'would restore'} auto-sync for ${name}`);
  if (!applying(yes)) return 0;

  restoreAutosync(kube, name, automated);
  if (!options.wait) return 0;
  if (!waitForGit(kube, app, driftedWorkloads(app, workloads), options.timeoutMinutes)) {
    console.error(`${name} did not return to git; check \`ortzi app status ${name}\``);
    return 1;
  }
  console.log(`${name} is back on what git says`);
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
  if (state === 'on') return autosyncOn(kube, app, yes, options);
  console.log(`${yes ? 'turning' : 'would turn'} auto-sync off for ${name}`);
  if (applying(yes)) turnAutosyncOff(kube, app);
  return 0;
}
