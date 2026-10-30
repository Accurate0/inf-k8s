import { fetchHeaders, fetchText } from './http';
import { parseDigest } from './image';

const MANIFEST_TYPES = [
  'application/vnd.oci.image.index.v1+json',
  'application/vnd.oci.image.manifest.v1+json',
  'application/vnd.docker.distribution.manifest.list.v2+json',
  'application/vnd.docker.distribution.manifest.v2+json',
].join(', ');

export function ghcrDigest(repository: string, tag: string): string | undefined {
  const path = repository.replace(/^ghcr\.io\//, '');
  if (path === repository) return undefined;
  const token = fetchText(`https://ghcr.io/token?scope=repository:${path}:pull`);
  if (!token) return undefined;
  const headers = fetchHeaders(`https://ghcr.io/v2/${path}/manifests/${tag}`, {
    headers: {
      Authorization: `Bearer ${JSON.parse(token).token}`,
      Accept: MANIFEST_TYPES,
    },
  });
  return headers ? parseDigest(headers) : undefined;
}
