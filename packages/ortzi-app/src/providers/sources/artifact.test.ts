import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parseAllDocuments } from 'yaml';
import { renderFlux } from '../../outputs/flux.ts';
import { planApp } from '../../plan.ts';
import { type App, validate } from '../../render.ts';
import { versions, target, platform, render, releaseValues } from '../../test-fixtures.ts';

const ofKind = (rendered: ReturnType<typeof render>, kind: string) =>
  rendered.objects.filter((o) => o.kind === kind);

test('artifact apps inline chart values but read image tags from a ConfigMap, and apply only the kustomization from the registry', () => {
  const files: Record<string, Record<string, unknown>> = {
    'kubernetes/foo/app/image.values.yaml': { images: { i: 'abc123' } },
    'projects/foo/r.yaml': { replicas: 2 },
  };
  const rendered = renderFlux(
    planApp(
      {
        name: 'foo',
        network: { ingress: [] },
        workloads: [{ name: 'foo', image: 'i' }],
        artifact: { repo: 'oci://registry.anurag.sh/manifests/foo', tag: 'main' },
        sources: [
          { type: 'manifests', path: '.' },
          {
            type: 'helm',
            chart: 'reloader',
            repo: 'https://example.com/charts',
            version: '2.0.0',
            values: 'r.yaml',
            releaseName: 'rel',
          },
        ] as App['sources'],
      },
      versions,
      target,
      { platform, apps: [], readValues: (path) => files[path] ?? {} },
    ),
  );
  const objects = [...rendered.values()].flatMap((c) => parseAllDocuments(c).map((d) => d.toJS()));
  const releases = objects.filter((o) => o.kind === 'HelmRelease');
  const named = (name: string) => objects.find((o) => o.metadata?.name === name);

  assert.deepEqual(releases.find((r) => r.metadata.name === 'foo-k8s-workload').spec.valuesFrom, [
    { kind: 'ConfigMap', name: 'foo-image-values', valuesKey: 'values.yaml' },
  ]);
  const reloader = releases.find((r) => r.metadata.name === 'foo-rel');
  assert.equal(reloader.spec.releaseName, 'rel');
  assert.deepEqual(reloader.spec.values, { replicas: 2 });
  assert.equal(named('foo-manifests').kind, 'OCIRepository');
  assert.equal(named('foo-manifests').spec.url, 'oci://registry.anurag.sh/manifests/foo');
  assert.deepEqual(named('foo-manifests').spec.ref, { tag: 'main' });

  const chartsOnly = render({
    name: 'foo',
    network: { ingress: [] },
    artifact: { repo: 'oci://registry.anurag.sh/manifests/foo', tag: 'main' },
  });
  assert.deepEqual(
    chartsOnly.releases.map((r) => r.metadata.name),
    ['foo-k8s-network'],
  );

  const app = (extra: Record<string, unknown>) => ({
    name: 'foo',
    network: { ingress: [] },
    ...extra,
  });
  assert.throws(() =>
    validate(app({ artifact: { repo: 'oci://registry.anurag.sh/manifests/foo' } }), 'x'),
  );
  assert.throws(() =>
    validate(app({ artifact: { repo: 'https://example.com/foo', tag: 'main' } }), 'x'),
  );
  assert.throws(
    () =>
      validate(
        app({
          artifact: { repo: 'oci://registry.anurag.sh/manifests/foo', tag: 'main' },
          sources: [
            { type: 'manifests', path: '.' },
            { type: 'git', repo: 'https://example.com/r.git', path: 'x', name: 'manifests' },
          ],
        }),
        'x',
      ),
    /duplicate source name manifests/,
  );
  assert.throws(
    () =>
      validate(
        app({
          artifact: { repo: 'oci://registry.anurag.sh/manifests/foo', tag: 'main' },
          sources: [
            { type: 'artifact', repo: 'oci://registry.anurag.sh/manifests/foo', tag: 'main' },
          ],
        }),
        'x',
      ),
    /own artifact/,
  );
  assert.throws(() =>
    validate(
      app({ sources: [{ type: 'artifact', repo: 'oci://registry.anurag.sh/manifests/x' }] }),
      'x',
    ),
  );
});

test('artifact apps keep git and artifact sources live next to the kustomization', () => {
  const rendered = render({
    name: 'foo',
    network: { ingress: [] },
    artifact: { repo: 'oci://registry.anurag.sh/manifests/foo', tag: 'main' },
    sources: [
      { type: 'manifests', path: '.' },
      { type: 'git', repo: 'https://git.anurag.sh/anurag/foo.git', name: 'config', path: 'config' },
      {
        type: 'artifact',
        repo: 'oci://registry.anurag.sh/manifests/foo-extra',
        tag: 'v1',
        name: 'extra',
      },
    ] as App['sources'],
  });

  assert.deepEqual(
    ofKind(rendered, 'Kustomization')
      .filter((ks) => ks.apiVersion.startsWith('kustomize.toolkit'))
      .map((ks) => [ks.metadata.name, ks.spec.sourceRef.kind, ks.spec.path]),
    [
      ['foo', 'OCIRepository', './foo/app'],
      ['foo-manifests', 'OCIRepository', './'],
      ['foo-config', 'GitRepository', './config'],
      ['foo-extra', 'OCIRepository', './'],
    ],
  );
  assert.equal(
    ofKind(rendered, 'GitRepository')[0].spec.url,
    'https://git.anurag.sh/anurag/foo.git',
  );
  assert.equal(
    ofKind(rendered, 'OCIRepository').find((r) => r.metadata.name === 'foo-extra').spec.url,
    'oci://registry.anurag.sh/manifests/foo-extra',
  );
});

test('artifact sources render without a hydrated artifact', () => {
  const rendered = render({
    name: 'foo',
    network: { ingress: [] },
    sources: [
      {
        type: 'artifact',
        repo: 'oci://registry.anurag.sh/manifests/foo-extra',
        tag: 'main',
        path: 'deploy',
      },
    ] as App['sources'],
  });

  const ks = ofKind(rendered, 'Kustomization').find((k) => k.metadata?.name === 'foo-artifact');
  assert.equal(ks.spec.path, './deploy');
  assert.deepEqual(
    ofKind(rendered, 'OCIRepository').find((r) => r.metadata.name === 'foo-artifact').spec.ref,
    { tag: 'main' },
  );
  assert.ok(releaseValues(rendered, 'k8s-network'));
});
