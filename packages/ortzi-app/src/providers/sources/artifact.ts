import type { Schema } from 'jsonschema/lib/index.js';
import type { SourceProvider } from '../provider.ts';
import { kustomization, manifestsRepository } from '../../flux-objects.ts';

export interface ArtifactSource {
  type: 'artifact';
  repo: string;
  tag: string;
  path?: string;
  name?: string;
}

declare module '../provider.ts' {
  interface SourceTypes {
    artifact: { source: ArtifactSource; plan: ArtifactSource };
  }
}

const schema: Schema = {
  type: 'object',
  additionalProperties: false,
  required: ['type', 'repo', 'tag'],
  properties: {
    type: {
      enum: ['artifact'],
    },
    repo: {
      type: 'string',
      pattern: '^oci://[^/]+/[^/]+/[^/]+$',
      description:
        'OCI artifact another pipeline publishes, synced by Argo CD as is, e.g. oci://registry.anurag.sh/manifests/<repo>.',
    },
    tag: {
      type: 'string',
      description: 'Tag or digest to sync.',
    },
    path: {
      type: 'string',
      description: 'Directory inside the artifact. Defaults to .',
    },
    name: {
      type: 'string',
      description: 'Argo CD source name.',
    },
  },
};

export const artifact: SourceProvider<'artifact'> = {
  type: 'artifact',
  schema,

  validate({ app, fail }, sources) {
    for (const source of sources) {
      if (source.repo === app.artifact?.repo) {
        fail(`sources: ${source.repo} is the app's own artifact`);
      }
    }
  },

  plan: (_ctx, source) => source,

  application: (_plan, source) => ({
    repoURL: source.repo,
    ...(source.name ? { name: source.name } : {}),
    path: source.path ?? '.',
    targetRevision: source.tag,
  }),

  flux(plan, source) {
    const name = `${plan.name}-${source.name ?? 'artifact'}`;
    return [
      manifestsRepository(plan, name, source.repo, source.tag),
      kustomization(plan, name, { kind: 'OCIRepository', name }, `./${source.path ?? ''}`),
    ];
  },
};
