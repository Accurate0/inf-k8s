import assert from 'node:assert/strict';
import { test } from 'node:test';
import { validateManifestSources, type App, validate } from '../../render.ts';
import { planApp } from '../../plan.ts';
import { platform, readValues, versions } from '../../test-fixtures.ts';

const artifact = { repo: 'oci://registry.anurag.sh/manifests/foo', tag: 'main' };

// publish builds this path and pushes it to the artifact flux applies
const built = (path: string) =>
  planApp(
    { name: 'foo', network: { ingress: [] }, artifact, sources: [{ type: 'manifests', path }] },
    versions,
    { location: 'platform-services', hasManifests: true },
    { platform, apps: [], readValues },
  ).sources.at(-1);

test('keeps manifests as an optional source', () => {
  assert.deepEqual(built('manifests'), {
    type: 'manifests',
    path: 'platform-services/foo/manifests',
  });
});

test('uses the app directory when it has its own kustomization', () => {
  assert.deepEqual(built('.'), { type: 'manifests', path: 'platform-services/foo' });
});

test('manifests sources are limited and stay in the app directory', () => {
  const app = (sources: unknown[], extra = {}) => ({
    name: 'foo',
    network: { ingress: [] },
    artifact,
    sources,
    ...extra,
  });

  assert.throws(
    () =>
      validate(
        { name: 'foo', network: { ingress: [] }, sources: [{ type: 'manifests', path: '.' }] },
        'x',
      ),
    /needs artifact/,
  );

  assert.throws(
    () =>
      validate(
        app([
          { type: 'manifests', path: '.' },
          { type: 'manifests', path: 'x' },
        ]),
        'x',
      ),
    /only one manifests/,
  );
  assert.throws(
    () => validate(app([{ type: 'manifests', path: '../other' }]), 'x'),
    /inside the app directory/,
  );
  const config = { resources: [{ name: 'c', type: 'config', configMap: 'c', files: ['a'] }] };

  assert.throws(
    () => validate(app([{ type: 'manifests', path: 'manifests' }], config), 'x'),
    /path must be \./,
  );
  assert.throws(() => validate(app([{ type: 'bogus', path: 'x' }]), 'x'));
  assert.throws(() => validate(app([{ type: 'helm', chart: 'a', repo: 'b', version: '1' }]), 'x'));
});

test('kustomizations on disk must be declared as a source', () => {
  const app = (sources: App['sources'], resources?: App['resources']): App => ({
    name: 'foo',
    network: { ingress: [] },
    sources,
    resources,
  });
  const none = { hasManifests: false, hasRootKustomization: false };
  validateManifestSources(app(undefined), none);

  assert.throws(
    () => validateManifestSources(app(undefined), { ...none, hasManifests: true }),
    /add a manifests source/,
  );
  assert.throws(
    () => validateManifestSources(app(undefined), { ...none, hasRootKustomization: true }),
    /add a manifests source/,
  );
  assert.throws(
    () =>
      validateManifestSources(
        app(undefined, [{ name: 'c', type: 'config', configMap: 'c', files: ['a'] }]),
        none,
      ),
    /add a manifests source/,
  );
  assert.throws(
    () =>
      validateManifestSources(app([{ type: 'manifests', path: 'manifests' }]), {
        hasManifests: true,
        hasRootKustomization: true,
      }),
    /must be \./,
  );
  validateManifestSources(app([{ type: 'manifests', path: '.' }]), {
    hasManifests: true,
    hasRootKustomization: true,
  });
});
