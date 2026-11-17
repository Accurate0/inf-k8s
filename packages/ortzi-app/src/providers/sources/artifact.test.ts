import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parse } from 'yaml';
import { type App, validate } from '../../render.ts';
import {
  versions,
  target,
  renderApplication,
  platform,
  render,
  source,
} from '../../test-fixtures.ts';

test('artifact apps sync charts with inline values and only the kustomization from the registry', () => {
  const files: Record<string, Record<string, unknown>> = {
    'projects/foo/image.values.yaml': { images: { i: 'abc123' } },
    'projects/foo/r.yaml': { replicas: 2 },
  };
  const rendered = parse(
    renderApplication(
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

  const sources = rendered.spec.sources as Record<string, any>[];
  assert.ok(sources.every((s) => !s.ref && !String(s.repoURL).includes('github.com')));
  assert.deepEqual(source(rendered, 'k8s-workload')?.helm.valuesObject.images, { i: 'abc123' });
  assert.equal(source(rendered, 'k8s-workload')?.helm.valueFiles, undefined);
  assert.deepEqual(source(rendered, 'reloader')?.helm, {
    releaseName: 'rel',
    valuesObject: { replicas: 2 },
  });
  assert.deepEqual(
    sources.find((s) => s.name === 'application'),
    {
      repoURL: 'oci://registry.anurag.sh/manifests/foo',
      name: 'application',
      path: '.',
      targetRevision: 'main',
    },
  );

  const chartsOnly = render({
    name: 'foo',
    network: { ingress: [] },
    artifact: { repo: 'oci://registry.anurag.sh/manifests/foo', tag: 'main' },
  });
  assert.deepEqual(
    chartsOnly.spec.sources.map((s: { chart?: string; name?: string }) => s.chart ?? s.name),
    ['k8s-network'],
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
            { type: 'git', repo: 'https://example.com/r.git', path: 'x', name: 'application' },
          ],
        }),
        'x',
      ),
    /duplicate source name application/,
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

  assert.deepEqual(rendered.spec.sources.slice(1), [
    {
      repoURL: 'oci://registry.anurag.sh/manifests/foo',
      name: 'application',
      path: '.',
      targetRevision: 'main',
    },
    { repoURL: 'https://git.anurag.sh/anurag/foo.git', name: 'config', path: 'config' },
    {
      repoURL: 'oci://registry.anurag.sh/manifests/foo-extra',
      name: 'extra',
      path: '.',
      targetRevision: 'v1',
    },
  ]);
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

  assert.deepEqual(rendered.spec.sources.at(-1), {
    repoURL: 'oci://registry.anurag.sh/manifests/foo-extra',
    path: 'deploy',
    targetRevision: 'main',
  });
});
