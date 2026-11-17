import { type Schema, Validator } from 'jsonschema/lib/index.js';
import schema from '../app.schema.json' with { type: 'json' };
import {
  declaredPorts,
  expandWorkload,
  overrideConflicts,
  SECRET_KINDS,
  type SecretKind,
} from './workload.ts';
import { type App, isValueResource, RESOURCE_TYPES, type Workload, type World } from './types.ts';
import {
  appDatabases,
  appMetrics,
  appNamespace,
  appOidc,
  appRoutes,
  appTcpRoutes,
  appWorkloads,
  declared,
  sourcesOf,
  workloadImages,
} from './app.ts';
import {
  appExportUrl,
  appServices,
  databaseHost,
  exportPort,
  refsFor,
  secretRef,
  storageClass,
} from './refs.ts';
import { validateSubgraphs } from './subgraphs.ts';
import { needsKustomization } from './manifests.ts';
import { ciProvider } from './workflows.ts';

const BUCKET_KEYS = [
  'S3_BUCKET',
  'S3_ENDPOINT',
  'S3_REGION',
  'S3_ACCESS_KEY_ID',
  'S3_SECRET_ACCESS_KEY',
];

function containerRefs(spec: Workload): Record<string, unknown>[] {
  const containers = [
    spec,
    ...((spec.initContainers ?? []) as Workload[]),
    ...((spec.sidecars ?? []) as Workload[]),
  ];

  return containers
    .flatMap((c) => [
      ...(c.env && !Array.isArray(c.env) ? Object.values(c.env) : []),
      ...(c.envFrom ?? []),
    ])
    .filter((ref): ref is Record<string, unknown> => typeof ref === 'object' && ref !== null);
}

const mountedVolumes = (app: App) =>
  Object.values(appWorkloads(app)).flatMap((spec) =>
    [
      spec,
      ...((spec.initContainers ?? []) as Workload[]),
      ...((spec.sidecars ?? []) as Workload[]),
    ].flatMap((container) =>
      Object.values((container.mounts ?? {}) as Record<string, { volume?: string }>).flatMap(
        (mount) => mount.volume ?? [],
      ),
    ),
  );

function validateRefs(app: App, external: boolean) {
  const fail = (message: string): never => {
    throw new Error(`${app.name}: ${message}`);
  };

  const refs = [
    ...Object.values(appWorkloads(app)).flatMap(containerRefs),
    ...declared(app, 'config').flatMap((config) => Object.values(config.values ?? {})),
  ];
  for (const resource of app.resources ?? []) {
    if (!RESOURCE_TYPES[resource.type].ref) {
      continue;
    }
    if (resource.type === 'volume') {
      if (!external && !mountedVolumes(app).includes(resource.name)) {
        fail(`resources.${resource.name}: no workload mounts this volume`);
      }
      continue;
    }
    if (resource.type === 'bucket' && resource.secretTemplate) {
      continue;
    }
    if (external) {
      continue;
    }
    const kinds = ['secret', 'database', 'bucket'].includes(resource.type)
      ? ['resource', resource.type]
      : ['resource'];
    if (!refs.some((ref) => kinds.some((kind) => ref[kind] === resource.name))) {
      fail(`resources.${resource.name}: no workload uses this ${resource.type}`);
    }
  }

  for (const ref of refs) {
    if (ref.bucket && ref.key && !BUCKET_KEYS.includes(ref.key as string)) {
      fail(
        `bucket ${ref.bucket}: no key named ${ref.key}, expected one of ${BUCKET_KEYS.join(', ')}`,
      );
    }
  }

  const services = appServices(app);
  for (const [name, route] of [
    ...Object.entries(appRoutes(app)),
    ...Object.entries(appTcpRoutes(app)),
  ]) {
    for (const backend of [
      ...(route.backend ? [route.backend] : []),
      ...(route.rules ?? []).map((rule) => rule.backend),
    ]) {
      const service = services.find((s) => s.service === backend.name);
      if (!service && external) {
        continue;
      }
      if (!service) {
        fail(`routes.${name}.backend: no service named ${backend.name}`);
      }
      if (!service?.ports.some((p) => p.port === backend.port)) {
        fail(`routes.${name}.backend: service ${backend.name} has no port ${backend.port}`);
      }
    }
  }

  for (const [name, spec] of Object.entries(appWorkloads(app))) {
    if (
      spec.serviceAccount &&
      !app.resources?.some((r) => r.type === 'serviceAccount' && r.name === spec.serviceAccount)
    ) {
      fail(`workloads.${name}: no serviceAccount resource named ${spec.serviceAccount}`);
    }
  }

  for (const { workload, port } of appMetrics(app)) {
    const service = services.find((s) => s.workload === workload);
    if (!service?.ports.length) {
      fail(`metrics: workload ${workload} has no service ports`);
    }
    if (!service?.ports.some((p) => p.name === port)) {
      fail(`metrics: workload ${workload} has no service port named ${port}`);
    }
  }

  if (app.build && !workloadImages(app).includes(app.build.image)) {
    fail(`build.image ${app.build.image} is not used by any workload`);
  }
}

