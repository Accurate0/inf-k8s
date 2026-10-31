import { stringify } from 'yaml';

const CONFIG_REPO = 'https://github.com/Accurate0/inf-k8s.git';
const CHART_REGISTRY = 'oci://ghcr.io/accurate0/charts';
const SCHEMA = 'https://k8s-schemas.anurag.sh/argoproj.io/application_v1alpha1.json';

export const HEADER = '# generated from app.yaml by `ortzi app render`; edit app.yaml instead';

export interface App {
  name: string;
  repository?: string;
  charts?: Record<string, string>;
  workload?: {
    image: string;
    port?: number;
    kind?: 'Deployment' | 'DaemonSet' | 'CronJob';
    replicas?: number;
    schedule?: string;
    service?: { name?: string; port?: number; portName?: string };
    health?: string;
    resources?: Record<string, unknown>;
    env?: Record<string, unknown>[];
    envFrom?: Record<string, unknown>[];
    readOnlyRootFilesystem?: boolean;
  };
  route?: { host: string; gateway?: string; unauthenticated?: boolean };
  oidc?: { public?: boolean; displayName?: string; allowLocalhostRedirect?: boolean };
  database?: { databaseName?: string; secretName?: string };
  secrets?: string[];
  network: { callers: string[] };
  metrics?: { path?: string; interval?: string };
}

export type Versions = Record<string, string>;

export interface Target {
  location: string;
  hasManifests: boolean;
}

const ALLOWED = new Set(['name', 'repository', 'charts', 'workload', 'route', 'oidc', 'database', 'secrets', 'network', 'metrics', 'build', 'lint']);

export function validate(raw: unknown, where: string): App {
  const app = raw as App;
  const fail = (message: string): never => {
    throw new Error(`${where}: ${message}`);
  };
  if (typeof app !== 'object' || app === null) fail('expected a mapping');
  for (const key of Object.keys(app)) {
    if (!ALLOWED.has(key)) fail(`unknown block "${key}"`);
  }
  if (!/^[a-z0-9]([a-z0-9-]*[a-z0-9])?$/.test(app.name ?? '')) fail('name must be kebab-case');
  if (!Array.isArray(app.network?.callers)) {
    fail('network.callers is required (use [] for no callers)');
  }
  if (app.workload && !app.workload.image) fail('workload.image is required');
  if (app.route && !app.route.host) fail('route.host is required');
  if (app.route && !app.workload?.port) fail('route needs workload.port');
  if (app.route && !app.oidc && !app.route.unauthenticated) {
    fail('route needs oidc so that every route authenticates, or route.unauthenticated: true');
  }
  if (app.oidc && !app.route) fail('oidc needs route');
  if (app.metrics && !app.workload?.port) fail('metrics needs workload.port');
  return app;
}

function chart(app: App, versions: Versions, name: string, helm: Record<string, unknown>) {
  const targetRevision = app.charts?.[name] ?? versions[name];
  if (!targetRevision) throw new Error(`${app.name}: no version for chart ${name}`);
  return { chart: name, repoURL: `${CHART_REGISTRY}/${name}`, targetRevision, helm };
}

export const imageValuesPath = (app: App, target: Target) =>
  `${target.location}/${app.name}/image.values.yaml`;

