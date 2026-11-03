import { run } from '@ortzi/host';

export interface FetchOptions {
  headers?: Record<string, string>;
  timeoutSeconds?: number;
}

function curl(flag: string, url: string, options: FetchOptions): string | undefined {
  const headers = Object.entries(options.headers ?? {}).flatMap(([k, v]) => ['-H', `${k}: ${v}`]);
  const output = run('curl', [flag, '-m', String(options.timeoutSeconds ?? 15), ...headers, url], {
    stdout: 'piped',
    stderr: 'piped',
    check: false,
  });
  return output.ok ? output.stdout : undefined;
}

export function fetchText(url: string, options: FetchOptions = {}): string | undefined {
  return curl('-fsS', url, options);
}

export function fetchHeaders(url: string, options: FetchOptions = {}): string | undefined {
  return curl('-fsSI', url, options);
}

export interface RequestOptions extends FetchOptions {
  method?: string;
  body?: string;
}

export interface Response {
  status: number;
  body: string;
}

const quoted = (value: string) =>
  `"${value
    .replace(/[\\"]/g, (c) => `\\${c}`)
    .replace(/\n/g, '\\n')
    .replace(/\r/g, '\\r')}"`;

// the config goes over stdin so headers and bodies never show up in argv
export function request(url: string, options: RequestOptions = {}): Response {
  const config = [
    `url = ${quoted(url)}`,
    `request = ${quoted(options.method ?? 'GET')}`,
    `max-time = ${options.timeoutSeconds ?? 30}`,
    'silent',
    'show-error',
    `write-out = "\\n%{http_code}"`,
    ...Object.entries(options.headers ?? {}).map(([k, v]) => `header = ${quoted(`${k}: ${v}`)}`),
    ...(options.body === undefined ? [] : [`data-binary = ${quoted(options.body)}`]),
  ];

  const output = run('curl', ['-K', '-'], {
    stdin: config.join('\n') + '\n',
    stdout: 'piped',
    stderr: 'piped',
    check: false,
  });
  if (!output.ok) throw new Error(`${url}: ${output.stderr.trim() || 'curl failed'}`);

  const split = output.stdout.lastIndexOf('\n');
  return {
    status: Number(output.stdout.slice(split + 1)),
    body: output.stdout.slice(0, split),
  };
}
