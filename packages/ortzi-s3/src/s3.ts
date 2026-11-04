import { md5 } from '@noble/hashes/legacy.js';
import { type HttpBytesResponse, httpSendBytes } from '@ortzi/host';
import { type Credentials, encodePath, encodeQuery, hashHex, presign, sign } from './sigv4';
import { blocks, escape, text } from './xml';

export const PART_SIZE = 64 * 1024 * 1024;

export interface Bucket {
  name: string;
  created: string;
}

export interface Entry {
  key: string;
  size: number;
  modified: string;
}

export interface Listing {
  objects: Entry[];
  prefixes: string[];
}

interface Options {
  query?: [string, string][];
  headers?: Record<string, string>;
  body?: Uint8Array;
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();

const base64 = (bytes: Uint8Array) => btoa(String.fromCharCode(...bytes));

export class S3 {
  readonly endpoint: string;
  private readonly host: string;

  constructor(
    endpoint: string,
    private readonly creds: Credentials,
  ) {
    this.endpoint = endpoint.replace(/\/+$/, '');
    this.host = this.endpoint.replace(/^https?:\/\//, '');
  }

  private path = (bucket?: string, key?: string) =>
    !bucket ? '/' : key === undefined ? `/${bucket}` : `/${bucket}/${key}`;

  private send(method: string, bucket?: string, key?: string, options: Options = {}) {
    const path = this.path(bucket, key);
    const query = options.query ?? [];
    const headers = sign(
      {
        method,
        host: this.host,
        path,
        query,
        headers: options.headers,
        payloadHash: hashHex(options.body ?? new Uint8Array()),
      },
      this.creds,
      new Date(),
    );

    const search = query.length ? `?${encodeQuery(query)}` : '';
    const response = httpSendBytes({
      method,
      url: `${this.endpoint}${encodePath(path)}${search}`,
      headers,
      body: options.body,
      timeoutMs: 3_600_000,
    });
    if (response.status >= 300) throw failure(method, path, response);

    return response;
  }

  private xml(method: string, bucket?: string, key?: string, options: Options = {}) {
    return decoder.decode(this.send(method, bucket, key, options).body);
  }

  listBuckets(): Bucket[] {
    return blocks(this.xml('GET'), 'Bucket').map((b) => ({
      name: text(b, 'Name') ?? '',
      created: text(b, 'CreationDate') ?? '',
    }));
  }

  list(bucket: string, prefix: string, delimiter?: string): Listing {
    const listing: Listing = { objects: [], prefixes: [] };
    let token: string | undefined;
    do {
      const query: [string, string][] = [
        ['list-type', '2'],
        ['prefix', prefix],
      ];
      if (delimiter) query.push(['delimiter', delimiter]);
      if (token) query.push(['continuation-token', token]);

      const xml = this.xml('GET', bucket, undefined, { query });
      for (const c of blocks(xml, 'Contents')) {
        listing.objects.push({
          key: text(c, 'Key') ?? '',
          size: Number(text(c, 'Size') ?? 0),
          modified: text(c, 'LastModified') ?? '',
        });
      }
      for (const p of blocks(xml, 'CommonPrefixes')) listing.prefixes.push(text(p, 'Prefix') ?? '');

      token = text(xml, 'IsTruncated') === 'true' ? text(xml, 'NextContinuationToken') : undefined;
    } while (token);

    return listing;
  }

  get(bucket: string, key: string): Uint8Array {
    return this.send('GET', bucket, key).body;
  }

  put(bucket: string, key: string, body: Uint8Array): void {
    if (body.length <= PART_SIZE) {
      this.send('PUT', bucket, key, { body });
      return;
    }

    const upload = text(this.xml('POST', bucket, key, { query: [['uploads', '']] }), 'UploadId');
    if (!upload) throw new Error(`no upload id for s3://${bucket}/${key}`);

    try {
      const parts: string[] = [];
      for (let offset = 0, n = 1; offset < body.length; offset += PART_SIZE, n++) {
        const response = this.send('PUT', bucket, key, {
          query: [
            ['partNumber', String(n)],
            ['uploadId', upload],
          ],
          body: body.subarray(offset, offset + PART_SIZE),
        });
        parts.push(
          `<Part><PartNumber>${n}</PartNumber><ETag>${escape(etag(response))}</ETag></Part>`,
        );
      }

      this.send('POST', bucket, key, {
        query: [['uploadId', upload]],
        body: encoder.encode(
          `<CompleteMultipartUpload>${parts.join('')}</CompleteMultipartUpload>`,
        ),
      });
    } catch (e) {
      this.send('DELETE', bucket, key, { query: [['uploadId', upload]] });
      throw e;
    }
  }

  copy(from: { bucket: string; key: string }, bucket: string, key: string): void {
    this.send('PUT', bucket, key, {
      headers: { 'x-amz-copy-source': encodePath(`/${from.bucket}/${from.key}`) },
    });
  }

  delete(bucket: string, key: string): void {
    this.send('DELETE', bucket, key);
  }

  deleteMany(bucket: string, keys: string[]): void {
    for (let i = 0; i < keys.length; i += 1000) {
      const objects = keys
        .slice(i, i + 1000)
        .map((key) => `<Object><Key>${escape(key)}</Key></Object>`)
        .join('');
      const body = encoder.encode(`<Delete><Quiet>true</Quiet>${objects}</Delete>`);

      const xml = this.xml('POST', bucket, undefined, {
        query: [['delete', '']],
        headers: { 'content-md5': base64(md5(body)) },
        body,
      });
      const errors = blocks(xml, 'Error').map((e) => `${text(e, 'Key')}: ${text(e, 'Message')}`);
      if (errors.length) throw new Error(`failed to delete:\n  ${errors.join('\n  ')}`);
    }
  }

  createBucket(bucket: string): void {
    this.send('PUT', bucket);
  }

  deleteBucket(bucket: string): void {
    this.send('DELETE', bucket);
  }

  presign(bucket: string, key: string, expiresSeconds: number): string {
    const path = this.path(bucket, key);
    const query = presign(
      { method: 'GET', host: this.host, path },
      this.creds,
      new Date(),
      expiresSeconds,
    );
    return `${this.endpoint}${encodePath(path)}?${query}`;
  }
}

function etag(response: HttpBytesResponse): string {
  const found = Object.entries(response.headers).find(([name]) => name.toLowerCase() === 'etag');
  if (!found) throw new Error('upload part returned no ETag');
  return found[1];
}

function failure(method: string, path: string, response: HttpBytesResponse): Error {
  const body = decoder.decode(response.body);
  const code = text(body, 'Code');
  const message = text(body, 'Message');
  const detail = code ? `${code}${message ? `: ${message}` : ''}` : body.trim();
  return new Error(`${method} ${path} returned ${response.status}${detail ? `: ${detail}` : ''}`);
}
