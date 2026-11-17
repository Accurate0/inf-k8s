import type { Schema } from 'jsonschema/lib/index.js';
import type { SourceProvider } from '../provider.ts';

export interface GitSource {
  type: 'git';
  repo: string;
  path: string;
  name?: string;
  revision?: string;
}

declare module '../provider.ts' {
  interface SourceTypes {
    git: { source: GitSource; plan: GitSource };
  }
}

const schema: Schema = {
  type: 'object',
  additionalProperties: false,
  required: ['type', 'repo', 'path'],
  properties: {
    type: {
      enum: ['git'],
    },
    repo: {
      type: 'string',
    },
    path: {
      type: 'string',
    },
    name: {
      type: 'string',
    },
    revision: {
      type: 'string',
    },
  },
};

export const git: SourceProvider<'git'> = {
  type: 'git',
  schema,

  plan: (_ctx, source) => source,

  application: (_plan, source) => ({
    repoURL: source.repo,
    ...(source.name ? { name: source.name } : {}),
    path: source.path,
    ...(source.revision ? { targetRevision: source.revision } : {}),
  }),
};
