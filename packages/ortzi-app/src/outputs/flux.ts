import { stringify } from 'yaml';
import { chartValues, type Plan, type PlanSource } from '../plan.ts';
import type { FluxOverrides } from '../types.ts';
import { HEADER } from '../generated.ts';
import { sourceProvider } from '../providers/index.ts';
import {
  APP_ANNOTATION,
  chartRepository,
  FLUX_NAMESPACE,
  type FluxObject,
  helmRelease,
} from '../flux-objects.ts';

const SCHEMAS = 'https://k8s-schemas.anurag.sh';
const KUSTOMIZE_SCHEMA = 'https://json.schemastore.org/kustomization';

// The bundle source every per-app Kustomization reads, see kubernetes/flux-system/apps.
export const BUNDLE_SOURCE = 'apps';

const FILES: Record<string, string> = {
  OCIRepository: 'ocirepository.yaml',
  HelmRepository: 'helmrepository.yaml',
  GitRepository: 'gitrepository.yaml',
  HelmRelease: 'helmrelease.yaml',
  Kustomization: 'ks.yaml',
};

export const bundlePath = (plan: Plan) => `flux/${plan.location}/${plan.name}`;

function sourceObjects(plan: Plan, source: PlanSource): FluxObject[] {
  if (source.type !== 'chart') {
    return sourceProvider(source.type).flux(plan, source as never);
  }
  if (source.valueFiles) {
    throw new Error(`${plan.name}: flux needs inlined values; set artifact`);
  }

  const name = `${plan.name}-${source.chart}`;
  const values = chartValues(source);
  return [
    chartRepository(plan, name, `${source.registry}/${source.chart}`, source.version),
    helmRelease(
      plan,
      name,
      name,
      { ociRepository: name },
      source.chart === 'k8s-workload' ? { ...values, createNamespace: false } : values,
    ),
  ];
}

function document(objects: Record<string, unknown>[], schema?: string): string {
  const body = objects
    .map((object) => stringify(object, { lineWidth: 0, aliasDuplicateObjects: false }))
    .join('---\n');
  return `${HEADER}\n${schema ? `# yaml-language-server: $schema=${schema}\n` : ''}${body}`;
}

function schemaFor({ apiVersion, kind }: Record<string, unknown>): string {
  const [group, version] = (apiVersion as string).split('/');
  return `${SCHEMAS}/${group}/${(kind as string).toLowerCase()}_${version}.json`;
}

export function renderFlux(plan: Plan, overrides: FluxOverrides = {}): Map<string, string> {
  const objects = plan.sources.flatMap((source) => sourceObjects(plan, source));
  const driftIgnore = overrides.helmRelease?.driftIgnore;
  for (const object of objects) {
    if (object.kind === 'HelmRelease' && driftIgnore) {
      const spec = object.spec as { driftDetection: Record<string, unknown> };
      spec.driftDetection.ignore = driftIgnore;
    }
  }

  const namespace = {
    apiVersion: 'v1',
    kind: 'Namespace',
    metadata: {
      name: plan.namespace,
      ...(plan.namespaceLabels ? { labels: plan.namespaceLabels } : {}),
      annotations: { 'kustomize.toolkit.fluxcd.io/prune': 'disabled' },
    },
  };

  const files = new Map<string, string>([['namespace.yaml', document([namespace])]]);
  for (const [kind, file] of Object.entries(FILES)) {
    const ofKind = objects.filter((object) => object.kind === kind);
    if (ofKind.length) {
      files.set(file, document(ofKind, schemaFor(ofKind[0])));
    }
  }

  const kustomize = {
    apiVersion: 'kustomize.config.k8s.io/v1beta1',
    kind: 'Kustomization',
    resources: [...files.keys()].sort(),
  };

  const { dependsOn, timeout } = overrides.kustomization ?? {};
  const app = {
    apiVersion: 'kustomize.toolkit.fluxcd.io/v1',
    kind: 'Kustomization',
    metadata: {
      name: plan.name,
      namespace: FLUX_NAMESPACE,
      annotations: { [APP_ANNOTATION]: plan.name },
    },
    spec: {
      interval: '1h',
      sourceRef: { kind: 'OCIRepository', name: BUNDLE_SOURCE, namespace: FLUX_NAMESPACE },
      path: `./${bundlePath(plan)}/app`,
      prune: true,
      wait: true,
      timeout: timeout ?? '5m',
      ...(dependsOn ? { dependsOn: dependsOn.map((name) => ({ name })) } : {}),
    },
  };

  const dir = `${plan.directory}/flux`;
  return new Map([
    [`${dir}/ks.yaml`, document([app], schemaFor(app))],
    [`${dir}/app/kustomization.yaml`, document([kustomize], KUSTOMIZE_SCHEMA)],
    ...[...files].map(([file, content]): [string, string] => [`${dir}/app/${file}`, content]),
  ]);
}
