import { kubeSend } from '@ortzi/host';
import type { Kube } from '@ortzi/kit';
import type { FluxObject } from './flux';

const WATCH_SECONDS = 30;

interface List {
  metadata: { resourceVersion?: string };
  items: FluxObject[];
}

export function resourcePath(kube: Kube, resource: string): string {
  const { group, version, plural } = kube.resource(resource);
  return `${group ? `/apis/${group}/${version}` : `/api/${version}`}/${plural}`;
}

// watches a whole resource type across namespaces until done holds for every tracked object
export function waitAll(
  kube: Kube,
  resource: string,
  ids: Set<string>,
  done: (object: FluxObject) => boolean,
  deadline: number,
): Map<string, FluxObject> {
  const path = resourcePath(kube, resource);
  const { kind } = kube.resource(resource);
  const key = (object: FluxObject) => `${object.metadata.namespace}/${object.metadata.name}`;
  const latest = new Map<string, FluxObject>();
  const pending = () => [...ids].filter((id) => !latest.has(id) || !done(latest.get(id)!));

  const relist = () => {
    const list = kube.request<List>('GET', path);
    for (const item of list.items) {
      if (ids.has(key(item))) {
        latest.set(key(item), { kind, ...item });
      }
    }
    return list.metadata.resourceVersion ?? '';
  };

  let resourceVersion = relist();
  while (pending().length > 0) {
    const remaining = Math.ceil((deadline - Date.now()) / 1000);
    if (remaining <= 0) {
      break;
    }

    const query = new URLSearchParams({
      watch: 'true',
      allowWatchBookmarks: 'true',
      resourceVersion,
      timeoutSeconds: String(Math.min(remaining, WATCH_SECONDS)),
    });
    const response = kubeSend({
      context: kube.context,
      method: 'GET',
      path: `${path}?${query}`,
      headers: { Accept: 'application/json' },
    });
    if (response.status >= 400) {
      throw new Error(`GET ${path}: watch failed with ${response.status}`);
    }

    for (const line of response.body.split('\n').filter(Boolean)) {
      const event = JSON.parse(line) as { type: string; object: FluxObject };
      if (event.type === 'ERROR') {
        resourceVersion = relist();
        break;
      }

      resourceVersion = event.object.metadata.resourceVersion ?? resourceVersion;
      if (event.type !== 'BOOKMARK' && ids.has(key(event.object))) {
        latest.set(key(event.object), { kind, ...event.object });
      }
    }
  }

  return latest;
}