export interface Listener {
  gateway: string;
  name?: string;
  protocol?: string;
  hostname?: string;
  labels?: Record<string, string>;
}

const listenerMatches = (hostname: string | undefined, host: string) =>
  hostname === host || (hostname?.startsWith('*.') && host.endsWith(hostname.slice(1)));

export function validateGatewayAccess(apps: App[], listeners: Listener[]) {
  for (const app of apps) {
    const labels: Record<string, string> = {
      ...app.namespace?.labels,
      'kubernetes.io/metadata.name': appNamespace(app),
    };
    for (const [name, route] of Object.entries(appTcpRoutes(app))) {
      const gateway = route.gateway ?? 'public-gateway';
      const listener = listeners.find((l) => l.gateway === gateway && l.name === route.listener);
      if (!listener?.labels || listener.protocol !== 'TCP') {
        throw new Error(
          `${app.name}: routes.${name}: no TCP listener ${route.listener} on ${gateway}`,
        );
      }
      for (const [key, value] of Object.entries(listener.labels)) {
        if (labels[key] !== value) {
          throw new Error(
            `${app.name}: routes.${name}: ${route.listener} needs namespace.labels ${key}: "${value}"`,
          );
        }
      }
    }

    for (const [name, route] of Object.entries(appRoutes(app))) {
      const gateway = route.gateway ?? 'public-gateway';
      for (const host of route.hosts) {
        const matching = listeners.filter(
          (l) => l.gateway === gateway && l.labels && listenerMatches(l.hostname, host),
        );
        if (!matching.length) {
          throw new Error(`${app.name}: routes.${name}: no ${gateway} listener accepts ${host}`);
        }
        const listener = matching.find((l) => l.hostname === host) ?? matching[0];
        for (const [key, value] of Object.entries(listener.labels ?? {})) {
          if (labels[key] !== value) {
            throw new Error(
              `${app.name}: routes.${name}: ${host} needs namespace.labels ${key}: "${value}"`,
            );
          }
        }
      }
    }
  }
}

const appSchema = new Validator();

const branchTypes = (branch: Schema) => (branch.properties?.type?.enum ?? []) as unknown[];

function schemaError(instance: unknown, against: Schema, path: string): string | undefined {
  const [invalid] = appSchema.validate(instance, against).errors;
  if (!invalid) {
    return undefined;
  }

  const at = invalid.property.replace(/^instance/, path);
  const where = at === 'instance' ? '' : `${at.replace(/^instance\.?/, '')} `;
  if (invalid.name === 'additionalProperties' && at === 'instance') {
    return `unknown block "${invalid.argument}"`;
  }

  if (invalid.name === 'oneOf') {
    const branches = (invalid.schema as Schema).oneOf as Schema[];
    const types = branches.flatMap(branchTypes);
    const type = (invalid.instance as { type?: unknown } | null)?.type;
    const branch = branches.find((candidate) => branchTypes(candidate).includes(type));
    if (branch) {
      return schemaError(invalid.instance, branch, at);
    }
    if (types.length) {
      return `${where}type must be one of ${types.join(', ')}`;
    }
  }

  return `${where}${invalid.message}`;
}

