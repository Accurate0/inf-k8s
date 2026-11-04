import * as hostEnv from 'ortzi:host/env@0.1.0';
import * as hostFs from 'ortzi:host/fs@0.1.0';
import * as hostHttp from 'ortzi:host/http@0.1.0';
import * as hostKube from 'ortzi:host/kube@0.1.0';
import * as process from 'ortzi:host/process@0.1.0';
import { getStderr } from 'wasi:cli/stderr@0.2.12';
import { getStdin } from 'wasi:cli/stdin@0.2.12';
import { getStdout } from 'wasi:cli/stdout@0.2.12';
import { getTerminalStdin } from 'wasi:cli/terminal-stdin@0.2.12';

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
    super(`${program} exited with ${output.code ?? 'a signal'}${detail ? `: ${detail}` : ''}`);
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
  const stdin = typeof options.stdin === 'string' ? encoder.encode(options.stdin) : options.stdin;
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

export function json<T = unknown>(
  program: string,
  args: string[] = [],
  options: RunOptions = {},
): T {
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

export interface KubeRequest {
  context?: string;
  method: string;
  path: string;
  headers?: Record<string, string>;
  body?: string;
}

export interface KubeResponse {
  status: number;
  body: string;
}

export function kubeSend(request: KubeRequest): KubeResponse {
  const raw = unwrap(() =>
    hostKube.send({
      context: request.context,
      method: request.method,
      path: request.path,
      headers: Object.entries(request.headers ?? {}),
      body: request.body === undefined ? undefined : encoder.encode(request.body),
    }),
  );
  return { status: raw.status, body: decoder.decode(raw.body) };
}

export interface HttpRequest {
  method?: string;
  url: string;
  headers?: Record<string, string>;
  body?: string | Uint8Array;
  timeoutMs?: number;
}

export interface HttpResponse {
  status: number;
  headers: Record<string, string>;
  body: string;
}

export interface HttpBytesResponse {
  status: number;
  headers: Record<string, string>;
  body: Uint8Array;
}

export function httpSendBytes(request: HttpRequest): HttpBytesResponse {
  const raw = unwrap(() =>
    hostHttp.send({
      method: request.method ?? 'GET',
      url: request.url,
      headers: Object.entries(request.headers ?? {}),
      body: typeof request.body === 'string' ? encoder.encode(request.body) : request.body,
      timeoutMs: request.timeoutMs,
    }),
  );
  return { status: raw.status, headers: Object.fromEntries(raw.headers), body: raw.body };
}

export function httpSend(request: HttpRequest): HttpResponse {
  const response = httpSendBytes(request);
  return { ...response, body: decoder.decode(response.body) };
}

export const args = (): string[] => hostEnv.args().slice(1);
export const invokedAs = (): string => hostEnv.args()[0] ?? '';
export const env = (): Record<string, string> => Object.fromEntries(hostEnv.vars());
export const cwd = (): string => hostEnv.cwd();

export const stdinIsTerminal = (): boolean => getTerminalStdin() !== undefined;

let stdin: ReturnType<typeof getStdin> | undefined;

export function readLine(): string | undefined {
  stdin ??= getStdin();
  const bytes: number[] = [];
  for (;;) {
    let chunk: Uint8Array;
    try {
      chunk = stdin.blockingRead(1n);
    } catch {
      chunk = new Uint8Array();
    }

    if (chunk.length === 0) {
      if (!bytes.length) return undefined;
      break;
    }
    if (chunk[0] === 0x0a) break;
    bytes.push(chunk[0]);
  }

  return decoder.decode(new Uint8Array(bytes)).replace(/\r$/, '');
}

export function readStdin(): Uint8Array {
  stdin ??= getStdin();
  const chunks: Uint8Array[] = [];
  let size = 0;
  for (;;) {
    let chunk: Uint8Array;
    try {
      chunk = stdin.blockingRead(65536n);
    } catch {
      break;
    }

    if (chunk.length === 0) break;
    chunks.push(chunk);
    size += chunk.length;
  }

  const out = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    out.set(chunk, offset);
    offset += chunk.length;
  }
  return out;
}

export function writeStdout(contents: string | Uint8Array): void {
  const bytes = typeof contents === 'string' ? encoder.encode(contents) : contents;
  const stdout = getStdout();
  for (let offset = 0; offset < bytes.length; offset += 4096) {
    stdout.blockingWriteAndFlush(bytes.subarray(offset, offset + 4096));
  }
}

export function prompt(text: string): string | undefined {
  getStderr().blockingWriteAndFlush(encoder.encode(text));
  return readLine();
}

export function exit(code = 0): never {
  hostEnv.exit(code);
  throw new Error('unreachable');
}

export interface Flags {
  [name: string]: {
    type: 'boolean' | 'string';
    short?: string;
    default?: string | boolean;
    multiple?: boolean;
  };
}

type FlagValue<F extends Flags[string]> = F['type'] extends 'boolean'
  ? boolean
  : F['multiple'] extends true
    ? string[]
    : string | undefined;

export function parseArgs<F extends Flags>(
  argv: string[],
  flags: F,
): {
  values: { [K in keyof F]: FlagValue<F[K]> };
  positionals: string[];
} {
  const values: Record<string, string | string[] | boolean | undefined> = {};
  for (const [name, flag] of Object.entries(flags)) {
    values[name] = flag.multiple
      ? []
      : (flag.default ?? (flag.type === 'boolean' ? false : undefined));
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
      values[name] = flag.multiple ? [...(values[name] as string[]), value] : value;
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
