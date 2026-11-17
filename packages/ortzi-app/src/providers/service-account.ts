import type { Schema } from 'jsonschema/lib/index.js';
import { stringify } from 'yaml';
import type { App, Target } from '../types.ts';
import { appNamespace, declared } from '../app.ts';
import { HEADER } from '../generated.ts';
import { appWorkloads } from './workload.ts';
import type { ResourceProvider } from './provider.ts';

export interface ServiceAccount {
  name: string;
  type: 'serviceAccount';
  clusterRole?: string;
  clusterRules?: Record<string, unknown>[];
}

declare module './provider.ts' {
  interface ResourceTypes {
    serviceAccount: ServiceAccount;
  }
}

const schema: Schema = {
  type: 'object',
  additionalProperties: false,
  required: ['name', 'type'],
  properties: {
    name: {
      type: 'string',
    },
    type: {
      enum: ['serviceAccount'],
    },
    clusterRole: {
      type: 'string',
    },
    clusterRules: {
      type: 'array',
      items: {
        type: 'object',
      },
    },
  },
};

const RBAC = 'manifests/rbac.generated.yaml';

export const rbacPath = (app: App, target: Target) => `${target.location}/${app.name}/${RBAC}`;

const clusterRoles = (accounts: ServiceAccount[]) =>
  accounts.filter((account) => account.clusterRules);

export function renderRbac(app: App): string | undefined {
  const documents = clusterRoles(declared(app, 'serviceAccount')).flatMap((account) => {
    const role = account.clusterRole ?? account.name;
    return [
      {
        apiVersion: 'rbac.authorization.k8s.io/v1',
        kind: 'ClusterRole',
        metadata: { name: role },
        rules: account.clusterRules,
      },
      {
        apiVersion: 'rbac.authorization.k8s.io/v1',
        kind: 'ClusterRoleBinding',
        metadata: { name: account.name },
        subjects: [{ kind: 'ServiceAccount', name: account.name, namespace: appNamespace(app) }],
        roleRef: { apiGroup: 'rbac.authorization.k8s.io', kind: 'ClusterRole', name: role },
      },
    ];
  });

  if (!documents.length) {
    return undefined;
  }
  return `${HEADER}\n${documents.map((doc) => stringify(doc, { lineWidth: 0, aliasDuplicateObjects: false })).join('---\n')}`;
}

export const serviceAccount: ResourceProvider<'serviceAccount'> = {
  type: 'serviceAccount',
  schema,

  check(app, accounts, { fail }) {
    for (const [name, spec] of Object.entries(appWorkloads(app))) {
      if (
        spec.serviceAccount &&
        !accounts.some((account) => account.name === spec.serviceAccount)
      ) {
        fail(`workloads.${name}: no serviceAccount resource named ${spec.serviceAccount}`);
      }
    }
  },

  kustomize: (_app, accounts) => ({ resources: clusterRoles(accounts).length ? [RBAC] : [] }),

  generate(app, target) {
    const rbac = renderRbac(app);
    return rbac ? [{ path: rbacPath(app, target), content: rbac }] : [];
  },
};