export function validate(raw: unknown, where: string): App {
  const app = raw as App;
  const fail = (message: string): never => {
    throw new Error(`${where}: ${message}`);
  };

  if (typeof app !== 'object' || app === null) {
    fail('expected a mapping');
  }
  const invalid = schemaError(app, schema as unknown as Schema, 'instance');
  if (invalid) {
    fail(invalid);
  }

  if (app.namespace?.name) {
    if (app.network.policy === undefined) {
      fail('namespace.name needs an explicit network.policy');
    }
    if (app.network.policy === false && app.network.ingress.length) {
      fail('network.ingress needs network.policy: true');
    }
    if (app.namespace.labels) {
      fail('namespace.labels cannot be set with namespace.name');
    }
  }

  const workloadNames = (app.workloads ?? []).map((workload) => workload.name);
  for (const name of workloadNames) {
    if (workloadNames.indexOf(name) !== workloadNames.lastIndexOf(name)) {
      fail(`workloads: duplicate name ${name}`);
    }
  }

  for (const [name, workload] of Object.entries(appWorkloads(app))) {
    if (workload.kind === 'Job' && !workload.phase) {
      fail(`workloads.${name}.phase is required for a Job`);
    }
    if (workload.phase && workload.kind !== 'Job') {
      fail(`workloads.${name}.phase is only for a Job`);
    }
    if (workload.kind === 'Job' && declaredPorts(workload).length) {
      fail(`workloads.${name}: a Job cannot expose ports`);
    }
    if (declaredPorts(workload).length && !workload.service?.name) {
      fail(`workloads.${name}.service.name is required with ports`);
    }
    for (const key of overrideConflicts(workload)) {
      fail(`workloads.${name}.overrides.${key} is already set on the workload`);
    }
    if (workload.autoscale) {
      const { min, max } = workload.autoscale;
      if ((workload.kind ?? 'Deployment') !== 'Deployment') {
        fail(`workloads.${name}.autoscale is only for a Deployment`);
      }
      if (workload.replicas !== undefined) {
        fail(`workloads.${name}: set either replicas or autoscale, not both`);
      }
      if (min > max) {
        fail(`workloads.${name}.autoscale.min is more than max`);
      }
      for (const container of [workload, ...((workload.sidecars ?? []) as Workload[])]) {
        if (!container.resources?.cpu) {
          fail(`workloads.${name}.autoscale needs resources.cpu on every container`);
        }
      }
    }
    for (const container of [
      workload,
      ...((workload.initContainers ?? []) as Workload[]),
      ...((workload.sidecars ?? []) as Workload[]),
    ]) {
      const targets = ['http', 'grpc', 'exec'].filter((target) => container.probes?.[target]);
      if (targets.length > 1) {
        fail(`workloads.${name}.probes: use one of ${targets.join(', ')}, not several`);
      }
      for (const [env, value] of Object.entries(
        !container.env || Array.isArray(container.env) ? {} : container.env,
      )) {
        if (typeof value === 'string' && value.includes('.svc.cluster.local')) {
          fail(
            `workloads.${name}.env.${env}: use {service} or {resource} instead of a cluster address`,
          );
        }
      }
    }
    for (const value of Object.values(
      !workload.env || Array.isArray(workload.env) ? {} : workload.env,
    )) {
      const used = (value as { resource?: string } | null)?.resource;
      if (used && !app.resources?.some((r) => r.name === used && isValueResource(r.type))) {
        fail(`workloads.${name}.env: no resource named ${used}; list a secret in envFrom`);
      }
    }
  }

  const routes = Object.entries(appRoutes(app));
  const oidc = appOidc(app);
  for (const [name, route] of routes) {
    if (route.listener) {
      fail(`routes.${name}.listener is only for protocol tcp`);
    }
    if (!route.hosts?.length) {
      fail(`routes.${name}.hosts is required`);
    }
    if (!route.backend === !route.rules) {
      fail(`routes.${name} needs either backend or rules`);
    }
    if (route.loadBalancer) {
      const { type, headers } = route.loadBalancer;
      if (type === 'consistentHash' && !headers?.length) {
        fail(`routes.${name}.loadBalancer.headers is required for consistentHash`);
      }
      if (type !== 'consistentHash' && headers) {
        fail(`routes.${name}.loadBalancer.headers is only for consistentHash`);
      }
    }
    const auth = route.auth;
    if (auth.mode === 'jwt') {
      if (!auth.clients?.length) {
        fail(`routes.${name}.auth.clients is required for jwt`);
      }
      for (const client of auth.clients ?? []) {
        if (!oidc[client]) {
          fail(`routes.${name}.auth: no oidc client named ${client}`);
        }
      }
    } else if (auth.mode === 'oidc') {
      if (!oidc[auth.client]) {
        fail(`routes.${name}.auth: no oidc client named ${auth.client}`);
      }
      if (
        routes.some(
          ([other, r]) =>
            other !== name && r.auth?.mode === 'oidc' && r.auth.client === auth.client,
        )
      ) {
        fail(`routes.${name}.auth: oidc client ${auth.client} is used by another oidc route`);
      }
    } else if (!auth.reason?.trim()) {
      fail(`routes.${name}.auth.reason is required for ${auth.mode}`);
    }
  }

  const tcpRoutes = Object.entries(appTcpRoutes(app));
  for (const [name, route] of tcpRoutes) {
    if (!route.listener) {
      fail(`routes.${name}.listener is required for protocol tcp`);
    }
    if (!route.backend) {
      fail(`routes.${name}.backend is required for protocol tcp`);
    }
    for (const field of HTTP_ONLY) {
      if (route[field] !== undefined) {
        fail(`routes.${name}.${field} is only for protocol http`);
      }
    }
    const auth = route.auth;
    if (auth.mode !== 'app' && auth.mode !== 'none') {
      fail(`routes.${name}.auth.mode must be app or none for protocol tcp`);
    } else if (!auth.reason?.trim()) {
      fail(`routes.${name}.auth.reason is required for ${auth.mode}`);
    }
  }

  for (const resource of app.resources ?? []) {
    if (resource.type === 'secret' && resource.generate) {
      const keys = Object.keys(resource.generate);
      const objects = keys.map((key) => key.toLowerCase().replaceAll('_', '-'));
      for (const [i, key] of keys.entries()) {
        if (!/^[A-Za-z0-9]([A-Za-z0-9_]*[A-Za-z0-9])?$/.test(key)) {
          fail(`resources.${resource.name}.generate.${key}: use letters, digits and underscores`);
        }
        if (objects.indexOf(objects[i]) !== i) {
          fail(
            `resources.${resource.name}.generate.${key} clashes with ${keys[objects.indexOf(objects[i])]}`,
          );
        }
      }
    }
    if (resource.type === 'config' && !resource.values !== !resource.secretName) {
      fail(`resources.${resource.name}: set secretName and values together`);
    }
  }

  const scraped = appMetrics(app).map((metrics) => metrics.workload);
  for (const workload of scraped) {
    if (scraped.indexOf(workload) !== scraped.lastIndexOf(workload)) {
      fail(`metrics: ${workload} is listed twice`);
    }
  }

  const resourceKeys = (app.resources ?? []).map(
    (r) => `${isValueResource(r.type) ? 'value' : r.type}/${r.name}`,
  );
  for (const key of resourceKeys) {
    if (resourceKeys.indexOf(key) !== resourceKeys.lastIndexOf(key)) {
      fail(`resources: duplicate name ${key.split('/')[1]}`);
    }
  }

  const releases = sourcesOf(app, 'helm').map((release) => release.releaseName ?? release.chart);
  for (const release of releases) {
    if (releases.indexOf(release) !== releases.lastIndexOf(release)) {
      fail(`sources: duplicate helm release ${release}, set releaseName`);
    }
  }

  const names = [
    ...(app.artifact || sourcesOf(app, 'manifests').length ? ['application'] : []),
    ...[...sourcesOf(app, 'git'), ...sourcesOf(app, 'artifact')].flatMap((source) =>
      source.name ? [source.name] : [],
    ),
  ];
  for (const name of names) {
    if (names.indexOf(name) !== names.lastIndexOf(name)) {
      fail(`sources: duplicate source name ${name}`);
    }
  }

  for (const source of sourcesOf(app, 'artifact')) {
    if (source.repo === app.artifact?.repo) {
      fail(`sources: ${source.repo} is the app's own artifact`);
    }
  }
  if (app.artifact && app.build && ciProvider(app) !== 'forgejo') {
    fail('artifact: image updates must run on forgejo ci to republish the artifact');
  }

  const manifests = sourcesOf(app, 'manifests');
  if (manifests.length > 1) {
    fail('sources: only one manifests source is allowed');
  }
  if (manifests[0]?.path.split('/').includes('..')) {
    fail('sources: manifests path must stay inside the app directory');
  }
  if (manifests[0] && needsKustomization(app) && manifests[0].path !== '.') {
    fail('sources: manifests path must be . with config, dashboard or clusterRules');
  }

  const exportNames = (app.exports ?? []).map((e) => e.name);
  for (const exported of app.exports ?? []) {
    if (exportNames.indexOf(exported.name) !== exportNames.lastIndexOf(exported.name)) {
      fail(`exports: duplicate name ${exported.name}`);
    }
    if (!appWorkloads(app)[exported.workload]) {
      fail(`exports.${exported.name}: no workload named ${exported.workload}`);
    }
    exportPort(app, exported);
  }

  return app;
}

