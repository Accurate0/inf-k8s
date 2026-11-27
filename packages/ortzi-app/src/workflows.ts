import { stringify } from 'yaml';
import type { App, AppWorkflow, Build, Target } from './types.ts';
import { appNamespace } from './app.ts';
import { envResource, openbaoPath, workflowResources } from './ci.ts';
import { HEADER } from './generated.ts';

export const ciProvider = (app: App) => (app.build as Build).ci;

export const workflowPath = (app: App) => `.${ciProvider(app)}/workflows/app-${app.name}.yaml`;

export const triggerPaths = (app: App, target: Target) => {
  const dir = `${target.location}/${app.name}`;

  return [
    `${dir}/**`,
    `!${dir}/manifests/**`,
    `!${dir}/app.yaml`,
    `!${dir}/image.values.yaml`,
    ...((app.build as Build).paths ?? []),
    workflowPath(app),
  ];
};

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
      push: { paths: triggerPaths(app, target) },
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
                        ...toolchainSetup(test.toolchain),
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

const nodejs = (version: string) =>
  `arch="$(uname -m | sed 's/x86_64/x64/;s/aarch64/arm64/')" && curl -fsSL "https://nodejs.org/dist/v${version}/node-v${version}-linux-$arch.tar.xz" | tar -xJ -C /opt && echo "/opt/node-v${version}-linux-$arch/bin" >> "$GITHUB_PATH" && "/opt/node-v${version}-linux-$arch/bin/npm" install -g pnpm`;

const toolchainSetup = (toolchain?: { rust?: string; node?: string }) => [
  ...(toolchain?.rust ? [rustup(toolchain.rust)] : []),
  ...(toolchain?.node ? [nodejs(toolchain.node)] : []),
];

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
      ...toolchainSetup(test.toolchain).map((run) => ({ run })),
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

  // flux's ImagePolicy deploys the highest v1.0.<n>-<sha> tag, so it lands only once tests pass
  const needs = test ? ['build', 'test'] : ['build'];
  const releaseJob = {
    needs,
    if: `\${{ ${ON_MAIN} && !cancelled() && needs.build.result == 'success'${
      test ? " && needs.test.result == 'success'" : ''
    } }}`,
    'runs-on': 'docker',
    steps: [
      { uses: ACTIONS.checkout, with: { 'fetch-depth': 0 } },
      { uses: ACTIONS.buildx },
      { uses: SETUP_ORTZI, with: { packages: 'app' } },
      { name: 'Log in to harbor', id: 'login', run: `ortzi registry login ${project}:push` },
      {
        name: 'Tag release',
        env: { DIGEST: '${{ needs.build.outputs.digest }}' },
        run: [
          `TAG=$(ortzi app version ${triggerPaths(app, target)
            .map((path) => `--path '${path}'`)
            .join(' ')})`,
          `docker buildx imagetools create --tag "${build.image}:$TAG" "${build.image}@$DIGEST"`,
        ].join('\n'),
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
      push: { paths: triggerPaths(app, target) },
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

const READ_SECRETS = 'https://git.anurag.sh/anurag/workflows/read-secrets@main';
const KUBE_SERVER = 'https://10.0.2.188:6443';

export const appWorkflowPath = (app: App, workflow: AppWorkflow) =>
  `.forgejo/workflows/app-${app.name}-${workflow.name}.yaml`;

export const appWorkflowPaths = (app: App) =>
  (app.workflows ?? []).map((workflow) => appWorkflowPath(app, workflow));

const envPrefix = (name: string) => `CI_${name.toUpperCase().replace(/[^A-Z0-9]/g, '_')}_`;

const kubeconfig = (namespace: string, kubectl: string) =>
  [
    'set -euo pipefail',
    'arch="$(uname -m | sed \'s/x86_64/amd64/;s/aarch64/arm64/\')"',
    `curl -fsSLo "$RUNNER_TEMP/kubectl" "https://dl.k8s.io/release/${kubectl}/bin/linux/$arch/kubectl"`,
    'chmod +x "$RUNNER_TEMP/kubectl"',
    'echo "$RUNNER_TEMP" >> "$GITHUB_PATH"',
    'export KUBECONFIG="$RUNNER_TEMP/kubeconfig"',
    'printf \'%s\\n\' "$KUBE_CA_CRT" > "$RUNNER_TEMP/kube-ca.crt"',
    `"$RUNNER_TEMP/kubectl" config set-cluster cluster --server=${KUBE_SERVER} --certificate-authority="$RUNNER_TEMP/kube-ca.crt" --embed-certs`,
    '"$RUNNER_TEMP/kubectl" config set-credentials ci --token="$KUBE_TOKEN"',
    `"$RUNNER_TEMP/kubectl" config set-context ci --cluster=cluster --user=ci --namespace=${namespace}`,
    '"$RUNNER_TEMP/kubectl" config use-context ci',
    'rm "$RUNNER_TEMP/kube-ca.crt"',
    'echo "KUBECONFIG=$KUBECONFIG" >> "$GITHUB_ENV"',
  ].join('\n');

export function renderAppWorkflow(
  app: App,
  workflow: AppWorkflow,
  actions: ActionVersions,
  tools: Record<string, string>,
): string {
  const path = appWorkflowPath(app, workflow);
  const resources = workflowResources(workflow);
  const env = Object.entries(workflow.env ?? {}).map(([key, value]) => {
    const resource = envResource(value);
    return [
      key,
      resource && typeof value !== 'string'
        ? `\${{ env.${envPrefix(resource)}${value.key} }}`
        : value,
    ];
  });

  const kubectl = tools.kubectl;
  if (workflow.serviceAccount && !kubectl) {
    throw new Error(
      'platform/versions.yaml: tools.kubectl is needed for workflows with serviceAccount',
    );
  }

  const steps = [
    { uses: pinned(actions, 'actions/checkout'), with: { 'persist-credentials': false } },
    ...resources.map((name) => ({
      name: `Read ${name} secrets`,
      uses: READ_SECRETS,
      with: { paths: openbaoPath(app, name), prefix: envPrefix(name) },
    })),
    ...(workflow.serviceAccount
      ? [
          {
            name: `Read ${workflow.serviceAccount} token`,
            uses: READ_SECRETS,
            with: { paths: openbaoPath(app, workflow.serviceAccount), prefix: 'KUBE_' },
          },
          {
            name: `Connect kubectl as ${workflow.serviceAccount}`,
            run: kubeconfig(appNamespace(app), kubectl),
          },
        ]
      : []),
    ...(workflow.setup ? [{ name: 'Setup', run: workflow.setup }] : []),
    {
      name: 'Run',
      ...(env.length ? { env: Object.fromEntries(env) } : {}),
      run: workflow.run,
      'working-directory': '${{ github.workspace }}',
    },
  ];

  const rendered = {
    name: `app ${app.name} ${workflow.name}`,
    on: {
      ...(workflow.schedule ? { schedule: [{ cron: workflow.schedule }] } : {}),
      ...(workflow.paths?.length
        ? { push: { branches: ['main'], paths: [...workflow.paths, path] } }
        : {}),
      workflow_dispatch: null,
    },
    concurrency: { group: `app-${app.name}-${workflow.name}`, 'cancel-in-progress': false },
    'enable-openid-connect': true,
    jobs: {
      run: {
        if: `\${{ ${ON_MAIN} }}`,
        'runs-on': 'docker',
        steps,
      },
    },
  };

  return `${HEADER}\n${stringify(rendered, { lineWidth: 0, aliasDuplicateObjects: false })}`;
}
