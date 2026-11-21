import { parseAllDocuments } from 'yaml';

const FLUX_COMMIT = 'event.toolkit.fluxcd.io/commit';
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
        document.setIn(['metadata', 'annotations', FLUX_COMMIT], revision);
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
