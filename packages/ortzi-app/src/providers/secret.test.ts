import assert from 'node:assert/strict';
import { test } from 'node:test';
import { validate, validateResources } from '../render.ts';
import { platform, render, source, base, checkRefs } from '../test-fixtures.ts';

test('a workload only gets the secrets it lists', () => {
  const rendered = render({
    name: 'foo-bar',
    workloads: [
      {
        name: 'foo-bar',
        image: 'ghcr.io/accurate0/foo-bar',
        envFrom: [{ secret: 'foo-bar' }, { database: 'foo-bar' }],
      },
    ],
    resources: [
      { name: 'foo-bar', type: 'secret', secretName: 'foo-bar-managed-secrets' },
      { name: 'foo-bar', type: 'database', secretName: 'foo-bar-database-secret' },
    ],
    network: { ingress: [] },
  });

  assert.deepEqual(source(rendered, 'k8s-workload')?.helm.valuesObject.workloads[0].envFrom, [
    { secretRef: { name: 'foo-bar-managed-secrets' } },
    { secretRef: { name: 'foo-bar-database-secret' } },
  ]);
  assert.deepEqual(source(rendered, 'k8s-workload')?.helm.valueFiles, [
    '$values/projects/foo-bar/image.values.yaml',
  ]);
  assert.equal(
    source(rendered, 'k8s-postgres')?.helm.valuesObject.databases[0].databaseName,
    'foo_bar',
  );
});

test('rejects duplicate secrets and a secret used in env', () => {
  const secret = { name: 'foo', type: 'secret', secretName: 'foo-managed-secrets' };

  assert.throws(
    () => validate({ ...base, resources: [secret, secret] }, 'test'),
    /duplicate name foo/,
  );
  assert.throws(
    () =>
      validate(
        {
          ...base,
          resources: [secret],
          workloads: [{ name: 'foo', image: 'i', env: { A: { resource: 'foo' } } }],
        },
        'test',
      ),
    /list a secret in envFrom/,
  );
});

test('secret resources default their kubernetes secret name', () => {
  const app = validate(
    {
      ...base,
      resources: [
        { name: 'foo', type: 'secret', secretName: 'foo-managed-secrets' },
        { name: 'other', type: 'secret', secretName: 'custom' },
      ],
    },
    'test',
  );

  assert.deepEqual(
    source(render(app), 'k8s-secrets')?.helm.valuesObject.secrets.map(
      (s: { managedSecrets: unknown }) => s.managedSecrets,
    ),
    [[{ secretName: 'foo-managed-secrets' }], [{ secretName: 'custom' }]],
  );
});

test('rejects the old secrets block', () => {
  assert.throws(() => validate({ ...base, secrets: ['foo'] }, 'test'), /unknown block "secrets"/);
});

test('workloads reference secrets by resource name', () => {
  const app = validate(
    {
      ...base,
      resources: [
        { name: 'foo', type: 'secret', secretName: 'foo-secret' },
        { name: 'foo', type: 'database', secretName: 'foo-db' },
        {
          name: 'images',
          type: 'bucket',
          bucket: 'b',
          provider: 'r2',
          secretName: 'images-secret',
        },
      ],
      workloads: [
        {
          name: 'foo',
          image: 'i',
          env: {
            A: { secret: 'foo', key: 'K' },
            B: { bucket: 'images', key: 'S3_ENDPOINT' },
          },
          envFrom: [
            { secret: 'foo', optional: true },
            { database: 'foo' },
            { bucket: 'images', prefix: 'X_' },
          ],
        },
      ],
    },
    'test',
  );
  const workload = source(render(app), 'k8s-workload')?.helm.valuesObject.workloads[0];

  assert.deepEqual(workload.envFrom, [
    { secretRef: { name: 'foo-secret', optional: true } },
    { secretRef: { name: 'foo-db' } },
    { prefix: 'X_', secretRef: { name: 'images-secret' } },
  ]);
  assert.deepEqual(workload.env, [
    { name: 'A', valueFrom: { secretKeyRef: { name: 'foo-secret', key: 'K' } } },
    { name: 'B', valueFrom: { secretKeyRef: { name: 'images-secret', key: 'S3_ENDPOINT' } } },
  ]);
  assert.deepEqual(
    source(render(app), 'k8s-secrets')?.helm.valuesObject.secrets[1].managedSecrets,
    [{ secretName: 'images-secret' }],
  );
});

