export interface Condition {
  type: string;
  status: string;
  reason?: string;
  message?: string;
}

export interface ExternalSecret {
  metadata: { name: string; namespace: string };
  spec: {
    secretStoreRef?: { name?: string; kind?: string };
    dataFrom?: { extract?: { key: string } }[];
    data?: { remoteRef: { key: string } }[];
  };
  status?: { refreshTime?: string; conditions?: Condition[] };
}

export const readsOpenbao = (es: ExternalSecret) =>
  es.spec.secretStoreRef?.kind !== 'ClusterSecretStore' &&
  (es.spec.secretStoreRef?.name ?? '').startsWith('openbao');

export const keys = (es: ExternalSecret): string[] => [
  ...(es.spec.dataFrom ?? []).flatMap((d) => (d.extract ? [d.extract.key] : [])),
  ...(es.spec.data ?? []).map((d) => d.remoteRef.key),
];

export const ready = (es: ExternalSecret) => es.status?.conditions?.find((c) => c.type === 'Ready');

export const isReady = (es: ExternalSecret) => ready(es)?.status === 'True';

export const fullName = (es: ExternalSecret) => `${es.metadata.namespace}/${es.metadata.name}`;

const trimSlashes = (path: string) => path.replace(/^\/+|\/+$/g, '');

export function matching(all: ExternalSecret[], target: string): ExternalSecret[] {
  const wanted = trimSlashes(target);
  const readsTarget = (es: ExternalSecret) => keys(es).some((k) => trimSlashes(k) === wanted);
  return all
    .filter(readsOpenbao)
    .filter((es) =>
      wanted.includes('/') ? readsTarget(es) : es.metadata.namespace === wanted || readsTarget(es),
    );
}
