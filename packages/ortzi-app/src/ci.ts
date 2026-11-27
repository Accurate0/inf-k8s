import type { App, AppWorkflow, WorkflowEnv } from './types.ts';
import type { Block, Fail } from './providers/provider.ts';
import { appNamespace, declared } from './app.ts';
import { tfRef } from './providers/provider.ts';

export const THIS_REPO = 'anurag/k8s';
export const OPENBAO_AUDIENCE = 'openbao-cli';

export const granted = (app: App) => new Set(app.ci?.secrets?.resources ?? []);

export const serviceAccountSecretPath = (name: string) => `generated/serviceaccount-${name}`;

export function openbaoPath(app: App, name: string): string {
  const namespace = appNamespace(app);
  const found = (app.resources ?? []).filter((r) => r.name === name);
  const ci = found.filter((r) => ['bucket', 'secret', 'serviceAccount'].includes(r.type));
  if (ci.length !== 1) {
    throw new Error(
      ci.length
        ? `${app.name}: ci.secrets.${name} names more than one resource`
        : `${app.name}: no bucket, secret or serviceAccount resource named ${name}`,
    );
  }

  const [resource] = ci;
  switch (resource.type) {
    case 'bucket':
      return `${namespace}/resource-${name}`;
    case 'secret':
      if (resource.generate) {
        return `${namespace}/generated/${name}`;
      }
      return name === namespace ? namespace : `${namespace}/${name}`;
    case 'serviceAccount':
      return `${namespace}/${serviceAccountSecretPath(name)}`;
    default:
      throw new Error(`${app.name}: ${resource.type} ${name} has no OpenBao secret`);
  }
}

export const envResource = (value: WorkflowEnv) =>
  typeof value === 'string' ? undefined : (value.bucket ?? value.secret);

export function workflowResources(workflow: AppWorkflow): string[] {
  const refs = Object.values(workflow.env ?? {}).flatMap((value) => envResource(value) ?? []);
  return [...new Set(refs)];
}

export function workflowRefs(app: App): Record<string, unknown>[] {
  return (app.workflows ?? []).flatMap((workflow) => [
    ...Object.values(workflow.env ?? {}).filter((value) => typeof value !== 'string'),
    ...(workflow.serviceAccount ? [{ serviceAccount: workflow.serviceAccount }] : []),
  ]);
}

export function validateCi(app: App, fail: Fail) {
  const grants = granted(app);
  for (const name of grants) {
    try {
      openbaoPath(app, name);
    } catch (error) {
      fail((error as Error).message.replace(`${app.name}: `, `ci.secrets.resources: `));
    }
  }

  const names = (app.workflows ?? []).map((workflow) => workflow.name);
  for (const workflow of app.workflows ?? []) {
    const where = `workflows.${workflow.name}`;
    if (names.indexOf(workflow.name) !== names.lastIndexOf(workflow.name)) {
      fail(`workflows: duplicate name ${workflow.name}`);
    }
    if (!workflow.schedule && !workflow.paths?.length) {
      fail(`${where}: needs a schedule or paths to run on`);
    }

    const used = [
      ...workflowResources(workflow),
      ...(workflow.serviceAccount ? [workflow.serviceAccount] : []),
    ];
    if (used.length && !app.ci?.secrets?.repos.includes(THIS_REPO)) {
      fail(`${where}: reads secrets, so ci.secrets.repos must include ${THIS_REPO}`);
    }
    for (const name of used) {
      if (!grants.has(name)) {
        fail(`${where}: ${name} is not in ci.secrets.resources`);
      }
    }

    const account = workflow.serviceAccount;
    if (account && !declared(app, 'serviceAccount').some((r) => r.name === account)) {
      fail(`${where}: no serviceAccount resource named ${account}`);
    }
  }
}

export function ciTerraform(app: App): Block[] {
  const secrets = app.ci?.secrets;
  if (!secrets) {
    return [];
  }

  const id = `${app.name}-ci`;
  const policy = secrets.resources
    .map((name) => openbaoPath(app, name))
    .map((path) => `path "${tfRef('var.kv_mount')}/data/${path}" {\n  capabilities = ["read"]\n}\n`)
    .join('\n');

  return [
    {
      vault_policy: {
        [id]: { name: id, policy },
      },
      vault_jwt_auth_backend_role: {
        [id]: {
          backend: tfRef('var.forgejo_auth_path'),
          role_name: appNamespace(app),
          role_type: 'jwt',
          bound_audiences: [OPENBAO_AUDIENCE],
          bound_claims: {
            repository: secrets.repos.join(','),
            ref: 'refs/heads/main',
          },
          user_claim: 'sub',
          token_policies: [tfRef(`vault_policy.${id}.name`)],
          token_ttl: 900,
          token_max_ttl: 3600,
        },
      },
    },
  ];
}