test('rejects resources without a secret name and unknown references', () => {
  for (const type of ['secret', 'database']) {
    assert.throws(
      () => validate({ ...base, resources: [{ name: 'a', type }] }, 'test'),
      /resources/,
    );
  }
  assert.throws(
    () =>
      validate(
        {
          ...base,
          resources: [
            { name: 'a', type: 'oidc', displayName: 'a', preferShortUsername: true, host: 'a.sh' },
          ],
        },
        'test',
      ),
    /resources/,
  );
  assert.throws(
    () => validate({ ...base, resources: [{ name: 'a', type: 'bucket', provider: 'r2' }] }, 'test'),
    /resources/,
  );
  assert.throws(
    () =>
      validateResources({
        platform,
        apps: [
          validate(
            { ...base, workloads: [{ name: 'foo', image: 'i', envFrom: [{ secret: 'nope' }] }] },
            'test',
          ),
        ],
      }),
    /no secret resource named nope/,
  );
});

test('rejects a resource no workload uses', () => {
  const unused = (resource: Record<string, unknown>) => ({
    resources: [resource],
    workloads: [{ name: 'foo', image: 'i' }],
  });

  assert.throws(
    () => checkRefs(unused({ name: 'a', type: 'platform', export: 'tracing' })),
    /no workload uses this platform/,
  );
  assert.throws(
    () => checkRefs(unused({ name: 'a', type: 'secret', secretName: 's' })),
    /no workload uses this secret/,
  );
  checkRefs({
    resources: [{ name: 'a', type: 'secret', secretName: 's' }],
    workloads: [
      { name: 'foo', image: 'i', initContainers: [{ image: 'j', envFrom: [{ secret: 'a' }] }] },
    ],
  });
});

test('secret templates can exclude other keys', () => {
  const rendered = render({
    name: 'foo',
    network: { ingress: [] },
    resources: [
      {
        name: 'foo',
        type: 'secret',
        secretName: 's',
        secretTemplate: { includeAllSecrets: false, data: { a: 'b' } },
      },
    ],
  });

  assert.deepEqual(source(rendered, 'k8s-secrets')?.helm.valuesObject.secrets[0].managedSecrets, [
    { secretName: 's', template: { includeAllSecrets: false, data: { a: 'b' } } },
  ]);
  validate(
    {
      name: 'foo',
      network: { ingress: [] },
      resources: [
        {
          name: 'foo',
          type: 'secret',
          secretName: 's',
          secretTemplate: { includeAllSecrets: false, data: { a: 'b' } },
        },
      ],
    },
    'x',
  );
});

test('a generated secret pushes each key to openbao and syncs it back', () => {
  const app = validate(
    {
      ...base,
      resources: [
        {
          name: 'app-key',
          type: 'secret',
          secretName: 'foo-app-key',
          generate: {
            APP_KEY: { length: 32, symbols: 0 },
            SESSION_SECRET: { length: 64, digits: 10, symbols: 4, encoding: 'hex' },
          },
        },
        { name: 'foo', type: 'secret', secretName: 'foo-secrets' },
      ],
      workloads: [{ name: 'foo', image: 'i', envFrom: [{ secret: 'app-key' }, { secret: 'foo' }] }],
    },
    'test',
  );
  const values = source(render(app), 'k8s-secrets')?.helm.valuesObject;

  assert.deepEqual(values.secrets, [
    { name: 'foo', managedSecrets: [{ secretName: 'foo-secrets' }] },
  ]);
  assert.deepEqual(values.generated, [
    {
      name: 'app-key',
      secretName: 'foo-app-key',
      keys: [
        { name: 'APP_KEY', length: 32, symbols: 0 },
        { name: 'SESSION_SECRET', length: 64, digits: 10, symbols: 4, encoding: 'hex' },
      ],
    },
  ]);

  const generated = (generate: unknown) =>
    validate(
      {
        ...base,
        resources: [{ name: 'k', type: 'secret', secretName: 'k', generate }],
        workloads: [{ name: 'foo', image: 'i', envFrom: [{ secret: 'k' }] }],
      },
      'test',
    );
  assert.throws(() => generated({}), /generate/);
  assert.throws(
    () => generated({ KEY: { length: 32 } }),
    /resources\[0\].generate.KEY requires property "symbols"/,
  );
  assert.throws(() => generated({ 'a-b': { length: 8, symbols: 0 } }), /letters, digits/);
  assert.throws(
    () => generated({ A_B: { length: 8, symbols: 0 }, a_b: { length: 8, symbols: 0 } }),
    /a_b clashes with A_B/,
  );
});
