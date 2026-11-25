import assert from 'node:assert/strict';
import { test } from 'node:test';
import { delivery, type DeliveryInput } from '../src/delivery.ts';

const live: DeliveryInput = {
  selectedTag: 'v1.0.75-204a20c',
  desiredTag: 'v1.0.75-204a20c',
  runningTags: ['v1.0.75-204a20c'],
  lastScanAt: '2026-11-25T13:06:27Z',
  build: {
    number: 2682,
    sha: '204a20cbb3b0bf88',
    status: 'success',
    stoppedAt: '2026-11-25T13:00:00Z',
  },
  workloads: [{ name: 'deployment/router', ready: 2, desired: 2 }],
};

test('a build that is running everywhere is live', () => {
  assert.deepEqual(delivery(live), { stage: 'LIVE' });
});

test('newer builds report where they are', () => {
  const newer = (status: string, extra = {}) =>
    delivery({
      ...live,
      build: { number: 2690, sha: '83de8f7aaaa', status, ...extra },
    });

  assert.deepEqual(newer('running'), {
    stage: 'BUILDING',
    blocker: 'build run #2690 is running',
  });
  assert.deepEqual(newer('failure', { failedJob: 'render' }), {
    stage: 'BUILD_FAILED',
    blocker: 'build run #2690 failure in render',
  });
});

test('a release after the last scan is waiting on the image repository', () => {
  const result = delivery({
    ...live,
    selectedTag: 'v1.0.69-20740f6',
    desiredTag: 'v1.0.69-20740f6',
    runningTags: ['v1.0.69-20740f6'],
    lastScanAt: '2026-11-25T13:06:27Z',
    build: {
      number: 2682,
      sha: '204a20cbb3',
      status: 'success',
      stoppedAt: '2026-11-25T13:08:00Z',
    },
  });
  assert.equal(result.stage, 'SCAN_PENDING');
  assert.match(
    result.blocker!,
    /204a20c was released at 2026-11-25T13:08:00Z, after the image repository last scanned at 2026-11-25T13:06:27Z/,
  );
});

test('later stages name the gap', () => {
  assert.deepEqual(
    delivery({
      ...live,
      build: undefined,
      desiredTag: 'v1.0.69-20740f6',
      runningTags: ['v1.0.69-20740f6'],
    }),
    {
      stage: 'SELECTED',
      blocker: "image automation hasn't committed v1.0.75-204a20c yet; git has v1.0.69-20740f6",
    },
  );
  assert.deepEqual(delivery({ ...live, runningTags: ['v1.0.69-20740f6'], build: undefined }), {
    stage: 'COMMITTED',
    blocker: 'git has v1.0.75-204a20c, the workloads run v1.0.69-20740f6',
  });
  assert.deepEqual(
    delivery({
      ...live,
      workloads: [{ name: 'deployment/router', ready: 1, desired: 2 }],
    }),
    { stage: 'ROLLING_OUT', blocker: 'deployment/router: 1/2 ready' },
  );
  assert.deepEqual(
    delivery({
      ...live,
      workloads: [
        {
          name: 'deployment/router',
          ready: 1,
          desired: 2,
          waitingReason: 'CrashLoopBackOff',
        },
      ],
    }),
    { stage: 'DEGRADED', blocker: 'deployment/router: CrashLoopBackOff' },
  );
});
