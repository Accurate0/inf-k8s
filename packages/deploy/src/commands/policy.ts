import { applying, type Kube, short } from '@ortzi/kit';
import {
  type Application,
  assertChangeable,
  getApp,
  restoreAutosync,
  savedAutomated,
  setSelfHeal,
  turnAutosyncOff,
} from '../argo';
import { syncedTags } from '../git';
import { listWorkloads, runningTags, workloadsFor } from '../workloads';

export function heal(kube: Kube, name: string, state: 'on' | 'off', yes: boolean): number {
  assertChangeable(name);
  if (!getApp(kube, name).spec.syncPolicy?.automated) {
    console.error(`auto-sync is off for ${name}, so self-heal has no effect; see \`ortzi deploy autosync\``);
    return 1;
  }
  console.log(`${yes ? 'setting' : 'would set'} self-heal ${state} for ${name}`);
  if (state === 'off') {
    console.log('auto-sync still applies new git commits; use `deploy autosync off` to stop that too');
  }
  if (applying(yes)) setSelfHeal(kube, name, state === 'on');
  return 0;
}

function warnAboutDrift(kube: Kube, app: Application): void {
  const running = runningTags(workloadsFor(app, listWorkloads(kube)));
  for (const [image, tag] of syncedTags(app)) {
    const now = running.get(image);
    if (now && now !== tag) {
      console.log(`warning: this replaces ${image}:${short(now)} with git's ${short(tag)}`);
    }
  }
}

function autosyncOn(kube: Kube, app: Application, yes: boolean): number {
  const name = app.metadata.name;
  if (app.spec.syncPolicy?.automated) {
    console.log(`auto-sync is already on for ${name}`);
    return 0;
  }
  const { automated, saved } = savedAutomated(app);
  if (!saved) console.log(`no saved policy for ${name}; using ${JSON.stringify(automated)}`);
  warnAboutDrift(kube, app);
  console.log(`${yes ? 'restoring' : 'would restore'} auto-sync for ${name}`);
  if (applying(yes)) restoreAutosync(kube, name, automated);
  return 0;
}

export function autosync(kube: Kube, name: string, state: 'on' | 'off', yes: boolean): number {
  assertChangeable(name);
  const app = getApp(kube, name);
  if (state === 'on') return autosyncOn(kube, app, yes);
  console.log(`${yes ? 'turning' : 'would turn'} auto-sync off for ${name}`);
  if (applying(yes)) turnAutosyncOff(kube, app);
  return 0;
}
