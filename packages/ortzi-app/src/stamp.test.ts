import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parse } from 'yaml';
import { stampRevision } from './stamp.ts';

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
