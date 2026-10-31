import { fetchText } from '@ortzi/kit';
import type { Application, Revisions, Source } from './argo';
import { parseKustomizationImages, parseValuesImages } from './images';

const REPO = 'Accurate0/inf-k8s';

type Entry = Revisions & { sources?: Source[] };

const sourcesOf = (app: Application, entry: Entry | undefined) =>
  entry?.sources ?? app.spec.sources ?? [];

const repoSource = (sources: Source[]) =>
  sources.findIndex((s) => (s.path || s.ref) && s.repoURL?.includes(REPO));

export function gitRevision(app: Application, entry: Entry | undefined): string | undefined {
  const index = repoSource(sourcesOf(app, entry));
  return (index >= 0 ? entry?.revisions?.[index] : undefined) ?? entry?.revision;
}

function workloadTags(sources: Source[], revision: string | undefined): Map<string, string> | undefined {
  const helm = sources.find((s) => s.chart === 'k8s-workload')?.helm;
  if (!helm) return undefined;
  const file = helm.valueFiles?.[0]?.replace('$values/', '');
  if (!file || !revision) return new Map();
  const text = fetchText(`https://raw.githubusercontent.com/${REPO}/${revision}/${file}`);
  return text ? parseValuesImages(text) : new Map();
}

export function tagsAt(app: Application, entry: Entry | undefined): Map<string, string> {
  const sources = sourcesOf(app, entry);
  const revision = gitRevision(app, entry);
  const workload = workloadTags(sources, revision);
  if (workload) return workload;
  const source = sources.find((s) => s.path && s.repoURL?.includes(REPO)) ?? app.spec.source;
  const path = source?.repoURL?.includes(REPO) ? source.path : undefined;
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

export const syncedTags = (app: Application) => tagsAt(app, app.status?.sync);
