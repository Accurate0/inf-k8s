import { env, run as exec } from '@ortzi/host';
import { UsageError, cli, columns } from '@ortzi/kit';
import { parseDuration } from './duration';
import { Grafana } from './grafana';
import * as loki from './loki';
import * as prometheus from './prometheus';

const DEFAULT_GRAFANA = 'https://grafana.ortzi.cloud';

const USAGE = `usage: ortzi obs <command> [flags]

commands:
  logs [LOGQL]             query Loki; built from --ns/--app/--pod/--grep when LOGQL is omitted
  metrics PROMQL           query Prometheus
  whoami                   show the Grafana user the token maps to

flags:
  --grafana URL            Grafana address (default: $OBS_GRAFANA_URL or ${DEFAULT_GRAFANA})
  --token TOKEN            use this token instead of the stored login (default: $OBS_TOKEN)
  --json                   logs, metrics: print JSON

logs:
  --ns NS, -n NS           namespace
  --app APP                app label
  --pod PREFIX             pods starting with this
  --grep TEXT, -g TEXT     only lines containing this text
  --since DURATION         how far back to look (default 1h)
  --limit N                most lines to show (default 200)
  --follow, -f             keep polling for new lines

metrics:
  --range DURATION         evaluate over this window instead of instantly, e.g. 1h
  --step DURATION          resolution for --range (default 1m)

\`ortzi logs\` and \`ortzi metrics\` are shortcuts for the subcommands.
log in with \`ortzi login obs\``;

const nowNs = () => BigInt(Date.now()) * 1_000_000n;

function fetchLogs(
  grafana: Grafana,
  query: string,
  start: bigint,
  end: bigint,
  limit: number,
  direction: 'backward' | 'forward',
): loki.Entry[] {
  const response = grafana.datasource<loki.Response>('loki', '/loki/api/v1/query_range', [
    ['query', query],
    ['start', start.toString()],
    ['end', end.toString()],
    ['limit', String(limit)],
    ['direction', direction],
  ]);
  return loki.flatten(response);
}

function parseLimit(value: string | undefined): number {
  const limit = Number(value);
  if (!Number.isInteger(limit) || limit <= 0) throw new UsageError(`invalid --limit ${value}`);
  return limit;
}

export const run = cli({
  usage: USAGE,
  flags: {
    grafana: { type: 'string' },
    token: { type: 'string' },
    json: { type: 'boolean' },
    ns: { type: 'string', short: 'n' },
    app: { type: 'string' },
    pod: { type: 'string' },
    grep: { type: 'string', short: 'g' },
    since: { type: 'string', default: '1h' },
    limit: { type: 'string', default: '200' },
    follow: { type: 'boolean', short: 'f' },
    range: { type: 'string' },
    step: { type: 'string', default: '1m' },
  },
  aliases: { logs: 'logs', metrics: 'metrics' },
  setup: (values) => {
    let grafana: Grafana | undefined;
    const vars = env();
    const base = values.grafana ?? vars.OBS_GRAFANA_URL ?? DEFAULT_GRAFANA;
    return {
      base,
      grafana: () => {
        const token = values.token || vars.OBS_TOKEN;
        if (!token) throw new Error('not logged in; run `ortzi login obs` or set OBS_TOKEN');
        return (grafana ??= new Grafana(base, token));
      },
    };
  },
  commands: {
    whoami: (ctx) => {
      const user = ctx.grafana().get<{ login: string; isGrafanaAdmin?: boolean }>('/api/user');
      console.log(`grafana  ${ctx.base}`);
      console.log(`user     ${user.login}`);
      console.log(`admin    ${user.isGrafanaAdmin ? 'yes' : 'no'}`);

      return 0;
    },
    logs: (ctx, values, [query]) => {
      const logql = loki.buildQuery({ ...values, query });
      const since = BigInt(parseDuration(values.since ?? '1h')) * 1_000_000n;
      const limit = parseLimit(values.limit);
      const grafana = ctx.grafana();

      const end = nowNs();
      const entries = fetchLogs(grafana, logql, end - since, end, limit, 'backward');
      if (values.json) {
        console.log(loki.toJson(entries));
        return 0;
      }

      for (const entry of entries) console.log(loki.formatEntry(entry));
      if (!values.follow) return 0;

      let last = entries.at(-1)?.ts ?? end;
      for (;;) {
        exec('sleep', ['2']);
        const fresh = fetchLogs(grafana, logql, last + 1n, nowNs(), 5000, 'forward');
        for (const entry of fresh) console.log(loki.formatEntry(entry));
        last = fresh.at(-1)?.ts ?? last;
      }
    },
    metrics: (ctx, values, [query]) => {
      if (!query) throw new UsageError(`missing PROMQL\n\n${USAGE}`);

      const now = Date.now();
      const seconds = (ms: number) => String(Math.floor(ms / 1000));
      const response =
        values.range === undefined
          ? ctx.grafana().datasource<prometheus.Response>('prometheus', '/api/v1/query', [
              ['query', query],
              ['time', seconds(now)],
            ])
          : ctx.grafana().datasource<prometheus.Response>('prometheus', '/api/v1/query_range', [
              ['query', query],
              ['start', seconds(now - parseDuration(values.range))],
              ['end', seconds(now)],
              ['step', String(Math.max(1, Math.floor(parseDuration(values.step ?? '1m') / 1000)))],
            ]);

      const { resultType, result } = response.data;
      if (values.json) {
        console.log(JSON.stringify(result, null, 2));
        return 0;
      }

      let header: string[];
      let rows: string[][];
      switch (resultType) {
        case 'vector':
          header = ['SERIES', 'VALUE'];
          rows = prometheus
            .bySeries(result as prometheus.Vector[])
            .map(({ series, row }) => [series, row.value[1]]);
          break;
        case 'matrix':
          header = ['SERIES', 'LAST', 'MIN', 'MAX', 'AVG'];
          rows = prometheus.bySeries(result as prometheus.Matrix[]).flatMap(({ series, row }) => {
            const s = prometheus.summarise(row.values);
            return s ? [[series, ...[s.last, s.min, s.max, s.avg].map(prometheus.number)]] : [];
          });
          break;
        case 'scalar':
        case 'string':
          console.log((result as prometheus.Sample)[1]);
          return 0;
        default:
          throw new Error(`unexpected result type ${resultType}`);
      }

      console.log(rows.length ? columns(header, rows) : 'no data');
      return 0;
    },
  },
});
