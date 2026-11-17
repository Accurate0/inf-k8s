import type { Schema } from 'jsonschema/lib/index.js';
import { appNamespace } from '../app.ts';
import { SECRET_KINDS, type SecretKind } from '../workload.ts';
import type { ResourceProvider } from './provider.ts';

export interface Config {
  name: string;
  type: 'config';
  configMap: string;
  files: string[];
  secretName?: string;
  values?: Record<string, Record<string, string>>;
}

declare module './provider.ts' {
  interface ResourceTypes {
    config: Config;
  }
}

const schema: Schema = {
  type: 'object',
  additionalProperties: false,
  required: ['name', 'type', 'configMap', 'files'],
  properties: {
    name: {
      type: 'string',
    },
    type: {
      enum: ['config'],
    },
    configMap: {
      type: 'string',
    },
    files: {
      type: 'array',
      description:
        'Relative to the app directory. Mounted at the same relative path; the key is the basename.',
      items: {
        type: 'string',
      },
    },
    secretName: {
      type: 'string',
      description:
        'Render the files as go templates into this secret, which is mounted instead of the configmap. Needs values.',
    },
    values: {
      type: 'object',
      description:
        'Template variables, read as {{ .NAME }}, taken from a key of a secret, database, bucket or oidc resource.',
      additionalProperties: {
        oneOf: [
          {
            type: 'object',
            additionalProperties: false,
            required: ['secret', 'key'],
            properties: {
              secret: {
                type: 'string',
              },
              key: {
                type: 'string',
              },
            },
          },
          {
            type: 'object',
            additionalProperties: false,
            required: ['database', 'key'],
            properties: {
              database: {
                type: 'string',
              },
              key: {
                type: 'string',
              },
            },
          },
          {
            type: 'object',
            additionalProperties: false,
            required: ['bucket', 'key'],
            properties: {
              bucket: {
                type: 'string',
              },
              key: {
                type: 'string',
              },
            },
          },
          {
            type: 'object',
            additionalProperties: false,
            required: ['oidc', 'key'],
            properties: {
              oidc: {
                type: 'string',
              },
              key: {
                type: 'string',
              },
            },
          },
        ],
      },
    },
  },
};

const valueRefs = (config: Config) =>
  Object.entries(config.values ?? {}).map(([variable, ref]) => {
    const kind = SECRET_KINDS.find((k) => ref[k]) as SecretKind;
    return { variable, kind, name: ref[kind], key: ref.key };
  });

export const config: ResourceProvider<'config'> = {
  type: 'config',
  schema,

  validate(_app, configs, fail) {
    for (const resource of configs) {
      if (!resource.values !== !resource.secretName) {
        fail(`resources.${resource.name}: set secretName and values together`);
      }
    }
  },

  uses: (_app, configs) => configs.flatMap((resource) => Object.values(resource.values ?? {})),

  check(_app, configs, { refs }) {
    for (const resource of configs) {
      for (const { kind, name } of valueRefs(resource)) {
        refs.secret(kind, name);
      }
    }
  },

  plan({ refs }, configs) {
    const templates = configs.flatMap((resource) =>
      resource.values
        ? [
            {
              secretName: resource.secretName,
              configMap: resource.configMap,
              files: resource.files,
              values: valueRefs(resource).map(({ variable, kind, name, key }) => ({
                name: variable,
                secret: refs.secret(kind, name),
                key,
              })),
            },
          ]
        : [],
    );
    return templates.length ? { secrets: { templates } } : {};
  },

  kustomize: (app, configs) => ({
    generators: configs.map(({ configMap, files }) => ({
      name: configMap,
      namespace: appNamespace(app),
      files,
    })),
  }),

  files: (_app, configs) => configs.flatMap((resource) => resource.files),
};
