import { parseDocument } from 'yaml';

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
