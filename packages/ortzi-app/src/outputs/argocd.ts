import { stringify } from 'yaml';
import { chartValues, type Plan, type PlanSource } from '../plan.ts';
import { type ArgocdOverrides, HEADER } from '../render.ts';

const CONFIG_REPO = 'https://github.com/Accurate0/inf-k8s.git';
const SCHEMA = 'https://k8s-schemas.anurag.sh/argoproj.io/application_v1alpha1.json';

const valueFiles = (files: string[]) => files.map((file) => `$values/${file}`);

function applicationSource(plan: Plan, source: PlanSource): Record<string, unknown> {
  switch (source.type) {
    case 'chart':
      return {
        chart: source.chart,
        repoURL: `${source.registry}/${source.chart}`,
        targetRevision: source.version,
        helm: {
          ...(source.valueFiles ? { valueFiles: valueFiles(source.valueFiles) } : {}),
          valuesObject: chartValues(source),
        },
      };
    case 'helm':
      return {
        chart: source.chart,
        repoURL: source.repo,
        targetRevision: source.version,
        helm: {
          ...(source.releaseName ? { releaseName: source.releaseName } : {}),
          ...(source.valueFiles ? { valueFiles: valueFiles(source.valueFiles) } : {}),
          ...(source.values ? { valuesObject: source.values } : {}),
        },
      };
    case 'git':
      return {
        repoURL: source.repo,
        ...(source.name ? { name: source.name } : {}),
        path: source.path,
        ...(source.revision ? { targetRevision: source.revision } : {}),
      };
    case 'artifact':
      return {
        repoURL: source.repo,
        ...(source.name ? { name: source.name } : {}),
        path: source.path ?? '.',
        targetRevision: source.tag,
      };
    case 'manifests':
      if (plan.artifact) {
        return {
          repoURL: plan.artifact.repo,
          name: 'application',
          path: '.',
          targetRevision: plan.artifact.tag,
        };
      }

      return { repoURL: CONFIG_REPO, name: 'application', path: source.path };
  }
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
