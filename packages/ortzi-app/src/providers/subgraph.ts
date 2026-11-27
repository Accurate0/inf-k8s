import type { Schema } from 'jsonschema/lib/index.js';
import type { App } from '../types.ts';
import { appNamespace, declared } from '../app.ts';
import { SCOPE_PATTERN, scopeGrants, supergraphApp, validateSubgraphs } from '../subgraphs.ts';
import type { ResourceProvider } from './provider.ts';
import { KANIDM_ISSUER_BASE } from '../supergraph.ts';

export interface ClaimPattern {
  issuer: string;
  claims: Record<string, string | string[]>;
}

export type ScopeGrant = { groups: string[] } | { external: ClaimPattern };

export interface SubgraphScope {
  name: string;
  description: string;
  grantedBy?: ScopeGrant[];
  groups?: string[];
  external?: ClaimPattern[];
}

export interface SubgraphAuth {
  clients: string[];
  groups?: string[];
  scopes?: SubgraphScope[];
  external?: ClaimPattern[];
}

export interface Subgraph {
  name: string;
  type: 'subgraph';
  workload: string;
  port: string | number;
  path: string;
  websocket?: string;
  prefixes: string[];
  allowedRootFields?: string[];
  auth?: SubgraphAuth;
  repository?: string;
  schema: string;
  ref?: string;
  persistedDocuments?: string;
}

declare module './provider.ts' {
  interface ResourceTypes {
    subgraph: Subgraph;
  }
}

const claimPattern: Schema = {
  type: 'object',
  additionalProperties: false,
  required: ['issuer', 'claims'],
  properties: {
    issuer: {
      type: 'string',
      description: 'Name of one of the externalIssuers, e.g. forgejo.',
    },
    claims: {
      type: 'object',
      minProperties: 1,
      description:
        'Claim name to a glob or list of globs, matched case-insensitively, e.g. {repository: "anurag/*", ref: refs/heads/main}.',
      additionalProperties: {
        oneOf: [
          { type: 'string', minLength: 1 },
          { type: 'array', minItems: 1, items: { type: 'string', minLength: 1 } },
        ],
      },
    },
  },
};

const schema: Schema = {
  type: 'object',
  additionalProperties: false,
  required: ['name', 'type', 'prefixes', 'workload', 'port', 'path', 'schema'],
  description:
    "Registers this app's GraphQL API as a federation subgraph of the app declaring a supergraph.",
  properties: {
    name: {
      type: 'string',
    },
    type: {
      enum: ['subgraph'],
    },
    prefixes: {
      type: 'array',
      minItems: 1,
      items: { type: 'string', pattern: '^[a-z][a-zA-Z0-9]*$' },
      description:
        'Root fields and types start with one of these, e.g. [home] -> homeEntities, HomeLight. A subgraph fronting several systems lists each, e.g. [forgejo, flux, kubernetes]; its scopes may then start with any of them.',
    },
    allowedRootFields: {
      type: 'array',
      description: 'Root fields allowed without the prefix, e.g. featureFlag.',
      items: {
        type: 'string',
      },
    },
    workload: {
      type: 'string',
      description: 'Workload serving the GraphQL API.',
    },
    port: {
      type: ['string', 'integer'],
      description: 'Port name or service port on that workload.',
    },
    path: {
      type: 'string',
      description: 'HTTP GraphQL endpoint, e.g. /graphql.',
    },
    websocket: {
      type: 'string',
      description: 'Subscriptions endpoint, when the subgraph has subscriptions.',
    },
    auth: {
      type: 'object',
      additionalProperties: false,
      required: ['clients'],
      anyOf: [{ required: ['groups'] }, { required: ['scopes'] }],
      description:
        'Early check at the router: operations against this subgraph, and its part of introspection, need a Kanidm JWT from one of clients holding one of groups or scopes. The subgraph still authorizes on its own.',
      properties: {
        clients: {
          type: 'array',
          minItems: 1,
          items: {
            type: 'string',
            minLength: 1,
          },
          description:
            'Kanidm OIDC clients (oidc resource names, in any app) whose tokens the router accepts for this subgraph.',
        },
        groups: {
          type: 'array',
          minItems: 1,
          items: {
            type: 'string',
            minLength: 1,
          },
          description:
            'Kanidm groups, short name or SPN; holding any one of them is enough. Superseded by scopes.',
        },
        external: {
          type: 'array',
          description:
            "Identities from the supergraph's externalIssuers admitted to the whole subgraph without groups. Superseded by scopes[].external.",
          items: claimPattern,
        },
        scopes: {
          type: 'array',
          description:
            'OAuth scopes this subgraph defines, named <subgraph>:<resource>:<action>. Each is added to the tokens of members of its groups on every client in clients (Kanidm supplementary scope maps); fields require them with @requiresScopes.',
          items: {
            type: 'object',
            additionalProperties: false,
            required: ['name', 'description'],
            oneOf: [
              {
                required: ['grantedBy'],
                not: { anyOf: [{ required: ['groups'] }, { required: ['external'] }] },
              },
              { required: ['groups'], not: { required: ['grantedBy'] } },
            ],
            properties: {
              name: {
                type: 'string',
                pattern: SCOPE_PATTERN,
                description: 'e.g. registry:package:publish',
              },
              description: {
                type: 'string',
                minLength: 1,
                description:
                  'What holding the scope allows, shown in the published scope registry.',
              },
              grantedBy: {
                type: 'array',
                minItems: 1,
                description: 'Who holds the scope; holding any one entry is enough.',
                items: {
                  oneOf: [
                    {
                      type: 'object',
                      additionalProperties: false,
                      required: ['groups'],
                      properties: {
                        groups: {
                          type: 'array',
                          minItems: 1,
                          items: { type: 'string', minLength: 1 },
                          description:
                            'Kanidm groups whose members get the scope, nested groups included.',
                        },
                      },
                    },
                    {
                      type: 'object',
                      additionalProperties: false,
                      required: ['external'],
                      properties: {
                        external: {
                          ...claimPattern,
                          description:
                            "An identity from the supergraph's externalIssuers that holds the scope when every listed claim matches.",
                        },
                      },
                    },
                  ],
                },
              },
              groups: {
                type: 'array',
                minItems: 1,
                items: { type: 'string', minLength: 1 },
                description: 'Deprecated, use grantedBy[].groups.',
              },
              external: {
                type: 'array',
                description: 'Deprecated, use grantedBy[].external.',
                items: claimPattern,
              },
            },
          },
        },
      },
    },
    repository: {
      type: 'string',
      description:
        'https://<host>/<owner>/<repo> holding the federation SDL. Leave out with ref to read schema from the app directory.',
    },
    schema: {
      type: 'string',
      description:
        'Path to the federation SDL in the repository, or relative to the app directory when repository is left out.',
    },
    ref: {
      type: 'string',
      description: 'Branch or tag to read the SDL from; required with repository.',
    },
    persistedDocuments: {
      type: 'string',
      description:
        "Path to a client's persisted document manifest ({id: document} JSON), read like schema; anonymous callers may only run these documents.",
    },
  },
};

