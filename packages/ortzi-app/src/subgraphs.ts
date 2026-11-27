import type { App } from './types.ts';
import type { ClaimPattern, Subgraph, SubgraphAuth, SubgraphScope } from './providers/subgraph.ts';
import { appNamespace, declared } from './app.ts';
import { exportPort, serviceUrl } from './services.ts';
import { appWorkloads } from './providers/workload.ts';

export const SCOPE_PATTERN = '^[a-z][a-z0-9-]*(:[a-z][a-z0-9-]*){2}$';

export function scopeGrants(scope: SubgraphScope): { groups: string[]; external: ClaimPattern[] } {
  const groups = new Set<string>();
  const external: ClaimPattern[] = [];

  for (const grant of scope.grantedBy) {
    if ('groups' in grant) {
      grant.groups.forEach((group) => groups.add(group));
    } else {
      external.push(grant.external);
    }
  }

  return { groups: [...groups], external };
}

export interface ResolvedSubgraph {
  name: string;
  app: string;
  prefixes: string[];
  allowedRootFields?: string[];
  url: string;
  websocket?: string;
  auth?: SubgraphAuth;
  schemaUrl?: string;
  schemaPath?: string;
  documentsUrl?: string;
  documentsPath?: string;
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

function fileSource(subgraph: Subgraph, key: 'schema' | 'persistedDocuments', where: string) {
  const path = subgraph[key] as string;
  if (subgraph.repository === undefined && subgraph.ref === undefined) {
    if (path.startsWith('/') || path.split('/').includes('..')) {
      throw new Error(`${where}: ${key} ${path} must stay inside the app directory`);
    }

    return { path };
  }
  if (subgraph.repository === undefined || subgraph.ref === undefined) {
    throw new Error(`${where}: repository and ref go together`);
  }

  return { url: rawFileUrl(subgraph.repository, subgraph.ref, path) };
}

function schemaSource(subgraph: Subgraph, where: string) {
  const { path, url } = fileSource(subgraph, 'schema', where);
  return path !== undefined ? { schemaPath: path } : { schemaUrl: url };
}

function documentsSource(subgraph: Subgraph, where: string) {
  if (subgraph.persistedDocuments === undefined) {
    return {};
  }

  const { path, url } = fileSource(subgraph, 'persistedDocuments', where);
  return path !== undefined ? { documentsPath: path } : { documentsUrl: url };
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

      const prefixes = subgraph.prefixes;
      if (!prefixes.length) {
        throw new Error(`${where}: prefixes must list at least one prefix`);
      }
      for (const prefix of prefixes) {
        if (!/^[a-z][a-zA-Z0-9]*$/.test(prefix)) {
          throw new Error(`${where}: prefix ${prefix} must be lowerCamelCase`);
        }
      }

      const clients = subgraph.auth?.clients;
      if (subgraph.auth && (!clients?.length || clients.some((item) => !item))) {
        throw new Error(`${where}: auth.clients must list at least one non-empty name`);
      }
      if (subgraph.auth && !subgraph.auth.groups?.length && !subgraph.auth.scopes?.length) {
        throw new Error(`${where}: auth needs groups or scopes`);
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
        prefixes,
        ...(subgraph.allowedRootFields?.length
          ? { allowedRootFields: subgraph.allowedRootFields }
          : {}),
        url: serviceUrl(
          spec.service?.name ?? subgraph.workload,
          appNamespace(app),
          port,
          subgraph.path,
        ),
        ...(subgraph.websocket ? { websocket: subgraph.websocket } : {}),
        ...(subgraph.auth ? { auth: subgraph.auth } : {}),
        ...schemaSource(subgraph, where),
        ...documentsSource(subgraph, where),
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
  const reserved = subgraphs.find((subgraph) => ['node', 'me'].includes(subgraph.name));
  if (reserved) {
    throw new Error(
      `${reserved.app}: subgraph name ${reserved.name} is reserved for the router's own plugins`,
    );
  }

  const issuers = supergraph
    ? declared(supergraph, 'supergraph').flatMap((resource) => resource.externalIssuers ?? [])
    : [];
  const issuerNames = new Set<string>();
  for (const issuer of issuers) {
    if (issuerNames.has(issuer.name)) {
      throw new Error(`${supergraph?.name}: external issuer ${issuer.name} is declared twice`);
    }
    issuerNames.add(issuer.name);
  }

  const scopeOwners = new Map<string, string>();
  for (const subgraph of subgraphs) {
    const where = `${subgraph.app}: resources.${subgraph.name}`;
    for (const scope of subgraph.auth?.scopes ?? []) {
      if (!new RegExp(SCOPE_PATTERN).test(scope.name)) {
        throw new Error(`${where}: scope ${scope.name} must be <subgraph>:<resource>:<action>`);
      }
      const owners = [subgraph.name, ...subgraph.prefixes];
      if (!owners.includes(scope.name.split(':')[0]!)) {
        throw new Error(
          `${where}: scope ${scope.name} must start with ${[...new Set(owners)].map((o) => `${o}:`).join(' or ')}`,
        );
      }
      const owner = scopeOwners.get(scope.name);
      if (owner) {
        throw new Error(`${where}: scope ${scope.name} is already declared by ${owner}`);
      }
      scopeOwners.set(scope.name, where);
    }
  }

  for (const subgraph of subgraphs) {
    const unknown = [
      ...(subgraph.auth?.external ?? []),
      ...(subgraph.auth?.scopes ?? []).flatMap((scope) => scopeGrants(scope).external),
    ]
      .map((pattern) => pattern.issuer)
      .filter((issuer) => !issuerNames.has(issuer));
    if (unknown.length) {
      throw new Error(
        `${subgraph.app}: resources.${subgraph.name}: auth names no external issuer ${unknown.join(', ')}; declare it in the supergraph's externalIssuers`,
      );
    }
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
      for (const value of key === 'name' ? [subgraph.name] : subgraph.prefixes) {
        const other = seen.get(value);
        if (other) {
          throw new Error(
            `subgraph ${key} ${value} is declared by both ${other} and ${subgraph.app}`,
          );
        }
        seen.set(value, subgraph.app);
      }
    }
  }
}
