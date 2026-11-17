import assert from 'node:assert/strict';
import { test } from 'node:test';
import { type Rollout, rolledOut } from './rollout.ts';

const deployment = (status: Rollout['status'], generation = 2): Rollout => ({
  kind: 'Deployment',
  metadata: { generation },
  spec: { replicas: 2 },
  status,
});

test('a deployment is rolled out once every replica is updated and available', () => {
  const done = { observedGeneration: 2, replicas: 2, updatedReplicas: 2, availableReplicas: 2 };

  assert.equal(rolledOut(deployment(done)), true);
  assert.equal(rolledOut(deployment({ ...done, availableReplicas: 1 })), false);
  assert.equal(rolledOut(deployment({ ...done, replicas: 3 })), false);
});

test('a deployment is not rolled out before the controller sees the new generation', () => {
  const stale = { observedGeneration: 1, replicas: 2, updatedReplicas: 2, availableReplicas: 2 };

  assert.equal(rolledOut(deployment(stale)), false);
});

test('a statefulset waits for the update revision to become current', () => {
  const statefulset = (currentRevision: string): Rollout => ({
    kind: 'StatefulSet',
    metadata: { generation: 1 },
    spec: { replicas: 1 },
    status: { observedGeneration: 1, readyReplicas: 1, currentRevision, updateRevision: 'b' },
  });

  assert.equal(rolledOut(statefulset('a')), false);
  assert.equal(rolledOut(statefulset('b')), true);
});

test('a daemonset is rolled out once every scheduled pod is updated and available', () => {
  const daemonset = (numberAvailable: number): Rollout => ({
    kind: 'DaemonSet',
    metadata: { generation: 1 },
    spec: {},
    status: {
      observedGeneration: 1,
      desiredNumberScheduled: 3,
      updatedNumberScheduled: 3,
      numberAvailable,
    },
  });

  assert.equal(rolledOut(daemonset(2)), false);
  assert.equal(rolledOut(daemonset(3)), true);
});

test('kinds without a rollout never count as rolled out', () => {
  assert.equal(rolledOut({ kind: 'Job', metadata: {}, spec: {} }), false);
});
