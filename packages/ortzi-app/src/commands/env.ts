import { columns, type Kube } from '@ortzi/kit';
import { type App, appNamespace, containerEnv, type World } from '../render';
import type { SecretKey } from '../workload';

interface Secret {
  data?: Record<string, string>;
}

export type SecretCheck = 'ok' | 'missing secret' | 'missing key' | 'optional';

export function secretChecker(kube: Kube, namespace: string): (key: SecretKey) => SecretCheck {
  const scoped = kube.namespace(namespace);
  const cache = new Map<string, Secret | undefined>();
  const fetch = (name: string) => {
    if (!cache.has(name)) {
      try {
        cache.set(name, scoped.get<Secret>('secrets', name));
      } catch {
        cache.set(name, undefined);
      }
    }
    return cache.get(name);
  };

  return ({ name, key, optional }) => {
    const secret = fetch(name);
    const found = secret && (key === undefined || secret.data?.[key] !== undefined);
    if (found) return 'ok';
    if (optional) return 'optional';
    return secret ? 'missing key' : 'missing secret';
  };
}

const MARKS: Record<SecretCheck, string> = {
  ok: '✓',
  optional: '- optional, not set',
  'missing key': '✗ key missing',
  'missing secret': '✗ secret missing',
};

export function env(kube: Kube, app: App, world: World, only?: string): number {
  const containers = containerEnv(app, world).filter(
    ({ container }) => !only || container === only || container.startsWith(`${only}/`),
  );
  if (only && !containers.length) throw new Error(`${app.name}: no workload named ${only}`);

  const check = secretChecker(kube, appNamespace(app));
  let missing = 0;

  for (const { container, rows } of containers) {
    if (!rows.length) continue;

    const table = rows.map((row) => {
      if (!('secret' in row)) return [row.name, row.from, row.value];

      const result = check(row.secret);
      if (result.startsWith('missing')) missing++;
      const where = row.secret.key ? `${row.secret.name}/${row.secret.key}` : row.secret.name;
      return [row.name, `${row.from} → ${where}`, MARKS[result]];
    });

    console.log(`${container}\n${columns(['NAME', 'FROM', 'VALUE'], table)}\n`);
  }

  if (missing) console.error(`${missing} required secret reference(s) missing`);
  return missing ? 1 : 0;
}
