import { parseAllDocuments } from 'yaml';

const FLUX_COMMIT = 'event.toolkit.fluxcd.io/commit';
const REQUESTED_AT = 'reconcile.fluxcd.io/requestedAt';

/**
 * Stamps the app's revision on its flux objects: the commit for notifications, and a reconcile
 * request on its sources so a changed manifests artifact is fetched now instead of next interval.
 */
export function stampFlux(files: Map<string, string>, revision: string): Map<string, string> {
  return new Map(
    [...files].map(([path, content]) => {
      const documents = parseAllDocuments(content);
      for (const document of documents) {
        const kind = document.get('kind');
        const flux = document.getIn(['apiVersion'])?.toString().includes('toolkit.fluxcd.io');
        if (!flux) {
          continue;
        }

        if (kind === 'Kustomization' || kind === 'HelmRelease') {
          document.setIn(['metadata', 'annotations', FLUX_COMMIT], revision);
        }
        if (kind === 'OCIRepository') {
          document.setIn(['metadata', 'annotations', REQUESTED_AT], revision);
        }
      }
      return [path, documents.map((document) => document.toString({ lineWidth: 0 })).join('')];
    }),
  );
}
