import type { Schema } from 'jsonschema/lib/index.js';
import type { App } from '../types.ts';
import { appNamespace } from '../app.ts';
import { supergraphApp, validateSubgraphs } from '../subgraphs.ts';
import type { ResourceProvider } from './provider.ts';

export interface SubgraphAuth {
  clients: string[];
  groups: string[];
}

export interface Subgraph {
  name: string;
  type: 'subgraph';
  workload: string;
  port: string | number;
  path: string;
  websocket?: string;
  prefix: string;
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

const schema: Schema = {
  type: 'object',
  additionalProperties: false,
  required: ['name', 'type', 'prefix', 'workload', 'port', 'path', 'schema'],
  description:
    "Registers this app's GraphQL API as a federation subgraph of the app declaring a supergraph.",
  properties: {
    name: {
      type: 'string',
    },
    type: {
      enum: ['subgraph'],
    },
    prefix: {
      type: 'string',
      pattern: '^[a-z][a-zA-Z0-9]*$',
      description:
        'Prefix every root field of this subgraph must start with, e.g. home -> homeEntities.',
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
      required: ['clients', 'groups'],
      description:
        'Early check at the router: operations against this subgraph, and its part of introspection, need a Kanidm JWT from one of clients holding one of groups. The subgraph still authorizes on its own.',
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
          description: 'Kanidm groups, short name or SPN; holding any one of them is enough.',
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

export const subgraph: ResourceProvider<'subgraph'> = {
  type: 'subgraph',
  schema,

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