const SYSTEM_NAMESPACES = new Set(['argocd', 'dragonfly', 'envoy-gateway-system', 'monitoring']);

export function validateIngress(apps: App[]) {
  const known = new Set(apps.map(appNamespace));
  for (const app of apps) {
    for (const from of app.network.ingress) {
      if (!known.has(from) && !SYSTEM_NAMESPACES.has(from)) {
        throw new Error(`${app.name}: network.ingress: no app or namespace named ${from}`);
      }
    }
  }
}

export function validateResources(world: World) {
  validateSubgraphs(world.apps);

  for (const app of world.apps) {
    validateRefs(app, world.external?.has(app.name) ?? false);

    for (const resource of app.resources ?? []) {
      if (resource.type === 'app') {
        appExportUrl(app, resource, world.apps);
      }
    }

    for (const spec of Object.values(appWorkloads(app))) {
      expandWorkload(spec, refsFor(app, world));
    }

    for (const config of declared(app, 'config')) {
      for (const ref of Object.values(config.values ?? {})) {
        const kind = SECRET_KINDS.find((k) => ref[k]) as SecretKind;
        secretRef(app, kind, ref[kind]);
      }
    }

    for (const volume of declared(app, 'volume')) {
      storageClass(app, volume, world.platform);
    }
    for (const spec of Object.values(appDatabases(app))) {
      databaseHost(app, spec, world.platform);
    }
  }
}

export function validateManifestSources(
  app: App,
  found: { hasManifests: boolean; hasRootKustomization: boolean },
) {
  const declared = sourcesOf(app, 'manifests')[0];
  if (!declared && (found.hasManifests || found.hasRootKustomization || needsKustomization(app))) {
    throw new Error(
      `${app.name}: sources: add a manifests source, this app has a kustomization that would not deploy`,
    );
  }

  if (declared && found.hasRootKustomization && declared.path !== '.') {
    throw new Error(
      `${app.name}: sources: manifests path must be . to deploy the app directory's kustomization`,
    );
  }
}

const HTTP_ONLY = [
  'hosts',
  'paths',
  'rules',
  'dns',
  'ratelimit',
  'timeouts',
  'loadBalancer',
] as const;
