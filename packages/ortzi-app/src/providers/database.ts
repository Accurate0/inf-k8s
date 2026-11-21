import type { Schema } from 'jsonschema/lib/index.js';
import { parseDocument, stringify } from 'yaml';
import type { App, PlatformExport } from '../types.ts';
import { appNamespace, declared, specs } from '../app.ts';
import { HEADER } from '../generated.ts';
import { fail, type KubeObject, ok } from '../checks.ts';
import { type ResourceProvider, tfRef } from './provider.ts';

export interface DatabaseSpec {
  secretName: string;
  enableConnectionPooling?: boolean;
  role?: string;
  extensions?: string[];
}

declare module './provider.ts' {
  interface ResourceTypes {
    database: { name: string; type: 'database' } & DatabaseSpec;
  }
}

const schema: Schema = {
  type: 'object',
  additionalProperties: false,
  required: ['name', 'type', 'secretName'],
  properties: {
    name: {
      type: 'string',
    },
    type: {
      enum: ['database'],
    },
    secretName: {
      type: 'string',
    },
    enableConnectionPooling: {
      type: 'boolean',
      description: 'Connect through the pooler. Defaults to true.',
    },
    role: {
      type: 'string',
      description:
        'Postgres role and database name. Defaults to the name with hyphens as underscores.',
    },
    extensions: {
      type: 'array',
      description: 'Postgres extensions to ensure are present.',
      items: {
        type: 'string',
      },
    },
  },
};

export const POSTGRES_DIR = 'kubernetes/cnpg-system/cloudnative-pg-cluster/config';
const POSTGRES_CLUSTER = `${POSTGRES_DIR}/postgres.yaml`;
const DATABASES = `${POSTGRES_DIR}/databases`;

export const appDatabases = (app: App): Record<string, DatabaseSpec> =>
  Object.fromEntries(specs(declared(app, 'database')));

export const databaseRole = (database: string, spec?: Pick<DatabaseSpec, 'role'>) =>
  spec?.role ?? database.replaceAll('-', '_');

export const appRoles = (apps: App[]) =>
  apps.flatMap((app) =>
    Object.entries(appDatabases(app)).map(([database, spec]) => databaseRole(database, spec)),
  );

export function databaseHost(app: App, spec: DatabaseSpec, platform: PlatformExport[]) {
  const name = spec.enableConnectionPooling === false ? 'direct' : 'pooled';
  const found = platform.find((e) => e.type === 'database' && e.name === name);
  if (!found?.host) {
    throw new Error(`${app.name}: no database export named ${name}`);
  }
  return found.host;
}

export const databasePath = (database: string) => `${DATABASES}/${database}.yaml`;

export function renderDatabase(
  database: string,
  spec?: Pick<DatabaseSpec, 'role' | 'extensions'>,
): string {
  const name = databaseRole(database, spec);
  const manifest = {
    apiVersion: 'postgresql.cnpg.io/v1',
    kind: 'Database',
    metadata: { name: database, namespace: 'cnpg-system' },
    spec: {
      name,
      owner: name,
      cluster: { name: 'cloudnative-pg-cluster' },
      databaseReclaimPolicy: 'retain',
      ...(spec?.extensions
        ? {
            extensions: spec.extensions.map((extension) => ({
              name: extension,
              ensure: 'present',
            })),
          }
        : {}),
    },
  };
  return `${HEADER}\n# yaml-language-server: $schema=https://k8s-schemas.anurag.sh/postgresql.cnpg.io/database_v1.json\n${stringify(manifest)}`;
}

export function addRoles(cluster: string, roles: string[]): string {
  const doc = parseDocument(cluster);
  const existing = (doc.toJS().spec.managed.roles as { name: string }[]).map((role) => role.name);
  const missing = roles.filter((role) => !existing.includes(role));
  if (!missing.length) {
    return cluster;
  }

  for (const name of missing) {
    doc.addIn(['spec', 'managed', 'roles'], {
      name,
      ensure: 'present',
      connectionLimit: -1,
      inherit: true,
      login: true,
    });
  }
  return doc.toString({ lineWidth: 0 });
}

