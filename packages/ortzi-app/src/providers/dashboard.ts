import type { Schema } from 'jsonschema/lib/index.js';
import { appNamespace } from '../app.ts';
import type { ResourceProvider } from './provider.ts';

export interface Dashboard {
  name: string;
  type: 'dashboard';
  configMap: string;
  file: string;
}

declare module './provider.ts' {
  interface ResourceTypes {
    dashboard: Dashboard;
  }
}

const schema: Schema = {
  type: 'object',
  additionalProperties: false,
  required: ['name', 'type', 'configMap', 'file'],
  properties: {
    name: {
      type: 'string',
    },
    type: {
      enum: ['dashboard'],
    },
    configMap: {
      type: 'string',
    },
    file: {
      type: 'string',
      description: 'Grafana dashboard json, relative to the app directory.',
    },
  },
};

export const dashboard: ResourceProvider<'dashboard'> = {
  type: 'dashboard',
  schema,

  kustomize: (app, dashboards) => ({
    generators: dashboards.map(({ configMap, file }) => ({
      name: configMap,
      namespace: appNamespace(app),
      files: [file],
      options: { disableNameSuffixHash: true, labels: { 'inf-k8s.net/grafana-dashboard': 'true' } },
    })),
  }),

  files: (_app, dashboards) => dashboards.map((resource) => resource.file),
};