export interface SubgraphAuthValue {
  providers: { issuer: string; audience: string }[];
  scopes: string[];
  groups: string[];
  external: {
    name: string;
    issuer: string;
    jwks: string;
    audience: string;
    grants: { scope: string; claims: ClaimPattern['claims'] }[];
  }[];
}

export function subgraphAuthValue(resource: Subgraph, apps: App[]): SubgraphAuthValue {
  const scopes = resource.auth?.scopes ?? [];
  const router = supergraphApp(apps);
  const issuers = router
    ? declared(router, 'supergraph').flatMap((supergraph) => supergraph.externalIssuers ?? [])
    : [];

  return {
    providers: (resource.auth?.clients ?? []).map((client) => ({
      issuer: `${KANIDM_ISSUER_BASE}/${client}`,
      audience: client,
    })),
    scopes: scopes.map((scope) => scope.name),
    groups: resource.auth?.groups ?? [],
    external: issuers
      .map((issuer) => ({
        ...issuer,
        grants: scopes.flatMap((scope) =>
          scopeGrants(scope)
            .external.filter((pattern) => pattern.issuer === issuer.name)
            .map((pattern) => ({ scope: scope.name, claims: pattern.claims })),
        ),
      }))
      .filter((issuer) => issuer.grants.length),
  };
}

export const subgraph: ResourceProvider<'subgraph'> = {
  type: 'subgraph',
  schema,

  check(_app, subgraphs, { uses, fail }) {
    for (const resource of subgraphs) {
      if (resource.auth?.scopes?.length && !uses.some((use) => use.subgraph === resource.name)) {
        fail(
          `resources.${resource.name}: declares scopes, so its workload needs env SUBGRAPH_AUTH: {subgraph: ${resource.name}} to enforce them`,
        );
      }
    }
  },

  checkWorld: (world) => validateSubgraphs(world.apps),

  plan: ({ app, world }, subgraphs) => ({
    appIngress: subgraphs.length
      ? [supergraphApp(world.apps)]
          .filter((other): other is App => !!other && other.name !== app.name)
          .map(appNamespace)
      : [],
  }),

  graph(app, _resource, { apps, edge }) {
    const supergraph = supergraphApp(apps);
    if (supergraph) {
      edge({ from: supergraph.name, to: app.name, label: 'subgraph' });
    }
  },
};
