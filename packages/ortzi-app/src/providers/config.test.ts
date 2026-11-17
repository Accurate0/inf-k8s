import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parse } from 'yaml';
import { renderKustomization, validate, validateResources } from '../render.ts';
import { target, platform, render, source, base } from '../test-fixtures.ts';

test('a config resource generates its configmap and mount', () => {
  const app = validate(
    {
      ...base,
      resources: [
        {
          name: 'rules',
          type: 'config',
          configMap: 'foo-rules',
          files: ['config.yaml', 'rules/a.yaml'],
        },
      ],
      workloads: [
        { name: 'foo', image: 'i', mounts: { rules: { path: '/etc/rules', config: 'rules' } } },
      ],
    },
    'test',
  );
  const workload = source(render(app), 'k8s-workload')?.helm.valuesObject.workloads[0];

  assert.deepEqual(workload.volumes, [
    {
      name: 'rules',
      configMap: {
        name: 'foo-rules',
        items: [
          { key: 'config.yaml', path: 'config.yaml' },
          { key: 'a.yaml', path: 'rules/a.yaml' },
        ],
      },
    },
  ]);
  assert.deepEqual(parse(renderKustomization(app, target) as string).configMapGenerator, [
    { name: 'foo-rules', namespace: 'foo', files: ['config.yaml', 'rules/a.yaml'] },
  ]);
  assert.throws(
    () =>
      validateResources({
        platform,
        apps: [
          validate(
            {
              ...base,
              workloads: [
                { name: 'foo', image: 'i', mounts: { r: { path: '/r', config: 'nope' } } },
              ],
            },
            'test',
          ),
        ],
      }),
    /no config resource named nope/,
  );
});

test('a config with values renders through a secret template', () => {
  const app = validate(
    {
      ...base,
      resources: [
        {
          name: 'login',
          type: 'oidc',
          displayName: 'login',
          preferShortUsername: true,
          host: 'foo.anurag.sh',
          secretName: 'foo-oidc',
        },
        { name: 'foo', type: 'database', secretName: 'foo-db' },
        {
          name: 'settings',
          type: 'config',
          configMap: 'foo-settings',
          files: ['config/app.json'],
          secretName: 'foo-settings-rendered',
          values: {
            CLIENT_ID: { oidc: 'login', key: 'client-id' },
            DB_PASSWORD: { database: 'foo', key: 'PGPASSWORD' },
          },
        },
      ],
      workloads: [
        {
          name: 'foo',
          image: 'i',
          mounts: { settings: { path: '/config', config: 'settings' } },
        },
      ],
    },
    'test',
  );
  validateResources({ platform, apps: [app] });
  const rendered = render(app);

  assert.deepEqual(source(rendered, 'k8s-secrets')?.helm.valuesObject.secrets, []);
  assert.deepEqual(source(rendered, 'k8s-secrets')?.helm.valuesObject.templates, [
    {
      secretName: 'foo-settings-rendered',
      configMap: 'foo-settings',
      files: ['config/app.json'],
      values: [
        { name: 'CLIENT_ID', secret: 'foo-oidc', key: 'client-id' },
        { name: 'DB_PASSWORD', secret: 'foo-db', key: 'PGPASSWORD' },
      ],
    },
  ]);
  const workload = source(rendered, 'k8s-workload')?.helm.valuesObject.workloads[0];
  assert.deepEqual(workload.volumes, [
    {
      name: 'settings',
      secret: {
        secretName: 'foo-settings-rendered',
        items: [{ key: 'app.json', path: 'config/app.json' }],
      },
    },
  ]);
  assert.deepEqual(workload.volumeMounts, [
    { name: 'settings', mountPath: '/config', readOnly: true },
  ]);
  assert.deepEqual(parse(renderKustomization(app, target) as string).configMapGenerator, [
    { name: 'foo-settings', namespace: 'foo', files: ['config/app.json'] },
  ]);

  const config = (extra: Record<string, unknown>) =>
    validate(
      {
        ...base,
        resources: [{ name: 'c', type: 'config', configMap: 'c', files: ['a'], ...extra }],
      },
      'test',
    );
  assert.throws(() => config({ secretName: 's' }), /set secretName and values together/);
  assert.throws(
    () => config({ values: { A: { secret: 'x', key: 'k' } } }),
    /set secretName and values together/,
  );
  assert.throws(
    () => config({ secretName: 's', values: { A: { secret: 'x' } } }),
    /resources\[0\].values.A is not exactly one/,
  );
  assert.throws(
    () =>
      validateResources({
        platform,
        apps: [config({ secretName: 's', values: { A: { secret: 'missing', key: 'k' } } })],
      }),
    /no secret resource named missing/,
  );
});

test('a secret used only by a config template counts as used', () => {
  const app = validate(
    {
      ...base,
      resources: [
        { name: 'creds', type: 'secret', secretName: 'foo-creds' },
        {
          name: 'settings',
          type: 'config',
          configMap: 'foo-settings',
          files: ['app.json'],
          secretName: 'foo-settings-rendered',
          values: { TOKEN: { secret: 'creds', key: 'TOKEN' } },
        },
      ],
      workloads: [{ name: 'foo', image: 'i', mounts: { s: { path: '/s', config: 'settings' } } }],
    },
    'test',
  );

  validateResources({ platform, apps: [app] });
});
