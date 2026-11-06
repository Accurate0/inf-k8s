import assert from 'node:assert/strict';
import { test } from 'node:test';
import { type Upstream, handle } from './lab.ts';

const port = 4280;

const upstream: Upstream = {
  page: () => '<html>lab</html>',
  forward: (body, accept) => ({ status: 200, body: JSON.stringify({ body, accept }) }),
};

const request = (method: string, headers: Record<string, string>, body = '') => ({
  method,
  path: '/',
  headers,
  body,
});

test('serves the lab page to localhost', () => {
  const response = handle(request('GET', { host: 'localhost:4280' }), port, upstream);

  assert.equal(response.status, 200);
  assert.equal(response.body, '<html>lab</html>');
  assert.match(response.headers!['content-type'], /text\/html/);
});

test('forwards the page’s graphql requests', () => {
  const response = handle(
    request(
      'POST',
      { host: '127.0.0.1:4280', origin: 'http://127.0.0.1:4280', accept: 'application/json' },
      '{"query":"{ a }"}',
    ),
    port,
    upstream,
  );

  assert.deepEqual(JSON.parse(response.body!), {
    body: '{"query":"{ a }"}',
    accept: 'application/json',
  });
});

test('refuses other hosts and origins without touching the router', () => {
  const untouched: Upstream = {
    page: () => assert.fail('served the page'),
    forward: () => assert.fail('forwarded a request'),
  };

  const rebinding = handle(request('POST', { host: 'evil.example:4280' }), port, untouched);
  assert.equal(rebinding.status, 403);

  const crossSite = handle(
    request('POST', { host: 'localhost:4280', origin: 'https://evil.example' }),
    port,
    untouched,
  );
  assert.equal(crossSite.status, 403);

  const otherPort = handle(
    request('POST', { host: 'localhost:4280', origin: 'http://localhost:3000' }),
    port,
    untouched,
  );
  assert.equal(otherPort.status, 403);
});

test('only GET and POST', () => {
  assert.equal(handle(request('DELETE', { host: 'localhost:4280' }), port, upstream).status, 405);
});
