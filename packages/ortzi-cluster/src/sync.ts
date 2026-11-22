import type { Kube } from '@ortzi/kit';
import {
  FLUX_NAMESPACE,
  type FluxObject,
  REQUESTED_AT,
  ROOT,
  SOURCES,
  changes,
  commitOf,
  id,
  matches,
  ready,
  readyMessage,
  refetchable,
  revisionOf,
  settled,
  shortRevision,
} from './flux';
import { waitAll } from './watch';

export interface SyncOptions {
  filters: string[];
  detach: boolean;
  sourcesOnly: boolean;
  timeoutMinutes: number;
}

function sources(kube: Kube): Map<string, FluxObject[]> {
  const found = new Map<string, FluxObject[]>();
  for (const resource of SOURCES) {
    try {
      kube.resource(resource);
    } catch {
      continue;
    }
    found.set(resource, kube.list<FluxObject>(resource));
  }
  return found;
}

function request(kube: Kube, resource: string, object: FluxObject, token: string): void {
  kube.namespace(object.metadata.namespace!).patch(resource, object.metadata.name, {
    metadata: { annotations: { [REQUESTED_AT]: token } },
  });
}

const key = (object: FluxObject) => `${object.metadata.namespace}/${object.metadata.name}`;

export function sync(kube: Kube, options: SyncOptions): number {
  const token = new Date().toISOString();
  const targets = new Map<string, FluxObject[]>();

  for (const [resource, objects] of sources(kube)) {
    const chosen = objects.filter(refetchable).filter((o) => matches(o, options.filters));
    if (chosen.length > 0) {
      targets.set(resource, chosen);
    }
  }

  const all = [...targets.values()].flat();
  if (all.length === 0) {
    console.error(
      options.filters.length > 0
        ? `no flux sources match ${options.filters.join(', ')}`
        : 'no flux sources to refetch',
    );
    return 1;
  }

  const before = new Map(all.map((object) => [id(object), revisionOf(object)]));
  for (const [resource, objects] of targets) {
    for (const object of objects) {
      request(kube, resource, object, token);
    }
  }
  console.log(`requested a refetch of ${all.length} sources`);

  const syncRoot = !options.sourcesOnly && options.filters.length === 0;
  if (options.detach) {
    if (syncRoot) {
      kube.namespace(FLUX_NAMESPACE).patch('kustomization', ROOT, {
        metadata: { annotations: { [REQUESTED_AT]: token } },
      });
    }
    return 0;
  }

  const deadline = Date.now() + options.timeoutMinutes * 60_000;
  const after: FluxObject[] = [];
  for (const [resource, objects] of targets) {
    const seen = waitAll(
      kube,
      resource,
      new Set(objects.map(key)),
      (o) => settled(o, token),
      deadline,
    );
    after.push(...seen.values());
  }

  const changed = changes(before, after);
  const failed = after.filter((object) => settled(object, token) && !ready(object));
  const waiting = after.filter((object) => !settled(object, token));

  for (const change of changed) {
    console.log(`  ${change.id}  ${shortRevision(change.from)} -> ${shortRevision(change.to)}`);
  }
  console.log(
    `${after.length - failed.length - waiting.length} fetched, ${changed.length} changed, ${failed.length} failed, ${waiting.length} still pending`,
  );

  for (const object of failed) {
    console.error(`✗ ${id(object)}: ${readyMessage(object)}`);
  }
  for (const object of waiting) {
    console.error(`… ${id(object)} did not finish within ${options.timeoutMinutes}m`);
  }

  let ok = failed.length === 0 && waiting.length === 0;
  if (syncRoot) {
    ok = syncRootKustomization(kube, token, deadline) && ok;
  }
  return ok ? 0 : 1;
}

function syncRootKustomization(kube: Kube, token: string, deadline: number): boolean {
  const flux = kube.namespace(FLUX_NAMESPACE);
  flux.patch('kustomization', ROOT, { metadata: { annotations: { [REQUESTED_AT]: token } } });

  const minutes = Math.max((deadline - Date.now()) / 60_000, 1);
  flux.waitFor<FluxObject>('kustomization', ROOT, (o) => settled(o, token), minutes);

  const root = flux.get<FluxObject>('kustomization', ROOT);
  const source = flux.get<FluxObject>('ocirepository', ROOT);
  const commit =
    root.status?.lastAppliedRevision === revisionOf(source) ? commitOf(source) : undefined;
  const applied = shortRevision(root.status?.lastAppliedRevision);

  if (!settled(root, token)) {
    console.error(`… kustomization/${ROOT} did not finish; last applied ${applied}`);
    return false;
  }
  if (!ready(root)) {
    console.error(`✗ kustomization/${ROOT}: ${readyMessage(root)}`);
    return false;
  }

  console.log(`✓ applied ${applied}${commit ? ` (commit ${commit.slice(0, 8)})` : ''}`);
  return true;
}
