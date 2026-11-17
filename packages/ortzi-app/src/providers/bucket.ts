import type { Schema } from 'jsonschema/lib/index.js';
import type { App } from '../types.ts';
import { appNamespace, declared } from '../app.ts';
import { managedSecret, type SecretTemplate } from './secret.ts';
import { type Block, type ResourceProvider, tfRef } from './provider.ts';

export interface Bucket {
  name: string;
  type: 'bucket';
  provider: 'r2' | 's3' | 'garage';
  bucket: string;
  user?: string;
  secretName: string;
  secretTemplate?: SecretTemplate;
}

declare module './provider.ts' {
  interface ResourceTypes {
    bucket: Bucket;
  }
}

const schema: Schema = {
  type: 'object',
  additionalProperties: false,
  required: ['name', 'type', 'provider', 'secretName', 'bucket'],
  properties: {
    name: {
      type: 'string',
    },
    type: {
      enum: ['bucket'],
    },
    provider: {
      enum: ['r2', 's3', 'garage'],
    },
    bucket: {
      type: 'string',
    },
    user: {
      type: 'string',
      description: 'IAM user name, for s3. Defaults to the bucket name.',
    },
    secretName: {
      type: 'string',
    },
    secretTemplate: {
      type: 'object',
      description: 'Reshape the synced secret, for consumers that expect other keys.',
      additionalProperties: false,
      required: ['data'],
      properties: {
        type: {
          type: 'string',
        },
        data: {
          type: 'object',
          additionalProperties: {
            type: 'string',
          },
        },
      },
    },
  },
};

const BUCKET_KEYS = [
  'S3_BUCKET',
  'S3_ENDPOINT',
  'S3_REGION',
  'S3_ACCESS_KEY_ID',
  'S3_SECRET_ACCESS_KEY',
];

const bucketSecret = (app: App, id: string, name: string, data: Record<string, string>): Block => ({
  vault_kv_secret_v2: {
    [id]: {
      mount: tfRef('var.kv_mount'),
      name: `${appNamespace(app)}/resource-${name}`,
      data_json: JSON.stringify(data),
    },
  },
});

function r2(app: App, id: string, resource: Bucket, bucket: string): Block {
  const account = tfRef('var.cloudflare_account_id');
  return {
    cloudflare_r2_bucket: {
      [id]: { account_id: account, name: bucket, lifecycle: { prevent_destroy: true } },
    },
    cloudflare_account_token: {
      [id]: {
        account_id: account,
        name: `r2-${bucket}`,
        policies: [
          {
            effect: 'allow',
            permission_groups: [{ id: tfRef('var.r2_write_permission_group') }],
            resources: JSON.stringify({
              [`com.cloudflare.edge.r2.bucket.${account}_default_${tfRef(`cloudflare_r2_bucket.${id}.name`)}`]:
                '*',
            }),
          },
        ],
      },
    },
    ...bucketSecret(app, id, resource.name, {
      S3_BUCKET: tfRef(`cloudflare_r2_bucket.${id}.name`),
      S3_ENDPOINT: `https://${account}.r2.cloudflarestorage.com`,
      S3_REGION: 'auto',
      S3_ACCESS_KEY_ID: tfRef(`cloudflare_account_token.${id}.id`),
      S3_SECRET_ACCESS_KEY: tfRef(`sha256(cloudflare_account_token.${id}.value)`),
    }),
  };
}

