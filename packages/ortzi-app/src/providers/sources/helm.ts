import type { Schema } from 'jsonschema/lib/index.js';
import type { SourceProvider } from '../provider.ts';
import { chartRepository, helmRelease, helmRepository } from '../../flux-objects.ts';

export interface HelmSource {
  type: 'helm';
  chart: string;
  repo: string;
  version: string;
  values: string;
  releaseName?: string;
}

export interface ExternalChart {
  type: 'helm';
  chart: string;
  repo: string;
  version: string;
  releaseName?: string;
  valueFiles?: string[];
  values?: Record<string, unknown>;
}

declare module '../provider.ts' {
  interface SourceTypes {
    helm: { source: HelmSource; plan: ExternalChart };
  }
}

const schema: Schema = {
  type: 'object',
  additionalProperties: false,
  required: ['type', 'chart', 'repo', 'version', 'values'],
  properties: {
    type: {
      enum: ['helm'],
    },
    chart: {
      type: 'string',
    },
    repo: {
      type: 'string',
    },
    version: {
      type: 'string',
    },
    values: {
      type: 'string',
      description: 'Values file, relative to the app directory.',
    },
    releaseName: {
      type: 'string',
      description: 'Defaults to the app name. Required when a chart is listed twice.',
    },
  },
};

function chartLocation({ chart, repo, version }: ExternalChart): string[] {
  if (!repo.startsWith('oci://')) {
    return [chart, '--repo', repo, '--version', version];
  }
  return [repo.endsWith(`/${chart}`) ? repo : `${repo}/${chart}`, '--version', version];
}

export const helm: SourceProvider<'helm'> = {
  type: 'helm',
  schema,

  validate({ fail }, sources) {
    const releases = sources.map((release) => release.releaseName ?? release.chart);
    for (const release of releases) {
      if (releases.indexOf(release) !== releases.lastIndexOf(release)) {
        fail(`sources: duplicate helm release ${release}, set releaseName`);
      }
    }
  },

  files: (_app, source) => [source.values],

  plan: ({ app, directory, readValues }, { chart, repo, version, values, releaseName }) => ({
    type: 'helm',
    chart,
    repo,
    version,
    ...(releaseName ? { releaseName } : {}),
    ...(app.artifact
      ? { values: readValues(`${directory}/${values}`) }
      : { valueFiles: [`${directory}/${values}`] }),
  }),

  application: (_plan, source) => ({
    chart: source.chart,
    repoURL: source.repo,
    targetRevision: source.version,
    helm: {
      ...(source.releaseName ? { releaseName: source.releaseName } : {}),
      ...(source.valueFiles
        ? { valueFiles: source.valueFiles.map((file) => `$values/${file}`) }
        : {}),
      ...(source.values ? { valuesObject: source.values } : {}),
    },
  }),

  flux(plan, source) {
    if (source.valueFiles) {
      throw new Error(`${plan.name}: flux needs inlined values; set artifact`);
    }

    const release = source.releaseName ?? plan.name;
    const name = `${plan.name}-${source.releaseName ?? source.chart}`;
    const values = source.values ?? {};
    if (source.repo.startsWith('oci://')) {
      const [url, , version] = chartLocation(source);
      return [
        chartRepository(plan, name, url, version),
        helmRelease(plan, name, release, { ociRepository: name }, values),
      ];
    }

    return [
      helmRepository(plan, name, source.repo),
      helmRelease(
        plan,
        name,
        release,
        { helmRepository: name, chart: source.chart, version: source.version },
        values,
      ),
    ];
  },

  helmTemplate: (plan, source) => ({
    release: source.releaseName ?? plan.name,
    location: chartLocation(source),
    values: source.values ?? {},
  }),
};
