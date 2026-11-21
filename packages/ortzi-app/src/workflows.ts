import { stringify } from 'yaml';
import type { App, Build, Target } from './types.ts';
import { HEADER } from './generated.ts';

export const ciProvider = (app: App) => (app.build as Build).ci;

export const workflowPath = (app: App) => `.${ciProvider(app)}/workflows/app-${app.name}.yaml`;

export function renderWorkflow(app: App, target: Target, actions: ActionVersions = {}): string {
  if (ciProvider(app) === 'forgejo') {
    return renderForgejoWorkflow(app, target, actions);
  }

  const build = app.build as Build;
  const dir = `${target.location}/${app.name}`;
  const context = build.context ?? dir;
  const args = Object.entries(build.args ?? {}).map(([k, v]) => `${k}=${v}`);
  const test = build.test;

  const workflow = {
    name: `app ${app.name}`,
    on: {
      push: {
        paths: [
          `${dir}/**`,
          `!${dir}/manifests/**`,
          `!${dir}/app.yaml`,
          `!${dir}/image.values.yaml`,
          ...(build.paths ?? []),
          workflowPath(app),
        ],
      },
      workflow_dispatch: null,
    },
    jobs: {
      app: {
        permissions: { contents: 'read', packages: 'write' },
        uses: './.github/workflows/app-build.yaml',
        with: {
          service: app.name,
          directory: dir,
          image: build.image,
          context,
          dockerfile: build.dockerfile ?? `${dir}/Dockerfile`,
          ...(args.length ? { 'build-args': args.join('\n') } : {}),
          ...(test
            ? {
                'test-run': test.run,
                ...(test.db ? { 'test-db-image': test.db.image } : {}),
                ...(test.db?.databaseName ? { 'test-db-name': test.db.databaseName } : {}),
                ...(test.packages ? { 'test-packages': test.packages.join(' ') } : {}),
                ...(test.toolchain || test.setup
                  ? {
                      'test-setup': [
                        ...(test.toolchain ? [rustup(test.toolchain.rust)] : []),
                        ...(test.setup ? [test.setup] : []),
                      ].join('\n'),
                    }
                  : {}),
              }
            : {}),
          deploy: "${{ github.ref == 'refs/heads/main' }}",
        },
        secrets: 'inherit',
      },
    },
  };

  return `${HEADER}\n${stringify(workflow, { lineWidth: 0, aliasDuplicateObjects: false })}`;
}

export type ActionVersions = Record<string, string>;

const pinned = (actions: ActionVersions, name: string) => {
  const version = actions[name];
  if (!version?.includes('@')) {
    throw new Error(`platform/versions.yaml: actions.${name} needs <tag>@<sha>`);
  }
  return `${name}@${version.split('@')[1]}`;
};

const rustup = (version: string) =>
  `curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain ${version} && echo "$HOME/.cargo/bin" >> "$GITHUB_PATH"`;

const SETUP_ORTZI = 'https://git.anurag.sh/anurag/workflows/setup-ortzi@main';

const ON_MAIN = "github.ref == 'refs/heads/main'";

