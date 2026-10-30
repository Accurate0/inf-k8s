import { fetchText } from '@ortzi/kit';
import type { Application, Revisions } from './argo';
import { parseKustomizationImages } from './images';

const REPO = 'Accurate0/inf-k8s';

function sourceIndex(app: Application): number {
  return (app.spec.sources ?? []).findIndex((s) => s.path && s.repoURL?.includes(REPO));
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

export function tagsAt(app: Application, revision: string | undefined): Map<string, string> {
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