export function renderApplication(app: App, versions: Versions, target: Target): string {
  const { name } = app;
  const annotations = {
    'inf-k8s.net/app': name,
    'inf-k8s.net/repository': app.repository ?? 'https://github.com/Accurate0/inf-k8s',
  };
  const secretName = app.database?.secretName ?? `${name}-database-secret`;
  const dir = `${target.location}/${name}`;
  const sources: Record<string, unknown>[] = [];

  if (app.workload) {
    const { health, envFrom, ...workload } = app.workload;
    const refs = [
      ...(envFrom ?? []),
      ...(app.secrets ?? []).map((s) => ({ secretRef: { name: `${s}-managed-secrets` } })),
      ...(app.database ? [{ secretRef: { name: secretName } }] : []),
    ];
    sources.push({ repoURL: CONFIG_REPO, ref: 'values' });
    sources.push(
      chart(app, versions, 'k8s-workload', {
        valueFiles: [`$values/${imageValuesPath(app, target)}`],
        valuesObject: {
          name,
          ...workload,
          ...(health ? { probes: { path: health } } : {}),
          ...(refs.length ? { envFrom: refs } : {}),
          ...(app.route
            ? {
                namespaceLabels: {
                  [`gateway.inf-k8s.net/${app.route.host.replaceAll('.', '-')}`]: 'true',
                },
              }
            : {}),
        },
      }),
    );
  }

  const callers = [
    ...(app.route ? ['envoy-gateway-system'] : []),
    ...(app.metrics ? ['monitoring'] : []),
    ...app.network.callers,
  ];
  sources.push(
    chart(app, versions, 'k8s-network', {
      valuesObject: { namespace: name, callers: [...new Set(callers)] },
    }),
  );

  if (app.route) {
    const service = app.workload?.service;
    sources.push(
      chart(app, versions, 'k8s-http-routes', {
        valuesObject: {
          routes: [
            {
              name,
              gateway: {
                name: app.route.gateway ?? 'public-gateway',
                namespace: 'envoy-gateway-system',
              },
              hostnames: [app.route.host],
              rules: [
                {
                  backendRefs: [
                    {
                      name: service?.name ?? name,
                      port: service?.port ?? app.workload?.port,
                    },
                  ],
                  matches: [{ path: { type: 'PathPrefix', value: '/' } }],
                },
              ],
            },
          ],
        },
      }),
    );
  }

  if (app.oidc) {
    sources.push(
      chart(app, versions, 'k8s-oidc', {
        valuesObject: {
          clients: [
            {
              name,
              namespace: name,
              displayName: app.oidc.displayName ?? name,
              host: app.route?.host,
              public: app.oidc.public ?? false,
              ...(app.oidc.allowLocalhostRedirect ? { allowLocalhostRedirect: true } : {}),
              preferShortUsername: true,
              secretName: `kanidm-${name}-oidc`,
              secretNamespace: name,
              annotations,
            },
          ],
        },
      }),
    );
  }

  if (app.database) {
    sources.push(
      chart(app, versions, 'k8s-postgres', {
        valuesObject: {
          databases: [
            {
              name,
              namespace: name,
              databaseName: app.database.databaseName ?? name.replaceAll('-', '_'),
              secretName,
              annotations,
            },
          ],
        },
      }),
    );
  }

  if (app.secrets?.length) {
    sources.push(
      chart(app, versions, 'k8s-secrets', {
        valuesObject: {
          defaults: { namespace: name, provider: 'openbao', resyncInterval: 900, annotations },
          secrets: app.secrets.map((s) => ({
            name: s,
            managedSecrets: [{ secretName: `${s}-managed-secrets` }],
          })),
        },
      }),
    );
  }

  if (app.metrics) {
    sources.push(
      chart(app, versions, 'k8s-observability', {
        valuesObject: {
          name,
          ...(app.workload?.service?.portName ? { port: app.workload.service.portName } : {}),
          ...app.metrics,
        },
      }),
    );
  }

  if (target.hasManifests) {
    sources.push({ repoURL: CONFIG_REPO, name: 'application', path: `${dir}/manifests` });
  }

  const application = {
    apiVersion: 'argoproj.io/v1alpha1',
    kind: 'Application',
    metadata: { name, namespace: 'argocd' },
    spec: {
      syncPolicy: {
        syncOptions: ['CreateNamespace=true', 'ServerSideApply=true'],
        automated: { prune: true, selfHeal: true },
      },
      project: target.location,
      sources,
      destination: { server: 'https://kubernetes.default.svc', namespace: name },
    },
    operation: {
      initiatedBy: { username: 'github-actions' },
      sync: { prune: true, syncStrategy: { hook: {} } },
    },
  };

  return `${HEADER}\n# yaml-language-server: $schema=${SCHEMA}\n${stringify(application, { lineWidth: 0, aliasDuplicateObjects: false })}`;
}
