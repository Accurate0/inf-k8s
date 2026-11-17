import { stringify } from 'yaml';
import type { App, Target } from './types.ts';
import { HEADER } from './generated.ts';
import { appNamespace, declared, sourcesOf } from './app.ts';

export const imageValuesPath = (app: App, target: Target) =>
  `${target.location}/${app.name}/image.values.yaml`;

export const rbacPath = (app: App, target: Target) =>
  `${target.location}/${app.name}/manifests/rbac.generated.yaml`;

const clusterRoles = (app: App) =>
  declared(app, 'serviceAccount').filter((account) => account.clusterRules);

export function renderRbac(app: App): string | undefined {
  const documents = clusterRoles(app).flatMap((account) => {
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

export const needsKustomization = (app: App) =>
  (app.resources ?? []).some(
    (r) =>
      r.type === 'config' ||
      r.type === 'dashboard' ||
      (r.type === 'serviceAccount' && r.clusterRules),
  );

export const kustomizationPath = (app: App, target: Target) =>
  `${target.location}/${app.name}/kustomization.yaml`;

export function renderKustomization(app: App, target: Target): string | undefined {
  if (!needsKustomization(app)) {
    return undefined;
  }

  const generators = [
    ...declared(app, 'config').map(({ configMap, files }) => ({
      name: configMap,
      namespace: appNamespace(app),
      files,
    })),
    ...declared(app, 'dashboard').map(({ configMap, file }) => ({
      name: configMap,
      namespace: appNamespace(app),
      files: [file],
      options: { disableNameSuffixHash: true, labels: { 'inf-k8s.net/grafana-dashboard': 'true' } },
    })),
  ];

  const resources = [
    ...(target.hasManifests ? ['manifests'] : []),
    ...(clusterRoles(app).length ? ['manifests/rbac.generated.yaml'] : []),
  ];

  const kustomization = {
    apiVersion: 'kustomize.config.k8s.io/v1beta1',
    kind: 'Kustomization',
    ...(resources.length ? { resources } : {}),
    ...(generators.length
      ? { configMapGenerator: generators, generatorOptions: { disableNameSuffixHash: true } }
      : {}),
  };

  return `${HEADER}\n${stringify(kustomization, { lineWidth: 0, aliasDuplicateObjects: false })}`;
}

export const appFiles = (app: App, target: Target) =>
  [
    ...declared(app, 'config').flatMap((config) => config.files),
    ...declared(app, 'dashboard').map((dashboard) => dashboard.file),
    ...sourcesOf(app, 'helm').map((release) => release.values),
    ...sourcesOf(app, 'manifests')
      .filter(() => !needsKustomization(app))
      .map(({ path }) => (path === '.' ? 'kustomization.yaml' : `${path}/kustomization.yaml`)),
  ].map((file) => `${target.location}/${app.name}/${file}`);
