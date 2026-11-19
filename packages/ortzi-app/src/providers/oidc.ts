import type { Schema } from 'jsonschema/lib/index.js';
import type { App } from '../types.ts';
import { declared, specs } from '../app.ts';
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

  plan({ app, namespace, annotations, chart }) {
    const clients = Object.entries(appOidc(app));
    if (!clients.length) {
      return {};
    }
    return {
      charts: [
        chart(
          'k8s-oidc',
          {
            clients: clients.map(([client, spec]) => ({
              name: client,
              namespace,
              ...spec,
              ...loginPolicy(app, client),
              secretName: spec.secretName,
              secretNamespace: namespace,
              annotations,
            })),
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
