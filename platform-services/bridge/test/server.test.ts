import assert from 'node:assert/strict';
import type { AddressInfo } from 'node:net';
import { test } from 'node:test';
import { Authenticator } from '../src/auth.ts';
import { imageValues } from '../src/flux.ts';
import { Forgejo } from '../src/forgejo.ts';
import { Kubernetes } from '../src/kubernetes.ts';
import { resolvers } from '../src/resolvers.ts';
import { executableSchema } from '../src/schema.ts';
import { server } from '../src/server.ts';
import { InMemorySpanExporter } from '@opentelemetry/sdk-trace-base';
import { startTracing } from '../src/tracing.ts';
import { fetcher, ISSUER, keypair } from './helpers.ts';

const APP = { annotations: { 'event.toolkit.fluxcd.io/app': 'graph' } };
const ready = {
  observedGeneration: 1,
  conditions: [{ type: 'Ready', status: 'True' }],
};

const kube = {
  '/kustomizations': {
    items: [
      {
        metadata: { name: 'graph', generation: 1, ...APP },
        spec: {},
        status: { ...ready, lastAppliedRevision: 'main@sha1:dc81672' },
      },
    ],
  },
  '/helmreleases': {
    items: [
      {
        metadata: { name: 'graph-k8s-workload', generation: 1, ...APP },
        spec: { releaseName: 'graph-k8s-workload', targetNamespace: 'graph' },
        status: {
          ...ready,
          history: [{ chartVersion: '1.2.0', lastDeployed: '2026-11-25T13:20:00Z' }],
        },
      },
    ],
  },
  '/flux-system/configmaps': {
    items: [
      {
        metadata: { name: 'graph-image-values' },
        data: {
          'values.yaml':
            'images:\n  curlimages/curl: 8.18.0\n  registry.anurag.sh/containers/graph-router: v1.0.69-20740f6 # {"$imagepolicy": "flux-system:graph-graph-router:tag"}\n',
        },
      },
    ],
  },
  '/imagepolicies': {
    items: [
      {
        metadata: { name: 'graph-graph-router', ...APP },
        spec: { imageRepositoryRef: { name: 'graph-graph-router' } },
        status: {
          latestRef: {
            name: 'registry.anurag.sh/containers/graph-router',
            tag: 'v1.0.69-20740f6',
          },
        },
      },
    ],
  },
  '/imagerepositories': {
    items: [
      {
        metadata: { name: 'graph-graph-router', ...APP },
        spec: { image: 'registry.anurag.sh/containers/graph-router' },
        status: { lastScanResult: { scanTime: '2026-11-25T13:06:27Z' } },
      },
    ],
  },
  '/apis/apps/v1/deployments': {
    items: [
      {
        metadata: {
          name: 'router',
          namespace: 'graph',
          annotations: {
            'meta.helm.sh/release-name': 'graph-k8s-workload',
            'meta.helm.sh/release-namespace': 'graph',
          },
        },
        spec: {
          replicas: 2,
          selector: { matchLabels: { app: 'router' } },
          template: {
            spec: {
              containers: [
                {
                  name: 'router',
                  image: 'registry.anurag.sh/containers/graph-router:v1.0.69-20740f6',
                },
              ],
            },
          },
        },
        status: { readyReplicas: 2 },
      },
    ],
  },
  '/statefulsets': { items: [] },
  '/daemonsets': { items: [] },
  '/cronjobs': { items: [] },
  '/batch/v1/jobs': {
    items: [
      {
        metadata: {
          name: 'supergraph-upload',
          namespace: 'graph',
          annotations: {
            'meta.helm.sh/release-name': 'graph-k8s-workload',
            'meta.helm.sh/release-namespace': 'graph',
          },
        },
        spec: {
          template: { spec: { containers: [{ name: 'upload', image: 'curlimages/curl:8.18.0' }] } },
        },
      },
    ],
  },
  '/namespaces/graph/pods': {
    items: [
      {
        metadata: { name: 'router-1', labels: { app: 'router' } },
        status: { containerStatuses: [{ restartCount: 1 }] },
      },
    ],
  },
};

