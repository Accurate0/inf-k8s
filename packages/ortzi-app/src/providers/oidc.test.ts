import assert from 'node:assert/strict';
import { test } from 'node:test';
import { validate } from '../render.ts';
import { render, base, releaseValues } from '../test-fixtures.ts';

test('an oidc client secret can be referenced by resource name', () => {
  const app = validate(
    {
      ...base,
      resources: [
        {
          name: 'foo',
          type: 'oidc',
          displayName: 'foo',
          preferShortUsername: true,
          host: 'a.sh',
          secretName: 'foo-oidc',
        },
      ],
      workloads: [
        {
          name: 'foo',
          image: 'i',
          env: { AUD: { oidc: 'foo', key: 'client-id', optional: true } },
        },
      ],
    },
    'test',
  );
  const rendered = render(app);

  assert.deepEqual(releaseValues(rendered, 'k8s-workload').workloads[0].env, [
    {
      name: 'AUD',
      valueFrom: { secretKeyRef: { name: 'foo-oidc', key: 'client-id', optional: true } },
    },
  ]);
  assert.equal(releaseValues(rendered, 'k8s-oidc').clients[0].secretName, 'foo-oidc');
});

test('rejects unknown oidc fields and envFrom keys', () => {
  assert.throws(
    () =>
      validate(
        {
          ...base,
          resources: [
            {
              name: 'a',
              type: 'oidc',
              displayName: 'a',
              preferShortUsername: true,
              secretName: 's',
              host: 'a.sh',
              nope: 1,
            },
          ],
        },
        'test',
      ),
    /resources/,
  );
  assert.throws(
    () =>
      validate(
        { ...base, workloads: [{ name: 'foo', image: 'i', envFrom: [{ secrets: 'a' }] }] },
        'test',
      ),
    /envFrom/,
  );
});

test('the login policy on a route reaches its oidc client', () => {
  const app = validate(
    {
      ...base,
      resources: [
        {
          name: 'web',
          type: 'route',
          gateway: 'public-gateway',
          paths: ['/'],
          hosts: ['a.sh'],
          backend: { name: 's', port: 1 },
          auth: { mode: 'oidc', client: 'c', scopes: ['openid'], forwardAccessToken: true },
        },
        {
          name: 'c',
          type: 'oidc',
          displayName: 'c',
          preferShortUsername: true,
          host: 'a.sh',
          secretName: 's',
        },
      ],
    },
    'test',
  );
  const client = releaseValues(render(app), 'k8s-oidc').clients[0];

  assert.deepEqual(client.securityPolicy, {
    httpRoute: 'web',
    scopes: ['openid'],
    forwardAccessToken: true,
  });
  const bare = validate(
    {
      ...base,
      resources: [
        {
          name: 'web',
          type: 'route',
          gateway: 'public-gateway',
          paths: ['/'],
          hosts: ['a.sh'],
          backend: { name: 's', port: 1 },
          auth: { mode: 'oidc', client: 'c' },
        },
        {
          name: 'c',
          type: 'oidc',
          displayName: 'c',
          preferShortUsername: true,
          host: 'a.sh',
          secretName: 's',
        },
      ],
    },
    'test',
  );

  assert.deepEqual(releaseValues(render(bare), 'k8s-oidc').clients[0].securityPolicy, {
    httpRoute: 'web',
  });
  assert.throws(
    () =>
      validate(
        {
          ...base,
          resources: [
            {
              name: 'a',
              type: 'route',
              gateway: 'public-gateway',
              paths: ['/'],
              hosts: ['a.sh'],
              backend: { name: 's', port: 1 },
              auth: { mode: 'oidc', client: 'c' },
            },
            {
              name: 'b',
              type: 'route',
              gateway: 'public-gateway',
              paths: ['/'],
              hosts: ['a.sh'],
              backend: { name: 's', port: 1 },
              auth: { mode: 'oidc', client: 'c' },
            },
            {
              name: 'c',
              type: 'oidc',
              displayName: 'c',
              preferShortUsername: true,
              host: 'a.sh',
              secretName: 's',
            },
          ],
        },
        'test',
      ),
    /used by another oidc route/,
  );
});

test('oidc service accounts pass through to the chart', () => {
  const serviceAccounts = [
    {
      name: 'graph_bot',
      displayName: 'Graph bot',
      entryManagedBy: 'idm_admins',
      secretName: 'graph-bot-token',
      scopes: ['openid', 'groups'],
    },
  ];
  const app = validate(
    {
      ...base,
      resources: [
        {
          name: 'graph',
          type: 'oidc',
          displayName: 'Graph',
          preferShortUsername: true,
          host: 'a.sh',
          secretName: 's',
          groups: { graph_users: ['graph_bot'] },
          serviceAccounts,
        },
      ],
    },
    'test',
  );

  assert.deepEqual(
    releaseValues(render(app), 'k8s-oidc').clients[0].serviceAccounts,
    serviceAccounts,
  );
  assert.throws(
    () =>
      validate(
        {
          ...base,
          resources: [
            {
              name: 'graph',
              type: 'oidc',
              displayName: 'Graph',
              preferShortUsername: true,
              host: 'a.sh',
              secretName: 's',
              serviceAccounts: [{ name: 'graph_bot', displayName: 'Graph bot', secretName: 't' }],
            },
          ],
        },
        'test',
      ),
    /resources/,
  );
});
