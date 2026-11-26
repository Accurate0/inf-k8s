import assert from 'node:assert/strict';
import { test } from 'node:test';
import { READ, stack, WRITE } from './stack.ts';

const BOTH = `${READ} ${WRITE}`;

test('suspend and resume patch every kustomization and helm release of the app', async () => {
  const s = await stack();
  try {
    const suspended = await s.post(BOTH, 'mutation { fluxSuspend(name: "graph") { suspended } }');
    assert.equal(suspended.body.errors, undefined, JSON.stringify(suspended.body.errors));
    assert.equal(suspended.body.data.fluxSuspend.suspended, true);

    const patches = s.kube.requests.filter((r) => r.method === 'PATCH');
    assert.deepEqual(
      patches.map((r) => [r.path.split('/').slice(-2).join('/'), r.contentType, r.body]),
      [
        ['kustomizations/graph', 'application/merge-patch+json', { spec: { suspend: true } }],
        [
          'kustomizations/graph-manifests',
          'application/merge-patch+json',
          { spec: { suspend: true } },
        ],
        [
          'helmreleases/graph-k8s-workload',
          'application/merge-patch+json',
          { spec: { suspend: true } },
        ],
      ],
    );

    const resumed = await s.post(BOTH, 'mutation { fluxResume(name: "graph") { suspended } }');
    assert.equal(resumed.body.data.fluxResume.suspended, false);
    const reconcile = s.kube.requests.filter((r) => r.method === 'PATCH').at(-1)!;
    assert.equal(reconcile.path.split('/').at(-1), 'graph');
    assert.match(
      (reconcile.body as any).metadata.annotations['reconcile.fluxcd.io/requestedAt'],
      /^\d{4}-\d{2}-\d{2}T/,
    );
  } finally {
    await s.close();
  }
});

test('flux-system is refused and unknown apps are null', async () => {
  const s = await stack();
  try {
    const refused = await s.post(BOTH, 'mutation { fluxSuspend(name: "flux-system") { name } }');
    assert.equal(refused.body.errors[0].extensions.code, 'FORBIDDEN');

    const missing = await s.post(BOTH, 'mutation { fluxReconcile(name: "nope") { name } }');
    assert.equal(missing.body.data.fluxReconcile, null);
    assert.equal(s.kube.requests.filter((r) => r.method === 'PATCH').length, 0);
  } finally {
    await s.close();
  }
});

test('restart stamps the pod template of rollable workloads only', async () => {
  const s = await stack();
  try {
    const { body } = await s.post(
      BOTH,
      'mutation { kubernetesRestart(namespace: "graph", kind: "Deployment", name: "router") { name ready } }',
    );
    assert.equal(body.errors, undefined, JSON.stringify(body.errors));
    assert.deepEqual(body.data.kubernetesRestart, { name: 'router', ready: 2 });

    const patch = s.kube.requests.find((r) => r.method === 'PATCH')!;
    assert.equal(patch.path, '/apis/apps/v1/namespaces/graph/deployments/router');
    assert.ok(
      (patch.body as any).spec.template.metadata.annotations['kubectl.kubernetes.io/restartedAt'],
    );

    const job = await s.post(
      BOTH,
      'mutation { kubernetesRestart(namespace: "graph", kind: "Job", name: "supergraph-upload") { name } }',
    );
    assert.equal(job.body.errors[0].extensions.code, 'BAD_USER_INPUT');
  } finally {
    await s.close();
  }
});

test('runs rerun and cancel by their node id', async () => {
  const s = await stack();
  try {
    for (const mutation of ['forgejoRerun', 'forgejoCancel']) {
      const { body } = await s.post(
        BOTH,
        `mutation { ${mutation}(id: "forgejo:run:anurag/k8s:3703") { number status } }`,
      );
      assert.equal(body.errors, undefined, JSON.stringify(body.errors));
      assert.deepEqual(body.data[mutation], { number: 2682, status: 'success' });
    }
    assert.deepEqual(
      s.forgejoCalls.filter((call) => call.method === 'POST').map((call) => call.url),
      [
        'https://git/api/v1/repos/anurag/k8s/actions/runs/3703/rerun',
        'https://git/api/v1/repos/anurag/k8s/actions/runs/3703/cancel',
      ],
    );

    const bad = await s.post(BOTH, 'mutation { forgejoRerun(id: "flux:app:graph") { number } }');
    assert.equal(bad.body.errors[0].extensions.code, 'BAD_USER_INPUT');
  } finally {
    await s.close();
  }
});

test('mutations need write scopes', async () => {
  const s = await stack();
  try {
    for (const mutation of [
      'fluxReconcile(name: "graph") { name }',
      'kubernetesRestart(namespace: "graph", kind: "Deployment", name: "router") { name }',
      'forgejoCancel(id: "forgejo:run:anurag/k8s:3703") { number }',
    ]) {
      const { body } = await s.post(READ, `mutation { ${mutation} }`);
      assert.equal(body.errors[0].extensions.code, 'FORBIDDEN', mutation);
    }
    assert.equal(s.kube.requests.filter((r) => r.method === 'PATCH').length, 0);
    assert.equal(s.forgejoCalls.filter((call) => call.method === 'POST').length, 0);
  } finally {
    await s.close();
  }
});
