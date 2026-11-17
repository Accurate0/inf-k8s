import assert from 'node:assert/strict';
import { test } from 'node:test';
import { validateManifestSources, type App, validate } from '../../render.ts';
import { render } from '../../test-fixtures.ts';

test('keeps manifests as an optional source', () => {
  const rendered = render(
    { name: 'foo', network: { ingress: [] }, sources: [{ type: 'manifests', path: 'manifests' }] },
    { location: 'platform-services', hasManifests: true },
  );

  assert.equal(rendered.spec.sources.at(-1).path, 'platform-services/foo/manifests');
});

test('uses the app directory when it has its own kustomization', () => {
  const rendered = render(
    { name: 'foo', network: { ingress: [] }, sources: [{ type: 'manifests', path: '.' }] },
    { location: 'platform-services', hasManifests: true },
  );

  assert.equal(rendered.spec.sources.at(-1).path, 'platform-services/foo');
});

test('manifests sources are limited and stay in the app directory', () => {
  const app = (sources: unknown[], extra = {}) => ({
    name: 'foo',
    network: { ingress: [] },
    sources,
    ...extra,
  });

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
