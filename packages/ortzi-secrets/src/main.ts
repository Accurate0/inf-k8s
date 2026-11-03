import { env, fs, prompt, readLine, run as exec, stdinIsTerminal } from '@ortzi/host';
import { Kube, UsageError, cli, columns } from '@ortzi/kit';
import { Bao } from './bao';
import { type Data, collect, display, exports, isName } from './env';
import { fullName, isReady, keys, readsOpenbao, ready } from './externalsecrets';
import { listExternalSecrets, sync } from './sync';

const DEFAULT_ADDR = 'https://vault.ortzi.cloud';
const MASK = '********';

const USAGE = `usage: ortzi secrets <command> [flags]

commands:
  whoami                   show the OpenBao identity and policies you get
  ls [PREFIX]              list namespaces, or the secrets under one
  get PATH [KEY]           show a secret's keys (values masked), or one raw value
  set PATH KEY...          set keys, reading each value from stdin, then sync its ExternalSecrets
  rm PATH KEY...           remove keys, or with --all delete the latest version
  history PATH             show a secret's versions
  sync TARGET              force the ExternalSecrets reading a path (or every one in a namespace) to refresh
  status                   show ExternalSecret sync status, failing first
  env PATH...              print \`export KEY='value'\` lines for \`eval "$(ortzi secrets env ns/app)"\`

flags:
  --addr URL               OpenBao address (default: $BAO_ADDR or ${DEFAULT_ADDR})
  --mount MOUNT            KV v2 mount (default kv)
  --token TOKEN            Kanidm access token to log in with (default: $SECRETSCTL_TOKEN)
  --bao-token TOKEN        use this OpenBao token directly (default: $BAO_TOKEN)
  --context CTX            kube context (default: current)
  --json                   ls, get: print JSON
  --reveal                 get: show values
  --file KEY=PATH          set: read a key's value from a file (repeatable)
  --no-sync                set, rm: don't sync ExternalSecrets afterwards
  --all                    rm: delete the latest version
  --yes, -y                rm --all: don't ask
  --namespace NS, -n NS    status: only this namespace
  --prefix PREFIX          env: prepend to every variable name

log in with \`ortzi login secrets\``;

interface Ctx {
  kube: () => Kube;
  bao: () => Bao;
}

function connect(addr: string, mount: string, token?: string, baoToken?: string): Bao {
  const vars = env();
  const direct = baoToken || vars.BAO_TOKEN;
  if (direct) return Bao.fromToken(addr, mount, direct);

  const jwt = token || vars.SECRETSCTL_TOKEN;
  if (!jwt) throw new Error('not logged in; run `ortzi login secrets` or set SECRETSCTL_TOKEN');
  return Bao.login(addr, mount, jwt);
}

function readSecret(key: string): string {
  if (!stdinIsTerminal()) {
    const line = readLine();
    if (line === undefined) throw new Error(`no value for ${key} on stdin`);
    return line;
  }

  exec('stty', ['-echo'], { check: false });
  try {
    return prompt(`${key}: `) ?? '';
  } finally {
    exec('stty', ['echo'], { check: false });
    console.error('');
  }
}

function confirm(question: string): boolean {
  if (!stdinIsTerminal()) throw new Error(`${question}: pass --yes to confirm non-interactively`);
  return ['y', 'Y', 'yes'].includes((prompt(`${question} [y/N] `) ?? '').trim());
}

function needed(value: string | undefined, what: string): string {
  if (!value) throw new UsageError(`missing ${what}\n\n${USAGE}`);
  return value;
}

const syncAfter = (ctx: Ctx, path: string, skip: boolean) =>
  skip || sync(ctx.kube(), path) ? 0 : 1;

