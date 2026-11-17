import type { Schema } from 'jsonschema/lib/index.js';
import type { ResourceProvider } from './provider.ts';

export interface Supergraph {
  name: string;
  type: 'supergraph';
  base: string;
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
  },
};

export const supergraph: ResourceProvider<'supergraph'> = {
  type: 'supergraph',
  schema,
};
