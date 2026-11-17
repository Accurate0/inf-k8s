import type { Schema } from 'jsonschema/lib/index.js';
import { platformUrl } from './platform.ts';
import type { ResourceProvider } from './provider.ts';

export interface Cache {
  name: string;
  type: 'cache';
  instance?: string;
}

declare module './provider.ts' {
  interface ResourceTypes {
    cache: Cache;
  }
}

const schema: Schema = {
  type: 'object',
  additionalProperties: false,
  required: ['name', 'type'],
  properties: {
    name: {
      type: 'string',
    },
    type: {
      enum: ['cache'],
    },
    instance: {
      type: 'string',
      description: 'A cache export in platform/exports. Defaults to cache.',
    },
  },
};

export const cache: ResourceProvider<'cache'> = {
  type: 'cache',
  ref: 'value',
  schema,

  value: (app, resource, world) =>
    platformUrl(
      app,
      resource.name,
      { type: 'cache', name: resource.instance ?? 'cache' },
      world.platform,
    ),

  graph(app, resource, { node, edge }) {
    const instance = resource.instance ?? 'cache';
    edge({ from: app.name, to: node(`cache/${instance}`, `cache ${instance}`, 'platform') });
  },
};
