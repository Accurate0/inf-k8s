import { httpSend } from '@ortzi/host';

interface Body {
  auth?: { client_token: string };
  data?: { data?: Record<string, string> };
  errors?: string[];
}

function call(
  url: string,
  init: { method?: string; headers?: Record<string, string>; body?: string },
) {
  const response = httpSend({ url, ...init, timeoutMs: 30_000 });

  let body: Body = {};
  try {
    body = JSON.parse(response.body) as Body;
  } catch {}

  if (response.status < 200 || response.status >= 300) {
    const detail = body.errors?.join('; ') || response.body.trim();
    throw new Error(`openbao returned ${response.status}: ${detail}`);
  }
  return body;
}

export function readSecret(addr: string, jwt: string, path: string): Record<string, string> {
  const base = addr.replace(/\/+$/, '');
  const token = call(`${base}/v1/auth/jwt/login`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ role: 'cli', jwt }),
  }).auth?.client_token;
  if (!token) throw new Error('openbao jwt login returned no token');

  const data = call(`${base}/v1/kv/data/${path}`, { headers: { 'X-Vault-Token': token } }).data
    ?.data;
  if (!data) throw new Error(`openbao has no secret at kv/${path}`);

  return data;
}
