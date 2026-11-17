import { stringify } from 'yaml';
import type { App, Build, Target } from './types.ts';
import { HEADER } from './generated.ts';

export const ciProvider = (app: App) => (app.build as Build | undefined)?.ci ?? 'github';

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
          `!${dir}/application.yaml`,
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
                ...(test.setup ? { 'test-setup': test.setup } : {}),
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

const ON_MAIN = "github.ref == 'refs/heads/main'";

const deployScript = (
  skipCi: boolean,
) => String.raw`COMMITTED_AT=$(git show -s --format=%ct "$COMMIT_SHA")
git config --global user.name ci-image-updater
git config --global user.email ci-image-updater@git.anurag.sh
git clone "https://x-access-token:$FORGEJO_TOKEN@git.anurag.sh/anurag/k8s.git" config-repo
cd config-repo
PR_BRANCH="ci/update-$SERVICE_NAME-app-$SERVICE_NAME-$TAG"
TITLE="chore(deploy): update $SERVICE_NAME images to $TAG${skipCi ? ' [skip ci]' : ''}"
git checkout -b "$PR_BRANCH"
IMAGE="$IMAGE" yq -I2 -P -i '.images[strenv(IMAGE)] = strenv(TAG)' "$VALUES_FILE"
git add "$MANIFESTS_PATH"
git commit -m "$TITLE" || { echo "No changes to commit"; exit 0; }
git push -f origin "$PR_BRANCH"
METADATA=$(jq -cn --arg service "$SERVICE_NAME" --arg tag "$TAG" --arg images "$IMAGE" \
  --arg path "$MANIFESTS_PATH" --arg repo "$REPOSITORY" --arg ref "$REF_NAME" --arg run "$RUN_NUMBER" \
  --arg sha "$COMMIT_SHA" --arg committed_at "$COMMITTED_AT" \
  '{service: $service, tag: $tag, images: $images, path: $path, branch: "main", repo: $repo, ref: $ref, run: $run, sha: $sha, committed_at: $committed_at}')
BODY=$(printf '## Image Update\n\n| Field | Value |\n|-------|-------|\n| **Service** | \x60%s\x60 |\n| **Tag** | \x60%s\x60 |\n| **Manifests Path** | \x60%s\x60 |\n| **Target Branch** | \x60main\x60 |\n\n**Triggered by:** [%s@%s](%s)\n\n<!-- metadata:%s -->\n' \
  "$SERVICE_NAME" "$TAG" "$MANIFESTS_PATH" "$REPOSITORY" "$REF_NAME" "$RUN_URL" "$METADATA")
PAYLOAD=$(jq -n --arg title "$TITLE" --arg head "$PR_BRANCH" --arg body "$BODY" \
  '{title: $title, head: $head, base: "main", body: $body}')
CODE=$(curl -s -o response.json -w '%{http_code}' -X POST \
  -H "Authorization: token $FORGEJO_TOKEN" -H 'Content-Type: application/json' \
  -d "$PAYLOAD" https://git.anurag.sh/api/v1/repos/anurag/k8s/pulls)
case "$CODE" in
  2*) jq -r .html_url response.json ;;
  409) echo "PR already exists for $PR_BRANCH" ;;
  *) echo "failed to create PR (HTTP $CODE)"; cat response.json; exit 1 ;;
esac
`;

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
  const [registry, project] = build.image.split('/');
  const cacheRef = `type=registry,ref=${build.image}:buildcache`;

  const buildJob = {
    'runs-on': 'docker',
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
        run: `curl -fsSL https://${registry}/install.sh | sh\necho "$HOME/.local/bin" >> "$GITHUB_PATH"`,
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
      ...(test.setup ? [{ run: test.setup }] : []),
      {
        uses: ACTIONS.cache,
        with: {
          path: `${dir}/target`,
          key: `\${{ runner.os }}-app-target-\${{ hashFiles('**/Cargo.lock') }}-${app.name}`,
        },
      },
      { run: test.run, 'working-directory': `${'$'}{{ github.workspace }}/${dir}` },
    ],
  };

  const needs = test ? ['build', 'test'] : ['build'];
  const deployJob = {
    needs,
    if: `\${{ ${ON_MAIN} && !cancelled() && needs.build.result == 'success'${
      test ? " && needs.test.result == 'success'" : ''
    } }}`,
    'runs-on': 'docker',
    steps: [
      { uses: ACTIONS.checkout },
      {
        name: 'Raise image update PR',
        env: {
          FORGEJO_TOKEN: '${{ secrets.CI_IMAGE_UPDATER_TOKEN }}',
          SERVICE_NAME: app.name,
          IMAGE: build.image,
          TAG: '${{ github.sha }}',
          COMMIT_SHA: '${{ github.sha }}',
          MANIFESTS_PATH: dir,
          VALUES_FILE: `${dir}/image.values.yaml`,
          REPOSITORY: '${{ github.repository }}',
          REF_NAME: '${{ github.ref_name }}',
          RUN_NUMBER: '${{ github.run_number }}',
          RUN_URL:
            '${{ github.server_url }}/${{ github.repository }}/actions/runs/${{ github.run_number }}',
        },
        run: deployScript(!app.artifact),
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
          `!${dir}/application.yaml`,
          `!${dir}/app.yaml`,
          `!${dir}/image.values.yaml`,
          ...(build.paths ?? []),
          workflowPath(app),
        ],
      },
      workflow_dispatch: null,
    },
    'enable-openid-connect': true,
    jobs: { build: buildJob, ...(testJob ? { test: testJob } : {}), deploy: deployJob },
  };

  return `${HEADER}\n${stringify(workflow, { lineWidth: 0, aliasDuplicateObjects: false })}`;
}
