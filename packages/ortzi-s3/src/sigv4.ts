import { hmac } from '@noble/hashes/hmac.js';
import { sha256 } from '@noble/hashes/sha2.js';
import { bytesToHex } from '@noble/hashes/utils.js';

export interface Credentials {
  accessKeyId: string;
  secretAccessKey: string;
  region: string;
}

export interface Signable {
  method: string;
  host: string;
  path: string;
  query?: [string, string][];
  headers?: Record<string, string>;
  payloadHash: string;
}

const encoder = new TextEncoder();

export const hashHex = (data: Uint8Array | string) =>
  bytesToHex(sha256(typeof data === 'string' ? encoder.encode(data) : data));

export const UNSIGNED = 'UNSIGNED-PAYLOAD';

const encode = (value: string) =>
  encodeURIComponent(value).replace(
    /[!'()*]/g,
    (c) => `%${c.charCodeAt(0).toString(16).toUpperCase()}`,
  );

export const encodePath = (path: string) => path.split('/').map(encode).join('/');

export const encodeQuery = (query: [string, string][]) =>
  query
    .map(([k, v]) => [encode(k), encode(v)])
    .sort(([ak, av], [bk, bv]) => (ak === bk ? (av < bv ? -1 : 1) : ak < bk ? -1 : 1))
    .map(([k, v]) => `${k}=${v}`)
    .join('&');

export const amzDate = (date: Date) => date.toISOString().replace(/[-:]/g, '').replace(/\.\d+/, '');

const scope = (creds: Credentials, date: string) =>
  `${date.slice(0, 8)}/${creds.region}/s3/aws4_request`;

function signature(creds: Credentials, date: string, canonical: string): string {
  const toSign = ['AWS4-HMAC-SHA256', date, scope(creds, date), hashHex(canonical)].join('\n');

  let key = encoder.encode(`AWS4${creds.secretAccessKey}`);
  for (const part of [date.slice(0, 8), creds.region, 's3', 'aws4_request']) {
    key = hmac(sha256, key, encoder.encode(part));
  }
  return bytesToHex(hmac(sha256, key, encoder.encode(toSign)));
}

function canonicalRequest(request: Signable, headers: Record<string, string>) {
  const names = Object.keys(headers).sort();
  const signed = names.join(';');
  const canonical = [
    request.method,
    encodePath(request.path),
    encodeQuery(request.query ?? []),
    names.map((name) => `${name}:${headers[name].trim()}\n`).join(''),
    signed,
    request.payloadHash,
  ].join('\n');

  return { canonical, signed };
}

export function sign(request: Signable, creds: Credentials, now: Date): Record<string, string> {
  const date = amzDate(now);
  const headers: Record<string, string> = {
    host: request.host,
    'x-amz-content-sha256': request.payloadHash,
    'x-amz-date': date,
  };
  for (const [name, value] of Object.entries(request.headers ?? {})) {
    headers[name.toLowerCase()] = value;
  }

  const { canonical, signed } = canonicalRequest(request, headers);
  const credential = `${creds.accessKeyId}/${scope(creds, date)}`;
  const { host: _, ...rest } = headers;

  return {
    ...rest,
    authorization: `AWS4-HMAC-SHA256 Credential=${credential}, SignedHeaders=${signed}, Signature=${signature(creds, date, canonical)}`,
  };
}

export function presign(
  request: Omit<Signable, 'payloadHash' | 'headers'>,
  creds: Credentials,
  now: Date,
  expiresSeconds: number,
): string {
  const date = amzDate(now);
  const query: [string, string][] = [
    ...(request.query ?? []),
    ['X-Amz-Algorithm', 'AWS4-HMAC-SHA256'],
    ['X-Amz-Credential', `${creds.accessKeyId}/${scope(creds, date)}`],
    ['X-Amz-Date', date],
    ['X-Amz-Expires', String(expiresSeconds)],
    ['X-Amz-SignedHeaders', 'host'],
  ];

  const { canonical } = canonicalRequest(
    { ...request, query, payloadHash: UNSIGNED },
    { host: request.host },
  );
  return `${encodeQuery(query)}&X-Amz-Signature=${signature(creds, date, canonical)}`;
}
