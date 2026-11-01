import assert from 'node:assert/strict';
import { test } from 'node:test';
import { planCommits } from './commits.ts';
import type { App } from './render.ts';

const app = (name: string, database?: string) =>
  ({ name, resources: database ? [{ type: 'database', name: database }] : [] }) as unknown as App;

test('groups each path under its app', () => {
  const paths = [
    'projects/bom/application.yaml',
    'platform-services/registry/manifests/rbac.generated.yaml',
    '.github/workflows/app-bom.yaml',
    'terraform/apps/registry.tf.json',
    'system-components/postgres-cluster/databases/registry-db.yaml',
  ];
  assert.deepEqual(planCommits(paths, [app('bom'), app('registry', 'registry-db')]), [
    {
      message: 'chore(render): update bom',
      paths: ['projects/bom/application.yaml', '.github/workflows/app-bom.yaml'],
    },
    {
      message: 'chore(render): update registry',
      paths: [
        'platform-services/registry/manifests/rbac.generated.yaml',
        'terraform/apps/registry.tf.json',
        'system-components/postgres-cluster/databases/registry-db.yaml',
      ],
    },
  ]);
});

test('attributes removed terraform to the app named by the file', () => {
  const [commit] = planCommits(['terraform/apps/gone.tf.json'], []);
  assert.equal(commit.message, 'chore(render): update gone');
});

test('sorts apps and puts shared files last', () => {
  const commits = planCommits(
    [
      'system-components/postgres-cluster/postgres.yaml',
      'terraform/apps/_index.tf.json',
      'projects/zeta/application.yaml',
      'projects/alpha/application.yaml',
    ],
    [],
  );
  assert.deepEqual(
    commits.map((commit) => commit.message),
    ['chore(render): update alpha', 'chore(render): update zeta', 'chore(render): update shared generated files'],
  );
  assert.deepEqual(commits[2].paths, ['system-components/postgres-cluster/postgres.yaml', 'terraform/apps/_index.tf.json']);
});

test('skips ci on every commit when only app directories changed', () => {
  const commits = planCommits(['projects/a/application.yaml', 'projects/b/application.yaml'], []);
  assert.deepEqual(
    commits.map((commit) => commit.message),
    ['chore(render): update a [skip ci]', 'chore(render): update b [skip ci]'],
  );
});

for (const path of ['.github/workflows/app-b.yaml', 'terraform/apps/b.tf.json', 'system-components/postgres-cluster/postgres.yaml']) {
  test(`keeps ci on every commit when ${path} changed`, () => {
    const commits = planCommits(['projects/a/application.yaml', path], []);
    assert.ok(commits.every((commit) => !commit.message.includes('[skip ci]')));
  });
}

test('plans nothing for no paths', () => {
  assert.deepEqual(planCommits([], []), []);
});
