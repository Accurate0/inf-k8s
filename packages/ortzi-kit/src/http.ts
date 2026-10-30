import { run } from '@ortzi/host';

export interface FetchOptions {
  headers?: Record<string, string>;
  timeoutSeconds?: number;
}

function curl(flag: string, url: string, options: FetchOptions): string | undefined {
  const headers = Object.entries(options.headers ?? {}).flatMap(([k, v]) => ['-H', `${k}: ${v}`]);
  const output = run(
    'curl',
    [flag, '-m', String(options.timeoutSeconds ?? 15), ...headers, url],
    { stdout: 'piped', stderr: 'piped', check: false },
  );
  return output.ok ? output.stdout : undefined;
}

export function fetchText(url: string, options: FetchOptions = {}): string | undefined {
  return curl('-fsS', url, options);
}

export function fetchHeaders(url: string, options: FetchOptions = {}): string | undefined {
  return curl('-fsSI', url, options);
}