const forgejo = {
  '/actions/runs?page=1&limit=50': {
    workflow_runs: [
      {
        id: 3703,
        index_in_repo: 2682,
        workflow_id: 'app-graph.yaml',
        commit_sha: '204a20cbb3b0bf88',
        status: 'success',
        title: 'graph: me plugin',
        stopped: '2026-11-25T13:08:00Z',
        html_url: 'https://git/runs/2682',
        prettyref: 'main',
      },
    ],
  },
  '/actions/runs/3703/jobs': [
    {
      id: 6108,
      name: 'release',
      status: 'success',
      html_url: 'https://git/jobs/2',
    },
  ],
  '/actions/jobs/6108/logs': 'line 1\nline 2\npushed v1.0.75-204a20c\n',
  '/git/commits/20740f6': {
    sha: '20740f6aaa',
    commit: {
      message: 'update schemas\n\nbody',
      author: { name: 'Anurag', date: '2026-11-25T11:05:00Z' },
    },
  },
};

async function stack() {
  const key = keypair('graph-1');
  const schema = executableSchema(
    resolvers({
      kubernetes: new Kubernetes({
        url: 'https://kube',
        token: async () => 't',
        fetcher: fetcher(kube),
      }),
      forgejo: new Forgejo('https://git', 't', 'https://git.example.test', fetcher(forgejo)),
      buildRepository: 'anurag/k8s',
    }),
  );
  const auth = new Authenticator(
    {
      providers: [{ issuer: ISSUER, audience: 'graph' }],
      scopes: ['flux:app:read', 'forgejo:run:read', 'forgejo:log:read', 'kubernetes:workload:read'],
    },
    fetcher({ '/public_key.jwk': { keys: [key.jwk] } }),
  );
  const app = server(schema, auth);
  await app.listen({ port: 0, host: '127.0.0.1' });
  const url = `http://127.0.0.1:${(app.server.address() as AddressInfo).port}`;

  const post = async (scope: string, query: string, extra: Record<string, string> = {}) => {
    const response = await fetch(`${url}/v1/graphql`, {
      method: 'POST',
      headers: {
        ...extra,
        'content-type': 'application/json',
        authorization: `Bearer ${key.token({ iss: ISSUER, aud: 'graph', sub: 'u', scope })}`,
      },
      body: JSON.stringify({ query }),
    });
    return { status: response.status, body: (await response.json()) as any };
  };
  return { url, post, close: () => app.close() };
}

const ALL = 'flux:app:read forgejo:run:read forgejo:log:read kubernetes:workload:read';

test('image values lines parse into repository tags', () => {
  assert.deepEqual(
    [...imageValues('images:\n  a/b: v1 # {"$imagepolicy": "x"}\n  c: v2\n')],
    [
      ['a/b', 'v1'],
      ['c', 'v2'],
    ],
  );
});

test('a flux app shows each image from build to pods, and where it is stuck', async () => {
  const s = await stack();
  try {
    const { body } = await s.post(
      ALL,
      `{ fluxApp(name: "graph") {
          id ready revision lastDeployedAt
          helmReleases { name chartVersion }
          workloads { kind name ready desired restarts }
          images { repository desiredTag selectedTag runningTags lastScanAt stage blocker
            commit { sha title author }
            latestBuild { number status jobs { name logTail(lines: 1) } } }
      } }`,
    );
    assert.equal(body.errors, undefined, JSON.stringify(body.errors));
    const app = body.data.fluxApp;
    assert.equal(app.id, 'flux:app:graph');
    assert.equal(app.ready, true);
    assert.deepEqual(app.helmReleases, [{ name: 'graph-k8s-workload', chartVersion: '1.2.0' }]);
    assert.deepEqual(app.workloads, [
      { kind: 'Deployment', name: 'router', ready: 2, desired: 2, restarts: 1 },
      { kind: 'Job', name: 'supergraph-upload', ready: 0, desired: 0, restarts: 0 },
    ]);

    const curl = app.images.find((i: { repository: string }) => i.repository === 'curlimages/curl');
    assert.equal(curl.latestBuild, null, 'images flux does not release are not built by the app');
    assert.deepEqual(curl.runningTags, ['8.18.0']);
    assert.equal(curl.stage, 'LIVE');

    const image = app.images.find(
      (i: { repository: string }) => i.repository === 'registry.anurag.sh/containers/graph-router',
    );
    assert.equal(image.selectedTag, 'v1.0.69-20740f6');
    assert.deepEqual(image.runningTags, ['v1.0.69-20740f6']);
    assert.equal(image.stage, 'SCAN_PENDING');
    assert.match(image.blocker, /after the image repository last scanned at 2026-11-25T13:06:27Z/);
    assert.deepEqual(image.commit, {
      sha: '20740f6aaa',
      title: 'update schemas',
      author: 'Anurag',
    });
    assert.deepEqual(image.latestBuild, {
      number: 2682,
      status: 'success',
      jobs: [{ name: 'release', logTail: 'pushed v1.0.75-204a20c' }],
    });
  } finally {
    await s.close();
  }
});

