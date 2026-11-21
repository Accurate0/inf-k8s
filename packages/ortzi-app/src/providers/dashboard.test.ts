import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parse } from 'yaml';
import { renderKustomization, validate, validateResources } from '../render.ts';
import { target, platform, base, readValues } from '../test-fixtures.ts';

test('a dashboard resource generates a labelled configmap', () => {
  const app = validate(
    {
      ...base,
      resources: [
        { name: 'd', type: 'dashboard', configMap: 'foo-dashboard', file: 'dashboards/foo.json' },
      ],
    },
    'test',
  );
  const { configMapGenerator, resources } = parse(
    renderKustomization(app, { ...target, hasManifests: true }) as string,
  );

  assert.equal(configMapGenerator[0].options.labels['inf-k8s.net/grafana-dashboard'], 'true');
  assert.deepEqual(configMapGenerator[0].files, ['dashboards/foo.json']);
  assert.deepEqual(resources, ['manifests']);
  validateResources({ platform, apps: [app] });
});
