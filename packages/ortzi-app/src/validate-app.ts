import { type Schema, Validator } from 'jsonschema/lib/index.js';
import { SECRET_KINDS } from './workload.ts';
import type { App, World } from './types.ts';
import type { Resource, ResourceType } from './providers/provider.ts';
import { appNamespace, declared, sourcesOf } from './app.ts';
import { exportPort } from './services.ts';
import { refsFor } from './refs.ts';
import { needsKustomization } from './manifests.ts';
import { ciProvider } from './workflows.ts';
import { validateCi, workflowRefs } from './ci.ts';
import { APP_SCHEMA } from './schema.ts';
import { appWorkloads, workloadImages, workloadRefs, workloads } from './providers/workload.ts';
import { isValueResource, RESOURCES, resourceProvider, SOURCES } from './providers/index.ts';

const resourcesOf = (app: App, type: ResourceType) => declared(app, type) as never[];

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
  const invalid = schemaError(app, APP_SCHEMA, 'instance');
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

  workloads.validate(app, fail);
  for (const [name, workload] of Object.entries(appWorkloads(app))) {
    for (const value of Object.values(
      !workload.env || Array.isArray(workload.env) ? {} : workload.env,
    )) {
      const used = (value as { resource?: string } | null)?.resource;
      if (used && !app.resources?.some((r) => r.name === used && isValueResource(r.type))) {
        fail(`workloads.${name}.env: no resource named ${used}; list a secret in envFrom`);
      }
    }
  }

  for (const provider of RESOURCES) {
    provider.validate?.(app, resourcesOf(app, provider.type), fail);
  }

  const resourceKeys = (app.resources ?? []).map(
    (r) => `${isValueResource(r.type) ? 'value' : r.type}/${r.name}`,
  );
  for (const key of resourceKeys) {
    if (resourceKeys.indexOf(key) !== resourceKeys.lastIndexOf(key)) {
      fail(`resources: duplicate name ${key.split('/')[1]}`);
    }
  }

  validateCi(app, fail);

  const kustomized = needsKustomization(app);
  for (const provider of SOURCES) {
    provider.validate?.({ app, kustomized, fail }, sourcesOf(app, provider.type) as never[]);
  }

  const names = [
    ...(sourcesOf(app, 'manifests').length ? ['manifests'] : []),
    ...[...sourcesOf(app, 'git'), ...sourcesOf(app, 'artifact')].flatMap((source) =>
      source.name ? [source.name] : [],
    ),
  ];
  for (const name of names) {
    if (names.indexOf(name) !== names.lastIndexOf(name)) {
      fail(`sources: duplicate source name ${name}`);
    }
  }

  if (app.artifact && app.build && ciProvider(app) !== 'forgejo') {
    fail('artifact: image updates must run on forgejo ci to republish the artifact');
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

const SYSTEM_NAMESPACES = new Set([
  'dragonfly',
  'envoy-gateway-system',
  'flux-system',
  'monitoring',
]);

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

function unused(resource: Resource, uses: Record<string, unknown>[]): string | false {
  const kinds = (SECRET_KINDS as string[]).includes(resource.type)
    ? ['resource', resource.type]
    : ['resource'];
  return uses.some((use) => kinds.some((kind) => use[kind] === resource.name))
    ? false
    : `no workload uses this ${resource.type}`;
}

export function validateResources(world: World) {
  for (const provider of RESOURCES) {
    provider.checkWorld?.(world);
  }

  for (const app of world.apps) {
    const fail = (message: string): never => {
      throw new Error(`${app.name}: ${message}`);
    };
    const external = world.external?.has(app.name) ?? false;
    const refs = refsFor(app, world);
    const uses = [
      ...workloadRefs(app),
      ...workflowRefs(app),
      ...RESOURCES.flatMap(
        (provider) => provider.uses?.(app, resourcesOf(app, provider.type)) ?? [],
      ),
    ];

    for (const resource of app.resources ?? []) {
      const provider = resourceProvider(resource.type);
      if (!provider.ref || external) {
        continue;
      }
      const message = provider.unused?.(app, resource as never, uses) ?? unused(resource, uses);
      if (message) {
        fail(`resources.${resource.name}: ${message}`);
      }
    }

    for (const provider of RESOURCES) {
      provider.check?.(app, resourcesOf(app, provider.type), { world, external, refs, uses, fail });
    }

    if (app.build && !workloadImages(app).includes(app.build.image)) {
      fail(`build.image ${app.build.image} is not used by any workload`);
    }

    workloads.check(app, refs);
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