test('fields need their scopes', async () => {
  const s = await stack();
  try {
    const noLogs = await s.post(
      'flux:app:read forgejo:run:read',
      '{ fluxApp(name: "graph") { images { latestBuild { jobs { name logTail } } } } }',
    );
    assert.equal(noLogs.body.errors[0].message, 'missing scope forgejo:log:read');
    assert.equal(noLogs.body.errors[0].extensions.code, 'FORBIDDEN');

    const none = await s.post('openid', '{ fluxApps { name } }');
    assert.equal(none.status, 403);
    assert.equal(none.body.errors[0].extensions.code, 'FORBIDDEN');

    const health = await fetch(`${s.url}/health`);
    assert.equal(health.status, 200);
  } finally {
    await s.close();
  }
});

test('node ids resolve as entities', async () => {
  const s = await stack();
  try {
    const { body } = await s.post(
      ALL,
      `{ _entities(representations: [{__typename: "FluxApp", id: "flux:app:graph"}, {__typename: "ForgejoCommit", id: "forgejo:commit:anurag/k8s:20740f6"}]) {
          ... on FluxApp { name } ... on ForgejoCommit { title } } }`,
    );
    assert.deepEqual(body.data._entities, [{ name: 'graph' }, { title: 'update schemas' }]);
  } finally {
    await s.close();
  }
});

test("requests continue the caller's trace with operation, field and outbound spans", async () => {
  const exporter = new InMemorySpanExporter();
  const stop = startTracing(undefined, exporter);
  const s = await stack();
  try {
    const parent = '0af7651916cd43dd8448eb211c80319c';
    await s.post(ALL, 'query Apps { fluxApp(name: "graph") { name } }', {
      traceparent: `00-${parent}-b7ad6b7169203331-01`,
    });

    const spans = exporter.getFinishedSpans();
    const names = spans.map((span) => span.name);
    for (const name of [
      'POST /v1/graphql',
      'graphql.parse',
      'graphql.validate',
      'graphql.Apps',
      'graphql.Query.fluxApp',
      'GET kube/apis/kustomize.toolkit.fluxcd.io/v1/namespaces/{namespace}/kustomizations',
      'GET idm.example.test/oauth2/openid/graph/public_key.jwk',
    ]) {
      assert.ok(names.includes(name), `${name} in ${names.join(', ')}`);
    }
    const kube = spans.find((span) => span.name.startsWith('GET kube/'))!;
    assert.equal(kube.attributes['http.response.status_code'], 200);
    assert.equal(kube.attributes['server.address'], 'kube');
    assert.ok(spans.every((span) => span.spanContext().traceId === parent));
  } finally {
    await s.close();
    await stop();
  }
});

test('bad requests get graphql-shaped errors', async () => {
  const s = await stack();
  try {
    const notFound = await fetch(`${s.url}/nope`);
    assert.equal(notFound.status, 404);
    assert.equal(((await notFound.json()) as any).errors[0].extensions.code, 'NOT_FOUND');

    const malformed = await fetch(`${s.url}/v1/graphql`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: '{',
    });
    assert.equal(malformed.status, 400);
    assert.equal(((await malformed.json()) as any).errors[0].extensions.code, 'BAD_REQUEST');

    const missing = await s.post(ALL, '');
    assert.equal(missing.status, 400);
    assert.equal(missing.body.errors[0].message, 'missing query');
  } finally {
    await s.close();
  }
});
