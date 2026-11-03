const UNITS: Record<string, number> = {
  s: 1_000,
  m: 60_000,
  h: 3_600_000,
  d: 86_400_000,
  w: 604_800_000,
};

export function parseDuration(value: string): number {
  const trimmed = value.trim();
  const match = /^(\d*)(.*)$/s.exec(trimmed);
  const [, number, unit] = match ?? ['', '', ''];
  if (!unit) throw new Error(`${JSON.stringify(trimmed)} needs a unit (s, m, h, d, w)`);
  if (!number) throw new Error(`invalid duration ${JSON.stringify(trimmed)}`);

  const scale = UNITS[unit];
  if (scale === undefined) {
    throw new Error(
      `unknown duration unit ${JSON.stringify(unit)} in ${JSON.stringify(trimmed)}; use s, m, h, d or w`,
    );
  }
  return Number(number) * scale;
}
