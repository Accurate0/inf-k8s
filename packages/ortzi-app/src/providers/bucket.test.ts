import assert from 'node:assert/strict';
import { test } from 'node:test';
import { renderAppTerraform, renderTerraformIndex } from '../terraform.ts';
import { type App, validate, validateResources } from '../render.ts';
import {
  platform,
  render,
  base,
  checkRefs,
  overridden,
  releaseValues,
  readValues,
} from '../test-fixtures.ts';

test('terraform covers an r2 bucket with a token for that bucket only', () => {
  const { resource } = JSON.parse(
    renderAppTerraform({
      ...base,
      resources: [
        {
          name: 'images',
          type: 'bucket',
          bucket: 'foo-images',
          secretName: 'images-bucket-secrets',
          provider: 'r2',
        },
      ],
    }) as string,
  );
  const id = 'foo-images';

  assert.equal(resource.cloudflare_r2_bucket[id].name, 'foo-images');
  assert.equal(resource.cloudflare_r2_bucket[id].lifecycle.prevent_destroy, true);
  assert.match(
    resource.cloudflare_account_token[id].policies[0].resources,
    /r2\.bucket\..*_default_/,
  );
  assert.equal(resource.vault_kv_secret_v2[id].name, 'foo/resource-images');
  assert.match(
    resource.vault_kv_secret_v2[id].data_json,
    /sha256\(cloudflare_account_token\.foo-images\.value\)/,
  );
  assert.equal(resource.aws_s3_bucket, undefined);
});

test('terraform covers an s3 bucket with its own user', () => {
  const { resource } = JSON.parse(
    renderAppTerraform({
      ...base,
      resources: [
        {
          name: 'images',
          type: 'bucket',
          secretName: 'images-bucket-secrets',
          provider: 's3',
          bucket: 'named',
        },
      ],
    }) as string,
  );
  const id = 'foo-images';

  assert.equal(resource.aws_s3_bucket[id].bucket, 'named');
  assert.equal(resource.aws_s3_bucket[id].lifecycle.prevent_destroy, true);
  assert.equal(resource.aws_s3_bucket_public_access_block[id].block_public_policy, true);
  assert.match(resource.aws_iam_policy[id].policy, /aws_s3_bucket\.foo-images\.arn/);
  assert.equal(resource.vault_kv_secret_v2[id].name, 'foo/resource-images');
  assert.equal(resource.cloudflare_r2_bucket, undefined);
});

test('terraform covers a garage bucket with a key bound to it', () => {
  const { resource } = JSON.parse(
    renderAppTerraform({
      ...base,
      resources: [
        {
          name: 'images',
          type: 'bucket',
          secretName: 'images-bucket-secrets',
          provider: 'garage',
          bucket: 'foo-images',
        },
      ],
    }) as string,
  );
  const id = 'foo-images';

  assert.equal(resource.garage_bucket[id].global_alias, 'foo-images');
  assert.equal(resource.garage_bucket[id].lifecycle.prevent_destroy, true);
  assert.equal(resource.garage_bucket_permission[id].bucket_id, '${garage_bucket.foo-images.id}');
  assert.equal(resource.garage_bucket_permission[id].access_key_id, '${garage_key.foo-images.id}');
  assert.equal(resource.garage_bucket_permission[id].write, true);
  assert.equal(resource.vault_kv_secret_v2[id].name, 'foo/resource-images');

  const data = JSON.parse(resource.vault_kv_secret_v2[id].data_json);
  assert.equal(data.S3_ENDPOINT, '${var.garage_s3_endpoint}');
  assert.equal(data.S3_SECRET_ACCESS_KEY, '${garage_key.foo-images.secret_access_key}');
  assert.equal(resource.aws_s3_bucket, undefined);
});

test('a bucket resource wires its name and managed secret', () => {
  const app = validate(
    {
      ...base,
      resources: [
        {
          name: 'images',
          type: 'bucket',
          bucket: 'foo-images',
          secretName: 'images-bucket-secrets',
          provider: 'r2',
        },
      ],
      workloads: [
        {
          name: 'foo',
          image: 'i',
          env: { BUCKET: { bucket: 'images', key: 'S3_BUCKET' } },
          envFrom: [{ bucket: 'images' }],
        },
      ],
    },
    'test',
  );
  const rendered = render(app);

  assert.deepEqual(releaseValues(rendered, 'k8s-workload').workloads[0].env, [
    {
      name: 'BUCKET',
      valueFrom: { secretKeyRef: { name: 'images-bucket-secrets', key: 'S3_BUCKET' } },
    },
  ]);
  assert.deepEqual(releaseValues(rendered, 'k8s-secrets').secrets, [
    { name: 'resource-images', managedSecrets: [{ secretName: 'images-bucket-secrets' }] },
  ]);
});

test('an s3 bucket can keep an existing iam user', () => {
  const app: App = {
    ...base,
    resources: [
      {
        name: 'storage',
        type: 'bucket',
        secretName: 'storage-bucket-secrets',
        provider: 's3',
        bucket: 'named',
        user: 'existing',
      },
    ],
  };
  const { resource } = JSON.parse(renderAppTerraform(app) as string);

  assert.equal(resource.aws_iam_user['foo-storage'].name, 'existing');
  assert.equal(resource.aws_iam_policy['foo-storage'].name, 'existing-access');
  assert.equal(
    resource.aws_iam_policy['foo-storage'].description,
    'Access policy for foo application',
  );
  const index = JSON.parse(renderTerraformIndex([app]));

  assert.equal(
    index.output.s3_access_keys.value['foo-storage'].id,
    '${aws_iam_access_key.foo-storage.id}',
  );
  assert.equal(index.output.s3_access_keys.sensitive, true);
});

test('the terraform index has no key output without s3 buckets', () => {
  assert.equal(JSON.parse(renderTerraformIndex([base])).output.s3_access_keys, undefined);
});

test('rejects a bucket key terraform does not write', () => {
  const bucket = (key: string) => ({
    resources: [{ name: 'b', type: 'bucket', bucket: 'b', provider: 'r2', secretName: 's' }],
    workloads: [{ name: 'foo', image: 'i', env: { A: { bucket: 'b', key } } }],
  });
  checkRefs(bucket('S3_ENDPOINT'));

  assert.throws(() => checkRefs(bucket('S3_NOPE')), /no key named S3_NOPE/);
});

test('a bucket secret template sets the secret type and data', () => {
  assert.deepEqual(releaseValues(render(overridden), 'k8s-secrets').secrets, [
    {
      name: 'resource-images',
      managedSecrets: [
        {
          secretName: 'images-bucket-secrets',
          type: 'example.io/config',
          template: { data: { bucket: '{{ .S3_BUCKET }}' } },
        },
      ],
    },
  ]);
});

test('a bucket with a secret template needs no workload', () => {
  assert.doesNotThrow(() => validateResources({ platform, apps: [overridden] }));
});
