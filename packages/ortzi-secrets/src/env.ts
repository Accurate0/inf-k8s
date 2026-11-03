export type Data = Record<string, unknown>;

export const isName = (key: string) => /^[A-Za-z_][A-Za-z0-9_]*$/.test(key);

export const quote = (value: string) => `'${value.replaceAll("'", `'\\''`)}'`;

export const display = (value: unknown) =>
  typeof value === 'string' ? value : JSON.stringify(value);

export const exports = (vars: Map<string, string>) =>
  [...vars]
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([k, v]) => `export ${k}=${quote(v)}\n`)
    .join('');

export function collect(secrets: [string, Data][], prefix: string): Map<string, string> {
  const vars = new Map<string, string>();
  for (const [path, data] of secrets) {
    for (const [key, value] of Object.entries(data)) {
      const name = `${prefix}${key}`;
      if (!isName(name)) {
        console.error(`skipping ${path}:${key}: not a valid environment variable name`);
        continue;
      }
      vars.set(name, display(value));
    }
  }

  return vars;
}
