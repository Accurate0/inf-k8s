import assert from 'node:assert/strict';
import { test } from 'node:test';
import { InMemorySpanExporter } from '@opentelemetry/sdk-trace-base';
import { imageValues } from '../src/flux.ts';
import { startTracing } from '../src/tracing.ts';
import { DIGEST, READ as ALL, stack } from './stack.ts';

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
  exporter.reset();
  try {
    const parent = '0af7651916cd43dd8448eb211c80319c';
    await s.post(
      ALL,
      'query Apps { fluxApp(name: "graph") { name workloads { events { reason } } } }',
      {
        traceparent: `00-${parent}-b7ad6b7169203331-01`,
      },
    );

    const spans = exporter.getFinishedSpans();
    const names = spans.map((span) => span.name);
    for (const name of [
      'POST /v1/graphql',
      'graphql.parse',
      'graphql.validate',
      'graphql.Apps',
      'graphql.Query.fluxApp',
      'GET kube/api/v1/namespaces/{namespace}/events',
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

test('workloads show their pods, containers, events and logs', async () => {
  const s = await stack();
  try {
    const { body } = await s.post(
      ALL,
      `{ kubernetesWorkloads(namespace: "graph") {
          name
          events(limit: 5) { type reason count object lastSeenAt }
          pods { name phase node ready restarts startedAt
            containers { name imageDigest ready restarts
              lastTermination { reason exitCode finishedAt }
              logTail(lines: 2)
              registryImage { repository digest } } }
      } }`,
    );
    assert.equal(body.errors, undefined, JSON.stringify(body.errors));
    const router = body.data.kubernetesWorkloads.find((w: { name: string }) => w.name === 'router');
    assert.deepEqual(router.pods, [
      {
        name: 'router-1',
        phase: 'Running',
        node: 'k8s-1',
        ready: true,
        restarts: 1,
        startedAt: '2026-11-25T13:30:00Z',
        containers: [
          {
            name: 'router',
            imageDigest: DIGEST,
            ready: true,
            restarts: 1,
            lastTermination: {
              reason: 'OOMKilled',
              exitCode: 137,
              finishedAt: '2026-11-25T13:29:00Z',
            },
            logTail: 'listening\nserving',
            registryImage: { repository: 'containers/graph-router', digest: DIGEST },
          },
        ],
      },
    ]);
    assert.deepEqual(router.events, [
      {
        type: 'Warning',
        reason: 'BackOff',
        count: 4,
        object: 'pod/router-1',
        lastSeenAt: '2026-11-25T13:29:30Z',
      },
      {
        type: 'Normal',
        reason: 'ScalingReplicaSet',
        count: 1,
        object: 'deployment/router',
        lastSeenAt: '2026-11-25T13:28:00Z',
      },
    ]);

    const noLogs = await s.post(
      'kubernetes:workload:read',
      '{ kubernetesWorkloads(namespace: "graph") { pods { containers { logTail } } } }',
    );
    assert.equal(noLogs.body.errors[0].message, 'missing scope kubernetes:log:read');
  } finally {
    await s.close();
  }
});

test('images link to the registry by repository and by the digests the pods run', async () => {
  const s = await stack();
  try {
    const { body } = await s.post(
      ALL,
      `{ fluxApp(name: "graph") { images { repository
          registryRepository { id } runningImages { repository digest } } } }`,
    );
    assert.equal(body.errors, undefined, JSON.stringify(body.errors));
    assert.deepEqual(body.data.fluxApp.images, [
      { repository: 'curlimages/curl', registryRepository: null, runningImages: null },
      {
        repository: 'registry.anurag.sh/containers/graph-router',
        registryRepository: { id: 'registry:imageRepository:containers/graph-router' },
        runningImages: [{ repository: 'containers/graph-router', digest: DIGEST }],
      },
    ]);
  } finally {
    await s.close();
  }
});
