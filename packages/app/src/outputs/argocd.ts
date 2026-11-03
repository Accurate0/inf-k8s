import { stringify } from 'yaml';
import type { Plan, PlanSource } from '../plan.ts';
import { type ArgocdOverrides, HEADER } from '../render.ts';

const CONFIG_REPO = 'https://github.com/Accurate0/inf-k8s.git';
const CHART_REGISTRY = 'oci://ghcr.io/accurate0/charts';
const SCHEMA = 'https://k8s-schemas.anurag.sh/argoproj.io/application_v1alpha1.json';

const RETAIN = { 'argocd.argoproj.io/sync-options': 'Prune=false,Delete=false' };

const valueFiles = (files: string[]) => files.map((file) => `$values/${file}`);

function applicationSource(source: PlanSource): Record<string, unknown> {
  switch (source.type) {
    case 'chart':
      return {
        chart: source.chart,
        repoURL: `${CHART_REGISTRY}/${source.chart}`,
        targetRevision: source.version,
        helm: {
          ...(source.valueFiles ? { valueFiles: valueFiles(source.valueFiles) } : {}),
          valuesObject: {
            ...source.values,
            ...(source.retain ? { retainAnnotations: RETAIN } : {}),
          },
        },
      };
    case 'helm':
      return {
        chart: source.chart,
        repoURL: source.repo,
        targetRevision: source.version,
        helm: {
          ...(source.releaseName ? { releaseName: source.releaseName } : {}),
          valueFiles: valueFiles(source.valueFiles),
        },
      };
    case 'git':
      return {
        repoURL: source.repo,
        ...(source.name ? { name: source.name } : {}),
        path: source.path,
        ...(source.revision ? { targetRevision: source.revision } : {}),
      };
    case 'manifests':
      return { repoURL: CONFIG_REPO, name: 'application', path: source.path };
  }
}

export function renderApplication(plan: Plan, overrides?: ArgocdOverrides): string {
  const usesValues = plan.sources.some((source) => 'valueFiles' in source);
  const application = {
    apiVersion: 'argoproj.io/v1alpha1',
    kind: 'Application',
    metadata: { name: plan.name, namespace: 'argocd' },
    spec: {
      syncPolicy: {
        syncOptions: overrides?.options ?? ['CreateNamespace=true', 'ServerSideApply=true'],
        ...(overrides?.automated === false ? {} : { automated: { prune: true, selfHeal: true } }),
        ...(plan.namespaceLabels && !plan.ownsNamespace
          ? { managedNamespaceMetadata: { labels: plan.namespaceLabels } }
          : {}),
      },
      project: plan.location,
      sources: [
        ...(usesValues ? [{ repoURL: CONFIG_REPO, ref: 'values' }] : []),
        ...plan.sources.map(applicationSource),
      ],
      destination: { server: 'https://kubernetes.default.svc', namespace: plan.namespace },
      ...(overrides?.ignoreDifferences ? { ignoreDifferences: overrides.ignoreDifferences } : {}),
    },
    operation: {
      initiatedBy: { username: 'github-actions' },
      sync: { prune: true, syncStrategy: { hook: {} } },
    },
  };

  return `${HEADER}\n# yaml-language-server: $schema=${SCHEMA}\n${stringify(application, { lineWidth: 0, aliasDuplicateObjects: false })}`;
}
