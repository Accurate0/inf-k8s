export interface ImageRef {
  repository: string;
  tag: string;
}

export function parseKustomizationImages(text: string): Map<string, string> {
  const images = new Map<string, string>();
  let inImages = false;
  let name: string | undefined;
  for (const line of text.split('\n')) {
    if (/^images:\s*$/.test(line)) {
      inImages = true;
      continue;
    }
    if (!inImages) continue;
    if (/^\S/.test(line)) break;
    const nameMatch = line.match(/^\s*-\s*name:\s*(\S+)/);
    if (nameMatch) {
      name = nameMatch[1];
      continue;
    }
    const tagMatch = line.match(/^\s*newTag:\s*["']?([^"'\s]+)/);
    if (tagMatch && name) images.set(name, tagMatch[1]);
  }
  return images;
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

export function short(tag: string): string {
  return /^[0-9a-f]{40}$/.test(tag) ? tag.slice(0, 8) : tag;
}

export function previousTags(
  running: Map<string, string>,
  history: Map<string, string>[],
): Map<string, string> | undefined {
  return history.find((tags) =>
    [...running].some(([image, tag]) => tags.has(image) && tags.get(image) !== tag),
  );
}

export function columns(header: string[], rows: string[][]): string {
  const widths = header.map((title, column) =>
    Math.max(title.length, ...rows.map((row) => row[column].length)),
  );
  return [header, ...rows]
    .map((row) => row.map((cell, column) => cell.padEnd(widths[column])).join('  ').trimEnd())
    .join('\n');
}

export function age(from: string, now: number): string {
  const minutes = Math.max(0, Math.floor((now - Date.parse(from)) / 60_000));
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  return hours < 24 ? `${hours}h` : `${Math.floor(hours / 24)}d`;
}
