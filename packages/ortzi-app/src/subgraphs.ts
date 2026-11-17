import type { App } from './types.ts';
import type { Subgraph, SubgraphAuth } from './providers/subgraph.ts';
import { appNamespace, declared } from './app.ts';
import { exportPort, serviceUrl } from './services.ts';
import { appWorkloads } from './providers/workload.ts';

export interface ResolvedSubgraph {
  name: string;
  app: string;
  prefix: string;
  url: string;
  websocket?: string;
  auth?: SubgraphAuth;
  schemaUrl?: string;
  schemaPath?: string;
}

export function rawFileUrl(repository: string, ref: string, path: string): string {
  const parsed = new URL(repository);
  const [owner, repo] = parsed.pathname
    .replace(/\.git$/, '')
    .split('/')
    .filter(Boolean);
  if (parsed.protocol !== 'https:' || !owner || !repo) {
    throw new Error(`${repository} is not an https://<host>/<owner>/<repo> url`);
  }
  if (parsed.hostname === 'github.com') {
    return `https://raw.githubusercontent.com/${owner}/${repo}/${ref}/${path}`;
  }
  return `${parsed.origin}/${owner}/${repo}/raw/branch/${ref}/${path}`;
}

function schemaSource(subgraph: Subgraph, where: string) {
  if (subgraph.repository === undefined && subgraph.ref === undefined) {
    if (subgraph.schema.startsWith('/') || subgraph.schema.split('/').includes('..')) {
      throw new Error(`${where}: schema ${subgraph.schema} must stay inside the app directory`);
    }

    return { schemaPath: subgraph.schema };
  }
  if (subgraph.repository === undefined || subgraph.ref === undefined) {
    throw new Error(`${where}: repository and ref go together`);
  }

  return { schemaUrl: rawFileUrl(subgraph.repository, subgraph.ref, subgraph.schema) };
}

export function supergraphApp(apps: App[]): App | undefined {
  const found = apps.filter((app) => declared(app, 'supergraph').length);
  if (found.length > 1) {
    throw new Error(
      `only one app may declare a supergraph, found ${found.map((app) => app.name).join(', ')}`,
    );
  }
  return found[0];
}

export function resolveSubgraphs(apps: App[]): ResolvedSubgraph[] {
  return apps.flatMap((app) =>
    declared(app, 'subgraph').map((subgraph) => {
      const where = `${app.name}: resources.${subgraph.name}`;
      for (const path of [subgraph.path, subgraph.websocket]) {
        if (path !== undefined && !path.startsWith('/')) {
          throw new Error(`${where}: ${path} must start with /`);
        }
      }

      if (!/^[a-z][a-zA-Z0-9]*$/.test(subgraph.prefix)) {
        throw new Error(`${where}: prefix ${subgraph.prefix} must be lowerCamelCase`);
      }

      for (const key of ['clients', 'groups'] as const) {
        const list = subgraph.auth?.[key];
        if (subgraph.auth && (!list?.length || list.some((item) => !item))) {
          throw new Error(`${where}: auth.${key} must list at least one non-empty name`);
        }
      }

      const spec = appWorkloads(app)[subgraph.workload];
      if (!spec) {
        throw new Error(`${where}: no workload named ${subgraph.workload}`);
      }
      const port = exportPort(app, {
        name: subgraph.name,
        workload: subgraph.workload,
        port: subgraph.port,
      });

      return {
        name: subgraph.name,
        app: app.name,
        prefix: subgraph.prefix,
        url: serviceUrl(
          spec.service?.name ?? subgraph.workload,
          appNamespace(app),
          port,
          subgraph.path,
        ),
        ...(subgraph.websocket ? { websocket: subgraph.websocket } : {}),
        ...(subgraph.auth ? { auth: subgraph.auth } : {}),
        ...schemaSource(subgraph, where),
      };
    }),
  );
}

export function validateSubgraphs(apps: App[]) {
  const supergraph = supergraphApp(apps);
  const subgraphs = resolveSubgraphs(apps);
  if (subgraphs.length && !supergraph) {
    throw new Error(
      `${subgraphs.map((s) => s.app).join(', ')}: subgraphs need an app declaring a supergraph`,
    );
  }
  const reserved = subgraphs.find((subgraph) => subgraph.name === 'node');
  if (reserved) {
    throw new Error(`${reserved.app}: subgraph name node is reserved for the router's node plugin`);
  }

  const clients = new Set(
    apps.flatMap((app) => declared(app, 'oidc').map((client) => client.name)),
  );
  for (const subgraph of subgraphs) {
    const missing = subgraph.auth?.clients.filter((client) => !clients.has(client)) ?? [];
    if (missing.length) {
      throw new Error(
        `${subgraph.app}: resources.${subgraph.name}: auth.clients has no oidc resource named ${missing.join(', ')}`,
      );
    }
  }

  for (const key of ['name', 'prefix'] as const) {
    const seen = new Map<string, string>();
    for (const subgraph of subgraphs) {
      const other = seen.get(subgraph[key]);
      if (other) {
        throw new Error(
          `subgraph ${key} ${subgraph[key]} is declared by both ${other} and ${subgraph.app}`,
        );
      }
      seen.set(subgraph[key], subgraph.app);
    }
  }
}
