import type { Schema } from 'jsonschema/lib/index.js';
import type { SourceProvider } from '../provider.ts';
import { gitRepository, kustomization } from '../../flux-objects.ts';

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

  flux(plan, source) {
    const name = `${plan.name}-${source.name ?? 'git'}`;
    return [
      gitRepository(plan, name, source.repo, source.revision),
      kustomization(plan, name, { kind: 'GitRepository', name }, `./${source.path}`),
    ];
  },
};