const terraform = (app: App, database: string, role: string) => ({
  vault_database_secret_backend_static_role: {
    [database]: {
      backend: tfRef('var.database_mount'),
      name: database,
      db_name: tfRef('var.database_name'),
      username: role,
      rotation_period: 315360000,
    },
  },
  vault_policy: {
    [database]: {
      name: `db-${database}`,
      policy: `path "${tfRef('var.database_mount')}/static-creds/${database}" {\n  capabilities = ["read"]\n}\n`,
    },
  },
  vault_kubernetes_auth_backend_role: {
    [database]: {
      backend: tfRef('var.kubernetes_auth_path'),
      role_name: `db-${database}`,
      bound_service_account_names: ['openbao-database'],
      bound_service_account_namespaces: [appNamespace(app)],
      token_policies: [tfRef(`vault_policy.${database}.name`)],
      token_ttl: 3600,
    },
  },
});

export const database: ResourceProvider<'database'> = {
  type: 'database',
  ref: 'value',
  schema,

  value(app, resource) {
    throw new Error(
      `${app.name}: resources.${resource.name}: list a database's secret in envFrom, not env`,
    );
  },

  check(app, databases, { world }) {
    for (const resource of databases) {
      databaseHost(app, resource, world.platform);
    }
  },

  plan({ app, namespace, annotations, world, chart }, databases) {
    if (!databases.length) {
      return {};
    }
    return {
      charts: [
        chart('k8s-postgres', {
          databases: databases.map((resource) => ({
            name: resource.name,
            namespace,
            databaseName: databaseRole(resource.name, resource),
            secretName: resource.secretName,
            host: databaseHost(app, resource, world.platform),
            annotations,
          })),
        }),
      ],
    };
  },

  generate: (_app, _target, databases) =>
    databases.map((resource) => ({
      path: databasePath(resource.name),
      content: renderDatabase(resource.name, resource),
      stamp: true,
    })),

  generateWorld: (world, read) => [
    { path: POSTGRES_CLUSTER, content: addRoles(read(POSTGRES_CLUSTER), appRoles(world.apps)) },
    {
      path: `${DATABASES}/kustomization.yaml`,
      content: `${HEADER}\n${stringify({
        apiVersion: 'kustomize.config.k8s.io/v1beta1',
        kind: 'Kustomization',
        resources: world.apps
          .flatMap((app) => Object.keys(appDatabases(app)))
          .sort()
          .map((database) => `${database}.yaml`),
      })}`,
      stamp: true,
    },
  ],

  terraform: (app, databases) =>
    databases.map((resource) =>
      terraform(app, resource.name, databaseRole(resource.name, resource)),
    ),

  terraformOutputs: (apps) => ({
    database_names: { value: apps.flatMap((app) => Object.keys(appDatabases(app))).sort() },
  }),

  graph(app, resource, { node, edge }) {
    const name = databaseRole(resource.name, resource);
    edge({ from: app.name, to: node(`database/${name}`, name, 'database') });
  },

  doctor: {
    label: 'databases',
    run({ kube }, databases) {
      if (!databases.length) {
        return [];
      }

      const cnpg = kube.namespace('cnpg-system');
      const reconciled: string[] =
        cnpg.get<KubeObject>('clusters.postgresql.cnpg.io', 'cloudnative-pg-cluster').status
          ?.managedRolesStatus?.byStatus?.reconciled ?? [];

      return databases.flatMap((resource) => {
        const role = databaseRole(resource.name, resource);
        const applied = cnpg.get<KubeObject>('databases.postgresql.cnpg.io', resource.name).status;
        return [
          applied?.applied
            ? ok(`database ${resource.name} applied`)
            : fail(
                `database ${resource.name} not applied${applied?.message ? `: ${applied.message}` : ''}`,
              ),
          reconciled.includes(role)
            ? ok(`role ${role} reconciled`)
            : fail(`role ${role} not reconciled`),
        ];
      });
    },
  },
};
