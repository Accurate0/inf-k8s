import { Document, isScalar, parse, stringify } from 'yaml';
import type { App, Target } from './types.ts';
import {
  IMAGE_VALUES,
  imageSetter,
  imageValuesPath as clusterImageValuesPath,
} from './flux-objects.ts';
import { releaseImages, workloadImages } from './providers/workload.ts';
import type { Kustomize, ResourceType } from './providers/provider.ts';
import { HEADER } from './generated.ts';
import { appNamespace, declared, sourcesOf } from './app.ts';
import { RESOURCES, sourceProvider } from './providers/index.ts';

export const imageValuesPath = (app: App) =>
  clusterImageValuesPath({ name: app.name, namespace: appNamespace(app) });

export const legacyImageValuesPath = (app: App, target: Target) =>
  `${target.location}/${app.name}/${IMAGE_VALUES}`;

// Tags live here; released images carry the setter flux's ImageUpdateAutomation rewrites.
export function renderImageValues(app: App, current: string | undefined): string {
  const existing: Record<string, string> = (current ? parse(current) : {})?.images ?? {};
  const images = { ...existing };
  for (const image of workloadImages(app)) {
    images[image] ??= 'latest';
  }

  const document = new Document({ images });
  for (const image of releaseImages(app)) {
    const node = document.getIn(['images', image], true);
    if (isScalar(node)) {
      node.comment = ` ${imageSetter(app, image)}`;
    }
  }
  return document.toString({ lineWidth: 0 });
}

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
