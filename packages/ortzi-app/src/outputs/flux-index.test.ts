import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parse } from 'yaml';
import { renderFluxIndex } from './flux-index.ts';
import type { Plan } from '../plan.ts';

const plan = (name: string, namespace: string, labels?: Record<string, string>) =>
  ({ name, namespace, ...(labels ? { namespaceLabels: labels } : {}) }) as Plan;

const tree = { namespaces: ['flux-system'], components: [] };

test('the root lists every namespace directory', () => {
  const files = renderFluxIndex([plan('foo', 'foo'), plan('etcd', 'kube-system')], tree);

  assert.deepEqual(parse(files.get('kubernetes/kustomization.yaml')!).resources, [
    'flux-system',
    'foo',
    'kube-system',
  ]);
  assert.equal(files.has('kubernetes/flux-system/kustomization.yaml'), false);
});

test('an app owning its namespace sits at the namespace level', () => {
  const files = renderFluxIndex([plan('foo', 'foo', { team: 'a' })], tree);
  const namespace = parse(files.get('kubernetes/foo/namespace.yaml')!);

  assert.deepEqual(parse(files.get('kubernetes/foo/kustomization.yaml')!).resources, [
    'namespace.yaml',
    'ks.yaml',
  ]);
  assert.deepEqual(namespace.metadata.labels, { team: 'a' });
  assert.equal(namespace.metadata.annotations['kustomize.toolkit.fluxcd.io/prune'], 'disabled');
});

test('a shared namespace lists its apps and hand-written components', () => {
  const files = renderFluxIndex([plan('waf', 'network'), plan('dns', 'network')], {
    namespaces: ['network'],
    components: ['network/envoy/ks.yaml', 'other/x/ks.yaml'],
  });

  assert.deepEqual(parse(files.get('kubernetes/network/kustomization.yaml')!).resources, [
    'namespace.yaml',
    'dns/ks.yaml',
    'envoy/ks.yaml',
    'waf/ks.yaml',
  ]);
});

test('apps sharing a namespace must agree on its labels', () => {
  assert.throws(
    () => renderFluxIndex([plan('a', 'n', { x: '1' }), plan('b', 'n', { x: '2' })], tree),
    /disagree on label x/,
  );
});
