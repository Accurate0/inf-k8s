import type { Schema } from 'jsonschema/lib/index.js';
import type { App, PlatformExport } from '../types.ts';
import type { ResourceProvider } from './provider.ts';

export interface PlatformResource {
  name: string;
  type: 'platform';
  export: string;
}

declare module './provider.ts' {
  interface ResourceTypes {
    platform: PlatformResource;
  }
}

const schema: Schema = {
  type: 'object',
  additionalProperties: false,
  required: ['name', 'type', 'export'],
  properties: {
    name: {
      type: 'string',
    },
    type: {
      enum: ['platform'],
    },
    export: {
      type: 'string',
      description: 'An export in platform/exports.',
    },
  },
};

export function platformUrl(
  app: App,
  resource: string,
  wanted: { type: string; name: string },
  platform: PlatformExport[],
): string {
  const provided = platform.filter((e) => e.type === wanted.type && e.name === wanted.name);
  if (provided.length !== 1) {
    throw new Error(
      `${app.name}: resources.${resource}: expected one platform ${wanted.type} export named ${wanted.name}, found ${provided.length}`,
    );
  }

  return provided[0].url as string;
}

export const platform: ResourceProvider<'platform'> = {
  type: 'platform',
  ref: 'value',
  schema,

  value: (app, resource, world) =>
    platformUrl(app, resource.name, { type: 'platform', name: resource.export }, world.platform),

  graph(app, resource, { node, edge }) {
    edge({
      from: app.name,
      to: node(`platform/${resource.export}`, resource.export, 'platform'),
    });
  },
};
