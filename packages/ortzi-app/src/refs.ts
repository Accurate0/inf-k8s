import {
  declaredPorts,
  describeEnv,
  describeEnvFrom,
  type EnvSource,
  type Refs,
  type SecretKind,
} from './workload.ts';
import {
  type App,
  type DatabaseSpec,
  type Export,
  isValueResource,
  type PlatformExport,
  type Resource,
  type Volume,
  type Workload,
  type World,
} from './types.ts';
import { appNamespace, appWorkloads, declared } from './app.ts';

function servicePorts(workload: string, spec: Workload) {
  const ports = declaredPorts(spec)
    .filter((p) => p.service)
    .map((p) => ({ name: p.name as string, port: p.service as number }));
  return { workload, service: spec.service?.name ?? workload, ports };
}

export function exportPort(provider: App, exported: Export): number {
  const spec = appWorkloads(provider)[exported.workload];
  const ports = spec ? servicePorts(exported.workload, spec).ports : [];
  const found = ports.find((p) => p.name === exported.port || p.port === exported.port)?.port;
  if (!found) {
    throw new Error(
      `${provider.name}: exports.${exported.name}: no matching port on workload ${exported.workload}`,
    );
  }

  return found;
}

export function appExportUrl(
  app: App,
  resource: Extract<Resource, { type: 'app' }>,
  apps: App[],
): string {
  const provider = apps.find((a) => a.name === resource.app);
  if (!provider) {
    throw new Error(`${app.name}: resources.${resource.name}: no app named ${resource.app}`);
  }

  const exported = provider.exports?.find((e) => e.name === resource.export);
  if (!exported) {
    throw new Error(
      `${app.name}: resources.${resource.name}: ${resource.app} has no export named ${resource.export}`,
    );
  }

  const service = appWorkloads(provider)[exported.workload]?.service?.name ?? exported.workload;
  return serviceUrl(service, provider.name, exportPort(provider, exported), resource.path);
}

export const serviceUrl = (service: string, namespace: string, port: number, path = '') =>
  `http://${service}.${namespace}.svc.cluster.local${port === 80 ? '' : `:${port}`}${path}`;

function ownServiceUrl(app: App, service: string, port: string | number, path?: string): string {
  const found = appServices(app).find((s) => s.service === service);
  if (!found) {
    throw new Error(`${app.name}: env: no service named ${service}`);
  }

  const exposed = found.ports.find((p) => p.name === port || p.port === port);
  if (!exposed) {
    throw new Error(`${app.name}: env: service ${service} has no port ${port}`);
  }

  return serviceUrl(service, appNamespace(app), exposed.port, path);
}

export type UrlPart = 'url' | 'host' | 'port';

const DEFAULT_PORTS: Record<string, string> = {
  'http:': '80',
  'https:': '443',
  'redis:': '6379',
  'rediss:': '6379',
};

function urlPart(url: string, part: UrlPart, where: string): string {
  if (part === 'url') {
    return url;
  }
  const parsed = new URL(url);
  if (part === 'host') {
    return parsed.hostname;
  }
  const port = parsed.port || DEFAULT_PORTS[parsed.protocol];
  if (!port) {
    throw new Error(`${where}: ${url} has no port`);
  }
  return port;
}

function resourceEnv(
  app: App,
  name: string,
  env: string,
  world: World,
  part: UrlPart = 'url',
): { name: string; value: string } {
  return {
    name: env,
    value: urlPart(resourceUrl(app, name, world), part, `${app.name}: env.${env}`),
  };
}

function resourceUrl(app: App, name: string, { platform, apps }: World): string {
  const resource = app.resources?.find(
    (r) => r.name === name && isValueResource(r.type),
  ) as Resource;
  if (resource.type === 'app') {
    return appExportUrl(app, resource, apps);
  }
  if (resource.type === 'bucket') {
    throw new Error(
      `${app.name}: resources.${name}: read a bucket with {bucket, key}, not {resource}`,
    );
  }
  if (resource.type === 'database') {
    throw new Error(`${app.name}: resources.${name}: list a database's secret in envFrom, not env`);
  }

  const wanted =
    resource.type === 'cache'
      ? { type: 'cache', name: resource.instance ?? 'cache' }
      : { type: 'platform', name: (resource as { export: string }).export };
  const provided = platform.filter((e) => e.type === wanted.type && e.name === wanted.name);
  if (provided.length !== 1) {
    throw new Error(
      `${app.name}: resources.${name}: expected one platform ${wanted.type} export named ${wanted.name}, found ${provided.length}`,
    );
  }

  return provided[0].url as string;
}

export const appServices = (app: App) =>
  Object.entries(appWorkloads(app)).map(([workload, spec]) => servicePorts(workload, spec));

export function secretRef(app: App, kind: SecretKind, name: string): string {
  const found = (app.resources ?? []).find((r) => r.type === kind && r.name === name) as
    { secretName?: string } | undefined;
  if (!found) {
    throw new Error(`${app.name}: no ${kind} resource named ${name}`);
  }
  return found.secretName as string;
}

export function storageClass(app: App, volume: Volume, platform: PlatformExport[]) {
  const found = platform.find((e) => e.type === 'storage' && e.name === volume.class);
  if (!found?.storageClass) {
    throw new Error(
      `${app.name}: resources.${volume.name}: no storage export named ${volume.class}`,
    );
  }

  if (volume.backup && !found.backupLabels) {
    throw new Error(
      `${app.name}: resources.${volume.name}: storage ${volume.class} has no backups`,
    );
  }

  return {
    storageClass: found.storageClass,
    labels: volume.backup ? found.backupLabels : undefined,
  };
}

export function databaseHost(app: App, spec: DatabaseSpec, platform: PlatformExport[]) {
  const name = spec.enableConnectionPooling === false ? 'direct' : 'pooled';
  const found = platform.find((e) => e.type === 'database' && e.name === name);
  if (!found?.host) {
    throw new Error(`${app.name}: no database export named ${name}`);
  }
  return found.host;
}

export const refsFor = (app: App, world: World): Refs => ({
  resource: (resource, env, part) => resourceEnv(app, resource, env, world, part),
  secret: (kind, name) => secretRef(app, kind, name),
  config: (name) => {
    const found = app.resources?.find((r) => r.type === 'config' && r.name === name) as
      Extract<Resource, { type: 'config' }> | undefined;
    if (!found) {
      throw new Error(`${app.name}: no config resource named ${name}`);
    }
    return found;
  },
  volume: (name) => {
    const found = declared(app, 'volume').find((volume) => volume.name === name);
    if (!found) {
      throw new Error(`${app.name}: no volume resource named ${name}`);
    }
    return found.claimName;
  },
  service: (service, port, path) => ownServiceUrl(app, service, port, path),
});

export function containerEnv(app: App, world: World): { container: string; rows: EnvSource[] }[] {
  const refs = refsFor(app, world);
  return Object.entries(appWorkloads(app)).flatMap(([workload, spec]) =>
    [
      [workload, spec] as const,
      ...((spec.initContainers ?? []) as (Workload & { name: string })[]).map(
        (c) => [`${workload}/${c.name}`, c] as const,
      ),
      ...((spec.sidecars ?? []) as (Workload & { name: string })[]).map(
        (c) => [`${workload}/${c.name}`, c] as const,
      ),
    ].map(([container, c]) => ({
      container,
      rows: [
        ...(c.env && !Array.isArray(c.env) ? describeEnv(c.env, refs) : []),
        ...(c.envFrom ?? []).map((from) => describeEnvFrom(from, refs)),
      ],
    })),
  );
}
