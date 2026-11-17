import { stringify } from 'yaml';
import type { App, Target } from './types.ts';
import type { Kustomize, ResourceType } from './providers/provider.ts';
import { HEADER } from './generated.ts';
import { declared, sourcesOf } from './app.ts';
import { RESOURCES, sourceProvider } from './providers/index.ts';

export const imageValuesPath = (app: App, target: Target) =>
  `${target.location}/${app.name}/image.values.yaml`;

const kustomizations = (app: App): Kustomize[] =>
  RESOURCES.flatMap((provider) =>
    provider.kustomize
      ? [provider.kustomize(app, declared(app, provider.type as ResourceType))]
      : [],
  );

export const needsKustomization = (app: App) =>
  kustomizations(app).some(
    (kustomize) => kustomize.generators?.length || kustomize.resources?.length,
  );

export const kustomizationPath = (app: App, target: Target) =>
  `${target.location}/${app.name}/kustomization.yaml`;

export function renderKustomization(app: App, target: Target): string | undefined {
  if (!needsKustomization(app)) {
    return undefined;
  }

  const generators = kustomizations(app).flatMap((kustomize) => kustomize.generators ?? []);
  const resources = [
    ...(target.hasManifests ? ['manifests'] : []),
    ...kustomizations(app).flatMap((kustomize) => kustomize.resources ?? []),
  ];

  const kustomization = {
    apiVersion: 'kustomize.config.k8s.io/v1beta1',
    kind: 'Kustomization',
    ...(resources.length ? { resources } : {}),
    ...(generators.length
      ? { configMapGenerator: generators, generatorOptions: { disableNameSuffixHash: true } }
      : {}),
  };

  return `${HEADER}\n${stringify(kustomization, { lineWidth: 0, aliasDuplicateObjects: false })}`;
}

export const appFiles = (app: App, target: Target) =>
  [
    ...RESOURCES.flatMap(
      (provider) => provider.files?.(app, declared(app, provider.type as ResourceType)) ?? [],
    ),
    ...(app.sources ?? []).flatMap(
      (source) => sourceProvider(source.type).files?.(app, source as never) ?? [],
    ),
    ...sourcesOf(app, 'manifests')
      .filter(() => !needsKustomization(app))
      .map(({ path }) => (path === '.' ? 'kustomization.yaml' : `${path}/kustomization.yaml`)),
  ].map((file) => `${target.location}/${app.name}/${file}`);
