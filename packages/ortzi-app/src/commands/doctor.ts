import type { Kube } from '@ortzi/kit';
import { getApp } from '../argo';
import {
  type App,
  appDatabases,
  appNamespace,
  appOidc,
  appRoutes,
  appWorkloads,
  containerEnv,
  databaseRole,
  declared,
  type World,
} from '../render';
import { secretChecker } from './env';

type Level = 'ok' | 'warn' | 'fail';

interface Check {
  level: Level;
  text: string;
}

interface Condition {
  type: string;
  status: string;
  reason?: string;
  message?: string;
}

interface ParentRef {
  name: string;
  namespace?: string;
  sectionName?: string;
}

const sameParent = (a: ParentRef, b: ParentRef) =>
  a.name === b.name && a.namespace === b.namespace && a.sectionName === b.sectionName;

interface KubeObject {
  metadata: { name: string; labels?: Record<string, string> };
  spec?: Record<string, any>;
  status?: Record<string, any>;
}

const MARKS: Record<Level, string> = { ok: '✓', warn: '!', fail: '✗' };

const ok = (text: string): Check => ({ level: 'ok', text });
const warn = (text: string): Check => ({ level: 'warn', text });
const fail = (text: string): Check => ({ level: 'fail', text });

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

const unmet = (conditions: Condition[] | undefined, wanted: string[]) =>
  wanted.flatMap((type) => {
    const found = conditions?.find((c) => c.type === type);
    if (found?.status === 'True') return [];
    return [`${type}${found?.message ? `: ${found.message}` : ' not reported'}`];
  });

function argo(kube: Kube, app: App): Check[] {
  const status = getApp(kube, app.name).status;
  const sync = status?.sync?.status ?? 'Unknown';
  const health = status?.health?.status ?? 'Unknown';
  const text = `argo ${sync}, ${health}`;
  if (health === 'Degraded' || health === 'Missing') return [fail(text)];
  return [sync === 'Synced' && health === 'Healthy' ? ok(text) : warn(text)];
}

function workloads(app: App, ns: Kube): Check[] {
  const checks: Check[] = [];
  const pods = ns.list<KubeObject>('pods');

  for (const [name, spec] of Object.entries(appWorkloads(app))) {
    const kind = spec.kind ?? 'Deployment';
    if (kind === 'CronJob' || kind === 'Job') continue;

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
      if (problems.length) checks.push(fail(`${text}: ${problems.join('; ')}`));
      else if (current >= spec.autoscale.max) checks.push(warn(`${text}, at max`));
      else checks.push(ok(text));
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

function volumes(app: App, ns: Kube): Check[] {
  return declared(app, 'volume').map(({ claimName }) => {
    const phase = ns.get<KubeObject>('persistentvolumeclaims', claimName).status?.phase;
    return phase === 'Bound'
      ? ok(`volume ${claimName} bound`)
      : fail(`volume ${claimName} ${phase}`);
  });
}

function routes(app: App, ns: Kube): Check[] {
  const checks = Object.keys(appRoutes(app)).map((name) => {
    const route = ns.get<KubeObject>('httproutes', name);
    const refs: ParentRef[] = route.spec?.parentRefs ?? [];
    const parents = (route.status?.parents ?? []).filter((parent: { parentRef: ParentRef }) =>
      refs.some((ref) => sameParent(ref, parent.parentRef)),
    );
    const problems = parents.length
      ? parents.flatMap((parent: { conditions?: Condition[] }) =>
          unmet(parent.conditions, ['Accepted', 'ResolvedRefs']),
        )
      : ['no gateway has picked it up'];
    return problems.length
      ? fail(`route ${name}: ${problems.join('; ')}`)
      : ok(`route ${name} accepted`);
  });

  for (const kind of ['backendtrafficpolicies', 'securitypolicies']) {
    for (const policy of ns.list<KubeObject>(kind)) {
      const problems = (policy.status?.ancestors ?? []).flatMap(
        (ancestor: { conditions?: Condition[] }) => unmet(ancestor.conditions, ['Accepted']),
      );
      const text = policy.metadata.name;
      checks.push(
        problems.length ? fail(`${text}: ${problems.join('; ')}`) : ok(`${text} accepted`),
      );
    }
  }

  return checks;
}

function oidc(app: App, ns: Kube): Check[] {
  if (!Object.keys(appOidc(app)).length) return [];
  return ns.list<KubeObject>('kanidmoauth2clients').map((client) => {
    const problems = unmet(client.status?.conditions, ['Accepted', 'Programmed']);
    const text = `oidc client ${client.metadata.name}`;
    return problems.length ? fail(`${text}: ${problems.join('; ')}`) : ok(`${text} provisioned`);
  });
}

function databases(kube: Kube, app: App): Check[] {
  const specs = Object.entries(appDatabases(app));
  if (!specs.length) return [];

  const cnpg = kube.namespace('cnpg-system');
  const reconciled: string[] =
    cnpg.get<KubeObject>('clusters.postgresql.cnpg.io', 'cloudnative-pg-cluster').status
      ?.managedRolesStatus?.byStatus?.reconciled ?? [];

  return specs.flatMap(([database, spec]) => {
    const role = databaseRole(database, spec);
    const applied = cnpg.get<KubeObject>('databases.postgresql.cnpg.io', database).status;
    return [
      applied?.applied
        ? ok(`database ${database} applied`)
        : fail(`database ${database} not applied${applied?.message ? `: ${applied.message}` : ''}`),
      reconciled.includes(role)
        ? ok(`role ${role} reconciled`)
        : fail(`role ${role} not reconciled`),
    ];
  });
}

function env(kube: Kube, app: App, world: World): Check[] {
  const check = secretChecker(kube, appNamespace(app));
  const missing = containerEnv(app, world).flatMap(({ container, rows }) =>
    rows.flatMap((row) => {
      if (!('secret' in row)) return [];
      const result = check(row.secret);
      if (!result.startsWith('missing')) return [];
      const where = row.secret.key ? `${row.secret.name}/${row.secret.key}` : row.secret.name;
      return [fail(`${container} ${row.name}: ${result} ${where}`)];
    }),
  );
  return missing.length ? missing : [ok('every referenced secret key exists')];
}

export function doctor(kube: Kube, app: App, world: World): number {
  const ns = kube.namespace(appNamespace(app));
  const sections: [string, () => Check[]][] = [
    ['argo', () => argo(kube, app)],
    ['workloads', () => workloads(app, ns)],
    ['secrets', () => externalSecrets(ns)],
    ['env', () => env(kube, app, world)],
    ['volumes', () => volumes(app, ns)],
    ['routes', () => routes(app, ns)],
    ['oidc', () => oidc(app, ns)],
    ['databases', () => databases(kube, app)],
  ];

  const checks = sections.flatMap(([label, check]) => attempt(label, check));
  for (const { level, text } of checks) console.log(`${MARKS[level]} ${text}`);

  const failed = checks.filter((c) => c.level === 'fail').length;
  const warned = checks.filter((c) => c.level === 'warn').length;
  console.log(`\n${failed} failing, ${warned} warning, ${checks.length - failed - warned} ok`);
  return failed ? 1 : 0;
}
