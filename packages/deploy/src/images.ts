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

export const parseValuesTag = (text: string): string | undefined =>
  text.match(/^tag:\s*["']?([^"'\s]+)/m)?.[1];

export function previousTags(
  running: Map<string, string>,
  history: Map<string, string>[],
): Map<string, string> | undefined {
  return history.find((tags) =>
    [...running].some(([image, tag]) => tags.has(image) && tags.get(image) !== tag),
  );
}
