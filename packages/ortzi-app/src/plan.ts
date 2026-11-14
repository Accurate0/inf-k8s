import {
  type App,
  appDatabases,
  appMetrics,
  appNamespace,
  appOidc,
  appRoutes,
  appSecrets,
  appTcpRoutes,
  appWorkloads,
  databaseHost,
  databaseRole,
  declared,
  imageValuesPath,
  refsFor,
  type RouteDns,
  type RouteLoadBalancer,
  type RouteRatelimit,
  storageClass,
  supergraphApp,
  type RouteAuth,
  type SecretTemplate,
  type Target,
  type Versions,
  type World,
} from './render.ts';
import { expandWorkload, SECRET_KINDS, type SecretKind } from './workload.ts';

export const CHART_REGISTRY = 'oci://registry.anurag.sh/charts';

export interface PlatformChart {
  type: 'chart';
  chart: string;
  registry: string;
  version: string;
  valueFiles?: string[];
  values: Record<string, unknown>;
  retain?: boolean;
}

export interface ExternalChart {
  type: 'helm';
  chart: string;
  repo: string;
  version: string;
  releaseName?: string;
  valueFiles: string[];
}

export interface GitPath {
  type: 'git';
  repo: string;
  path: string;
  name?: string;
  revision?: string;
}

export interface LocalManifests {
  type: 'manifests';
  path: string;
  artifact?: { repo: string; tag: string; path: string };
}

export type PlanSource = PlatformChart | ExternalChart | GitPath | LocalManifests;

export interface Plan {
  name: string;
  namespace: string;
  location: string;
  directory: string;
  namespaceLabels?: Record<string, string>;
  ownsNamespace: boolean;
  sources: PlanSource[];
}

function dnsAnnotations(dns: RouteDns): Record<string, string> {
  if (dns.provider === 'unifi') return { 'external-dns.kubernetes.io/managed-unifi': 'true' };
  return {
    'external-dns.kubernetes.io/managed': 'true',
    'external-dns.kubernetes.io/cloudflare-proxied': String(dns.proxied),
  };
}

function ratelimitRules({ requests, per, header }: RouteRatelimit) {
  return {
    rules: [
      {
        clientSelectors: [{ headers: [{ name: header, invert: false, type: 'Distinct' }] }],
        limit: { requests, unit: per },
      },
    ],
  };
}

const LOAD_BALANCERS = {
  roundRobin: 'RoundRobin',
  leastRequest: 'LeastRequest',
  random: 'Random',
  consistentHash: 'ConsistentHash',
} as const;

function loadBalancer({ type, headers }: RouteLoadBalancer) {
  return {
    type: LOAD_BALANCERS[type],
    ...(headers
      ? { consistentHash: { type: 'Headers', headers: headers.map((name) => ({ name })) } }
      : {}),
  };
}

