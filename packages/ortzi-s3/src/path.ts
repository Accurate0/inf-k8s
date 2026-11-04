export interface Remote {
  bucket: string;
  key: string;
}

export const isRemote = (path: string) => path.startsWith('s3://');

export function parseRemote(path: string): Remote {
  const rest = path.replace(/^s3:\/\//, '');
  const slash = rest.indexOf('/');
  const bucket = slash === -1 ? rest : rest.slice(0, slash);
  if (!bucket) throw new Error(`no bucket in ${path}`);

  return { bucket, key: slash === -1 ? '' : rest.slice(slash + 1) };
}

export const basename = (path: string) => path.replace(/\/+$/, '').split('/').pop() ?? '';

export const join = (dir: string, name: string) =>
  !dir ? name : dir.endsWith('/') ? `${dir}${name}` : `${dir}/${name}`;

export function parseDuration(value: string): number {
  const match = /^(\d+)([smhd])$/.exec(value);
  if (!match) throw new Error(`invalid duration ${value}; use e.g. 30m, 1h, 7d`);

  const unit = { s: 1, m: 60, h: 3600, d: 86400 }[match[2] as 's' | 'm' | 'h' | 'd'];
  return Number(match[1]) * unit;
}

export function size(bytes: number): string {
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return unit === 0 ? `${value} B` : `${value.toFixed(1)} ${units[unit]}`;
}
