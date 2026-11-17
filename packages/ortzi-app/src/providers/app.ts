import type { Schema } from 'jsonschema/lib/index.js';
import type { App } from '../types.ts';
import { appNamespace, declared } from '../app.ts';
import { exportPort, serviceUrl } from '../services.ts';
import { appWorkloads } from './workload.ts';
import type { ResourceProvider } from './provider.ts';

export interface AppResource {
  name: string;
  type: 'app';
  app: string;
  export: string;
  path?: string;
}

declare module './provider.ts' {
  interface ResourceTypes {
    app: AppResource;
  }
}

const schema: Schema = {
  type: 'object',
  additionalProperties: false,
  required: ['name', 'type', 'app', 'export'],
  properties: {
    name: {
      type: 'string',
    },
    type: {
      enum: ['app'],
    },
    app: {
      type: 'string',
    },
    export: {
      type: 'string',
    },
    path: {
      type: 'string',
      description: 'Appended to the exported URL.',
    },
  },
};

export function appExportUrl(app: App, resource: AppResource, apps: App[]): string {
  const provider = apps.find((a) => a.name === resource.app);
  if (!provider) {
    throw new Error(`${app.name}: resources.${resource.name}: no app named ${resource.app}`);
  }

  const exported = provider.exports?.find((e) => e.name === resource.export);
  if (!exported) {
    throw new Error(
      `${app.name}: resources.${resource.name}: ${resource.app} has no export named ${resource.export}`,
    );
  }

  const service = appWorkloads(provider)[exported.workload]?.service?.name ?? exported.workload;
  return serviceUrl(service, provider.name, exportPort(provider, exported), resource.path);
}

export const app: ResourceProvider<'app'> = {
  type: 'app',
  ref: 'value',
  schema,

  value: (consumer, resource, world) => appExportUrl(consumer, resource, world.apps),

  check(consumer, resources, { world }) {
    for (const resource of resources) {
      appExportUrl(consumer, resource, world.apps);
    }
  },

  plan: ({ app: provider, world }) => ({
    appIngress: world.apps
      .filter(
        (other) =>
          other.name !== provider.name &&
          declared(other, 'app').some((resource) => resource.app === provider.name),
      )
      .map(appNamespace)
      .sort(),
  }),

  graph(consumer, resource, { edge }) {
    edge({ from: consumer.name, to: resource.app, label: resource.export });
  },
};
