import type { Kube } from '@ortzi/kit';
import { parse } from 'yaml';
import {
  APP_ANNOTATION,
  FLUX_NAMESPACE,
  COMMIT_ANNOTATION,
  imageValuesConfigMap,
} from './flux-objects';

const MANUAL_SYNC = 'cli.ortzi.cloud/manual-sync';

interface Condition {
  type: string;
  status: string;
  message?: string;
  lastTransitionTime?: string;
}

export interface FluxObject {
  metadata: {
    name: string;
    generation?: number;
    resourceVersion?: string;
    annotations?: Record<string, string>;
  };
  spec: {
    suspend?: boolean;
    releaseName?: string;
    targetNamespace?: string;
    values?: { images?: Record<string, string> };
    driftDetection?: { mode?: string };
  };
  status?: {
    observedGeneration?: number;
    conditions?: Condition[];
    lastAppliedRevision?: string;
    history?: {
      chartName?: string;
      chartVersion?: string;
      version?: number;
      status?: string;
      firstDeployed?: string;
      lastDeployed?: string;
    }[];
  };
}

export interface App {
  name: string;
  kustomization: FluxObject;
  // the app's own nested kustomizations, e.g. <app>-manifests
  kustomizations: FluxObject[];
  releases: FluxObject[];
  images?: Record<string, string>;
}

interface ConfigMap {
  metadata: { name: string };
  data?: Record<string, string>;
}

const flux = (kube: Kube) => kube.namespace(FLUX_NAMESPACE);

const belongs = (name: string) => (object: FluxObject) =>
  object.metadata.annotations?.[APP_ANNOTATION] === name;

function imagesOf(name: string, configMaps: ConfigMap[]): Record<string, string> | undefined {
  const values = configMaps.find((cm) => cm.metadata.name === imageValuesConfigMap(name))?.data?.[
    'values.yaml'
  ];
  return values === undefined ? undefined : (parse(values)?.images ?? {});
}

function assemble(
  name: string,
  kustomizations: FluxObject[],
  releases: FluxObject[],
  configMaps: ConfigMap[],
): App {
  const own = kustomizations.filter(belongs(name));
  const kustomization = own.find((ks) => ks.metadata.name === name);
  if (!kustomization) {
    throw new Error(`no flux kustomization named ${name}`);
  }
  return {
    name,
    kustomization,
    kustomizations: own.filter((ks) => ks !== kustomization),
    releases: releases.filter(belongs(name)),
    images: imagesOf(name, configMaps),
  };
}

export function getApp(kube: Kube, name: string): App {
  return assemble(
    name,
    flux(kube).list<FluxObject>('kustomizations'),
    flux(kube).list<FluxObject>('helmreleases'),
    flux(kube).list<ConfigMap>('configmaps'),
  );
}

export function listApps(kube: Kube): App[] {
  const kustomizations = flux(kube).list<FluxObject>('kustomizations');
  const releases = flux(kube).list<FluxObject>('helmreleases');
  const configMaps = flux(kube).list<ConfigMap>('configmaps');
  return kustomizations
    .filter((ks) => ks.metadata.annotations?.[APP_ANNOTATION] === ks.metadata.name)
    .map((ks) => assemble(ks.metadata.name, kustomizations, releases, configMaps));
}

export const objectsOf = (app: App) => [app.kustomization, ...app.kustomizations, ...app.releases];

export function ready(object: FluxObject): boolean {
  const condition = object.status?.conditions?.find((c) => c.type === 'Ready');
  return (
    object.status?.observedGeneration === object.metadata.generation && condition?.status === 'True'
  );
}

export const readyMessage = (object: FluxObject) =>
  object.status?.conditions?.find((c) => c.type === 'Ready')?.message ?? 'not reconciled yet';

export const appReady = (app: App) => objectsOf(app).every(ready);

export const suspended = (app: App) => app.kustomization.spec.suspend === true;

export const isManualSync = (app: App) =>
  app.kustomization.metadata.annotations?.[MANUAL_SYNC] !== undefined;

export const driftCorrected = (app: App) =>
  app.releases.every((release) => release.spec.driftDetection?.mode !== 'warn');

export const revision = (app: App) =>
  app.kustomization.metadata.annotations?.[COMMIT_ANNOTATION] ??
  app.kustomization.status?.lastAppliedRevision;

export const workloadRelease = (app: App) =>
  app.releases.find((release) => release.metadata.name.endsWith('-k8s-workload'));

// What the app's image values ConfigMap asks k8s-workload to run; releases rendered before it inline them.
export const desiredTags = (app: App) =>
  new Map(Object.entries(app.images ?? workloadRelease(app)?.spec.values?.images ?? {}));

export function lastDeployed(app: App): string | undefined {
  return app.releases
    .map((release) => release.status?.history?.[0]?.lastDeployed)
    .filter((at): at is string => Boolean(at))
    .sort()
    .at(-1);
}

export function assertChangeable(name: string): void {
  if (name === FLUX_NAMESPACE) {
    throw new Error(`refusing to change ${name}: it manages every other app`);
  }
}

export function setSuspended(kube: Kube, app: App, suspend: boolean): void {
  for (const ks of [app.kustomization, ...app.kustomizations]) {
    flux(kube).patch('kustomization', ks.metadata.name, { spec: { suspend } });
  }
  for (const release of app.releases) {
    flux(kube).patch('helmrelease', release.metadata.name, { spec: { suspend } });
  }
}

// The app kustomization re-applies driftDetection from git, so it stays suspended while healing is off.
export function setDriftCorrection(kube: Kube, app: App, enabled: boolean): void {
  flux(kube).patch('kustomization', app.name, { spec: { suspend: !enabled } });
  for (const release of app.releases) {
    flux(kube).patch('helmrelease', release.metadata.name, {
      spec: { driftDetection: { mode: enabled ? 'enabled' : 'warn' } },
    });
  }
}

export function requestReconcile(kube: Kube, app: App): void {
  const requestedAt = { 'reconcile.fluxcd.io/requestedAt': new Date().toISOString() };
  flux(kube).patch('kustomization', app.name, { metadata: { annotations: requestedAt } });
}

export function waitForReady(kube: Kube, app: App, minutes: number): boolean {
  let ok = true;
  for (const ks of [app.kustomization, ...app.kustomizations]) {
    ok = flux(kube).waitFor<FluxObject>('kustomization', ks.metadata.name, ready, minutes) && ok;
  }
  for (const release of app.releases) {
    ok = flux(kube).waitFor<FluxObject>('helmrelease', release.metadata.name, ready, minutes) && ok;
  }
  return ok;
}