export function planApp(app: App, versions: Versions, target: Target, world: World): Plan {
  const { name } = app;
  const namespace = appNamespace(app);
  const annotations = {
    'inf-k8s.net/app': name,
    'inf-k8s.net/repository': app.repository ?? 'https://github.com/Accurate0/inf-k8s',
  };

  const databaseSecret = (database: string) => appDatabases(app)[database].secretName;
  const directory = `${target.location}/${name}`;
  const routes = Object.entries(appRoutes(app));
  const tcpRoutes = Object.entries(appTcpRoutes(app));
  const sources: PlanSource[] = [];
  const serviceAccounts = [...declared(app, 'serviceAccount').map((account) => account.name)];

  const loginPolicy = (client: string) => {
    const route = routes.find(([, r]) => r.auth?.mode === 'oidc' && r.auth.client === client);
    if (!route) return {};
    const {
      mode: _mode,
      client: _client,
      ...policy
    } = route[1].auth as Extract<RouteAuth, { mode: 'oidc' }>;
    return { securityPolicy: { httpRoute: route[0], ...policy } };
  };

  const chart = (
    chartName: string,
    values: Record<string, unknown>,
    { valueFiles, retain }: { valueFiles?: string[]; retain?: boolean } = {},
  ): PlatformChart => {
    const version = app.charts?.[chartName] ?? versions[chartName];
    if (!version) throw new Error(`${app.name}: no version for chart ${chartName}`);
    return {
      type: 'chart',
      chart: chartName,
      registry: app.chartRegistry ?? CHART_REGISTRY,
      version,
      ...(valueFiles ? { valueFiles } : {}),
      values,
      ...(retain ? { retain } : {}),
    };
  };

  const hasWorkloads = Object.keys(appWorkloads(app)).length > 0;
  if (hasWorkloads) {
    sources.push(
      chart(
        'k8s-workload',
        {
          namespace,
          ...(app.namespace?.labels ? { namespaceLabels: app.namespace.labels } : {}),
          ...(serviceAccounts.length ? { serviceAccounts } : {}),
          workloads: Object.entries(appWorkloads(app)).map(([workload, spec]) => {
            return { name: workload, ...expandWorkload(spec, refsFor(app, world)) };
          }),
        },
        { valueFiles: [imageValuesPath(app, target)] },
      ),
    );
  }

  const ingress = [
    ...(routes.length || tcpRoutes.length ? ['envoy-gateway-system'] : []),
    ...(appMetrics(app).length ? ['monitoring'] : []),
    ...app.network.ingress,
    ...world.apps
      .filter(
        (other) =>
          other.name !== name && other.resources?.some((r) => r.type === 'app' && r.app === name),
      )
      .map(appNamespace)
      .sort(),
    ...(declared(app, 'subgraph').length
      ? [supergraphApp(world.apps)]
          .filter((other): other is App => !!other && other.name !== name)
          .map(appNamespace)
      : []),
  ];
  if (app.network.policy !== false) {
    sources.push(chart('k8s-network', { namespace, ingress: [...new Set(ingress)] }));
  }

  if (routes.length || tcpRoutes.length) {
    sources.push(
      chart('k8s-http-routes', {
        ...(tcpRoutes.length
          ? {
              tcpRoutes: tcpRoutes.map(([routeName, route]) => ({
                name: routeName,
                gateway: {
                  name: route.gateway ?? 'public-gateway',
                  namespace: 'envoy-gateway-system',
                },
                listener: route.listener,
                backend: route.backend,
              })),
            }
          : {}),
        routes: routes.map(([routeName, route]) => {
          return {
            name: routeName,
            ...(route.auth?.mode === 'jwt' ? { jwtClients: route.auth.clients } : {}),
            gateway: {
              name: route.gateway ?? 'public-gateway',
              namespace: 'envoy-gateway-system',
            },
            ...(route.dns ? { annotations: dnsAnnotations(route.dns) } : {}),
            hostnames: route.hosts,
            ...(route.ratelimit ? { ratelimit: ratelimitRules(route.ratelimit) } : {}),
            ...(route.loadBalancer ? { loadBalancer: loadBalancer(route.loadBalancer) } : {}),
            rules: (
              route.rules ?? [
                {
                  paths: route.paths ?? ['/'],
                  backend: route.backend as { name: string; port: number },
                },
              ]
            ).map((rule) => ({
              backendRefs: [{ group: '', kind: 'Service', ...rule.backend, weight: 1 }],
              matches: rule.paths.map((match) => ({
                path: typeof match === 'string' ? { type: 'PathPrefix', value: match } : match,
              })),
              ...(route.timeouts ? { timeouts: route.timeouts } : {}),
            })),
          };
        }),
      }),
    );
  }

  if (Object.keys(appOidc(app)).length) {
    sources.push(
      chart(
        'k8s-oidc',
        {
          clients: Object.entries(appOidc(app)).map(([client, spec]) => ({
            name: client,
            namespace,
            displayName: client,
            preferShortUsername: true,
            ...spec,
            ...loginPolicy(client),
            secretName: spec.secretName,
            secretNamespace: namespace,
            annotations,
          })),
        },
        { retain: true },
      ),
    );
  }

  if (Object.keys(appDatabases(app)).length) {
    sources.push(
      chart('k8s-postgres', {
        databases: Object.entries(appDatabases(app)).map(([database, spec]) => ({
          name: database,
          namespace,
          databaseName: databaseRole(database, spec),
          secretName: databaseSecret(database),
          host: databaseHost(app, spec, world.platform),
          annotations,
        })),
      }),
    );
  }

  const bucketSecrets = (app.resources ?? []).flatMap((resource) =>
    resource.type === 'bucket'
      ? [
          {
            name: `resource-${resource.name}`,
            secretName: resource.secretName,
            secretTemplate: resource.secretTemplate,
          },
        ]
      : [],
  );

  const managedSecret = ({
    secretName,
    secretTemplate,
  }: {
    secretName: string;
    secretTemplate?: SecretTemplate;
  }) => ({
    secretName,
    ...(secretTemplate?.type ? { type: secretTemplate.type } : {}),
    ...(secretTemplate
      ? {
          template: {
            ...(secretTemplate.includeAllSecrets === undefined
              ? {}
              : { includeAllSecrets: secretTemplate.includeAllSecrets }),
            data: secretTemplate.data,
          },
        }
      : {}),
  });

  const synced = [...appSecrets(app).filter((s) => !s.generate), ...bucketSecrets];
  const generated = appSecrets(app).flatMap(
    ({ name: secret, secretName, secretTemplate, generate }) =>
      generate
        ? [
            {
              name: secret,
              secretName,
              ...(secretTemplate?.type ? { type: secretTemplate.type } : {}),
              ...(secretTemplate ? { template: { data: secretTemplate.data } } : {}),
              keys: Object.entries(generate).map(([key, spec]) => ({ name: key, ...spec })),
            },
          ]
        : [],
  );
  const refs = refsFor(app, world);
  const templates = declared(app, 'config').flatMap(({ configMap, files, secretName, values }) =>
    values
      ? [
          {
            secretName,
            configMap,
            files,
            values: Object.entries(values).map(([variable, ref]) => {
              const kind = SECRET_KINDS.find((k) => ref[k]) as SecretKind;
              return { name: variable, secret: refs.secret(kind, ref[kind]), key: ref.key };
            }),
          },
        ]
      : [],
  );

  if (synced.length || generated.length || templates.length) {
    sources.push(
      chart('k8s-secrets', {
        defaults: { namespace, provider: 'openbao', resyncInterval: 900, annotations },
        secrets: synced.map((s) => ({
          name: s.name,
          managedSecrets: [managedSecret(s)],
        })),
        ...(generated.length ? { generated } : {}),
        ...(templates.length ? { templates } : {}),
      }),
    );
  }

  const volumes = declared(app, 'volume');
  if (volumes.length) {
    sources.push(
      chart(
        'k8s-storage',
        {
          volumes: volumes.map((volume) => {
            const { storageClass: className, labels } = storageClass(app, volume, world.platform);
            return {
              name: volume.claimName,
              namespace,
              storageClass: className,
              size: volume.size,
              access: volume.access,
              ...(labels ? { labels } : {}),
              annotations,
            };
          }),
        },
        { retain: true },
      ),
    );
  }

  for (const scrape of appMetrics(app)) {
    const { workload, ...metrics } = scrape;
    sources.push(chart('k8s-observability', { name: workload, namespace, ...metrics }));
  }

  for (const source of app.sources ?? []) {
    if (source.type === 'helm') {
      const { chart: helmChart, repo, version, values, releaseName } = source;
      sources.push({
        type: 'helm',
        chart: helmChart,
        repo,
        version,
        ...(releaseName ? { releaseName } : {}),
        valueFiles: [`${directory}/${values}`],
      });
    } else if (source.type === 'git') {
      sources.push(source);
    } else {
      sources.push({
        type: 'manifests',
        path: source.path === '.' ? directory : `${directory}/${source.path}`,
        ...(source.artifact && source.tag
          ? { artifact: { repo: source.artifact, tag: source.tag, path: source.path } }
          : {}),
      });
    }
  }

  return {
    name,
    namespace,
    location: target.location,
    directory,
    ...(app.namespace?.labels ? { namespaceLabels: app.namespace.labels } : {}),
    ownsNamespace: hasWorkloads,
    sources,
  };
}

function externalChartLocation({ chart, repo, version }: ExternalChart): string[] {
  if (!repo.startsWith('oci://')) return [chart, '--repo', repo, '--version', version];
  return [repo.endsWith(`/${chart}`) ? repo : `${repo}/${chart}`, '--version', version];
}

export function helmTemplates(plan: Plan): { args: string[]; values: string }[] {
  return plan.sources.flatMap((source) => {
    if (source.type !== 'chart' && source.type !== 'helm') return [];
    const release = source.type === 'helm' ? (source.releaseName ?? plan.name) : plan.name;
    const location =
      source.type === 'chart' ? [`charts/${source.chart}`] : externalChartLocation(source);
    return [
      {
        args: [
          'template',
          release,
          ...location,
          '--namespace',
          plan.namespace,
          ...(source.valueFiles ?? []).flatMap((file) => ['-f', file]),
          '-f',
          '-',
        ],
        values: JSON.stringify(source.type === 'chart' ? source.values : {}),
      },
    ];
  });
}
