import assert from 'node:assert/strict';
import { test } from 'node:test';
import { helmTemplates, planApp } from '../../plan.ts';
import { type App, validate } from '../../render.ts';
import { versions, target, platform, render, readValues } from '../../test-fixtures.ts';

const release = {
  type: 'helm' as const,
  chart: 'forgejo',
  repo: 'oci://code.forgejo.org/forgejo-helm/forgejo',
  version: '1.0.0',
  values: 'forgejo.values.yaml',
};

test('helm releases inline values from a file beside app.yaml', () => {
  const read: string[] = [];
  const plan = planApp(
    { name: 'foo', network: { ingress: [] }, sources: [release] },
    versions,
    { location: 'selfhosted', hasManifests: false },
    {
      platform,
      apps: [],
      readValues: (path) => {
        read.push(path);
        return { replicas: 2 };
      },
    },
  );
  const forgejo = render({
    name: 'foo',
    network: { ingress: [] },
    sources: [release],
  }).releases.find((r) => r.metadata.name === 'foo-forgejo');

  assert.deepEqual(read, ['selfhosted/foo/forgejo.values.yaml']);
  assert.deepEqual(plan.sources.at(-1), { ...release, values: { replicas: 2 } });
  assert.equal(forgejo.spec.releaseName, 'foo');
  assert.deepEqual(forgejo.spec.chartRef, {
    kind: 'OCIRepository',
    name: 'foo-forgejo',
    namespace: 'flux-system',
  });
});

test('a chart listed twice needs distinct release names', () => {
  const app = (sources: App['sources']) => ({ name: 'foo', network: { ingress: [] }, sources });

  assert.throws(() => validate(app([release, release]), 'x'), /duplicate helm release forgejo/);
  validate(app([release, { ...release, releaseName: 'other' }]), 'x');
});

test('only one helm release may default to the app name', () => {
  const app = (sources: App['sources']) => ({ name: 'foo', network: { ingress: [] }, sources });
  const other = { ...release, chart: 'runner' };

  assert.throws(() => validate(app([release, other]), 'x'), /set releaseName on all but one/);
  validate(app([release, { ...other, releaseName: 'foo-runner' }]), 'x');
});

test('helm templates pull upstream charts', () => {
  const plan = planApp(
    {
      name: 'foo',
      network: { ingress: [] },
      sources: [
        release,
        {
          type: 'helm',
          chart: 'reloader',
          repo: 'https://example.com/charts',
          version: '2.0.0',
          values: 'r.yaml',
          releaseName: 'rel',
        },
      ],
    },
    versions,
    target,
    { platform, apps: [], readValues },
  );
  const [, oci, https] = helmTemplates(plan).map(({ args }) => args);

  assert.deepEqual(oci.slice(1, 5), ['foo', release.repo, '--version', '1.0.0']);
  assert.deepEqual(https.slice(1, 7), [
    'rel',
    'reloader',
    '--repo',
    'https://example.com/charts',
    '--version',
    '2.0.0',
  ]);
});
