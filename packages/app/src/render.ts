import { stringify } from 'yaml';

const CONFIG_REPO = 'https://github.com/Accurate0/inf-k8s.git';
const CHART_REGISTRY = 'oci://ghcr.io/accurate0/charts';
const SCHEMA = 'https://k8s-schemas.anurag.sh/argoproj.io/application_v1alpha1.json';

export const HEADER = '# generated from app.yaml by `ortzi app render`; edit app.yaml instead';

const STAMP = /^# generated at .*\n/m;

export const stamped = (rendered: string, now: Date) =>
  rendered.replace(`${HEADER}\n`, `${HEADER}\n# generated at ${now.toISOString()}\n`);

export const isCurrent = (existing: string, rendered: string) =>
  STAMP.test(existing) && existing.replace(STAMP, '') === rendered;

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
  secrets?: string[];
}

export type Secret = string | { name: string; secretName: string };

export interface Route {
  hosts: string[];
  name?: string;
  gateway?: string;
  unauthenticated?: boolean;
  workload?: string;
  backend?: { name: string; port: number };
  paths?: string[];
  annotations?: Record<string, string>;
  ratelimit?: Record<string, unknown>;
  timeouts?: Record<string, unknown>;
}

export interface App {
  name: string;
  repository?: string;
  charts?: Record<string, string>;
  workloads?: Record<string, Workload>;
  route?: Route;
  oidc?: { host?: string; public?: boolean; displayName?: string } & Record<string, unknown>;
  database?: { databaseName?: string; secretName?: string };
  secrets?: Secret[];
  sources?: Record<string, unknown>[];
  network: { ingress: string[] };
  metrics?: { path?: string; interval?: string; workload?: string };
}

export type Versions = Record<string, string>;

export interface Target {
  location: string;
  hasManifests: boolean;
}

const secretFor = (secret: Secret) =>
  typeof secret === 'string'
    ? { name: secret, secretName: `${secret}-managed-secrets` }
    : secret;

function served(app: App, wanted: string | undefined, block: string): [string, Workload] {
  const workloads = Object.entries(app.workloads ?? {}).filter(([, w]) => w.port);
  const found = wanted ? workloads.find(([name]) => name === wanted) : workloads[0];
  if (!found) throw new Error(`${block} needs a workload with a port`);
  if (!wanted && workloads.length > 1) throw new Error(`${block}.workload is required with several ported workloads`);
  return found;
}

const ALLOWED = new Set(['name', 'repository', 'charts', 'workloads', 'route', 'oidc', 'database', 'secrets', 'sources', 'network', 'metrics', 'build', 'lint']);

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
  if (app.route && !app.route.hosts?.length) fail('route.hosts is required');
  const secretNames = (app.secrets ?? []).map(secretFor).map((s) => s.name);
  for (const [name, workload] of Object.entries(app.workloads ?? {})) {
    for (const secret of workload.secrets ?? []) {
      if (!secretNames.includes(secret)) fail(`workloads.${name}.secrets: no secret named ${secret}`);
    }
  }
  if (app.route && !app.oidc && !app.route.unauthenticated) {
    fail('route needs oidc so that every route authenticates, or route.unauthenticated: true');
  }
  if (app.oidc && !app.oidc.host && !app.route) fail('oidc needs route or oidc.host');
  try {
    if (app.route && !app.route.backend) served(app, app.route.workload, 'route');
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
    const secrets = (app.secrets ?? []).map(secretFor);
    const database = app.database ? [{ secretRef: { name: secretName } }] : [];
    sources.push({ repoURL: CONFIG_REPO, ref: 'values' });
    sources.push(
      chart(app, versions, 'k8s-workload', {
        valueFiles: [`$values/${imageValuesPath(app, target)}`],
        valuesObject: {
          namespace: name,
          ...(app.route
            ? {
                namespaceLabels: Object.fromEntries(
                  app.route.hosts.map((host) => [
                    `gateway.inf-k8s.net/${host.replaceAll('.', '-')}`,
                    'true',
                  ]),
                ),
              }
            : {}),
          workloads: Object.entries(app.workloads).map(([workload, spec]) => {
            const { health, envFrom, secrets: only, ...rest } = spec;
            const from = [
              ...(envFrom ?? []),
              ...secrets
                .filter((s) => !only || only.includes(s.name))
                .map((s) => ({ secretRef: { name: s.secretName } })),
              ...database,
            ];
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
    const { route } = app;
    let backend = route.backend;
    if (!backend) {
      const [workload, spec] = served(app, route.workload, 'route');
      backend = { name: spec.service?.name ?? workload, port: spec.service?.port ?? (spec.port as number) };
    }
    sources.push(
      chart(app, versions, 'k8s-http-routes', {
        valuesObject: {
          routes: [
            {
              name: route.name ?? name,
              gateway: {
                name: route.gateway ?? 'public-gateway',
                namespace: 'envoy-gateway-system',
              },
              ...(route.annotations ? { annotations: route.annotations } : {}),
              hostnames: route.hosts,
              ...(route.ratelimit ? { ratelimit: route.ratelimit } : {}),
              rules: [
                {
                  backendRefs: [{ group: '', kind: 'Service', ...backend, weight: 1 }],
                  matches: (route.paths ?? ['/']).map((value) => ({
                    path: { type: 'PathPrefix', value },
                  })),
                  ...(route.timeouts ? { timeouts: route.timeouts } : {}),
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
              displayName: name,
              host: app.route?.hosts[0],
              public: false,
              preferShortUsername: true,
              ...app.oidc,
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
          secrets: app.secrets.map(secretFor).map((s) => ({
            name: s.name,
            managedSecrets: [{ secretName: s.secretName }],
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
  sources.push(...(app.sources ?? []));

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
