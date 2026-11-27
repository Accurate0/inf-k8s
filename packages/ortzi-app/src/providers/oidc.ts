import type { Schema } from 'jsonschema/lib/index.js';
import type { App } from '../types.ts';
import { declared, specs } from '../app.ts';
import { scopeGrants } from '../subgraphs.ts';
import { fail, type KubeObject, ok, unmet } from '../checks.ts';
import { appRoutes, type RouteAuth } from './route.ts';
import type { ResourceProvider } from './provider.ts';

export type OidcClient = { host: string; displayName?: string; secretName: string } & Record<
  string,
  unknown
>;

declare module './provider.ts' {
  interface ResourceTypes {
    oidc: { name: string; type: 'oidc' } & OidcClient;
  }
}

const schema: Schema = {
  type: 'object',
  additionalProperties: false,
  properties: {
    name: {
      type: 'string',
    },
    type: {
      enum: ['oidc'],
    },
    allowInsecureClientDisablePkce: {
      type: 'boolean',
    },
    callbackPath: {
      type: 'string',
    },
    claimMaps: {
      type: 'object',
    },
    enableLegacyCrypto: {
      type: 'boolean',
    },
    extraRedirectUrls: {
      type: 'array',
      items: {
        type: 'string',
      },
    },
    groups: {
      type: 'object',
    },
    icon: {
      type: 'object',
    },
    origin: {
      type: 'string',
    },
    preferShortUsername: {
      type: 'boolean',
    },
    scopeMapGroup: {
      type: 'string',
    },
    scopeMaps: {
      type: 'object',
    },
    scopes: {
      type: 'array',
      items: {
        type: 'string',
      },
    },
    secretKeys: {
      type: 'object',
    },
    secretLabels: {
      type: 'object',
    },
    serviceAccounts: {
      type: 'array',
      description:
        'Kanidm service accounts for programmatic access. Each API token lands in secretName and is exchanged at this client (RFC 8693) for its tokens; list the account in groups to grant access.',
      items: {
        type: 'object',
        additionalProperties: false,
        properties: {
          name: { type: 'string', pattern: '^[a-z0-9]([a-z0-9_-]*[a-z0-9])?$' },
          displayName: { type: 'string' },
          entryManagedBy: { type: 'string' },
          secretName: { type: 'string' },
          secretNamespace: {
            type: 'string',
            description:
              "Namespace of the token secret, for an app that exchanges the token on the account's behalf. Defaults to this app's namespace.",
          },
          scopes: { type: 'array', items: { type: 'string' } },
        },
        required: ['name', 'displayName', 'entryManagedBy', 'secretName'],
      },
    },
    public: {
      type: 'boolean',
    },
    displayName: {
      type: 'string',
    },
    allowLocalhostRedirect: {
      type: 'boolean',
    },
    host: {
      type: 'string',
    },
    secretName: {
      type: 'string',
    },
  },
  required: ['name', 'type', 'secretName', 'host', 'displayName', 'preferShortUsername'],
};

export function appOidc(app: App): Record<string, OidcClient> {
  return Object.fromEntries(specs(declared(app, 'oidc'))) as Record<string, OidcClient>;
}

export function supplementaryScopeMaps(
  apps: App[],
  client: string,
): Record<string, string[]> | undefined {
  const byGroup = new Map<string, Set<string>>();
  for (const app of apps) {
    for (const subgraph of declared(app, 'subgraph')) {
      if (!subgraph.auth?.clients.includes(client)) {
        continue;
      }
      for (const scope of subgraph.auth.scopes ?? []) {
        for (const group of scopeGrants(scope).groups) {
          byGroup.set(group, (byGroup.get(group) ?? new Set()).add(scope.name));
        }
      }
    }
  }

  if (!byGroup.size) {
    return undefined;
  }
  return Object.fromEntries(
    [...byGroup.entries()]
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([group, scopes]) => [group, [...scopes].sort()]),
  );
}

function loginPolicy(app: App, client: string) {
  const route = Object.entries(appRoutes(app)).find(
    ([, r]) => r.auth?.mode === 'oidc' && r.auth.client === client,
  );
  if (!route) {
    return {};
  }
  const {
    mode: _mode,
    client: _client,
    ...policy
  } = route[1].auth as Extract<RouteAuth, { mode: 'oidc' }>;
  return { securityPolicy: { httpRoute: route[0], ...policy } };
}

export const oidc: ResourceProvider<'oidc'> = {
  type: 'oidc',
  schema,

  plan({ app, namespace, annotations, chart, world }) {
    const clients = Object.entries(appOidc(app));
    if (!clients.length) {
      return {};
    }
    return {
      charts: [
        chart(
          'k8s-oidc',
          {
            clients: clients.map(([client, spec]) => {
              const supplementary = supplementaryScopeMaps(world.apps, client);
              return {
                name: client,
                namespace,
                ...spec,
                ...(supplementary ? { supplementaryScopeMaps: supplementary } : {}),
                ...loginPolicy(app, client),
                secretName: spec.secretName,
                secretNamespace: namespace,
                annotations,
              };
            }),
          },
          { retain: true },
        ),
      ],
    };
  },

  doctor: {
    label: 'oidc',
    run({ ns }, clients) {
      if (!clients.length) {
        return [];
      }
      return ns.list<KubeObject>('kanidmoauth2clients').map((client) => {
        const problems = unmet(client.status?.conditions, ['Accepted', 'Programmed']);
        const text = `oidc client ${client.metadata.name}`;
        return problems.length
          ? fail(`${text}: ${problems.join('; ')}`)
          : ok(`${text} provisioned`);
      });
    },
  },
};