function s3(app: App, id: string, resource: Bucket, bucket: string): Block {
  const arn = tfRef(`aws_s3_bucket.${id}.arn`);
  const user = resource.user ?? bucket;
  return {
    aws_s3_bucket: { [id]: { bucket, lifecycle: { prevent_destroy: true } } },
    aws_s3_bucket_public_access_block: {
      [id]: {
        bucket: tfRef(`aws_s3_bucket.${id}.id`),
        block_public_acls: true,
        block_public_policy: true,
        restrict_public_buckets: true,
        ignore_public_acls: true,
      },
    },
    aws_iam_user: { [id]: { name: user } },
    aws_iam_policy: {
      [id]: {
        name: `${user}-access`,
        description: `Access policy for ${app.name} application`,
        policy: JSON.stringify({
          Version: '2012-10-17',
          Statement: [
            {
              Sid: 'AllowBucketAccess',
              Effect: 'Allow',
              Action: ['s3:DeleteObject', 's3:GetObject', 's3:PutObject', 's3:ListBucket'],
              Resource: [arn, `${arn}/*`],
            },
          ],
        }),
      },
    },
    aws_iam_user_policy_attachment: {
      [id]: {
        user: tfRef(`aws_iam_user.${id}.name`),
        policy_arn: tfRef(`aws_iam_policy.${id}.arn`),
      },
    },
    aws_iam_access_key: { [id]: { user: tfRef(`aws_iam_user.${id}.name`) } },
    ...bucketSecret(app, id, resource.name, {
      S3_BUCKET: tfRef(`aws_s3_bucket.${id}.bucket`),
      S3_ENDPOINT: `https://${tfRef(`aws_s3_bucket.${id}.bucket_regional_domain_name`)}`,
      S3_REGION: tfRef(`aws_s3_bucket.${id}.region`),
      S3_ACCESS_KEY_ID: tfRef(`aws_iam_access_key.${id}.id`),
      S3_SECRET_ACCESS_KEY: tfRef(`aws_iam_access_key.${id}.secret`),
    }),
  };
}

function garage(app: App, id: string, resource: Bucket, bucket: string): Block {
  return {
    garage_bucket: { [id]: { global_alias: bucket, lifecycle: { prevent_destroy: true } } },
    garage_key: { [id]: { name: id } },
    garage_bucket_permission: {
      [id]: {
        bucket_id: tfRef(`garage_bucket.${id}.id`),
        access_key_id: tfRef(`garage_key.${id}.id`),
        read: true,
        write: true,
      },
    },
    ...bucketSecret(app, id, resource.name, {
      S3_BUCKET: bucket,
      S3_ENDPOINT: tfRef('var.garage_s3_endpoint'),
      S3_REGION: 'garage',
      S3_ACCESS_KEY_ID: tfRef(`garage_key.${id}.id`),
      S3_SECRET_ACCESS_KEY: tfRef(`garage_key.${id}.secret_access_key`),
    }),
  };
}

const BACKENDS = { r2, s3, garage };

const terraformId = (app: App, resource: Bucket) => `${app.name}-${resource.name}`;

export const bucket: ResourceProvider<'bucket'> = {
  type: 'bucket',
  ref: 'value',
  schema,

  value(app, resource) {
    throw new Error(
      `${app.name}: resources.${resource.name}: read a bucket with {bucket, key}, not {resource}`,
    );
  },

  unused: (_app, resource) => (resource.secretTemplate ? false : undefined),

  check(_app, _buckets, { uses, fail }) {
    for (const ref of uses) {
      if (ref.bucket && ref.key && !BUCKET_KEYS.includes(ref.key as string)) {
        fail(
          `bucket ${ref.bucket}: no key named ${ref.key}, expected one of ${BUCKET_KEYS.join(', ')}`,
        );
      }
    }
  },

  plan: (_ctx, buckets) => ({
    secrets: {
      synced: buckets.map((resource) => ({
        name: `resource-${resource.name}`,
        managedSecrets: [managedSecret(resource)],
      })),
    },
  }),

  terraform: (app, buckets) =>
    buckets.map((resource) =>
      BACKENDS[resource.provider](app, terraformId(app, resource), resource, resource.bucket),
    ),

  terraformOutputs(apps) {
    const s3 = apps
      .flatMap((app) =>
        declared(app, 'bucket')
          .filter((resource) => resource.provider === 's3')
          .map((resource) => terraformId(app, resource)),
      )
      .sort();
    if (!s3.length) {
      return {};
    }

    return {
      s3_access_keys: {
        sensitive: true,
        value: Object.fromEntries(
          s3.map((id) => [
            id,
            {
              id: tfRef(`aws_iam_access_key.${id}.id`),
              secret: tfRef(`aws_iam_access_key.${id}.secret`),
            },
          ]),
        ),
      },
    };
  },

  graph(app, resource, { node, edge }) {
    edge({
      from: app.name,
      to: node(
        `bucket/${resource.bucket}`,
        `${resource.bucket} (${resource.provider})`,
        'database',
      ),
    });
  },
};
