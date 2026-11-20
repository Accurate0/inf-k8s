import { parseAllDocuments, parseDocument } from 'yaml';

const REVISION = 'inf-k8s.net/revision';

export function stampRevision(
  application: string,
  revision: string,
  images: Record<string, string> = {},
): string {
  const document = parseDocument(application);
  document.setIn(['metadata', 'annotations', REVISION], revision);

  const sources = (document.getIn(['spec', 'sources']) as { items?: unknown[] } | undefined)?.items;
  const index = (sources ?? []).findIndex(
    (source) => (source as { get?: (key: string) => unknown }).get?.('chart') === 'k8s-workload',
  );
  for (const [image, tag] of Object.entries(images)) {
    if (index < 0) {
      throw new Error(`${image} is released but the application has no k8s-workload source`);
    }
    document.setIn(['spec', 'sources', index, 'helm', 'valuesObject', 'images', image], tag);
  }
  return document.toString({ lineWidth: 0 });
}

const FLUX_REVISION = 'event.toolkit.fluxcd.io/revision';
const WORKLOAD_RELEASE = /-k8s-workload$/;

export function stampFlux(
  files: Map<string, string>,
  revision: string,
  images: Record<string, string> = {},
): Map<string, string> {
  let landed = false;
  const stamped = new Map(
    [...files].map(([path, content]) => {
      const documents = parseAllDocuments(content);
      for (const document of documents) {
        const kind = document.get('kind');
        if (kind !== 'Kustomization' && kind !== 'HelmRelease') {
          continue;
        }
        if (!document.getIn(['apiVersion'])?.toString().includes('toolkit.fluxcd.io')) {
          continue;
        }
        document.setIn(['metadata', 'annotations', FLUX_REVISION], revision);
        if (
          kind === 'HelmRelease' &&
          WORKLOAD_RELEASE.test(String(document.getIn(['metadata', 'name'])))
        ) {
          landed = true;
          for (const [image, tag] of Object.entries(images)) {
            document.setIn(['spec', 'values', 'images', image], tag);
          }
        }
      }
      return [path, documents.map((document) => document.toString({ lineWidth: 0 })).join('')];
    }),
  );

  const [image] = Object.keys(images);
  if (image && !landed) {
    throw new Error(`${image} is released but the app has no k8s-workload release`);
  }
  return stamped;
}
