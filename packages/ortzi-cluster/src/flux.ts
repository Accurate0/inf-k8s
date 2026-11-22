export const FLUX_NAMESPACE = 'flux-system';
export const ROOT = 'flux-system';
export const REQUESTED_AT = 'reconcile.fluxcd.io/requestedAt';
const COMMIT = 'org.opencontainers.image.revision';

export const SOURCES = [
  'ocirepositories',
  'gitrepositories',
  'helmrepositories',
  'helmcharts',
  'buckets',
];

export interface Condition {
  type: string;
  status: string;
  reason?: string;
  message?: string;
}

export interface FluxObject {
  kind?: string;
  metadata: {
    name: string;
    namespace?: string;
    generation?: number;
    resourceVersion?: string;
    annotations?: Record<string, string>;
  };
  spec?: { suspend?: boolean; type?: string };
  status?: {
    observedGeneration?: number;
    conditions?: Condition[];
    lastHandledReconcileAt?: string;
    lastAppliedRevision?: string;
    artifact?: {
      revision?: string;
      lastUpdateTime?: string;
      metadata?: Record<string, string>;
    };
  };
}

const condition = (object: FluxObject, type: string) =>
  object.status?.conditions?.find((c) => c.type === type);

export const id = (object: FluxObject) =>
  `${object.kind?.toLowerCase() ?? '?'}/${object.metadata.namespace ?? '-'}/${object.metadata.name}`;

export function ready(object: FluxObject): boolean {
  return (
    object.status?.observedGeneration === object.metadata.generation &&
    condition(object, 'Ready')?.status === 'True'
  );
}

export const stalled = (object: FluxObject) => condition(object, 'Stalled')?.status === 'True';

export const suspended = (object: FluxObject) => object.spec?.suspend === true;

export const readyMessage = (object: FluxObject) =>
  condition(object, 'Ready')?.message ?? 'not reconciled yet';

export type State = 'ready' | 'reconciling' | 'failing' | 'suspended';

export function state(object: FluxObject): State {
  if (suspended(object)) {
    return 'suspended';
  }
  if (ready(object)) {
    return 'ready';
  }
  if (
    !stalled(object) &&
    (condition(object, 'Reconciling')?.status === 'True' ||
      condition(object, 'Ready')?.status === 'True')
  ) {
    return 'reconciling';
  }
  return 'failing';
}

export const handled = (object: FluxObject, token: string) =>
  object.status?.lastHandledReconcileAt === token;

// done once flux picked up the request and settled, either way
export const settled = (object: FluxObject, token: string) =>
  handled(object, token) &&
  condition(object, 'Reconciling')?.status !== 'True' &&
  condition(object, 'Ready') !== undefined;

// oci helm repositories are static pointers with no artifact to refetch
export const refetchable = (object: FluxObject) =>
  !suspended(object) && !(object.kind === 'HelmRepository' && object.spec?.type === 'oci');

export function matches(object: FluxObject, filters: string[]): boolean {
  if (filters.length === 0) {
    return true;
  }

  const kind = object.kind?.toLowerCase();
  const name = object.metadata.name;
  return filters.some((filter) => {
    const [want, wantName] = filter.toLowerCase().split('/', 2);
    return wantName === undefined ? want === name : want === kind && wantName === name;
  });
}

export const revisionOf = (object: FluxObject) => object.status?.artifact?.revision;

export const commitOf = (object: FluxObject) => object.status?.artifact?.metadata?.[COMMIT];

export function shortRevision(revision: string | undefined): string {
  if (!revision) {
    return '-';
  }

  const digest = revision.match(/^(.*?)@?sha256:([0-9a-f]{64})$/);
  if (digest) {
    return `${digest[1] ? `${digest[1]}@` : ''}${digest[2].slice(0, 12)}`;
  }
  return revision.replace(/([0-9a-f]{40})$/, (sha) => sha.slice(0, 8));
}

export interface Change {
  id: string;
  from?: string;
  to?: string;
}

export function changes(before: Map<string, string | undefined>, after: FluxObject[]): Change[] {
  return after
    .filter((object) => before.has(id(object)) && before.get(id(object)) !== revisionOf(object))
    .map((object) => ({ id: id(object), from: before.get(id(object)), to: revisionOf(object) }));
}
