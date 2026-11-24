import type { Schema } from 'jsonschema/lib/index.js';
import type { ResourceProvider } from './provider.ts';

export interface ExternalIssuer {
  name: string;
  issuer: string;
  jwks: string;
  audience: string;
}

export interface Supergraph {
  name: string;
  type: 'supergraph';
  base: string;
  externalIssuers?: ExternalIssuer[];
}

declare module './provider.ts' {
  interface ResourceTypes {
    supergraph: Supergraph;
  }
}

const schema: Schema = {
  type: 'object',
  additionalProperties: false,
  required: ['name', 'type', 'base'],
  description:
    'Marks this app as the federation router; `ortzi app supergraph` composes every subgraph into it.',
  properties: {
    name: {
      type: 'string',
    },
    type: {
      enum: ['supergraph'],
    },
    base: {
      type: 'string',
      description:
        'Hand-written router config the generated router.yaml extends; router.yaml, supergraph.graphql, schema.graphql and subgraphs/ are written beside it.',
    },
    externalIssuers: {
      type: 'array',
      description:
        'Non-Kanidm OIDC issuers the router trusts, e.g. forgejo actions. Their tokens carry no groups; a subgraph admits them with auth.external claim patterns.',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['name', 'issuer', 'jwks', 'audience'],
        properties: {
          name: {
            type: 'string',
            pattern: '^[a-z][a-z0-9-]*$',
            description: 'Short name subgraphs refer to in auth.external, e.g. forgejo.',
          },
          issuer: {
            type: 'string',
            description: 'The iss claim, e.g. https://git.anurag.sh/api/actions.',
          },
          jwks: {
            type: 'string',
            description: "URL of the issuer's signing keys.",
          },
          audience: {
            type: 'string',
            description: 'The aud tokens must be minted for, e.g. graph.ortzi.cloud.',
          },
        },
      },
    },
  },
};

export const supergraph: ResourceProvider<'supergraph'> = {
  type: 'supergraph',
  schema,
};
