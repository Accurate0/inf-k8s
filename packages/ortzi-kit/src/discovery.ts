export const DISCOVERY = 'application/json;g=apidiscovery.k8s.io;v=v2;as=APIGroupDiscoveryList';

interface DiscoveredResource {
  resource: string;
  singularResource?: string;
  shortNames?: string[];
  scope: 'Namespaced' | 'Cluster';
  responseKind?: { kind: string };
}

export interface DiscoveryList {
  items: {
    metadata?: { name?: string };
    versions: { version: string; resources?: DiscoveredResource[] }[];
  }[];
}

export interface Resource {
  group: string;
  version: string;
  plural: string;
  kind: string;
  namespaced: boolean;
  names: string[];
}

export function discovered(list: DiscoveryList): Resource[] {
  return list.items.flatMap((group) => {
    const [preferred] = group.versions;
    if (!preferred) return [];

    return (preferred.resources ?? []).map((r) => {
      const kind = r.responseKind?.kind ?? '';
      return {
        group: group.metadata?.name ?? '',
        version: preferred.version,
        plural: r.resource,
        kind,
        namespaced: r.scope === 'Namespaced',
        names: [r.resource, r.singularResource, kind.toLowerCase(), ...(r.shortNames ?? [])].filter(
          (name): name is string => !!name,
        ),
      };
    });
  });
}

export function findResource(resources: Resource[], name: string): Resource | undefined {
  const [short, ...group] = name.toLowerCase().split('.');
  const wantedGroup = group.join('.');
  return resources.find(
    (r) => r.names.includes(short) && (!wantedGroup || r.group === wantedGroup),
  );
}
