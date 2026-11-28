import assert from 'node:assert/strict';
import { test } from 'node:test';
import { setTimeout as sleep } from 'node:timers/promises';
import { createClient } from 'graphql-ws/client';
import { PATHS, READ, ROUTER, ROUTER_POD, stack } from './stack.ts';

type Stack = Awaited<ReturnType<typeof stack>>;

const until = async (check: () => Promise<boolean>) => {
  for (let i = 0; i < 100; i++) {
    if (await check()) {
      return;
    }
    await sleep(10);
  }
  assert.fail('condition never held');
};

const socket = (s: Stack, connectionParams: Record<string, unknown>) =>
  createClient({
    url: `${s.url.replace('http', 'ws')}/v1/graphql/ws`,
    connectionParams,
    webSocketImpl: WebSocket,
    retryAttempts: 0,
  });

const routerReady = async (s: Stack) => {
  const { body } = await s.post(READ, '{ kubernetesWorkloads(namespace: "graph") { name ready } }');
  return body.data.kubernetesWorkloads.find((w: { name: string }) => w.name === 'router').ready;
};

test('watched lists follow the stream and relist when it expires', async () => {
  const s = await stack();
  try {
    await s.kube.watching(PATHS.deployments);
    const lists = () =>
      s.kube.requests.filter((r) => r.path === PATHS.deployments && !r.search.includes('watch'))
        .length;
    assert.equal(lists(), 1);

    s.kube.push(PATHS.deployments, 'MODIFIED', { ...ROUTER, status: { readyReplicas: 1 } });
    await until(async () => (await routerReady(s)) === 1);
    assert.equal(lists(), 1, 'reads are served from the watch, not relisted');

    s.kube.push(PATHS.deployments, 'MODIFIED', { ...ROUTER, status: { readyReplicas: 2 } });
    s.kube.expire(PATHS.deployments);
    await until(async () => lists() === 2);
    assert.equal(await routerReady(s), 2);

    s.kube.push(PATHS.deployments, 'DELETED', ROUTER);
    await until(async () => (await routerReady(s).catch(() => undefined)) === undefined);
  } finally {
    await s.close();
  }
});

test('subscriptions push an app again only when something of its changes', async () => {
  const s = await stack();
  const client = socket(s, { 'x-graph-passport': s.token(READ) });
  try {
    const updates = client.iterate<any>({
      query: 'subscription { fluxAppUpdates(name: "graph") { name workloads { name ready } } }',
    });

    const first = await updates.next();
    assert.equal(first.value.data.fluxAppUpdates.name, 'graph');
    assert.deepEqual(first.value.data.fluxAppUpdates.workloads, [
      { name: 'router', ready: 2 },
      { name: 'supergraph-upload', ready: 0 },
    ]);

    s.kube.push(PATHS.pods, 'MODIFIED', {
      metadata: { name: 'router-1', namespace: 'other', labels: { app: 'router' } },
      status: { phase: 'Failed' },
    });
    await sleep(50);
    s.kube.push(PATHS.deployments, 'MODIFIED', { ...ROUTER, status: { readyReplicas: 1 } });

    const second = await updates.next();
    assert.deepEqual(
      second.value.data.fluxAppUpdates.workloads[0],
      { name: 'router', ready: 1 },
      "another namespace's pod didn't push an update",
    );
    await updates.return?.();
  } finally {
    await client.dispose();
    await s.close();
  }
});

test('workload subscriptions send each workload, then the ones that change', async () => {
  const s = await stack();
  const client = socket(s, { 'X-Graph-Passport': s.token(READ) });
  try {
    const updates = client.iterate<any>({
      query:
        'subscription { kubernetesWorkloadUpdates(namespace: "graph") { name restarts pods { name } } }',
    });
    const names = [
      (await updates.next()).value.data.kubernetesWorkloadUpdates.name,
      (await updates.next()).value.data.kubernetesWorkloadUpdates.name,
    ].sort();
    assert.deepEqual(names, ['router', 'supergraph-upload']);

    s.kube.push(PATHS.pods, 'MODIFIED', {
      ...ROUTER_POD,
      status: {
        ...ROUTER_POD.status,
        containerStatuses: [{ ...ROUTER_POD.status.containerStatuses[0]!, restartCount: 2 }],
      },
    });
    const next = await updates.next();
    assert.deepEqual(next.value.data.kubernetesWorkloadUpdates, {
      name: 'router',
      restarts: 2,
      pods: [{ name: 'router-1' }],
    });
    await updates.return?.();
  } finally {
    await client.dispose();
    await s.close();
  }
});

test('sockets authenticate from connection_init and subscriptions need their scopes', async () => {
  const s = await stack();
  const anonymous = socket(s, {});
  const forged = socket(s, { 'x-graph-passport': 'not.a.token' });
  const narrow = socket(s, { 'x-graph-passport': s.token('kubernetes:workload:read') });
  const query = 'subscription { fluxAppUpdates(name: "graph") { name } }';
  try {
    for (const client of [anonymous, forged]) {
      await assert.rejects(client.iterate({ query }).next(), (error: { code?: number }) => {
        assert.equal(error.code, 4403);
        return true;
      });
    }

    const denied = await narrow.iterate<any>({ query }).next();
    assert.equal(denied.value.errors[0].message, 'missing scope flux:app:read');
    assert.equal(denied.value.errors[0].extensions.code, 'FORBIDDEN');
  } finally {
    await Promise.all([anonymous.dispose(), forged.dispose(), narrow.dispose()]);
    await s.close();
  }
});
