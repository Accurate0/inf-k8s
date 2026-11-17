import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parse } from 'yaml';
import { renderAppTerraform, renderTerraformIndex } from '../terraform.ts';
import { addRoles, renderDatabase } from '../providers/database.ts';
import { type App, appRoles, validate, validateResources } from '../render.ts';
import { platform, render, source, base } from '../test-fixtures.ts';

test('database hosts come from platform exports', () => {
  const rendered = render({
    name: 'foo',
    resources: [
      { name: 'foo', type: 'database', secretName: 'foo-database-secret' },
      {
        name: 'direct',
        type: 'database',
        secretName: 'direct-database-secret',
        enableConnectionPooling: false,
      },
    ],
    network: { ingress: [] },
  });
  const [pooled, direct] = source(rendered, 'k8s-postgres')?.helm.valuesObject.databases;

  assert.equal(pooled.host, 'pooler.db');
  assert.equal(direct.host, 'direct.db');
});

test('renders a database owned by its role', () => {
  assert.deepEqual(parse(renderDatabase('foo-bar')), {
    apiVersion: 'postgresql.cnpg.io/v1',
    kind: 'Database',
    metadata: { name: 'foo-bar', namespace: 'cnpg-system' },
    spec: {
      name: 'foo_bar',
      owner: 'foo_bar',
      cluster: { name: 'cloudnative-pg-cluster' },
      databaseReclaimPolicy: 'retain',
    },
  });
});

test('only adds missing roles and never removes one', () => {
  const cluster = '# keep\nspec:\n  managed:\n    roles:\n      - name: old\n        login: true\n';

  assert.equal(addRoles(cluster, ['old']), cluster);
  const added = addRoles(cluster, ['new']);

  assert.match(added, /# keep/);
  assert.deepEqual(parse(added).spec.managed.roles, [
    { name: 'old', login: true },
    { name: 'new', ensure: 'present', connectionLimit: -1, inherit: true, login: true },
  ]);
  assert.deepEqual(
    appRoles([
      {
        name: 'a',
        network: { ingress: [] },
        resources: [
          { name: 'foo-bar', type: 'database', secretName: 'foo-bar-database-secret' },
          { name: 'baz', type: 'database', secretName: 'baz-database-secret' },
        ],
      },
    ]),
    ['foo_bar', 'baz'],
  );
});

test('terraform covers an app database', () => {
  const { resource } = JSON.parse(
    renderAppTerraform({
      ...base,
      resources: [{ name: 'foo-bar', type: 'database', secretName: 'foo-bar-database-secret' }],
    }) as string,
  );

  assert.deepEqual(
    resource.vault_database_secret_backend_static_role['foo-bar'].username,
    'foo_bar',
  );
  assert.equal(resource.vault_policy['foo-bar'].name, 'db-foo-bar');
  assert.deepEqual(
    resource.vault_kubernetes_auth_backend_role['foo-bar'].bound_service_account_namespaces,
    ['foo'],
  );
});

test('the terraform index lists database names sorted', () => {
  const index = JSON.parse(
    renderTerraformIndex([
      { ...base, resources: [{ name: 'z', type: 'database', secretName: 'z-database-secret' }] },
      {
        ...base,
        name: 'a',
        resources: [{ name: 'b', type: 'database', secretName: 'b-database-secret' }],
      },
    ]),
  );

  assert.deepEqual(index.output.database_names.value, ['b', 'z']);
});

test('rejects a database resource used in env', () => {
  const app = validate(
    {
      ...base,
      resources: [{ name: 'db', type: 'database', secretName: 'db-database-secret' }],
      workloads: [{ name: 'foo', image: 'i', env: { DB: { resource: 'db' } } }],
    },
    'test',
  );

  assert.throws(() => validateResources({ platform, apps: [app] }), /in envFrom, not env/);
});

test('rejects the old databases block', () => {
  assert.throws(
    () => validate({ ...base, databases: { foo: {} } }, 'test'),
    /unknown block "databases"/,
  );
});

test('a secret can share a name with a database', () => {
  const app = validate(
    {
      ...base,
      resources: [
        { name: 'foo', type: 'secret', secretName: 'foo-managed-secrets' },
        { name: 'foo', type: 'database', secretName: 'foo-database-secret' },
      ],
    },
    'test',
  );

  assert.equal(source(render(app), 'k8s-secrets')?.helm.valuesObject.secrets[0].name, 'foo');
  assert.equal(source(render(app), 'k8s-postgres')?.helm.valuesObject.databases[0].name, 'foo');
});

test('rejects references to missing database and bucket resources', () => {
  const missing = (envFrom: unknown[]) =>
    validateResources({
      platform,
      apps: [validate({ ...base, workloads: [{ name: 'foo', image: 'i', envFrom }] }, 'test')],
    });

  assert.throws(() => missing([{ database: 'nope' }]), /no database resource named nope/);
  assert.throws(() => missing([{ bucket: 'nope' }]), /no bucket resource named nope/);
});

test('database role and extensions flow through every output', () => {
  const app: App = {
    name: 'foo',
    network: { ingress: [] },
    resources: [
      {
        name: 'foo',
        type: 'database',
        secretName: 'foo-db',
        role: 'foo_2',
        extensions: ['vector'],
      },
    ],
  };

  assert.deepEqual(appRoles([app]), ['foo_2']);
  assert.equal(
    source(render(app), 'k8s-postgres')?.helm.valuesObject.databases[0].databaseName,
    'foo_2',
  );
  const database = parse(renderDatabase('foo', { role: 'foo_2', extensions: ['vector'] }));

  assert.equal(database.metadata.name, 'foo');
  assert.equal(database.spec.name, 'foo_2');
  assert.equal(database.spec.owner, 'foo_2');
  assert.deepEqual(database.spec.extensions, [{ name: 'vector', ensure: 'present' }]);
  const terraform = JSON.parse(renderAppTerraform(app) as string);

  assert.equal(terraform.resource.vault_database_secret_backend_static_role.foo.username, 'foo_2');
});

test('a missing database export is an error', () => {
  const app = validate(
    {
      name: 'foo',
      network: { ingress: [] },
      resources: [{ name: 'foo', type: 'database', secretName: 'foo-secret' }],
      workloads: [
        {
          name: 'api',
          image: 'ghcr.io/x/api',
          resources: { memory: '1Mi' },
          envFrom: [{ database: 'foo' }],
        },
      ],
    },
    'x',
  );

  assert.throws(
    () => validateResources({ platform: [], apps: [app] }),
    /no database export named pooled/,
  );
});
