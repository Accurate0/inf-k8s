export interface Selector {
  query?: string;
  ns?: string;
  app?: string;
  pod?: string;
  grep?: string;
}

export interface Response {
  data: { result: { stream: Record<string, string>; values: [string, string][] }[] };
}

export interface Entry {
  ts: bigint;
  labels: Record<string, string>;
  line: string;
}

const escape = (value: string) => value.replaceAll('\\', '\\\\').replaceAll('"', '\\"');

export function buildQuery(selector: Selector): string {
  if (selector.query !== undefined) return selector.query;

  const matchers: string[] = [];
  if (selector.ns !== undefined) matchers.push(`namespace="${escape(selector.ns)}"`);
  if (selector.app !== undefined) matchers.push(`app="${escape(selector.app)}"`);
  if (selector.pod !== undefined) matchers.push(`pod=~"${escape(selector.pod)}.*"`);
  if (!matchers.length) throw new Error('pass a LogQL query or at least one of --ns, --app, --pod');

  const query = `{${matchers.join(',')}}`;
  return selector.grep === undefined ? query : `${query} |= "${escape(selector.grep)}"`;
}

const parseTs = (ts: string) => {
  try {
    return BigInt(ts);
  } catch {
    return 0n;
  }
};

export function flatten(response: Response): Entry[] {
  return response.data.result
    .flatMap(({ stream, values }) =>
      values.map(([ts, line]) => ({ ts: parseTs(ts), labels: stream, line })),
    )
    .sort((a, b) => (a.ts < b.ts ? -1 : a.ts > b.ts ? 1 : 0));
}

const pad = (n: number) => String(n).padStart(2, '0');

export function stamp(ts: bigint): string {
  const at = new Date(Number(ts / 1_000_000n));
  const date = `${pad(at.getMonth() + 1)}-${pad(at.getDate())}`;
  return `${date} ${pad(at.getHours())}:${pad(at.getMinutes())}:${pad(at.getSeconds())}`;
}

export function source(labels: Record<string, string>): string {
  const { namespace, pod, job } = labels;
  if (namespace !== undefined) return pod === undefined ? namespace : `${namespace}/${pod}`;
  return job ?? '-';
}

export const formatEntry = (entry: Entry) =>
  `${stamp(entry.ts)} ${source(entry.labels)} ${entry.line}`;

export const toJson = (entries: Entry[]) =>
  JSON.stringify(
    entries.map((e) => ({ ts: e.ts.toString(), labels: e.labels, line: e.line })),
    null,
    2,
  );
