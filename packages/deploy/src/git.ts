import { fetchText } from '@ortzi/kit';
import type { Application, Revisions } from './argo';
import { parseKustomizationImages, parseValuesImages } from './images';

const REPO = 'Accurate0/inf-k8s';

function sourceIndex(app: Application): number {
  return (app.spec.sources ?? []).findIndex((s) => (s.path || s.ref) && s.repoURL?.includes(REPO));
}

function sourcePath(app: Application): string | undefined {
  const index = sourceIndex(app);
  if (index >= 0) return app.spec.sources?.[index].path;
  return app.spec.source?.repoURL?.includes(REPO) ? app.spec.source.path : undefined;
}

export function gitRevision(app: Application, entry: Revisions | undefined): string | undefined {
  const index = sourceIndex(app);
  return (index >= 0 ? entry?.revisions?.[index] : undefined) ?? entry?.revision;
}

function workloadTags(app: Application, revision: string | undefined): Map<string, string> | undefined {
  const helm = app.spec.sources?.find((s) => s.chart === 'k8s-workload')?.helm;
  if (!helm) return undefined;
  const file = helm.valueFiles?.[0]?.replace('$values/', '');
  if (!file || !revision) return new Map();
  const text = fetchText(`https://raw.githubusercontent.com/${REPO}/${revision}/${file}`);
  return text ? parseValuesImages(text) : new Map();
}

export function tagsAt(app: Application, revision: string | undefined): Map<string, string> {
  const workload = workloadTags(app, revision);
  if (workload) return workload;
  const path = sourcePath(app);
  if (!path || !revision) return new Map();
  for (const dir of [path, `${path}/manifests`]) {
    const text = fetchText(
      `https://raw.githubusercontent.com/${REPO}/${revision}/${dir}/kustomization.yaml`,
    );
    const images = text ? parseKustomizationImages(text) : new Map<string, string>();
    if (images.size > 0) return images;
  }
  return new Map();
}

export const syncedTags = (app: Application) => tagsAt(app, gitRevision(app, app.status?.sync));
