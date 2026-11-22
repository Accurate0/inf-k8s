import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  type FluxObject,
  changes,
  matches,
  refetchable,
  settled,
  shortRevision,
  state,
} from './flux.ts';

const DIGEST = 'a'.repeat(64);

function object(
  overrides: Partial<FluxObject> = {},
  conditions: [string, string][] = [],
): FluxObject {
  return {
    kind: 'OCIRepository',
    metadata: { name: 'app', namespace: 'flux-system', generation: 2 },
    ...overrides,
    status: {
      observedGeneration: 2,
      conditions: conditions.map(([type, status]) => ({ type, status })),
      ...overrides.status,
    },
  };
}

test('shortens digests and commits', () => {
  assert.equal(shortRevision(`main@sha256:${DIGEST}`), 'main@aaaaaaaaaaaa');
  assert.equal(shortRevision(`sha256:${DIGEST}`), 'aaaaaaaaaaaa');
  assert.equal(shortRevision(`main@sha1:${'b'.repeat(40)}`), 'main@sha1:bbbbbbbb');
  assert.equal(shortRevision('1.2.3'), '1.2.3');
  assert.equal(shortRevision(undefined), '-');
});

test('matches by name or kind/name', () => {
  const repo = object();
  assert.ok(matches(repo, []));
  assert.ok(matches(repo, ['app']));
  assert.ok(matches(repo, ['OCIRepository/app']));
  assert.ok(!matches(repo, ['gitrepository/app']));
  assert.ok(!matches(repo, ['other']));
});

test('skips suspended sources and oci helm repositories', () => {
  assert.ok(refetchable(object()));
  assert.ok(!refetchable(object({ spec: { suspend: true } })));
  assert.ok(!refetchable(object({ kind: 'HelmRepository', spec: { type: 'oci' } })));
  assert.ok(refetchable(object({ kind: 'HelmRepository', spec: {} })));
});

test('settles only once the request was handled and reconciling stopped', () => {
  const token = '2026-11-22T00:00:00.000Z';
  const handled = { status: { lastHandledReconcileAt: token } };

  assert.ok(!settled(object({}, [['Ready', 'True']]), token));
  assert.ok(
    !settled(
      object(handled, [
        ['Ready', 'Unknown'],
        ['Reconciling', 'True'],
      ]),
      token,
    ),
  );
  assert.ok(settled(object(handled, [['Ready', 'True']]), token));
  assert.ok(settled(object(handled, [['Ready', 'False']]), token));
});

test('classifies object state', () => {
  assert.equal(state(object({}, [['Ready', 'True']])), 'ready');
  assert.equal(state(object({ spec: { suspend: true } })), 'suspended');
  assert.equal(
    state(
      object({}, [
        ['Ready', 'Unknown'],
        ['Reconciling', 'True'],
      ]),
    ),
    'reconciling',
  );
  assert.equal(state(object({}, [['Ready', 'False']])), 'failing');
  assert.equal(
    state(
      object({}, [
        ['Ready', 'False'],
        ['Reconciling', 'True'],
        ['Stalled', 'True'],
      ]),
    ),
    'failing',
  );
});

test('reports only revisions that moved', () => {
  const moved = object({ status: { artifact: { revision: 'new' } } });
  const same = object({
    metadata: { name: 'same', namespace: 'flux-system' },
    status: { artifact: { revision: 'old' } },
  });
  const before = new Map([
    ['ocirepository/flux-system/app', 'old'],
    ['ocirepository/flux-system/same', 'old'],
  ]);

  assert.deepEqual(changes(before, [moved, same]), [
    { id: 'ocirepository/flux-system/app', from: 'old', to: 'new' },
  ]);
});
