import type { Plan } from '../plan.ts';
import { CLUSTER_ROOT, kustomizationDocument, namespaceDocument } from './flux.ts';

export interface ClusterTree {
  namespaces: string[];
  // hand-written <namespace>/<component>/ks.yaml, relative to kubernetes/
  components: string[];
}

export function renderFluxIndex(plans: Plan[], tree: ClusterTree): Map<string, string> {
  const byNamespace = new Map<string, Plan[]>();
  for (const plan of plans) {
    byNamespace.set(plan.namespace, [...(byNamespace.get(plan.namespace) ?? []), plan]);
  }

  const namespaces = new Set([...tree.namespaces, ...byNamespace.keys()]);
  const files = new Map([
    [`${CLUSTER_ROOT}/kustomization.yaml`, kustomizationDocument([...namespaces].sort())],
  ]);

  for (const [namespace, members] of byNamespace) {
    const labels: Record<string, string> = {};
    for (const [key, value] of members.flatMap((plan) =>
      Object.entries(plan.namespaceLabels ?? {}),
    )) {
      if (labels[key] !== undefined && labels[key] !== value) {
        throw new Error(`namespace ${namespace}: apps disagree on label ${key}`);
      }
      labels[key] = value;
    }

    const children = new Set([
      ...members.map((plan) => (plan.name === namespace ? 'ks.yaml' : `${plan.name}/ks.yaml`)),
      ...tree.components
        .filter((path) => path.startsWith(`${namespace}/`))
        .map((path) => path.slice(namespace.length + 1)),
    ]);

    const dir = `${CLUSTER_ROOT}/${namespace}`;
    files.set(
      `${dir}/namespace.yaml`,
      namespaceDocument(namespace, Object.keys(labels).length ? labels : undefined),
    );
    files.set(
      `${dir}/kustomization.yaml`,
      kustomizationDocument(['namespace.yaml', ...[...children].sort()]),
    );
  }

  return files;
}