export const run = cli({
  usage: USAGE,
  flags: {
    addr: { type: 'string' },
    mount: { type: 'string', default: 'kv' },
    token: { type: 'string' },
    'bao-token': { type: 'string' },
    context: { type: 'string' },
    json: { type: 'boolean' },
    reveal: { type: 'boolean' },
    file: { type: 'string', multiple: true },
    'no-sync': { type: 'boolean' },
    all: { type: 'boolean' },
    yes: { type: 'boolean', short: 'y' },
    namespace: { type: 'string', short: 'n' },
    prefix: { type: 'string', default: '' },
  },
  setup: (values): Ctx => {
    let kube: Kube | undefined;
    let bao: Bao | undefined;
    const addr = values.addr ?? env().BAO_ADDR ?? DEFAULT_ADDR;
    return {
      kube: () => (kube ??= Kube.connect(values.context)),
      bao: () => (bao ??= connect(addr, values.mount ?? 'kv', values.token, values['bao-token'])),
    };
  },
  commands: {
    whoami: (ctx) => {
      const bao = ctx.bao();
      const me = bao.whoami();
      console.log(`user      ${me.display_name}`);
      console.log(`policies  ${(me.policies ?? []).join(', ')}`);
      console.log(`expires   in ${me.ttl}s`);
      console.log(`openbao   ${bao.addr}`);

      return 0;
    },
    ls: (ctx, values, [prefix]) => {
      const found = ctx.bao().list(prefix ?? '');
      if (values.json) {
        console.log(JSON.stringify(found, null, 2));
        return 0;
      }

      const base = prefix?.replace(/^\/+|\/+$/g, '');
      for (const key of found) console.log(base === undefined ? key : `${base}/${key}`);

      return 0;
    },
    get: (ctx, values, [path, key]) => {
      const secret = ctx.bao().require(needed(path, 'PATH'));
      if (key !== undefined) {
        if (!Object.hasOwn(secret.data, key)) throw new Error(`${path} has no key ${key}`);
        console.log(display(secret.data[key]));
        return 0;
      }

      const shown = (value: unknown) => (values.reveal ? display(value) : MASK);
      if (values.json) {
        const data = values.reveal
          ? secret.data
          : Object.fromEntries(Object.keys(secret.data).map((k) => [k, MASK]));
        console.log(JSON.stringify(data, null, 2));
        return 0;
      }

      console.log(
        columns(
          ['KEY', 'VALUE'],
          Object.entries(secret.data).map(([k, v]) => [k, shown(v)]),
        ),
      );
      console.log(`version ${secret.metadata.version} (${secret.metadata.created_time ?? ''})`);

      return 0;
    },
    set: (ctx, values, [path, ...names]) => {
      needed(path, 'PATH');
      if (!names.length && !values.file.length)
        throw new UsageError(`pass KEY... or --file KEY=PATH\n\n${USAGE}`);

      const changes: Record<string, string> = {};
      for (const key of names) {
        if (!isName(key) && /[= ]/.test(key)) {
          throw new UsageError('pass key names only; values are read from stdin');
        }
        changes[key] = readSecret(key);
      }

      for (const file of values.file) {
        const split = file.indexOf('=');
        if (split < 0) throw new UsageError('--file takes KEY=PATH');
        changes[file.slice(0, split)] = fs.readText(file.slice(split + 1));
      }

      const version = ctx.bao().patch(path, changes);
      console.log(`wrote ${path} version ${version}`);

      return syncAfter(ctx, path, values['no-sync']);
    },
    rm: (ctx, values, [path, ...names]) => {
      needed(path, 'PATH');
      const bao = ctx.bao();
      if (values.all) {
        if (!values.yes && !confirm(`delete the latest version of ${path}?`)) {
          throw new Error('cancelled');
        }

        bao.deleteLatest(path);
        console.log(`deleted the latest version of ${path}`);
      } else {
        if (!names.length) throw new UsageError(`pass KEY... or --all\n\n${USAGE}`);

        const secret = bao.require(path);
        const missing = names.find((key) => !Object.hasOwn(secret.data, key));
        if (missing) throw new Error(`${path} has no key ${missing}`);

        const version = bao.patch(path, Object.fromEntries(names.map((k) => [k, null])));
        console.log(`removed ${names.join(', ')} from ${path} (version ${version})`);
      }

      return syncAfter(ctx, path, values['no-sync']);
    },
    history: (ctx, _values, [path]) => {
      const meta = ctx.bao().metadata(needed(path, 'PATH'));
      const rows = Object.entries(meta.versions ?? {})
        .sort(([a], [b]) => Number(b) - Number(a))
        .map(([number, v]) => [
          number,
          v.created_time,
          v.deletion_time ?? '',
          v.destroyed ? 'destroyed' : Number(number) === meta.current_version ? 'current' : '',
        ]);
      console.log(columns(['VERSION', 'CREATED', 'DELETED', ''], rows));

      return 0;
    },
    sync: (ctx, _values, [target]) => (sync(ctx.kube(), needed(target, 'TARGET')) ? 0 : 1),
    status: (ctx, values) => {
      const all = listExternalSecrets(ctx.kube(), values.namespace)
        .filter(readsOpenbao)
        .sort(
          (a, b) =>
            Number(isReady(a)) - Number(isReady(b)) || fullName(a).localeCompare(fullName(b)),
        );

      const rows = all.map((es) => [
        fullName(es),
        keys(es).join(', '),
        ready(es)?.status ?? '?',
        ready(es)?.reason ?? '',
        es.status?.refreshTime ?? '',
      ]);
      console.log(columns(['EXTERNALSECRET', 'KEY', 'READY', 'REASON', 'REFRESHED'], rows));

      return 0;
    },
    env: (ctx, values, paths) => {
      if (!paths.length) throw new UsageError(`missing PATH\n\n${USAGE}`);
      const secrets = paths.map((path): [string, Data] => [path, ctx.bao().require(path).data]);
      const lines = exports(collect(secrets, values.prefix ?? ''));
      if (lines) console.log(lines.trimEnd());

      return 0;
    },
  },
});
