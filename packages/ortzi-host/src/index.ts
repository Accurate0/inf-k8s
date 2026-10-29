import * as hostEnv from 'ortzi:host/env@0.1.0';
import * as hostFs from 'ortzi:host/fs@0.1.0';
import * as process from 'ortzi:host/process@0.1.0';

export type Stdio = process.Stdio;

export interface RunOptions {
  env?: Record<string, string>;
  cwd?: string;
  stdin?: string | Uint8Array;
  stdout?: Stdio;
  stderr?: Stdio;
  check?: boolean;
}

export interface Output {
  code: number | undefined;
  stdout: string;
  stderr: string;
  ok: boolean;
}

export class CommandError extends Error {
  constructor(
    readonly program: string,
    readonly output: Output,
  ) {
    const detail = output.stderr.trim() || output.stdout.trim();
    super(
      `${program} exited with ${output.code ?? 'a signal'}${detail ? `: ${detail}` : ''}`,
    );
  }
}

const decoder = new TextDecoder();
const encoder = new TextEncoder();

function unwrap<T>(call: () => T): T {
  try {
    return call();
  } catch (e) {
    const payload = (e as { payload?: unknown }).payload;
    throw new Error(typeof payload === 'string' ? payload : String(e));
  }
}

export function run(program: string, args: string[] = [], options: RunOptions = {}): Output {
  const stdin =
    typeof options.stdin === 'string' ? encoder.encode(options.stdin) : options.stdin;
  const raw = unwrap(() =>
    process.run({
      program,
      args,
      env: Object.entries(options.env ?? {}),
      cwd: options.cwd,
      stdin,
      stdout: options.stdout ?? 'inherit',
      stderr: options.stderr ?? 'inherit',
    }),
  );
  const output = {
    code: raw.code,
    stdout: decoder.decode(raw.stdout),
    stderr: decoder.decode(raw.stderr),
    ok: raw.code === 0,
  };
  if ((options.check ?? true) && !output.ok) {
    throw new CommandError(program, output);
  }
  return output;
}

export function capture(program: string, args: string[] = [], options: RunOptions = {}): string {
  return run(program, args, { stderr: 'piped', ...options, stdout: 'piped' }).stdout;
}

export function json<T = unknown>(program: string, args: string[] = [], options: RunOptions = {}): T {
  return JSON.parse(capture(program, args, options)) as T;
}

export const fs = {
  readText: (path: string) => decoder.decode(unwrap(() => hostFs.readFile(path))),
  read: (path: string) => unwrap(() => hostFs.readFile(path)),
  write: (path: string, contents: string | Uint8Array) =>
    unwrap(() =>
      hostFs.writeFile(path, typeof contents === 'string' ? encoder.encode(contents) : contents),
    ),
  readDir: (path: string) => unwrap(() => hostFs.readDir(path)),
  exists: (path: string) => unwrap(() => hostFs.exists(path)),
  mkdir: (path: string) => unwrap(() => hostFs.createDirAll(path)),
  remove: (path: string) => unwrap(() => hostFs.remove(path)),
};

export const args = (): string[] => hostEnv.args().slice(1);
export const env = (): Record<string, string> => Object.fromEntries(hostEnv.vars());
export const cwd = (): string => hostEnv.cwd();

export function exit(code = 0): never {
  hostEnv.exit(code);
  throw new Error('unreachable');
}

export interface Flags {
  [name: string]: { type: 'boolean' | 'string'; short?: string; default?: string | boolean };
}

export function parseArgs<F extends Flags>(
  argv: string[],
  flags: F,
): {
  values: { [K in keyof F]: F[K]['type'] extends 'boolean' ? boolean : string | undefined };
  positionals: string[];
} {
  const values: Record<string, string | boolean | undefined> = {};
  for (const [name, flag] of Object.entries(flags)) {
    values[name] = flag.default ?? (flag.type === 'boolean' ? false : undefined);
  }
  const positionals: string[] = [];
  const byShort = new Map(
    Object.entries(flags)
      .filter(([, f]) => f.short)
      .map(([name, f]) => [f.short as string, name]),
  );

  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === '--') {
      positionals.push(...argv.slice(i + 1));
      break;
    }
    let name: string | undefined;
    let inline: string | undefined;
    if (arg.startsWith('--')) {
      [name, inline] = arg.slice(2).split(/=(.*)/s, 2);
    } else if (arg.startsWith('-') && arg.length > 1) {
      name = byShort.get(arg.slice(1));
      if (!name) throw new Error(`unknown flag ${arg}`);
    } else {
      positionals.push(arg);
      continue;
    }
    const flag = flags[name];
    if (!flag) throw new Error(`unknown flag --${name}`);
    if (flag.type === 'boolean') {
      values[name] = true;
    } else {
      const value = inline ?? argv[++i];
      if (value === undefined) throw new Error(`--${name} needs a value`);
      values[name] = value;
    }
  }
  return { values: values as never, positionals };
}

export function main(body: () => void | number): { run(): void } {
  return {
    run() {
      try {
        const code = body();
        if (typeof code === 'number' && code !== 0) exit(code);
      } catch (e) {
        console.error(e instanceof Error ? e.message : String(e));
        exit(1);
      }
    },
  };
}
