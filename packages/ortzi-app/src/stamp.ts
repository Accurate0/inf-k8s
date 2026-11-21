import { parseAllDocuments } from 'yaml';

const FLUX_COMMIT = 'event.toolkit.fluxcd.io/commit';

export function stampFlux(files: Map<string, string>, revision: string): Map<string, string> {
  return new Map(
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
      }
      return [path, documents.map((document) => document.toString({ lineWidth: 0 })).join('')];
    }),
  );
}
