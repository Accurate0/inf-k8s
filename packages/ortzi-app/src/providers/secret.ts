import type { Schema } from 'jsonschema/lib/index.js';
import type { App } from '../types.ts';
import { declared } from '../app.ts';
import type { ResourceProvider } from './provider.ts';

export interface SecretTemplate {
  type?: string;
  includeAllSecrets?: boolean;
  data: Record<string, string>;
}

export interface GeneratedKey {
  length: number;
  digits?: number;
  symbols: number;
  encoding?: 'raw' | 'base64' | 'base64url' | 'base32' | 'hex';
}

export interface Secret {
  name: string;
  type: 'secret';
  secretName: string;
  secretTemplate?: SecretTemplate;
  generate?: Record<string, GeneratedKey>;
}

declare module './provider.ts' {
  interface ResourceTypes {
    secret: Secret;
  }
}

const schema: Schema = {
  type: 'object',
  additionalProperties: false,
  required: ['name', 'type', 'secretName'],
  properties: {
    name: {
      type: 'string',
    },
    type: {
      enum: ['secret'],
    },
    secretName: {
      type: 'string',
    },
    secretTemplate: {
      type: 'object',
      description: 'Reshape the synced secret, for consumers that expect other keys.',
      additionalProperties: false,
      required: ['data'],
      properties: {
        type: {
          type: 'string',
        },
        includeAllSecrets: {
          type: 'boolean',
        },
        data: {
          type: 'object',
          additionalProperties: {
            type: 'string',
          },
        },
      },
    },
    generate: {
      type: 'object',
      description:
        'Generate these keys once and keep them in OpenBao at kv/<namespace>/generated/<name>, instead of reading values someone stored.',
      minProperties: 1,
      additionalProperties: {
        type: 'object',
        additionalProperties: false,
        required: ['length', 'symbols'],
        properties: {
          length: {
            type: 'integer',
            minimum: 1,
          },
          digits: {
            type: 'integer',
            minimum: 0,
            description: 'Defaults to a quarter of the length.',
          },
          symbols: {
            type: 'integer',
            minimum: 0,
          },
          encoding: {
            enum: ['raw', 'base64', 'base64url', 'base32', 'hex'],
            description: 'Defaults to raw.',
          },
        },
      },
    },
  },
};

export const appSecrets = (app: App) =>
  declared(app, 'secret').map(({ name, secretName, secretTemplate, generate }) => ({
    name,
    secretName,
    secretTemplate,
    generate,
  }));

export const managedSecret = ({
  secretName,
  secretTemplate,
}: {
  secretName: string;
  secretTemplate?: SecretTemplate;
}) => ({
  secretName,
  ...(secretTemplate?.type ? { type: secretTemplate.type } : {}),
  ...(secretTemplate
    ? {
        template: {
          ...(secretTemplate.includeAllSecrets === undefined
            ? {}
            : { includeAllSecrets: secretTemplate.includeAllSecrets }),
          data: secretTemplate.data,
        },
      }
    : {}),
});

export const secret: ResourceProvider<'secret'> = {
  type: 'secret',
  ref: 'secret',
  schema,

  validate(_app, secrets, fail) {
    for (const resource of secrets) {
      if (!resource.generate) {
        continue;
      }
      const keys = Object.keys(resource.generate);
      const objects = keys.map((key) => key.toLowerCase().replaceAll('_', '-'));
      for (const [i, key] of keys.entries()) {
        if (!/^[A-Za-z0-9]([A-Za-z0-9_]*[A-Za-z0-9])?$/.test(key)) {
          fail(`resources.${resource.name}.generate.${key}: use letters, digits and underscores`);
        }
        if (objects.indexOf(objects[i]) !== i) {
          fail(
            `resources.${resource.name}.generate.${key} clashes with ${keys[objects.indexOf(objects[i])]}`,
          );
        }
      }
    }
  },

  plan(_ctx, secrets) {
    const synced = secrets
      .filter((s) => !s.generate)
      .map((s) => ({ name: s.name, managedSecrets: [managedSecret(s)] }));
    const generated = secrets.flatMap(({ name, secretName, secretTemplate, generate }) =>
      generate
        ? [
            {
              name,
              secretName,
              ...(secretTemplate?.type ? { type: secretTemplate.type } : {}),
              ...(secretTemplate ? { template: { data: secretTemplate.data } } : {}),
              keys: Object.entries(generate).map(([key, spec]) => ({ name: key, ...spec })),
            },
          ]
        : [],
    );
    return { secrets: { synced, generated } };
  },
};
