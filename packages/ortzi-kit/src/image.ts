export interface ImageRef {
  repository: string;
  tag: string;
}

export function parseImage(image: string): ImageRef {
  const bare = image.split('@')[0];
  const slash = bare.lastIndexOf('/');
  const colon = bare.lastIndexOf(':');
  if (colon > slash) {
    return { repository: bare.slice(0, colon), tag: bare.slice(colon + 1) };
  }
  return { repository: bare, tag: 'latest' };
}

export function parseDigest(headers: string): string | undefined {
  return headers.match(/^docker-content-digest:\s*(\S+)/im)?.[1];
}
