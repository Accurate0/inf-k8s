import { stringify } from 'yaml';
import { chartValues, type Plan, type PlanSource } from '../plan.ts';
import type { ArgocdOverrides } from '../types.ts';
import { HEADER } from '../generated.ts';
import { sourceProvider } from '../providers/index.ts';
import { CONFIG_REPO } from '../providers/sources/manifests.ts';

const SCHEMA = 'https://k8s-schemas.anurag.sh/argoproj.io/application_v1alpha1.json';

function applicationSource(plan: Plan, source: PlanSource): Record<string, unknown> {
  if (source.type !== 'chart') {
    return sourceProvider(source.type).application(plan, source as never);
  }

  return {
    chart: source.chart,
    repoURL: `${source.registry}/${source.chart}`,
    targetRevision: source.version,
    helm: {
      ...(source.valueFiles
        ? { valueFiles: source.valueFiles.map((file) => `$values/${file}`) }
        : {}),
      valuesObject: chartValues(source),
    },
  };
}

export function renderApplication(plan: Plan, overrides?: ArgocdOverrides): string {
  const usesValues = plan.sources.some((source) => 'valueFiles' in source && source.valueFiles);
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
        ...plan.sources.map((source) => applicationSource(plan, source)),
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
