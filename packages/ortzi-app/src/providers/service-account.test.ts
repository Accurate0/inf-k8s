import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parse } from 'yaml';
import { renderKustomization, renderRbac, validate } from '../render.ts';
import { target, render, base, checkRefs, releaseValues } from '../test-fixtures.ts';

test('a serviceAccount resource generates its account and cluster role', () => {
  const rules = [{ apiGroups: [''], resources: ['secrets'], verbs: ['get'] }];
  const app = validate(
    {
      ...base,
      resources: [
        { name: 'sa', type: 'serviceAccount', clusterRole: 'foo-access', clusterRules: rules },
      ],
      workloads: [{ name: 'foo', image: 'i', serviceAccount: 'sa' }],
    },
    'test',
  );
  const values = releaseValues(render(app), 'k8s-workload');

  assert.deepEqual(values.serviceAccounts, ['sa']);
  assert.equal(values.workloads[0].serviceAccountName, 'sa');
  assert.equal(values.workloads[0].serviceAccount, undefined);
  const rbac = renderRbac(app) as string;

  assert.match(rbac, /kind: ClusterRole\nmetadata:\n  name: foo-access/);
  assert.match(rbac, /kind: ClusterRoleBinding\nmetadata:\n  name: sa/);
  assert.deepEqual(parse(renderKustomization(app, target) as string).resources, [
    'manifests/rbac.generated.yaml',
  ]);
  assert.throws(
    () => checkRefs({ workloads: [{ name: 'foo', image: 'i', serviceAccount: 'nope' }] }),
    /no serviceAccount resource named nope/,
  );
});
