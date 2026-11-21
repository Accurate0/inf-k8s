import type { Kube } from '@ortzi/kit';
import { getApp, objectsOf, ready, readyMessage, suspended } from '../flux';
import {
  type App,
  appNamespace,
  appWorkloads,
  containerEnv,
  declared,
  type World,
} from '../render';
import { type Check, fail, type KubeObject, type Level, ok, unmet, warn } from '../checks';
import type { ResourceType } from '../providers/provider';
import { RESOURCES } from '../providers/index';
import { secretChecker } from './env';

const MARKS: Record<Level, string> = { ok: '✓', warn: '!', fail: '✗' };

const WAITING = new Set([
  'CrashLoopBackOff',
  'ImagePullBackOff',
  'ErrImagePull',
  'CreateContainerConfigError',
]);

function attempt(label: string, check: () => Check[]): Check[] {
  try {
    return check();
  } catch (error) {
    return [fail(`${label}: ${(error as Error).message}`)];
  }
}

function flux(kube: Kube, app: App): Check[] {
  const live = getApp(kube, app.name);
  const checks = objectsOf(live).map((object) =>
    ready(object)
      ? ok(`${object.metadata.name} ready`)
      : fail(`${object.metadata.name}: ${readyMessage(object)}`),
  );
  return suspended(live)
    ? [...checks, warn('suspended; resume with ortzi app autosync on')]
    : checks;
}

function workloads(app: App, ns: Kube): Check[] {
  const checks: Check[] = [];
  const pods = ns.list<KubeObject>('pods');

  for (const [name, spec] of Object.entries(appWorkloads(app))) {
    const kind = spec.kind ?? 'Deployment';
    if (kind === 'CronJob' || kind === 'Job') {
      continue;
    }

    const resource = kind === 'DaemonSet' ? 'daemonsets' : 'deployments';
    const workload = ns.get<KubeObject>(resource, name);
    const wanted =
      kind === 'DaemonSet' ? workload.status?.desiredNumberScheduled : workload.spec?.replicas;
    const ready = workload.status?.[kind === 'DaemonSet' ? 'numberReady' : 'readyReplicas'] ?? 0;
    checks.push(
      ready === wanted
        ? ok(`${name} ${ready}/${wanted} ready`)
        : fail(`${name} ${ready}/${wanted} ready`),
    );

    for (const pod of pods.filter((p) => p.metadata.labels?.app === name)) {
      for (const container of pod.status?.containerStatuses ?? []) {
        const waiting = container.state?.waiting?.reason;
        const last = container.lastState?.terminated?.reason;
        if (waiting && WAITING.has(waiting)) {
          checks.push(
            fail(
              `${pod.metadata.name}/${container.name} ${waiting}${last ? `, last exit ${last}` : ''}, ${container.restartCount} restarts`,
            ),
          );
        } else if (last === 'OOMKilled') {
          checks.push(warn(`${pod.metadata.name}/${container.name} was OOMKilled`));
        }
      }
    }

    if (spec.autoscale) {
      const hpa = ns.get<KubeObject>('horizontalpodautoscalers', name);
      const current = hpa.status?.currentReplicas ?? 0;
      const problems = unmet(hpa.status?.conditions, ['AbleToScale', 'ScalingActive']);
      const text = `${name} autoscaling at ${current} (${spec.autoscale.min}-${spec.autoscale.max})`;
      if (problems.length) {
        checks.push(fail(`${text}: ${problems.join('; ')}`));
      } else if (current >= spec.autoscale.max) {
        checks.push(warn(`${text}, at max`));
      } else {
        checks.push(ok(text));
      }
    }
  }

  return checks;
}

function externalSecrets(ns: Kube): Check[] {
  return ns.list<KubeObject>('externalsecrets').map((secret) => {
    const problems = unmet(secret.status?.conditions, ['Ready']);
    const text = `externalsecret ${secret.metadata.name}`;
    return problems.length ? fail(`${text}: ${problems.join('; ')}`) : ok(`${text} synced`);
  });
}

function env(kube: Kube, app: App, world: World): Check[] {
  const check = secretChecker(kube, appNamespace(app));
  const missing = containerEnv(app, world).flatMap(({ container, rows }) =>
    rows.flatMap((row) => {
      if (!('secret' in row)) {
        return [];
      }
      const result = check(row.secret);
      if (!result.startsWith('missing')) {
        return [];
      }
      const where = row.secret.key ? `${row.secret.name}/${row.secret.key}` : row.secret.name;
      return [fail(`${container} ${row.name}: ${result} ${where}`)];
    }),
  );
  return missing.length ? missing : [ok('every referenced secret key exists')];
}

export function doctor(kube: Kube, app: App, world: World): number {
  const ns = kube.namespace(appNamespace(app));
  const sections: [string, () => Check[]][] = [
    ['flux', () => flux(kube, app)],
    ['workloads', () => workloads(app, ns)],
    ['secrets', () => externalSecrets(ns)],
    ['env', () => env(kube, app, world)],
    ...RESOURCES.flatMap(({ type, doctor: check }): [string, () => Check[]][] =>
      check
        ? [
            [
              check.label,
              () => check.run({ kube, ns, app }, declared(app, type as ResourceType) as never[]),
            ],
          ]
        : [],
    ),
  ];

  const checks = sections.flatMap(([label, check]) => attempt(label, check));
  for (const { level, text } of checks) {
    console.log(`${MARKS[level]} ${text}`);
  }

  const failed = checks.filter((c) => c.level === 'fail').length;
  const warned = checks.filter((c) => c.level === 'warn').length;
  console.log(`\n${failed} failing, ${warned} warning, ${checks.length - failed - warned} ok`);
  return failed ? 1 : 0;
}
