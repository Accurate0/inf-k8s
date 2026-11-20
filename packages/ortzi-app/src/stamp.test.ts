import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parse, parseAllDocuments } from 'yaml';
import { stampFlux, stampRevision } from './stamp.ts';

const FLUX_REVISION = 'event.toolkit.fluxcd.io/revision';

const application = `apiVersion: argoproj.io/v1alpha1
kind: Application
metadata:
  name: foo
spec:
  sources:
    - chart: k8s-workload
      helm:
        valuesObject:
          images:
            curlimages/curl: 8.10.1
          namespace: foo
    - chart: k8s-network
`;

test('publish stamps the revision and injects released images into the workload chart', () => {
  const stamped = parse(
    stampRevision(application, 'abc', {
      'registry.anurag.sh/containers/foo': 'f00@sha256:1234',
    }),
  );

  assert.equal(stamped.metadata.annotations['inf-k8s.net/revision'], 'abc');
  assert.deepEqual(stamped.spec.sources[0].helm.valuesObject.images, {
    'curlimages/curl': '8.10.1',
    'registry.anurag.sh/containers/foo': 'f00@sha256:1234',
  });
  assert.equal(stamped.spec.sources[0].helm.valuesObject.namespace, 'foo');
  assert.equal(stamped.spec.sources[1].helm, undefined);
});

test('a released image needs a workload chart to land in', () => {
  const without = application.replace('chart: k8s-workload', 'chart: k8s-other');
  assert.throws(
    () => stampRevision(without, 'abc', { 'registry.anurag.sh/containers/foo': 't' }),
    /has no k8s-workload source/,
  );
  assert.doesNotThrow(() => stampRevision(without, 'abc'));
});

const releases = `# generated from app.yaml by \`ortzi app render\`; edit app.yaml instead
apiVersion: helm.toolkit.fluxcd.io/v2
kind: HelmRelease
metadata:
  name: foo-k8s-workload
spec:
  values:
    images:
      curlimages/curl: 8.10.1
---
apiVersion: helm.toolkit.fluxcd.io/v2
kind: HelmRelease
metadata:
  name: foo-k8s-network
spec:
  values: {}
`;

const ks = `apiVersion: kustomize.toolkit.fluxcd.io/v1
kind: Kustomization
metadata:
  name: foo
`;

const namespace = `apiVersion: v1
kind: Namespace
metadata:
  name: foo
`;

test('flux publish stamps the revision and injects released images into the workload release', () => {
  const stamped = stampFlux(
    new Map([
      ['ks.yaml', ks],
      ['app/helmrelease.yaml', releases],
      ['app/namespace.yaml', namespace],
    ]),
    'abc',
    { 'registry.anurag.sh/containers/foo': 'f00@sha256:1234' },
  );
  const [workload, network] = parseAllDocuments(stamped.get('app/helmrelease.yaml')!).map((d) =>
    d.toJS(),
  );

  assert.match(stamped.get('app/helmrelease.yaml')!, /^# generated from app.yaml/);
  assert.equal(parse(stamped.get('ks.yaml')!).metadata.annotations[FLUX_REVISION], 'abc');
  assert.equal(workload.metadata.annotations[FLUX_REVISION], 'abc');
  assert.equal(network.metadata.annotations[FLUX_REVISION], 'abc');
  assert.deepEqual(workload.spec.values.images, {
    'curlimages/curl': '8.10.1',
    'registry.anurag.sh/containers/foo': 'f00@sha256:1234',
  });
  assert.deepEqual(network.spec.values, {});
  assert.equal(stamped.get('app/namespace.yaml'), namespace);
});

test('a flux released image needs a workload release to land in', () => {
  const without = new Map([
    ['app/helmrelease.yaml', releases.replace('foo-k8s-workload', 'foo-k8s-other')],
  ]);

  assert.throws(
    () => stampFlux(without, 'abc', { 'registry.anurag.sh/containers/foo': 't' }),
    /has no k8s-workload release/,
  );
  assert.doesNotThrow(() => stampFlux(without, 'abc'));
});
