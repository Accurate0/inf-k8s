import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parse } from 'yaml';
import { type App, renderWorkflow, workflowPath, validate } from './render.ts';

test('generates a workflow from the build block', () => {
  const app: App = {
    name: 'foo',
    build: {
      ci: 'github',
      image: 'ghcr.io/accurate0/foo',
      context: '.',
      args: { BINARY_NAME: 'foo' },
      paths: ['crates/x/**'],
      test: {
        run: 'cargo test',
        db: { image: 'postgres:17', databaseName: 'foo' },
        packages: ['a', 'b'],
      },
    },
    network: { ingress: [] },
  };
  const workflow = parse(renderWorkflow(app, { location: 'projects', hasManifests: false }));
  const paths = workflow.on.push.paths;

  assert.ok(paths.includes('projects/foo/**'));
  assert.ok(paths.includes('!projects/foo/image.values.yaml'));
  assert.ok(paths.includes('crates/x/**'));
  assert.ok(paths.includes('.github/workflows/app-foo.yaml'));
  const inputs = workflow.jobs.app.with;

  assert.equal(inputs.dockerfile, 'projects/foo/Dockerfile');
  assert.equal(inputs.context, '.');
  assert.equal(inputs['build-args'], 'BINARY_NAME=foo');
  assert.equal(inputs['test-packages'], 'a b');
  assert.equal(inputs['test-db-image'], 'postgres:17');
  assert.equal(inputs['test-db-name'], 'foo');
  assert.equal(workflow.jobs.app.uses, './.github/workflows/app-build.yaml');
});

test('generates a forgejo workflow that pushes to the build image registry', () => {
  const app: App = {
    name: 'foo',
    build: {
      ci: 'forgejo',
      image: 'registry.anurag.sh/containers/foo',
      ci: 'forgejo',
      test: { run: 'cargo test', db: { image: 'postgres:17', databaseName: 'foo' } },
    },
    network: { ingress: [] },
  };
  const actions = {
    'actions/checkout': 'v7@aaa',
    'docker/setup-buildx-action': 'v4@bbb',
    'docker/build-push-action': 'v7@ddd',
    'actions/cache': 'v6@eee',
  };
  const workflow = parse(
    renderWorkflow(app, { location: 'projects', hasManifests: false }, actions),
  );

  assert.equal(workflow.jobs.build.steps[0].uses, 'actions/checkout@aaa');
  assert.throws(
    () => renderWorkflow(app, { location: 'projects', hasManifests: false }, {}),
    /actions\.actions\/checkout/,
  );
  assert.equal(workflowPath(app), '.forgejo/workflows/app-foo.yaml');
  assert.ok(workflow.on.push.paths.includes('.forgejo/workflows/app-foo.yaml'));
  const step = (name: string) =>
    workflow.jobs.build.steps.find((step: { name?: string }) => step.name === name);

  assert.equal(workflow['enable-openid-connect'], true);
  assert.equal(
    step('Install ortzi').uses,
    'https://git.anurag.sh/anurag/workflows/setup-ortzi@main',
  );
  assert.equal(step('Log in to harbor').run, 'ortzi registry login containers:push');
  assert.equal(
    step('Sign image').run,
    'ortzi registry sign "registry.anurag.sh/containers/foo@$DIGEST"',
  );
  assert.equal(step('Sign image').env.DIGEST, '${{ steps.build.outputs.digest }}');
  assert.match(step('Log out of harbor').if, /always\(\) && steps\.login\.outcome == 'success'/);
  assert.ok(
    !workflow.jobs.build.steps.some((s: { uses?: string }) => s.uses?.includes('login-action')),
  );
  assert.equal(workflow.jobs.test.env.DATABASE_URL, 'postgres://postgres:postgres@db:5432/foo');
  assert.equal(
    workflow.jobs.test.steps.at(-1)['working-directory'],
    '${{ github.workspace }}/projects/foo',
  );
  assert.deepEqual(workflow.jobs.deploy.needs, ['build', 'test']);
  assert.match(workflow.jobs.deploy.steps[1].run, /yq .*image.values|strenv\(IMAGE\)/);
  assert.equal(workflow.jobs.deploy.steps[1].env.VALUES_FILE, 'projects/foo/image.values.yaml');
  assert.equal(
    workflow.jobs.deploy.steps[1].env.FORGEJO_TOKEN,
    '${{ secrets.CI_IMAGE_UPDATER_TOKEN }}',
  );
});

test('installs the rust toolchain and caches by the app lockfile', () => {
  const app = {
    name: 'foo',
    workloads: [{ name: 'foo', image: 'registry.anurag.sh/containers/foo' }],
    build: {
      ci: 'forgejo',
      image: 'registry.anurag.sh/containers/foo',
      ci: 'forgejo',
      test: { run: 'cargo test', toolchain: { rust: '1.99.1' }, setup: 'echo extra' },
    },
    network: { ingress: [] },
  };
  const actions = {
    'actions/checkout': 'v7@aaa',
    'docker/setup-buildx-action': 'v4@bbb',
    'docker/build-push-action': 'v7@ddd',
    'actions/cache': 'v6@eee',
  };
  const steps = parse(renderWorkflow(app, { location: 'projects', hasManifests: false }, actions))
    .jobs.test.steps;
  const runs = steps.map((step: { run?: string }) => step.run).filter(Boolean);
  const cache = steps.find((step: { uses?: string }) => step.uses === 'actions/cache@eee');

  assert.match(runs[0], /--default-toolchain 1\.99\.1 /);
  assert.equal(runs[1], 'echo extra');
  assert.equal(cache.with.path, 'projects/foo/target\n~/.cargo/registry');
  assert.match(cache.with.key, /hashFiles\('projects\/foo\/Cargo\.lock'\)/);
  assert.equal(validate(app, 'x'), app);
  assert.throws(
    () =>
      validate(
        {
          ...app,
          build: { ci: 'forgejo', ...app.build, test: { run: 'x', toolchain: { rust: '1.99' } } },
        },
        'x',
      ),
    /toolchain\.rust does not match pattern/,
  );
});
