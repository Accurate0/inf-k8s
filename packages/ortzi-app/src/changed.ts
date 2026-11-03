const component = (path: string) => path.split('/').slice(0, 2).join('/');

export const parseStatus = (output: string) =>
  output
    .split('\0')
    .filter(Boolean)
    .map((entry) => entry.slice(3));

export function changedScope(changed: string[]): (path: string) => boolean {
  if (changed.some((path) => path.startsWith('charts/'))) return () => true;
  const components = new Set(changed.map(component));
  return (path) => components.has(component(path));
}
