export type Sample = [number, string];

export interface Response {
  data: { resultType: string; result: unknown };
}

export interface Vector {
  metric: Record<string, string>;
  value: Sample;
}

export interface Matrix {
  metric: Record<string, string>;
  values: Sample[];
}

export interface Summary {
  last: number;
  min: number;
  max: number;
  avg: number;
}

export function labels(metric: Record<string, string>): string {
  const name = metric.__name__ ?? '';
  const rest = Object.keys(metric)
    .filter((k) => k !== '__name__')
    .sort()
    .map((k) => `${k}="${metric[k]}"`);

  if (!rest.length) return name || '{}';
  return `${name}{${rest.join(', ')}}`;
}

export function summarise(values: Sample[]): Summary | undefined {
  const numbers = values.map(([, v]) => Number.parseFloat(v)).filter(Number.isFinite);
  if (!numbers.length) return undefined;

  return {
    last: numbers[numbers.length - 1],
    min: Math.min(...numbers),
    max: Math.max(...numbers),
    avg: numbers.reduce((a, b) => a + b, 0) / numbers.length,
  };
}

export const number = (value: number) =>
  Number.isInteger(value) && Math.abs(value) < 1e15 ? value.toFixed(0) : value.toFixed(4);

export const bySeries = <T extends { metric: Record<string, string> }>(rows: T[]) =>
  rows
    .map((row) => ({ series: labels(row.metric), row }))
    .sort((a, b) => (a.series < b.series ? -1 : a.series > b.series ? 1 : 0));
