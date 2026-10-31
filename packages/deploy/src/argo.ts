import type { Kube } from '@ortzi/kit';

const ARGOCD = 'argocd';
const SAVED_AUTOMATED = 'cli.ortzi.cloud/saved-automated';
const MANUAL_SYNC = 'cli.ortzi.cloud/manual-sync';
const DEFAULT_AUTOMATED: Automated = { prune: true, selfHeal: true };

export interface Automated {
  prune?: boolean;
  selfHeal?: boolean;
}

export interface Source {
  repoURL?: string;
  path?: string;
  ref?: string;
  chart?: string;
  helm?: { valueFiles?: string[]; valuesObject?: { image?: string } };
}

export interface Revisions {
  revision?: string;
  revisions?: string[];
}

export interface HistoryEntry extends Revisions {
  deployedAt?: string;
}

export interface Application {
  metadata: { name: string; annotations?: Record<string, string> };
  spec: { source?: Source; sources?: Source[]; syncPolicy?: { automated?: Automated } };
  status?: {
    sync?: Revisions & { status?: string };
    health?: { status?: string };
    resources?: { kind: string; name: string; namespace?: string }[];
    history?: HistoryEntry[];
  };
}

const argocd = (kube: Kube) => kube.namespace(ARGOCD);

export const getApp = (kube: Kube, name: string) => argocd(kube).get<Application>('application', name);

export const listApps = (kube: Kube) => argocd(kube).list<Application>('applications');

function patchApp(kube: Kube, name: string, patch: unknown): void {
  argocd(kube).capture(['patch', 'application', name, '--type', 'merge', '-p', JSON.stringify(patch)]);
}

export function waitForSync(kube: Kube, name: string, minutes: number): boolean {
  return argocd(kube).run(
    [
      'wait',
      `application/${name}`,
      '--for=jsonpath={.status.sync.status}=Synced',
      `--timeout=${minutes * 60}s`,
    ],
    { check: false },
  ).ok;
}

export const isEmergency = (app: Application) =>
  app.metadata.annotations?.[SAVED_AUTOMATED] !== undefined;

export const isManualSync = (app: Application) =>
  app.metadata.annotations?.[MANUAL_SYNC] !== undefined;

export const historyNewestFirst = (app: Application) => [...(app.status?.history ?? [])].reverse();

export function assertChangeable(name: string): void {
  if (name === ARGOCD || name.endsWith('-apps')) {
    throw new Error(`refusing to change ${name}: it manages other applications`);
  }
}

export function setSelfHeal(kube: Kube, name: string, enabled: boolean): void {
  patchApp(kube, name, { spec: { syncPolicy: { automated: { selfHeal: enabled } } } });
}

export function turnAutosyncOff(kube: Kube, app: Application): void {
  const automated = app.spec.syncPolicy?.automated;
  if (!automated) return;
  const annotations = isEmergency(app) ? {} : { [SAVED_AUTOMATED]: JSON.stringify(automated) };
  patchApp(kube, app.metadata.name, {
    metadata: { annotations },
    spec: { syncPolicy: { automated: null } },
  });
}

export function savedAutomated(app: Application): { automated: Automated; saved: boolean } {
  const saved = app.metadata.annotations?.[SAVED_AUTOMATED];
  return saved
    ? { automated: JSON.parse(saved), saved: true }
    : { automated: DEFAULT_AUTOMATED, saved: false };
}

export function restoreAutosync(kube: Kube, name: string, automated: Automated): void {
  patchApp(kube, name, {
    metadata: { annotations: { [SAVED_AUTOMATED]: null } },
    spec: { syncPolicy: { automated } },
  });
}
