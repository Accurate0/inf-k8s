import assert from 'node:assert/strict';
import { test } from 'node:test';
import { changedScope, parseStatus } from './changed.ts';

test('parses staged, unstaged, untracked and spaced paths', () => {
  const output = 'M  projects/bom/app.yaml\0 M system-components/envoy/gateway.yaml\0?? selfhosted/a b/new file.yaml\0';
  assert.deepEqual(parseStatus(output), [
    'projects/bom/app.yaml',
    'system-components/envoy/gateway.yaml',
    'selfhosted/a b/new file.yaml',
  ]);
});

test('parses a clean tree as no paths', () => {
  assert.deepEqual(parseStatus(''), []);
});

test('selects everything in a changed component and nothing else', () => {
  const selected = changedScope(['system-components/envoy/public/gateway.yaml']);
  assert.equal(selected('system-components/envoy/private'), true);
  assert.equal(selected('system-components/envoy'), true);
  assert.equal(selected('system-components/envoy-extra'), false);
  assert.equal(selected('projects/bom/application.yaml'), false);
});

test('selects nothing when nothing changed', () => {
  assert.equal(changedScope([])('projects/bom/application.yaml'), false);
});

test('selects everything when a chart changed', () => {
  const selected = changedScope(['charts/k8s-workload/values.yaml']);
  assert.equal(selected('projects/bom/application.yaml'), true);
  assert.equal(selected('system-components/envoy'), true);
});
