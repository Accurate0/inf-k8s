import type { Schema } from 'jsonschema/lib/index.js';
import { stringify } from 'yaml';
import type { App, Target } from '../types.ts';
import { appNamespace, declared } from '../app.ts';
import { HEADER } from '../generated.ts';
import { appWorkloads } from './workload.ts';
import { granted, serviceAccountSecretPath } from '../ci.ts';
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

export const ciTokenSecret = (account: ServiceAccount) => `${account.name}-ci-token`;

const forCi = (app: App, accounts: ServiceAccount[]) =>
  accounts.filter((account) => granted(app).has(account.name));

const needsManifests = (app: App, accounts: ServiceAccount[]) =>
  clusterRoles(accounts).length > 0 || forCi(app, accounts).length > 0;

export function renderRbac(app: App): string | undefined {
  const accounts = declared(app, 'serviceAccount');
  const namespace = appNamespace(app);
  const unowned = Object.keys(appWorkloads(app)).length ? [] : forCi(app, accounts);

  const documents = [
    ...unowned.map((account) => ({
      apiVersion: 'v1',
      kind: 'ServiceAccount',
      metadata: { name: account.name, namespace },
    })),
    ...forCi(app, accounts).map((account) => ({
      apiVersion: 'v1',
      kind: 'Secret',
      type: 'kubernetes.io/service-account-token',
      metadata: {
        name: ciTokenSecret(account),
        namespace,
        annotations: { 'kubernetes.io/service-account.name': account.name },
      },
    })),
    ...clusterRoles(accounts).flatMap((account) => {
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
          subjects: [{ kind: 'ServiceAccount', name: account.name, namespace }],
          roleRef: { apiGroup: 'rbac.authorization.k8s.io', kind: 'ClusterRole', name: role },
        },
      ];
    }),
  ];

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

  kustomize: (app, accounts) => ({ resources: needsManifests(app, accounts) ? [RBAC] : [] }),

  plan({ app }, accounts) {
    const pushed = forCi(app, accounts).map((account) => ({
      name: serviceAccountSecretPath(account.name).replace(/^generated\//, ''),
      secretName: ciTokenSecret(account),
      keys: [
        { secretKey: 'token', property: 'TOKEN' },
        { secretKey: 'ca.crt', property: 'CA_CRT' },
      ],
    }));
    return pushed.length ? { secrets: { pushed } } : {};
  },

  generate(app, target) {
    const rbac = renderRbac(app);
    return rbac ? [{ path: rbacPath(app, target), content: rbac }] : [];
  },
};
