import { httpSend } from '@ortzi/host';

export const KEY_PREFIX = 'ortzi-s3-';

export interface Key {
  accessKeyId: string;
  secretAccessKey: string;
  expiration: string;
}

interface KeyInfo {
  accessKeyId: string;
  secretAccessKey?: string | null;
  expiration?: string | null;
}

interface KeyItem {
  id: string;
  name: string;
  expired: boolean;
}

export class GarageAdmin {
  private readonly base: string;

  constructor(
    base: string,
    private readonly token: string,
  ) {
    this.base = base.replace(/\/+$/, '');
  }

  private call<T>(method: string, endpoint: string, query = '', body?: unknown): T {
    const response = httpSend({
      method,
      url: `${this.base}/v2/${endpoint}${query}`,
      headers: {
        Authorization: `Bearer ${this.token}`,
        ...(body === undefined ? {} : { 'Content-Type': 'application/json' }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      timeoutMs: 30_000,
    });
    if (response.status < 200 || response.status >= 300) {
      throw new Error(`garage ${endpoint} returned ${response.status}: ${response.body.trim()}`);
    }

    return (response.body ? JSON.parse(response.body) : undefined) as T;
  }

  pruneExpired(): number {
    const stale = this.call<KeyItem[]>('GET', 'ListKeys').filter(
      (key) => key.expired && key.name.startsWith(KEY_PREFIX),
    );
    for (const key of stale) this.call('POST', 'DeleteKey', `?id=${encodeURIComponent(key.id)}`);

    return stale.length;
  }

  mint(name: string, ttlSeconds: number): Key {
    const expiration = new Date(Date.now() + ttlSeconds * 1000).toISOString();
    const key = this.call<KeyInfo>('POST', 'CreateKey', '', {
      name,
      expiration,
      allow: { createBucket: true },
    });
    if (!key.secretAccessKey) throw new Error('garage CreateKey returned no secret');

    for (const bucket of this.call<{ id: string }[]>('GET', 'ListBuckets')) {
      this.call('POST', 'AllowBucketKey', '', {
        bucketId: bucket.id,
        accessKeyId: key.accessKeyId,
        permissions: { owner: true, read: true, write: true },
      });
    }

    return { accessKeyId: key.accessKeyId, secretAccessKey: key.secretAccessKey, expiration };
  }
}
