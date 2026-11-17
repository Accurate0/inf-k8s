import type {
  App,
  DatabaseSpec,
  GeneratedKey,
  Metrics,
  OidcClient,
  Resource,
  Route,
  SecretTemplate,
  Source,
  Workload,
} from './types.ts';

export function appWorkloads(app: App): Record<string, Workload> {
  return Object.fromEntries((app.workloads ?? []).map(({ name, ...spec }) => [name, spec]));
}

export const appNamespace = (app: App) => app.namespace?.name ?? app.name;

export const workloadImages = (app: App) =>
  Object.values(appWorkloads(app)).flatMap((w) => [
    w.image,
    ...(w.initContainers ?? []).map((c) => c.image),
    ...(w.sidecars ?? []).map((c) => c.image),
  ]);

export const sourcesOf = <T extends Source['type']>(app: App, type: T) =>
  (app.sources ?? []).filter(
    (source): source is Extract<Source, { type: T }> => source.type === type,
  );

export const declared = <T extends Resource['type']>(app: App, type: T) =>
  (app.resources ?? []).filter(
    (resource): resource is Extract<Resource, { type: T }> => resource.type === type,
  );

const specs = <S extends { name: string; type: string }>(resources: S[]) =>
  resources.map(({ name, type: _type, ...spec }) => [name, spec] as const);

export function appRoutes(app: App): Record<string, Route> {
  return Object.fromEntries(
    specs(declared(app, 'route').filter((route) => route.protocol !== 'tcp')),
  ) as Record<string, Route>;
}

export function appTcpRoutes(app: App): Record<string, Route> {
  return Object.fromEntries(
    specs(declared(app, 'route').filter((route) => route.protocol === 'tcp')),
  ) as Record<string, Route>;
}

export function appOidc(app: App): Record<string, OidcClient> {
  return Object.fromEntries(specs(declared(app, 'oidc'))) as Record<string, OidcClient>;
}

export function appMetrics(app: App): Metrics[] {
  return specs(declared(app, 'metrics')).map(([, spec]) => spec as Metrics);
}

export function appSecrets(app: App): {
  name: string;
  secretName: string;
  secretTemplate?: SecretTemplate;
  generate?: Record<string, GeneratedKey>;
}[] {
  return (app.resources ?? []).flatMap((resource) =>
    resource.type === 'secret'
      ? [
          {
            name: resource.name,
            secretName: resource.secretName,
            secretTemplate: resource.secretTemplate,
            generate: resource.generate,
          },
        ]
      : [],
  );
}

export function appDatabases(app: App): Record<string, DatabaseSpec> {
  const declared = (app.resources ?? []).flatMap((resource) => {
    if (resource.type !== 'database') {
      return [];
    }
    const { name, type: _type, ...spec } = resource;
    return [[name, spec] as const];
  });
  return Object.fromEntries(declared);
}

export const databaseRole = (database: string, spec?: Pick<DatabaseSpec, 'role'>) =>
  spec?.role ?? database.replaceAll('-', '_');

export const appRoles = (apps: App[]) =>
  apps.flatMap((app) =>
    Object.entries(appDatabases(app)).map(([database, spec]) => databaseRole(database, spec)),
  );
