import { describeEnv, describeEnvFrom, type EnvSource, type Refs } from './workload.ts';
import type { App, Workload, World } from './types.ts';
import type { Resource } from './providers/provider.ts';
import { declared, secretRef } from './app.ts';
import { ownServiceUrl } from './services.ts';
import { appWorkloads } from './providers/workload.ts';
import { subgraphAuthValue } from './providers/subgraph.ts';
import { isValueResource, resourceProvider } from './providers/index.ts';

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

function resourceUrl(app: App, name: string, world: World): string {
  const resource = app.resources?.find(
    (r) => r.name === name && isValueResource(r.type),
  ) as Resource;
  const provider = resourceProvider(resource.type);
  return provider.value!(app, resource as never, world);
}

export const refsFor = (app: App, world: World): Refs => ({
  resource: (resource, env, part = 'url') => ({
    name: env,
    value: urlPart(resourceUrl(app, resource, world), part, `${app.name}: env.${env}`),
  }),
  secret: (kind, name) => secretRef(app, kind, name),
  config: (name) => {
    const found = declared(app, 'config').find((config) => config.name === name);
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
  subgraph: (name) => {
    const found = declared(app, 'subgraph').find((subgraph) => subgraph.name === name);
    if (!found) {
      throw new Error(`${app.name}: no subgraph resource named ${name}`);
    }
    return JSON.stringify(subgraphAuthValue(found, world.apps));
  },
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
