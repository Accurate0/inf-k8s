import { stringify } from 'yaml';
import { chartValues, type Plan, type PlanSource, type PlatformChart } from '../plan.ts';
import type { FluxOverrides } from '../types.ts';
import { HEADER } from '../generated.ts';
import { sourceProvider } from '../providers/index.ts';
import {
  APP_ANNOTATION,
  appPath,
  chartRepository,
  FLUX_NAMESPACE,
  fluxDir,
  type FluxObject,
  helmRelease,
  IMAGE_VALUES,
  imagePolicy,
  imageRepository,
} from '../flux-objects.ts';

const SCHEMAS = 'https://k8s-schemas.anurag.sh';
const KUSTOMIZE_SCHEMA = 'https://json.schemastore.org/kustomization';

// The cluster artifact FluxInstance syncs; every per-app Kustomization reads it too.
const CLUSTER_SOURCE = 'flux-system';

const PRUNE_DISABLED = { 'kustomize.toolkit.fluxcd.io/prune': 'disabled' };

const FILES: Record<string, string> = {
  OCIRepository: 'ocirepository.yaml',
  HelmRepository: 'helmrepository.yaml',
  GitRepository: 'gitrepository.yaml',
  HelmRelease: 'helmrelease.yaml',
  Kustomization: 'ks.yaml',
  ImageRepository: 'imagerepository.yaml',
  ImagePolicy: 'imagepolicy.yaml',
};

// A chart an app uses more than once gets one release each, told apart by its values' name.
function chartName(plan: Plan, source: PlatformChart, index: number): string {
  const name = `${plan.name}-${source.chart}`;
  const twins = plan.sources.filter((s) => s.type === 'chart' && s.chart === source.chart);
  if (twins.length === 1) {
    return name;
  }
  const label = source.values.name;
  return `${name}-${typeof label === 'string' ? label : twins.indexOf(source) + 1 || index}`;
}

function sourceObjects(plan: Plan, source: PlanSource, index: number): FluxObject[] {
  if (source.type !== 'chart') {
    return sourceProvider(source.type).flux(plan, source as never);
  }

  const name = chartName(plan, source, index);
  const values = chartValues(source);
  return [
    chartRepository(plan, name, `${source.registry}/${source.chart}`, source.version),
    helmRelease(
      plan,
      name,
      name,
      { ociRepository: name },
      source.chart === 'k8s-workload' ? { ...values, createNamespace: false } : values,
      source.valuesFrom?.configMap,
    ),
  ];
}

export function document(objects: Record<string, unknown>[], schema?: string): string {
  const body = objects
    .map((object) => stringify(object, { lineWidth: 0, aliasDuplicateObjects: false }))
    .join('---\n');
  return `${HEADER}\n${schema ? `# yaml-language-server: $schema=${schema}\n` : ''}${body}`;
}

function schemaFor({ apiVersion, kind }: Record<string, unknown>): string {
  const [group, version] = (apiVersion as string).split('/');
  return `${SCHEMAS}/${group}/${(kind as string).toLowerCase()}_${version}.json`;
}

export const namespaceDocument = (name: string, labels?: Record<string, string>) =>
  document([
    {
      apiVersion: 'v1',
      kind: 'Namespace',
      metadata: { name, ...(labels ? { labels } : {}), annotations: PRUNE_DISABLED },
    },
  ]);

export const kustomizationDocument = (resources: string[], configMaps: string[] = []) =>
  document(
    [
      {
        apiVersion: 'kustomize.config.k8s.io/v1beta1',
        kind: 'Kustomization',
        resources,
        ...(configMaps.length
          ? {
              configMapGenerator: configMaps.map((name) => ({
                name,
                namespace: FLUX_NAMESPACE,
                files: [`values.yaml=${IMAGE_VALUES}`],
                options: {
                  disableNameSuffixHash: true,
                  labels: { 'reconcile.fluxcd.io/watch': 'Enabled' },
                },
              })),
            }
          : {}),
      },
    ],
    KUSTOMIZE_SCHEMA,
  );

export function renderFlux(plan: Plan, overrides: FluxOverrides = {}): Map<string, string> {
  const objects = [
    ...plan.sources.flatMap((source, index) => sourceObjects(plan, source, index)),
    ...(plan.releases ?? []).flatMap((image) => [
      imageRepository(plan, image),
      imagePolicy(plan, image),
    ]),
  ];
  const driftIgnore = overrides.helmRelease?.driftIgnore;
  for (const object of objects) {
    if (object.kind === 'HelmRelease' && driftIgnore) {
      const spec = object.spec as { driftDetection: Record<string, unknown> };
      spec.driftDetection.ignore = driftIgnore;
    }
  }

  const files = new Map<string, string>();
  for (const [kind, file] of Object.entries(FILES)) {
    const ofKind = objects.filter((object) => object.kind === kind);
    if (ofKind.length) {
      files.set(file, document(ofKind, schemaFor(ofKind[0])));
    }
  }

  const { dependsOn, timeout } = overrides.kustomization ?? {};
  const app = {
    apiVersion: 'kustomize.toolkit.fluxcd.io/v1',
    kind: 'Kustomization',
    metadata: {
      name: plan.name,
      namespace: FLUX_NAMESPACE,
      annotations: { [APP_ANNOTATION]: plan.name, ...PRUNE_DISABLED },
    },
    spec: {
      interval: '1h',
      sourceRef: { kind: 'OCIRepository', name: CLUSTER_SOURCE, namespace: FLUX_NAMESPACE },
      path: `./${appPath(plan)}/app`,
      prune: true,
      wait: true,
      timeout: timeout ?? '5m',
      ...(dependsOn ? { dependsOn: dependsOn.map((name) => ({ name })) } : {}),
    },
  };

  const configMaps = plan.sources.flatMap((source) =>
    source.type === 'chart' && source.valuesFrom ? [source.valuesFrom.configMap] : [],
  );

  const dir = fluxDir(plan);
  return new Map([
    [`${dir}/ks.yaml`, document([app], schemaFor(app))],
    [`${dir}/app/kustomization.yaml`, kustomizationDocument([...files.keys()].sort(), configMaps)],
    ...[...files].map(([file, content]): [string, string] => [`${dir}/app/${file}`, content]),
  ]);
}
