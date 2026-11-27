import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parse, parseAllDocuments } from 'yaml';
import { renderAppWorkflow, renderRbac, validate } from './render.ts';
import { renderAppTerraform } from './terraform.ts';
import { base, checkRefs, releaseValues, render } from './test-fixtures.ts';

const actions = { 'actions/checkout': 'v7.0.1@abc' };
const tools = { kubectl: 'v1.37.1' };

const schemas = {
  ...base,
  resources: [
    { name: 'schemas', type: 'bucket', bucket: 'schemas', provider: 'r2', secretName: 's' },
    {
      name: 'reader',
      type: 'serviceAccount',
      clusterRules: [{ apiGroups: ['apiextensions.k8s.io'], resources: ['*'], verbs: ['list'] }],
    },
  ],
  ci: { secrets: { repos: ['anurag/k8s'], resources: ['schemas', 'reader'] } },
  workflows: [
    {
      name: 'publish',
      schedule: '0 0 * * 0',
      paths: ['charts/**'],
      serviceAccount: 'reader',
      setup: 'pip install x',
      run: './publish.sh',
      env: {
        AWS_ACCESS_KEY_ID: { bucket: 'schemas', key: 'S3_ACCESS_KEY_ID' },
        K8S_VERSION: 'v1.37.1',
      },
    },
  ],
};

test('a workflow reads its secrets and connects kubectl as its service account', () => {
  const app = validate(schemas, 'test');
  const workflow = parse(renderAppWorkflow(app, app.workflows![0], actions, tools));

  assert.equal(workflow.name, 'app foo publish');
  assert.deepEqual(workflow.on.schedule, [{ cron: '0 0 * * 0' }]);
  assert.deepEqual(workflow.on.push, {
    branches: ['main'],
    paths: ['charts/**', '.forgejo/workflows/app-foo-publish.yaml'],
  });
  assert.equal(workflow['enable-openid-connect'], true);

  const steps = workflow.jobs.run.steps;
  const read = steps.filter((step: { uses?: string }) => step.uses?.includes('read-secrets'));

  assert.deepEqual(
    read.map((step: { with: unknown }) => step.with),
    [
      { paths: 'foo/resource-schemas', prefix: 'CI_SCHEMAS_' },
      { paths: 'foo/generated/serviceaccount-reader', prefix: 'KUBE_' },
    ],
  );
  assert.match(
    steps.find((s: { name?: string }) => s.name?.startsWith('Connect')).run,
    /v1\.37\.1/,
  );

  const run = steps.at(-1);

  assert.equal(run.run, './publish.sh');
  assert.deepEqual(run.env, {
    AWS_ACCESS_KEY_ID: '${{ env.CI_SCHEMAS_S3_ACCESS_KEY_ID }}',
    K8S_VERSION: 'v1.37.1',
  });
});

test('ci.secrets grants the repo a read-only openbao role for the namespace', () => {
  const { resource } = JSON.parse(renderAppTerraform(validate(schemas, 'test')) as string);
  const role = resource.vault_jwt_auth_backend_role['foo-ci'];

  assert.equal(role.role_name, 'foo');
  assert.equal(role.backend, '${var.forgejo_auth_path}');
  assert.deepEqual(role.bound_claims, { repository: 'anurag/k8s', ref: 'refs/heads/main' });
  assert.deepEqual(role.bound_audiences, ['openbao-cli']);
  assert.equal(
    resource.vault_policy['foo-ci'].policy,
    [
      'path "${var.kv_mount}/data/foo/resource-schemas" {\n  capabilities = ["read"]\n}\n',
      'path "${var.kv_mount}/data/foo/generated/serviceaccount-reader" {\n  capabilities = ["read"]\n}\n',
    ].join('\n'),
  );
});

test('a service account granted to ci is created, given a token and pushed to openbao', () => {
  const app = validate(schemas, 'test');
  const rbac = parseAllDocuments(renderRbac(app) as string).map((doc) => doc.toJS());

  assert.deepEqual(
    rbac.map((doc: { kind: string }) => doc.kind),
    ['ServiceAccount', 'Secret', 'ClusterRole', 'ClusterRoleBinding'],
  );
  assert.equal(rbac[1].type, 'kubernetes.io/service-account-token');

  const values = releaseValues(render(app), 'k8s-secrets');

  assert.deepEqual(values.pushed, [
    {
      name: 'serviceaccount-reader',
      secretName: 'reader-ci-token',
      keys: [
        { secretKey: 'token', property: 'TOKEN' },
        { secretKey: 'ca.crt', property: 'CA_CRT' },
      ],
    },
  ]);
});

test('workflow secrets must be granted to this repo', () => {
  assert.throws(
    () =>
      validate(
        {
          ...schemas,
          ci: { secrets: { repos: ['anurag/other'], resources: ['schemas', 'reader'] } },
        },
        'test',
      ),
    /ci.secrets.repos must include anurag\/k8s/,
  );
  assert.throws(
    () =>
      validate(
        { ...schemas, ci: { secrets: { repos: ['anurag/k8s'], resources: ['schemas'] } } },
        'test',
      ),
    /reader is not in ci.secrets.resources/,
  );
  assert.throws(
    () =>
      validate(
        { ...schemas, ci: { secrets: { repos: ['anurag/k8s'], resources: ['nope'] } } },
        'test',
      ),
    /no bucket, secret or serviceAccount resource named nope/,
  );
});

test('a bucket only a workflow uses is not unused', () => {
  assert.doesNotThrow(() => checkRefs(schemas));
});
