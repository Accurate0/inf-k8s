import type { Schema } from 'jsonschema/lib/index.js';
import type { SourceProvider } from '../provider.ts';
import { kustomization, manifestsRepository } from '../../flux-objects.ts';

export interface ManifestsSource {
  type: 'manifests';
  path: string;
}

declare module '../provider.ts' {
  interface SourceTypes {
    manifests: { source: ManifestsSource; plan: ManifestsSource };
  }
}

const schema: Schema = {
  type: 'object',
  additionalProperties: false,
  required: ['type', 'path'],
  properties: {
    type: {
      enum: ['manifests'],
    },
    path: {
      type: 'string',
      description:
        'Kustomization directory, relative to the app directory. Use . for the app directory.',
    },
  },
};

export const CONFIG_REPO = 'https://github.com/Accurate0/inf-k8s.git';

export const manifests: SourceProvider<'manifests'> = {
  type: 'manifests',
  schema,

  validate({ kustomized, fail }, sources) {
    if (sources.length > 1) {
      fail('sources: only one manifests source is allowed');
    }
    if (sources[0]?.path.split('/').includes('..')) {
      fail('sources: manifests path must stay inside the app directory');
    }
    if (sources[0] && kustomized && sources[0].path !== '.') {
      fail('sources: manifests path must be . with config, dashboard or clusterRules');
    }
  },

  plan: ({ directory }, source) => ({
    type: 'manifests',
    path: source.path === '.' ? directory : `${directory}/${source.path}`,
  }),

  application(plan, source) {
    if (plan.artifact) {
      return {
        repoURL: plan.artifact.repo,
        name: 'application',
        path: '.',
        targetRevision: plan.artifact.tag,
      };
    }

    return { repoURL: CONFIG_REPO, name: 'application', path: source.path };
  },

  flux(plan) {
    if (!plan.artifact) {
      throw new Error(`${plan.name}: flux reads manifests from the artifact; set artifact`);
    }

    const name = `${plan.name}-manifests`;
    return [
      manifestsRepository(plan, name, plan.artifact.repo, plan.artifact.tag),
      kustomization(plan, name, { kind: 'OCIRepository', name }, './'),
    ];
  },
};
