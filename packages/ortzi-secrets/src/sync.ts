import { run } from '@ortzi/host';
import type { Kube } from '@ortzi/kit';
import { type ExternalSecret, fullName, isReady, matching, ready } from './externalsecrets';

export const RESOURCE = 'externalsecrets.external-secrets.io';

const TIMEOUT_MS = 60_000;

export function listExternalSecrets(kube: Kube, namespace?: string): ExternalSecret[] {
  return namespace
    ? kube.namespace(namespace).list<ExternalSecret>(RESOURCE)
    : kube.list<ExternalSecret>(RESOURCE, '-A');
}

export function sync(kube: Kube, target: string): boolean {
  const found = matching(listExternalSecrets(kube), target);
  if (!found.length) {
    console.error(`no ExternalSecrets read ${target}`);
    return true;
  }

  const started = Math.floor(Date.now() / 1000);
  for (const es of found) {
    kube
      .namespace(es.metadata.namespace)
      .capture(['annotate', RESOURCE, es.metadata.name, `force-sync=${started}`, '--overwrite']);
  }

  const deadline = Date.now() + TIMEOUT_MS;
  let pending = found;
  let ok = true;
  while (pending.length) {
    const still: ExternalSecret[] = [];
    for (const es of pending) {
      const current = kube
        .namespace(es.metadata.namespace)
        .get<ExternalSecret>(RESOURCE, es.metadata.name);
      const refreshTime = current.status?.refreshTime;
      const refreshed = !!refreshTime && Math.floor(Date.parse(refreshTime) / 1000) >= started;
      const timedOut = Date.now() >= deadline;

      if (refreshed && isReady(current)) {
        console.log(`synced ${fullName(current)}`);
      } else if (timedOut && !isReady(current)) {
        const condition = ready(current);
        console.log(
          `failed ${fullName(current)}: ${condition?.message ?? condition?.reason ?? 'not ready'}`,
        );
        ok = false;
      } else if (timedOut) {
        console.log(`timed out waiting for ${fullName(current)}`);
        ok = false;
      } else {
        still.push(current);
      }
    }

    pending = still;
    if (pending.length) run('sleep', ['1']);
  }

  return ok;
}
