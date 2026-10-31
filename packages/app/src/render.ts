import { stringify } from 'yaml';

const CONFIG_REPO = 'https://github.com/Accurate0/inf-k8s.git';
const CHART_REGISTRY = 'oci://ghcr.io/accurate0/charts';
const SCHEMA = 'https://k8s-schemas.anurag.sh/argoproj.io/application_v1alpha1.json';

export const HEADER = '# generated from app.yaml by `ortzi app render`; edit app.yaml instead';

export interface Workload {
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
}

export interface App {
  name: string;
  repository?: string;
  charts?: Record<string, string>;
  workloads?: Record<string, Workload>;
  route?: { host: string; gateway?: string; unauthenticated?: boolean; workload?: string };
  oidc?: { public?: boolean; displayName?: string; allowLocalhostRedirect?: boolean };
  database?: { databaseName?: string; secretName?: string };
  secrets?: string[];
  network: { ingress: string[] };
  metrics?: { path?: string; interval?: string; workload?: string };
}

export type Versions = Record<string, string>;

export interface Target {
  location: string;
  hasManifests: boolean;
}

function served(app: App, wanted: string | undefined, block: string): [string, Workload] {
  const workloads = Object.entries(app.workloads ?? {}).filter(([, w]) => w.port);
  const found = wanted ? workloads.find(([name]) => name === wanted) : workloads[0];
  if (!found) throw new Error(`${block} needs a workload with a port`);
  if (!wanted && workloads.length > 1) throw new Error(`${block}.workload is required with several ported workloads`);
  return found;
}

const ALLOWED = new Set(['name', 'repository', 'charts', 'workloads', 'route', 'oidc', 'database', 'secrets', 'network', 'metrics', 'build', 'lint']);

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
  if (!Array.isArray(app.network?.ingress)) {
    fail('network.ingress is required (use [] for none)');
  }
  for (const [name, workload] of Object.entries(app.workloads ?? {})) {
    if (!workload.image) fail(`workloads.${name}.image is required`);
  }
  if (app.route && !app.route.host) fail('route.host is required');
  if (app.route && !app.oidc && !app.route.unauthenticated) {
    fail('route needs oidc so that every route authenticates, or route.unauthenticated: true');
  }
  if (app.oidc && !app.route) fail('oidc needs route');
  try {
    if (app.route) served(app, app.route.workload, 'route');
    if (app.metrics) served(app, app.metrics.workload, 'metrics');
  } catch (e) {
    fail((e as Error).message);
  }
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

  if (app.workloads && Object.keys(app.workloads).length) {
    const refs = [
      ...(app.secrets ?? []).map((s) => ({ secretRef: { name: `${s}-managed-secrets` } })),
      ...(app.database ? [{ secretRef: { name: secretName } }] : []),
    ];
    sources.push({ repoURL: CONFIG_REPO, ref: 'values' });
    sources.push(
      chart(app, versions, 'k8s-workload', {
        valueFiles: [`$values/${imageValuesPath(app, target)}`],
        valuesObject: {
          namespace: name,
          ...(app.route
            ? {
                namespaceLabels: {
                  [`gateway.inf-k8s.net/${app.route.host.replaceAll('.', '-')}`]: 'true',
                },
              }
            : {}),
          workloads: Object.entries(app.workloads).map(([workload, spec]) => {
            const { health, envFrom, ...rest } = spec;
            const from = [...(envFrom ?? []), ...refs];
            return {
              name: workload,
              ...rest,
              ...(health ? { probes: { path: health } } : {}),
              ...(from.length ? { envFrom: from } : {}),
            };
          }),
        },
      }),
    );
  }

  const ingress = [
    ...(app.route ? ['envoy-gateway-system'] : []),
    ...(app.metrics ? ['monitoring'] : []),
    ...app.network.ingress,
  ];
  sources.push(
    chart(app, versions, 'k8s-network', {
      valuesObject: { namespace: name, ingress: [...new Set(ingress)] },
    }),
  );

  if (app.route) {
    const [workload, spec] = served(app, app.route.workload, 'route');
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
                      name: spec.service?.name ?? workload,
                      port: spec.service?.port ?? spec.port,
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
    const { workload: _workload, ...metrics } = app.metrics;
    const [workload, spec] = served(app, app.metrics.workload, 'metrics');
    sources.push(
      chart(app, versions, 'k8s-observability', {
        valuesObject: {
          name: workload,
          namespace: name,
          ...(spec.service?.portName ? { port: spec.service.portName } : {}),
          ...metrics,
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
