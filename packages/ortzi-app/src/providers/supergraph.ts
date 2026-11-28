import type { Schema } from 'jsonschema/lib/index.js';
import { appNamespace, declared } from '../app.ts';
import type { ResourceProvider } from './provider.ts';

export interface ExternalIssuer {
  name: string;
  issuer: string;
  jwks: string;
  audience: string;
}

export interface Passport {
  export: string;
  seedEnv: string;
  ttlSeconds: number;
}

export interface Supergraph {
  name: string;
  type: 'supergraph';
  base: string;
  passport: Passport;
  externalIssuers?: ExternalIssuer[];
}

export interface RouterAuth {
  externalIssuers: ExternalIssuer[];
  passport?: Passport;
}

export const PASSPORT_JWKS_PATH = '/.well-known/passport.jwks';

declare module './provider.ts' {
  interface ResourceTypes {
    supergraph: Supergraph;
  }
}

const schema: Schema = {
  type: 'object',
  additionalProperties: false,
  required: ['name', 'type', 'base', 'passport'],
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
    passport: {
      type: 'object',
      additionalProperties: false,
      required: ['export', 'seedEnv', 'ttlSeconds'],
      description:
        'The router signs a short-lived passport for every subgraph fetch, saying who the caller is; subgraphs verify it against the keys the router serves instead of verifying the caller themselves.',
      properties: {
        export: {
          type: 'string',
          description: `This app's export subgraphs fetch the router's keys from, at ${'/.well-known/passport.jwks'}.`,
        },
        seedEnv: {
          type: 'string',
          pattern: '^[A-Z][A-Z0-9_]*$',
          description:
            "Env var on the router holding the seed its signing key derives from, e.g. from a secret resource's generate; changing it rotates the key.",
        },
        ttlSeconds: {
          type: 'integer',
          minimum: 1,
          description: 'How long a passport is valid for after it is signed.',
        },
      },
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

  validate(_app, supergraphs, fail) {
    for (const resource of supergraphs) {
      if (resource.externalIssuers?.some((issuer) => issuer.name === 'kanidm')) {
        fail(`resources.${resource.name}: externalIssuers: kanidm is the router's own issuer name`);
      }
    }
  },

  check(app, supergraphs, { fail }) {
    for (const resource of supergraphs) {
      if (!app.exports?.some((exported) => exported.name === resource.passport.export)) {
        fail(
          `resources.${resource.name}: passport.export: ${app.name} has no export named ${resource.passport.export}`,
        );
      }
    }
  },

  // subgraphs fetch the router's passport keys
  plan: ({ app, world }, supergraphs) =>
    supergraphs.length
      ? {
          appIngress: [
            ...new Set(
              world.apps
                .filter((other) => declared(other, 'subgraph').length)
                .map(appNamespace)
                .filter((namespace) => namespace !== appNamespace(app)),
            ),
          ].sort(),
        }
      : {},
};
