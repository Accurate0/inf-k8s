import type { Plan } from './providers/provider.ts';

export type FluxObject = Record<string, unknown> & { kind: string };

export const FLUX_NAMESPACE = 'flux-system';
export const APP_ANNOTATION = 'event.toolkit.fluxcd.io/app';
export const COMMIT_ANNOTATION = 'event.toolkit.fluxcd.io/commit';

export const CLUSTER_ROOT = 'kubernetes';
export const IMAGE_VALUES = 'image.values.yaml';

// kubernetes/<namespace>/<app>, collapsed to kubernetes/<app> when the app owns a namespace of its name
export const appPath = (plan: Pick<Plan, 'name' | 'namespace'>) =>
  plan.namespace === plan.name ? plan.name : `${plan.namespace}/${plan.name}`;

export const fluxDir = (plan: Pick<Plan, 'name' | 'namespace'>) =>
  `${CLUSTER_ROOT}/${appPath(plan)}`;

// Tags live beside the app's flux objects so an image bump is the whole deploy.
export const imageValuesPath = (plan: Pick<Plan, 'name' | 'namespace'>) =>
  `${fluxDir(plan)}/app/${IMAGE_VALUES}`;

export const imageValuesConfigMap = (name: string) => `${name}-image-values`;

const HARBOR = 'oci://registry.anurag.sh/';
const HELM_LAYER = 'application/vnd.cncf.helm.chart.content.v1.tar+gzip';
const MANIFESTS_LAYER = 'application/vnd.oci.image.layer.v1.tar+gzip';

const metadata = (plan: Plan, name: string) => ({
  name,
  namespace: FLUX_NAMESPACE,
  annotations: { [APP_ANNOTATION]: plan.name },
});

const ref = (kind: string, name: string) => ({ kind, name, namespace: FLUX_NAMESPACE });

const harbor = (url: string, verify: boolean) =>
  url.startsWith(HARBOR)
    ? {
        secretRef: { name: 'harbor' },
        ...(verify ? { verify: { provider: 'cosign', secretRef: { name: 'cosign' } } } : {}),
      }
    : {};

export const chartRepository = (plan: Plan, name: string, url: string, tag: string) => ({
  apiVersion: 'source.toolkit.fluxcd.io/v1',
  kind: 'OCIRepository',
  metadata: metadata(plan, name),
  spec: {
    interval: '1h',
    url,
    ref: { tag },
    layerSelector: { mediaType: HELM_LAYER, operation: 'copy' },
    ...harbor(url, false),
  },
});

export const manifestsRepository = (plan: Plan, name: string, url: string, tag: string) => ({
  apiVersion: 'source.toolkit.fluxcd.io/v1',
  kind: 'OCIRepository',
  metadata: metadata(plan, name),
  spec: {
    interval: '1h',
    url,
    ref: { tag },
    layerSelector: { mediaType: MANIFESTS_LAYER, operation: 'extract' },
    ...harbor(url, true),
  },
});

export const helmRepository = (plan: Plan, name: string, url: string) => ({
  apiVersion: 'source.toolkit.fluxcd.io/v1',
  kind: 'HelmRepository',
  metadata: metadata(plan, name),
  spec: { interval: '1h', url },
});

export const gitRepository = (plan: Plan, name: string, url: string, revision?: string) => ({
  apiVersion: 'source.toolkit.fluxcd.io/v1',
  kind: 'GitRepository',
  metadata: metadata(plan, name),
  spec: {
    interval: '1h',
    url,
    ref:
      revision && /^[0-9a-f]{40}$/.test(revision)
        ? { commit: revision }
        : { branch: revision ?? 'main' },
  },
});

export const helmRelease = (
  plan: Plan,
  name: string,
  release: string,
  chart: { ociRepository: string } | { helmRepository: string; chart: string; version: string },
  values: Record<string, unknown>,
  valuesFrom?: string,
) => ({
  apiVersion: 'helm.toolkit.fluxcd.io/v2',
  kind: 'HelmRelease',
  metadata: metadata(plan, name),
  spec: {
    interval: '1h',
    releaseName: release,
    targetNamespace: plan.namespace,
    storageNamespace: plan.namespace,
    ...('ociRepository' in chart
      ? { chartRef: ref('OCIRepository', chart.ociRepository) }
      : {
          chart: {
            spec: {
              chart: chart.chart,
              version: chart.version,
              sourceRef: ref('HelmRepository', chart.helmRepository),
            },
          },
        }),
    maxHistory: 3,
    install: { strategy: { name: 'RetryOnFailure' } },
    upgrade: {
      cleanupOnFail: true,
      strategy: { name: 'RemediateOnFailure' },
      remediation: { retries: 2 },
    },
    rollback: { cleanupOnFail: true },
    driftDetection: { mode: 'enabled' },
    ...(valuesFrom
      ? { valuesFrom: [{ kind: 'ConfigMap', name: valuesFrom, valuesKey: 'values.yaml' }] }
      : {}),
    values,
  },
});

export const kustomization = (
  plan: Plan,
  name: string,
  source: { kind: string; name: string },
  path: string,
) => ({
  apiVersion: 'kustomize.toolkit.fluxcd.io/v1',
  kind: 'Kustomization',
  metadata: metadata(plan, name),
  spec: {
    interval: '1h',
    sourceRef: ref(source.kind, source.name),
    path,
    prune: true,
    wait: true,
    timeout: '5m',
    targetNamespace: plan.namespace,
  },
});

// CI tags every main build main-<commit time>-<sha>, so the newest commit sorts last.
export const RELEASE_TAG = '^main-(?P<ts>[0-9]+)-[0-9a-f]{40}$';

export const imagePolicyName = (plan: Pick<Plan, 'name'>, image: string) =>
  `${plan.name}-${image.split('/').pop()}`;

export const imageSetter = (plan: Pick<Plan, 'name'>, image: string) =>
  `{"$imagepolicy": "${FLUX_NAMESPACE}:${imagePolicyName(plan, image)}:tag"}`;

export const imageRepository = (plan: Plan, image: string) => ({
  apiVersion: 'image.toolkit.fluxcd.io/v1',
  kind: 'ImageRepository',
  metadata: metadata(plan, imagePolicyName(plan, image)),
  spec: {
    interval: '10m',
    image,
    secretRef: { name: 'harbor' },
  },
});

export const imagePolicy = (plan: Plan, image: string) => ({
  apiVersion: 'image.toolkit.fluxcd.io/v1',
  kind: 'ImagePolicy',
  metadata: metadata(plan, imagePolicyName(plan, image)),
  spec: {
    imageRepositoryRef: { name: imagePolicyName(plan, image) },
    filterTags: { pattern: RELEASE_TAG, extract: '$ts' },
    policy: { numerical: { order: 'asc' } },
  },
});
