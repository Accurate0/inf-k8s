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

export function parseMinutes(value: string | undefined, fallback: number): number {
  const minutes = value === undefined ? fallback : Number(value);
  if (!Number.isFinite(minutes) || minutes <= 0) {
    throw new Error(`invalid number of minutes: ${value}`);
  }
  return minutes;
}

export function short(tag: string): string {
  return /^[0-9a-f]{40}$/.test(tag) ? tag.slice(0, 8) : tag;
}

export function minuteTimestamp(iso: string): string {
  return iso.replace('T', ' ').replace(/:\d\dZ$/, '');
}
