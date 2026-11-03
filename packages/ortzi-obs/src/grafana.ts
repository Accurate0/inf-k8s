import { httpSend } from '@ortzi/host';

export type Query = [string, string][];

const encode = (query: Query) =>
  query.map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(v)}`).join('&');

export class Grafana {
  readonly base: string;

  constructor(
    base: string,
    private readonly token: string,
  ) {
    this.base = base.replace(/\/+$/, '');
  }

  get<T>(path: string, query: Query = []): T {
    const url = `${this.base}${path}${query.length ? `?${encode(query)}` : ''}`;
    const response = httpSend({
      url,
      headers: { 'X-JWT-Assertion': this.token },
      timeoutMs: 60_000,
    });
    if (response.status < 200 || response.status >= 300) {
      throw new Error(`grafana returned ${response.status}: ${response.body.trim()}`);
    }

    try {
      return JSON.parse(response.body) as T;
    } catch {
      throw new Error(`unexpected response from ${path}`);
    }
  }

  datasource<T>(uid: string, path: string, query: Query): T {
    return this.get(`/api/datasources/proxy/uid/${uid}${path}`, query);
  }
}
