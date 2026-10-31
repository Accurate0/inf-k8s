import type { App } from './render.ts';
import { appDatabases } from './render.ts';
import { bucketName } from './terraform.ts';

interface Node {
  id: string;
  label: string;
  kind: 'app' | 'database' | 'platform' | 'namespace';
}

interface Edge {
  from: string;
  to: string;
  label?: string;
  dashed?: boolean;
}

export function buildGraph(apps: App[], only?: string): { nodes: Node[]; edges: Edge[] } {
  const known = new Set(apps.map((app) => app.name));
  const nodes = new Map<string, Node>();
  const edges: Edge[] = [];
  const node = (id: string, label: string, kind: Node['kind']) => {
    if (!nodes.has(id)) nodes.set(id, { id, label, kind });
    return id;
  };
  for (const app of apps) {
    node(app.name, app.name, 'app');
    for (const resource of app.resources ?? []) {
      if (resource.type === 'app') {
        edges.push({ from: app.name, to: resource.app, label: resource.export });
      } else if (['database', 'secret', 'route', 'oidc', 'metrics'].includes(resource.type)) {
        continue;
      } else if (resource.type === 'platform') {
        edges.push({ from: app.name, to: node(`platform/${resource.export}`, resource.export, 'platform') });
      } else if (resource.type === 'cache') {
        edges.push({ from: app.name, to: node(`cache/${resource.instance}`, `cache ${resource.instance}`, 'platform') });
      } else if (resource.type === 'bucket') {
        const name = bucketName(app, resource);
        edges.push({ from: app.name, to: node(`bucket/${name}`, `${name} (${resource.provider})`, 'database') });
      }
    }
    for (const [database, spec] of Object.entries(appDatabases(app))) {
      const name = spec.databaseName ?? database.replaceAll('-', '_');
      edges.push({ from: app.name, to: node(`database/${name}`, name, 'database') });
    }
  }
  for (const app of apps) {
    for (const from of app.network.ingress) {
      if (edges.some((edge) => edge.from === from && edge.to === app.name)) continue;
      if (!known.has(from)) node(from, from, 'namespace');
      edges.push({ from, to: app.name, label: 'ingress', dashed: true });
    }
  }
  if (!only) return { nodes: [...nodes.values()], edges };
  const near = edges.filter((edge) => edge.from === only || edge.to === only);
  const keep = new Set([only, ...near.flatMap((edge) => [edge.from, edge.to])]);
  return { nodes: [...nodes.values()].filter((n) => keep.has(n.id)), edges: near };
}

const id = (name: string) => name.replace(/[^a-zA-Z0-9]/g, '_');

export function toMermaid({ nodes, edges }: ReturnType<typeof buildGraph>): string {
  const shape = (n: Node) =>
    n.kind === 'database' ? `[("${n.label}")]` : n.kind === 'platform' ? `{{"${n.label}"}}` : n.kind === 'namespace' ? `(["${n.label}"])` : `["${n.label}"]`;
  return [
    'graph LR',
    ...nodes.map((n) => `  ${id(n.id)}${shape(n)}`),
    ...edges.map((e) => `  ${id(e.from)} ${e.dashed ? '-.->' : '-->'}${e.label ? `|${e.label}|` : ''} ${id(e.to)}`),
  ].join('\n');
}

export function toDot({ nodes, edges }: ReturnType<typeof buildGraph>): string {
  const shapes = { app: 'box', database: 'cylinder', platform: 'hexagon', namespace: 'oval' };
  return [
    'digraph apps {',
    '  rankdir=LR;',
    ...nodes.map((n) => `  "${n.id}" [label="${n.label}", shape=${shapes[n.kind]}];`),
    ...edges.map((e) => {
      const attrs = [...(e.label ? [`label="${e.label}"`] : []), ...(e.dashed ? ['style=dashed'] : [])];
      return `  "${e.from}" -> "${e.to}"${attrs.length ? ` [${attrs.join(', ')}]` : ''};`;
    }),
    '}',
  ].join('\n');
}
