import type { ResourceProvider, ResourceType, SourceProvider, SourceType } from './provider.ts';
import { config } from './config.ts';
import { secret } from './secret.ts';
import { database } from './database.ts';
import { cache } from './cache.ts';
import { bucket } from './bucket.ts';
import { volume } from './volume.ts';
import { serviceAccount } from './service-account.ts';
import { app } from './app.ts';
import { platform } from './platform.ts';
import { supergraph } from './supergraph.ts';
import { subgraph } from './subgraph.ts';
import { route } from './route.ts';
import { oidc } from './oidc.ts';
import { metrics } from './metrics.ts';
import { dashboard } from './dashboard.ts';
import { helm } from './sources/helm.ts';
import { git } from './sources/git.ts';
import { artifact } from './sources/artifact.ts';
import { manifests } from './sources/manifests.ts';

// Order is the order `ortzi app fmt` sorts resources in.
export const RESOURCES = [
  config,
  secret,
  database,
  cache,
  bucket,
  volume,
  serviceAccount,
  app,
  platform,
  supergraph,
  subgraph,
  route,
  oidc,
  metrics,
  dashboard,
] as ResourceProvider[];

export const SOURCES = [helm, git, artifact, manifests] as SourceProvider[];

export const resourceProvider = <T extends ResourceType>(type: T) =>
  RESOURCES.find((provider) => provider.type === type) as ResourceProvider<T>;

export const sourceProvider = <T extends SourceType>(type: T) =>
  SOURCES.find((provider) => provider.type === type) as SourceProvider<T>;

export const isValueResource = (type: ResourceType) => resourceProvider(type)?.ref === 'value';
