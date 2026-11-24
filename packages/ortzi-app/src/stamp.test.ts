import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parse, parseAllDocuments } from 'yaml';
import { stampFlux } from './stamp.ts';

const FLUX_COMMIT = 'event.toolkit.fluxcd.io/commit';

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

const sources = `apiVersion: source.toolkit.fluxcd.io/v1
kind: OCIRepository
metadata:
  name: foo-manifests
  annotations:
    event.toolkit.fluxcd.io/app: foo
spec:
  interval: 1h
`;

const namespace = `apiVersion: v1
kind: Namespace
metadata:
  name: foo
`;

test('flux publish stamps the revision on every flux object', () => {
  const stamped = stampFlux(
    new Map([
      ['ks.yaml', ks],
      ['app/helmrelease.yaml', releases],
      ['app/namespace.yaml', namespace],
      ['app/ocirepository.yaml', sources],
    ]),
    'abc',
  );
  const [workload, network] = parseAllDocuments(stamped.get('app/helmrelease.yaml')!).map((d) =>
    d.toJS(),
  );

  assert.match(stamped.get('app/helmrelease.yaml')!, /^# generated from app.yaml/);
  assert.equal(parse(stamped.get('ks.yaml')!).metadata.annotations[FLUX_COMMIT], 'abc');
  assert.equal(workload.metadata.annotations[FLUX_COMMIT], 'abc');
  assert.equal(network.metadata.annotations[FLUX_COMMIT], 'abc');
  assert.deepEqual(workload.spec.values.images, { 'curlimages/curl': '8.10.1' });
  assert.deepEqual(network.spec.values, {});
  assert.equal(stamped.get('app/namespace.yaml'), namespace);

  const source = parse(stamped.get('app/ocirepository.yaml')!);
  assert.equal(source.metadata.annotations['reconcile.fluxcd.io/requestedAt'], 'abc');
  assert.equal(source.metadata.annotations['event.toolkit.fluxcd.io/app'], 'foo');
  assert.equal(source.metadata.annotations[FLUX_COMMIT], undefined);
});