function renderForgejoWorkflow(app: App, target: Target, actions: ActionVersions): string {
  const build = app.build as Build;
  const ACTIONS = {
    checkout: pinned(actions, 'actions/checkout'),
    buildx: pinned(actions, 'docker/setup-buildx-action'),
    buildPush: pinned(actions, 'docker/build-push-action'),
    cache: pinned(actions, 'actions/cache'),
  };
  const dir = `${target.location}/${app.name}`;
  const context = build.context ?? dir;
  const args = Object.entries(build.args ?? {}).map(([k, v]) => `${k}=${v}`);
  const test = build.test;
  const [, project] = build.image.split('/');
  const cacheRef = `type=registry,ref=${build.image}:buildcache`;

  const buildJob = {
    'runs-on': 'docker',
    outputs: {
      digest: '${{ steps.build.outputs.digest }}',
      'committed-at': '${{ steps.commit-meta.outputs.committed-at }}',
    },
    steps: [
      { uses: ACTIONS.checkout },
      {
        name: 'Compute commit metadata',
        id: 'commit-meta',
        run: 'echo "committed-at=$(git show -s --format=%ct ${{ github.sha }})" >> "$GITHUB_OUTPUT"',
      },
      { uses: ACTIONS.buildx },
      {
        name: 'Install ortzi',
        if: ON_MAIN,
        uses: SETUP_ORTZI,
      },
      {
        name: 'Log in to harbor',
        id: 'login',
        if: ON_MAIN,
        run: `ortzi registry login ${project}:push`,
      },
      {
        id: 'build',
        uses: ACTIONS.buildPush,
        with: {
          context,
          file: build.dockerfile ?? `${dir}/Dockerfile`,
          push: `\${{ ${ON_MAIN} }}`,
          tags: `${build.image}:latest,${build.image}:\${{ github.sha }}`,
          labels: [
            'net.inf-k8s.commit-sha=${{ github.sha }}',
            'net.inf-k8s.committed-at=${{ steps.commit-meta.outputs.committed-at }}',
          ].join('\n'),
          ...(args.length ? { 'build-args': args.join('\n') } : {}),
          'cache-from': cacheRef,
          'cache-to': `\${{ ${ON_MAIN} && '${cacheRef},mode=max' || '' }}`,
        },
      },
      {
        name: 'Sign image',
        if: ON_MAIN,
        env: { DIGEST: '${{ steps.build.outputs.digest }}' },
        run: `ortzi registry sign "${build.image}@$DIGEST"`,
      },
      {
        name: 'Log out of harbor',
        if: `\${{ always() && steps.login.outcome == 'success' }}`,
        run: 'ortzi registry logout',
      },
    ],
  };

  const testJob = test && {
    'runs-on': 'docker',
    ...(test.db
      ? {
          services: {
            db: {
              image: test.db.image,
              env: {
                POSTGRES_USER: 'postgres',
                POSTGRES_PASSWORD: 'postgres',
                POSTGRES_DB: test.db.databaseName ?? 'postgres',
              },
              options:
                '--health-cmd "pg_isready -U postgres" --health-interval 10s --health-timeout 5s --health-retries 5',
            },
          },
        }
      : {}),
    env: {
      ...(test.db
        ? {
            DATABASE_URL: `postgres://postgres:postgres@db:5432/${test.db.databaseName ?? 'postgres'}`,
          }
        : {}),
      SQLX_OFFLINE: 'true',
    },
    steps: [
      { uses: ACTIONS.checkout },
      ...(test.packages
        ? [{ run: `apt-get update && apt-get install -y ${test.packages.join(' ')}` }]
        : []),
      ...(test.toolchain ? [{ run: rustup(test.toolchain.rust) }] : []),
      ...(test.setup ? [{ run: test.setup }] : []),
      {
        uses: ACTIONS.cache,
        with: {
          path: [`${dir}/target`, '~/.cargo/registry'].join('\n'),
          key: `\${{ runner.os }}-app-target-\${{ hashFiles('${dir}/Cargo.lock') }}-${app.name}`,
        },
      },
      { run: test.run, 'working-directory': `${'$'}{{ github.workspace }}/${dir}` },
    ],
  };

  // flux's ImagePolicy deploys the newest main-<time>-<sha> tag, so it lands only once tests pass
  const needs = test ? ['build', 'test'] : ['build'];
  const releaseJob = {
    needs,
    if: `\${{ ${ON_MAIN} && !cancelled() && needs.build.result == 'success'${
      test ? " && needs.test.result == 'success'" : ''
    } }}`,
    'runs-on': 'docker',
    steps: [
      { uses: ACTIONS.buildx },
      { uses: SETUP_ORTZI },
      { name: 'Log in to harbor', id: 'login', run: `ortzi registry login ${project}:push` },
      {
        name: 'Tag release',
        env: {
          DIGEST: '${{ needs.build.outputs.digest }}',
          TAG: 'main-${{ needs.build.outputs.committed-at }}-${{ github.sha }}',
        },
        run: `docker buildx imagetools create --tag "${build.image}:$TAG" "${build.image}@$DIGEST"`,
      },
      {
        name: 'Log out of harbor',
        if: `\${{ always() && steps.login.outcome == 'success' }}`,
        run: 'ortzi registry logout',
      },
    ],
  };

  const workflow = {
    name: `app ${app.name}`,
    on: {
      push: {
        paths: [
          `${dir}/**`,
          `!${dir}/manifests/**`,
          `!${dir}/app.yaml`,
          `!${dir}/image.values.yaml`,
          ...(build.paths ?? []),
          workflowPath(app),
        ],
      },
      workflow_dispatch: null,
    },
    'enable-openid-connect': true,
    jobs: {
      build: buildJob,
      ...(testJob ? { test: testJob } : {}),
      release: releaseJob,
    },
  };

  return `${HEADER}\n${stringify(workflow, { lineWidth: 0, aliasDuplicateObjects: false })}`;
}
