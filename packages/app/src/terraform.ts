import type { App, Bucket } from './render.ts';
import { appDatabases, databaseRole, HEADER_TEXT } from './render.ts';

type Block = Record<string, Record<string, unknown>>;

export const TERRAFORM_APPS = 'terraform/apps';

const ref = (expr: string) => `\${${expr}}`;

function databaseResources(app: App, database: string, role: string): Block {
  return {
    vault_database_secret_backend_static_role: {
      [database]: {
        backend: ref('var.database_mount'),
        name: database,
        db_name: ref('var.database_name'),
        username: role,
        rotation_period: 315360000,
      },
    },
    vault_policy: {
      [database]: {
        name: `db-${database}`,
        policy: `path "${ref('var.database_mount')}/static-creds/${database}" {\n  capabilities = ["read"]\n}\n`,
      },
    },
    vault_kubernetes_auth_backend_role: {
      [database]: {
        backend: ref('var.kubernetes_auth_path'),
        role_name: `db-${database}`,
        bound_service_account_names: ['openbao-database'],
        bound_service_account_namespaces: [app.name],
        token_policies: [ref(`vault_policy.${database}.name`)],
        token_ttl: 3600,
      },
    },
  };
}

const bucketSecret = (app: App, id: string, name: string, data: Record<string, string>): Block => ({
  vault_kv_secret_v2: {
    [id]: {
      mount: ref('var.kv_mount'),
      name: `${app.name}/resource-${name}`,
      data_json: JSON.stringify(data),
    },
  },
});

function r2Resources(app: App, id: string, resource: Bucket, bucket: string): Block {
  const account = ref('var.cloudflare_account_id');
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
            permission_groups: [{ id: ref('var.r2_write_permission_group') }],
            resources: JSON.stringify({
              [`com.cloudflare.edge.r2.bucket.${account}_default_${ref(`cloudflare_r2_bucket.${id}.name`)}`]: '*',
            }),
          },
        ],
      },
    },
    ...bucketSecret(app, id, resource.name, {
      S3_BUCKET: ref(`cloudflare_r2_bucket.${id}.name`),
      S3_ENDPOINT: `https://${account}.r2.cloudflarestorage.com`,
      S3_REGION: 'auto',
      S3_ACCESS_KEY_ID: ref(`cloudflare_account_token.${id}.id`),
      S3_SECRET_ACCESS_KEY: ref(`sha256(cloudflare_account_token.${id}.value)`),
    }),
  };
}

function s3Resources(app: App, id: string, resource: Bucket, bucket: string): Block {
  const arn = ref(`aws_s3_bucket.${id}.arn`);
  const user = resource.user ?? bucket;
  return {
    aws_s3_bucket: { [id]: { bucket, lifecycle: { prevent_destroy: true } } },
    aws_s3_bucket_public_access_block: {
      [id]: {
        bucket: ref(`aws_s3_bucket.${id}.id`),
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
      [id]: { user: ref(`aws_iam_user.${id}.name`), policy_arn: ref(`aws_iam_policy.${id}.arn`) },
    },
    aws_iam_access_key: { [id]: { user: ref(`aws_iam_user.${id}.name`) } },
    ...bucketSecret(app, id, resource.name, {
      S3_BUCKET: ref(`aws_s3_bucket.${id}.bucket`),
      S3_ENDPOINT: `https://${ref(`aws_s3_bucket.${id}.bucket_regional_domain_name`)}`,
      S3_REGION: ref(`aws_s3_bucket.${id}.region`),
      S3_ACCESS_KEY_ID: ref(`aws_iam_access_key.${id}.id`),
      S3_SECRET_ACCESS_KEY: ref(`aws_iam_access_key.${id}.secret`),
    }),
  };
}

const merge = (blocks: Block[]): Block => {
  const out: Block = {};
  for (const block of blocks) {
    for (const [type, resources] of Object.entries(block)) out[type] = { ...out[type], ...resources };
  }
  return out;
};

const json = (value: Record<string, unknown>) => `${JSON.stringify({ '//': HEADER_TEXT, ...value }, null, 2)}\n`;

export const appTerraformPath = (app: App) => `${TERRAFORM_APPS}/${app.name}.tf.json`;

export const bucketName = (app: App, resource: Bucket) => resource.bucket ?? `${app.name}-${resource.name}`;

export const bucketSecretName = (resource: Bucket) => `${resource.name}-bucket-secrets`;

export function renderAppTerraform(app: App): string | undefined {
  const blocks = [
    ...Object.entries(appDatabases(app)).map(([database, spec]) =>
      databaseResources(app, database, databaseRole(database, spec)),
    ),
    ...(app.resources ?? []).flatMap((resource) => {
      if (resource.type !== 'bucket') return [];
      const id = `${app.name}-${resource.name}`;
      const render = resource.provider === 'r2' ? r2Resources : s3Resources;
      return [render(app, id, resource, bucketName(app, resource))];
    }),
  ];
  return blocks.length ? json({ resource: merge(blocks) }) : undefined;
}

export function renderTerraformIndex(apps: App[]): string {
  const names = apps.flatMap((app) => Object.keys(appDatabases(app))).sort();
  const s3 = apps
    .flatMap((app) =>
      (app.resources ?? []).flatMap((resource) =>
        resource.type === 'bucket' && resource.provider === 's3' ? [`${app.name}-${resource.name}`] : [],
      ),
    )
    .sort();
  return json({
    output: {
      database_names: { value: names },
      ...(s3.length
        ? {
            s3_access_keys: {
              sensitive: true,
              value: Object.fromEntries(
                s3.map((id) => [
                  id,
                  { id: ref(`aws_iam_access_key.${id}.id`), secret: ref(`aws_iam_access_key.${id}.secret`) },
                ]),
              ),
            },
          }
        : {}),
    },
  });
}
