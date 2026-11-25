import { capture, fs, run as exec } from '@ortzi/host';
import { cli, columns, Kube, parseMinutes, UsageError } from '@ortzi/kit';
import { listApps } from './flux';
import { changedScope, parseStatus } from './changed';
import { planCommits } from './commits';
import { buildGraph, toDot, toMermaid } from './graph';
import { doctor } from './commands/doctor';
import { env } from './commands/env';
import { fmt } from './commands/fmt';
import { history } from './commands/history';
import { autosync, heal } from './commands/policy';
import { buildApp } from './commands/build';
import { rollback } from './commands/rollback';
import { version } from './commands/version';
import { status } from './commands/status';
import { supergraph, supergraphCheck } from './commands/supergraph';
import { parse, stringify } from 'yaml';
import {
  type App,
  appDatabases,
  appFiles,
  appMetrics,
  appOidc,
  appRoutes,
  appSecrets,
  imageValuesPath,
  legacyImageValuesPath,
  renderImageValues,
  isCurrent,
  appWorkloads,
  HEADER,
  needsKustomization,
  type Listener,
  type PlatformExport,
  sourcesOf,
  validateManifestSources,
  renderWorkflow,
  stamped,
  type Target,
  passthroughWarnings,
  validate,
  validateGatewayAccess,
  validateIngress,
  validateResources,
  workflowPath,
  type World,
} from './render';
import { publishArtifact, publishCluster } from './hydrate';
import { renderOutputs } from './outputs';
import { type ClusterTree, renderFluxIndex } from './outputs/flux-index';
import { appPath, CLUSTER_ROOT } from './flux-objects';
import { planApp } from './plan';
import { generatedFiles } from './files';
import { appTerraformPath, renderAppTerraform, TERRAFORM_APPS } from './terraform';
import { type RenderedOutput, validateManifests } from './validate';

const LOCATIONS = ['projects', 'platform-services', 'selfhosted'];
const VERSIONS = 'platform/versions.yaml';
const APPS_BUNDLE = 'platform/apps.yaml';
const PLATFORM_EXPORTS = 'platform/exports';
const GATEWAYS = 'kubernetes/envoy-gateway-system/envoy-gateway/config';

const platformExports = (): PlatformExport[] =>
  fs
    .readDir(PLATFORM_EXPORTS)
    .filter((entry) => !entry.dir && entry.name.endsWith('.yaml'))
    .flatMap((entry) => parse(fs.readText(`${PLATFORM_EXPORTS}/${entry.name}`)).exports);

const gatewayListeners = (): Listener[] =>
  fs
    .readDir(GATEWAYS)
    .filter((entry) => entry.dir && fs.exists(`${GATEWAYS}/${entry.name}/gateway.yaml`))
    .flatMap((entry) => {
      const gateway = parse(fs.readText(`${GATEWAYS}/${entry.name}/gateway.yaml`));
      return gateway.spec.listeners.map((listener: Record<string, any>) => ({
        gateway: gateway.metadata.name,
        name: listener.name,
        protocol: listener.protocol,
        hostname: listener.hostname,
        labels: listener.allowedRoutes?.namespaces?.selector?.matchLabels,
      }));
    });

const USAGE = `usage: ortzi app <command> [flags]

definition (run from the root of the k8s repo):
  render                   validate manifests, then write each app's flux manifests under kubernetes/
  diff [app]               show what render would change, exit 1 if anything would
  fmt [file...]            put every app.yaml (or the given files) in canonical key order and style
  list                     list apps and what they declare
  graph [app]              print how apps depend on each other, as mermaid (--format dot for graphviz)
  publish [app...]         push the kustomize build of each app with an artifact and manifests to its oci
                           repo (default: all of them), then push kubernetes/ as the
                           platform/apps.yaml cluster artifact; unchanged pushes are skipped
  version [--path P]...    print v1.0.<commits touching the paths>-<short sha>; paths use workflow
                           on.push.paths syntax (!x excludes), default the whole repo
  supergraph               fetch every subgraph resource's federation sdl and compose the supergraph,
                           public schema and router.yaml into the app declaring the supergraph
  supergraph check <subgraph> --schema F [--documents F]
                           compose with a candidate sdl (and persisted documents) for one subgraph
                           against the others, fail if it doesn't compose or breaks a document,
                           and list other breaking changes to the public schema

cluster:
  status                   what needs attention (--all lists every app)
  status <app>             one app's state and images
  env <app> [workload]     each env var, where it comes from, and whether its secret key exists
  doctor <app>             check workloads, secrets, volumes, routes, oidc and databases on the cluster
  history <app>            helm release history
  build <app>              build the app's image from its build block for --platform (default
                           linux/amd64), push it tagged with the commit sha (sha-dirty-<time>
                           with uncommitted changes) and sign it
  rollback <app> <tag>     edit the app's images to the given tag on the cluster, suspending
                           the app first
  heal <app> on|off        set flux drift correction; off also holds new git commits
  autosync <app> on|off    suspend the app in flux, or resume it (returns the app to git)

flags:
  --format FMT     graph: mermaid or dot (default mermaid)
  --check          render, supergraph, fmt: write nothing, exit 1 if invalid or out of date
  --commit         render: commit what changed, one commit per app
  --changed        render: only validate what the working tree touches
  --force          render: regenerate every file, even those already up to date
  --context CTX    kube context (default: current)
  --all            status: list every app
  --latest         status: also compare against ghcr's latest tag
  --image IMAGE    rollback: only this image
  --path P         version: count commits touching P (repeatable)
  --platform P     build: target platform (default linux/amd64)
  --schema F       supergraph check: the candidate subgraph sdl
  --documents F    supergraph check: the candidate persisted documents manifest
  --yes            apply; writes are a dry run without this
  --wait           rollback, autosync on: follow the change and fail if it doesn't finish
  --timeout MIN    with --wait: minutes to allow for each step (default 5)`;

interface Ctx {
  kube: () => Kube;
}

const DEFAULT_TIMEOUT_MINUTES = 5;

const appNames = (ctx: Ctx, position: number) =>
  position === 0 ? listApps(ctx.kube()).map((app) => app.name) : [];

const appThenOnOff = (ctx: Ctx, position: number) =>
  position === 1 ? ['on', 'off'] : appNames(ctx, position);

function requireApp(name: string | undefined, command: string): string {
  if (!name) {
    throw new UsageError(`usage: ortzi app ${command} <app>`);
  }
  return name;
}

function requireOnOff(
  command: string,
  name: string | undefined,
  state: string | undefined,
): { name: string; state: 'on' | 'off' } {
  if (!name || (state !== 'on' && state !== 'off')) {
    throw new UsageError(`usage: ortzi app ${command} <app> on|off`);
  }
  return { name, state };
}

const readValues = (path: string): Record<string, unknown> =>
  fs.exists(path) ? (parse(fs.readText(path)) ?? {}) : {};

const worldOf = (apps: App[]): World => ({ platform: platformExports(), apps, readValues });

const appPaths = () =>
  LOCATIONS.flatMap((location) =>
    fs
      .readDir(location)
      .filter((entry) => entry.dir)
      .map((entry) => `${location}/${entry.name}/app.yaml`)
      .filter((path) => fs.exists(path)),
  );

function discover(): { app: App; target: Target }[] {
  const found = [];
  for (const location of LOCATIONS) {
    for (const entry of fs.readDir(location)) {
      if (!entry.dir) {
        continue;
      }
      const path = `${location}/${entry.name}/app.yaml`;
      if (!fs.exists(path)) {
        continue;
      }
      const app = validate(parse(fs.readText(path)), path);
      if (app.name !== entry.name) {
        throw new Error(`${path}: name must match its directory`);
      }

      const target = {
        location,
        hasManifests: fs.exists(`${location}/${entry.name}/manifests/kustomization.yaml`),
      };
      const hasRootKustomization = fs.exists(`${location}/${entry.name}/kustomization.yaml`);
      validateManifestSources(app, { hasManifests: target.hasManifests, hasRootKustomization });

      for (const file of appFiles(app, target)) {
        if (!fs.exists(file)) {
          throw new Error(`${path}: no file named ${file}`);
        }
      }

      found.push({ app, target });
    }
  }

  validateIngress(found.map(({ app }) => app));

  const external = found.filter(
    ({ app, target }) =>
      sourcesOf(app, 'helm').length ||
      target.hasManifests ||
      (sourcesOf(app, 'manifests').length && !needsKustomization(app)),
  );
  validateResources({
    platform: platformExports(),
    apps: found.map(({ app }) => app),
    external: new Set(external.map(({ app }) => app.name)),
  });

  validateGatewayAccess(
    found.map(({ app }) => app),
    gatewayListeners(),
  );

  return found;
}

function loadApp(name: string | undefined, command: string) {
  const app = requireApp(name, command);
  const apps = discover().map(({ app }) => app);
  const found = apps.find((candidate) => candidate.name === app);
  if (!found) {
    throw new Error(`no app named ${app}`);
  }
  return { app: found, world: worldOf(apps) };
}

function staleFiles(only?: string, force = false): { path: string; wanted: string }[] {
  const { charts: versions, actions } = parse(fs.readText(VERSIONS));
  const stale = [];
  const apps = discover();
  const world = worldOf(apps.map(({ app }) => app));

  if (only && !apps.some(({ app }) => app.name === only)) {
    throw new Error(`no app named ${only}`);
  }

  for (const { app, target } of apps) {
    if (only && app.name !== only) {
      continue;
    }
    for (const [path, wanted] of renderOutputs(planApp(app, versions, target, world), app)) {
      if (force || !fs.exists(path) || !isCurrent(fs.readText(path), wanted)) {
        stale.push({ path, wanted: stamped(wanted, new Date()) });
      }
    }

    if (app.build) {
      const workflow = workflowPath(app);
      const rendered = renderWorkflow(app, target, actions);
      if (force || !fs.exists(workflow) || !isCurrent(fs.readText(workflow), rendered)) {
        stale.push({ path: workflow, wanted: stamped(rendered, new Date()) });
      }
    }

    if (app.workloads) {
      const values = imageValuesPath(app);
      const current = fs.exists(values) ? fs.readText(values) : undefined;
      const output = renderImageValues(app, current);
      if (force || current !== output) {
        stale.push({ path: values, wanted: output });
      }
    }
  }

  const selected = apps.filter(({ app }) => !only || app.name === only);
  for (const { path, content, stamp } of generatedFiles(world, selected, fs.readText)) {
    const current = fs.exists(path) ? fs.readText(path) : undefined;
    if (stamp) {
      if (force || current === undefined || !isCurrent(current, content)) {
        stale.push({ path, wanted: stamped(content, new Date()) });
      }
    } else if (force || current !== content) {
      stale.push({ path, wanted: content });
    }
  }

  const plans = apps.map(({ app, target }) => planApp(app, versions, target, world));
  for (const [path, wanted] of renderFluxIndex(
    plans.filter((plan) => plan.artifact),
    clusterTree(),
  )) {
    if (force || !fs.exists(path) || !isCurrent(fs.readText(path), wanted)) {
      stale.push({ path, wanted: stamped(wanted, new Date()) });
    }
  }

  return stale;
}

function clusterTree(): ClusterTree {
  const dirs = (path: string) =>
    fs.exists(path)
      ? fs
          .readDir(path)
          .filter((entry) => entry.dir)
          .map((entry) => entry.name)
      : [];

  const namespaces = dirs(CLUSTER_ROOT);
  const components = namespaces.flatMap((namespace) =>
    dirs(`${CLUSTER_ROOT}/${namespace}`)
      .filter((name) => name !== 'app' && name !== 'config')
      .map((name) => `${namespace}/${name}/ks.yaml`)
      .filter((path) => {
        const file = `${CLUSTER_ROOT}/${path}`;
        return fs.exists(file) && !fs.readText(file).startsWith(HEADER);
      }),
  );
  return { namespaces, components };
}

function staleWorkflows(apps: App[]): string[] {
  const wanted = new Set(apps.filter((app) => app.build).map(workflowPath));
  return ['.github/workflows', '.forgejo/workflows'].flatMap((dir) =>
    fs
      .readDir(dir)
      .map((entry) => `${dir}/${entry.name}`)
      .filter(
        (path) =>
          /\/app-[^/]+\.yaml$/.test(path) &&
          !wanted.has(path) &&
          fs.readText(path).startsWith(HEADER),
      ),
  );
}

function staleTerraform(apps: App[]): string[] {
  const wanted = new Set(apps.filter((app) => renderAppTerraform(app)).map(appTerraformPath));
  return fs
    .readDir(TERRAFORM_APPS)
    .map((entry) => `${TERRAFORM_APPS}/${entry.name}`)
    .filter(
      (path) => path.endsWith('.tf.json') && !path.endsWith('/_index.tf.json') && !wanted.has(path),
    );
}

function commitAll(paths: string[], apps: App[]) {
  for (const { message, paths: group } of planCommits(paths, apps)) {
    exec('git', ['add', '-A', '--', ...group]);
    if (exec('git', ['diff', '--cached', '--quiet'], { check: false }).ok) {
      continue;
    }
    exec('git', ['commit', '--quiet', '-m', message]);
    console.log(`committed ${message}`);
  }
}

function changedPaths(generated: Map<string, RenderedOutput>): string[] {
  const status = exec('git', ['status', '--porcelain', '-z', '--no-renames', '-uall'], {
    stdout: 'piped',
  });

  const rerendered = [...generated]
    .filter(([path, { content }]) => !fs.exists(path) || !isCurrent(fs.readText(path), content))
    .map(([path]) => path);
  return [...parseStatus(status.stdout), ...rerendered];
}

// image.values.yaml moved from the app dir to kubernetes/; carry the live tags across once
function migrateImageValues(check: boolean): string[] {
  return discover().flatMap(({ app, target }) => {
    const legacy = legacyImageValuesPath(app, target);
    if (!fs.exists(legacy)) {
      return [];
    }

    const moved = imageValuesPath(app);
    console.log(`${check ? 'stale' : 'moved'} ${legacy} -> ${moved}`);
    if (!check) {
      if (!fs.exists(moved)) {
        fs.mkdir(moved.slice(0, moved.lastIndexOf('/')));
        fs.write(moved, fs.readText(legacy));
      }
      fs.remove(legacy);
    }
    return [legacy, moved];
  });
}

function render(check: boolean, commit: boolean, changed: boolean, force: boolean): number {
  if (check && commit) {
    throw new UsageError('--check and --commit cannot be used together');
  }
  if (force && check) {
    throw new UsageError('--force and --check cannot be used together');
  }
  if (force && changed) {
    throw new UsageError('--force and --changed cannot be used together');
  }

  const migrated = migrateImageValues(check);

  const versions = parse(fs.readText(VERSIONS)).charts;
  const apps = discover();
  for (const { app } of apps) {
    for (const warning of passthroughWarnings(app)) {
      console.error(`warning: ${warning}`);
    }
  }

  const world = worldOf(apps.map(({ app }) => app));
  const generated = new Map(
    apps.flatMap(({ app, target }) => {
      const plan = planApp(app, versions, target, world);
      return renderOutputs(plan, app).map(([path, content]): [string, RenderedOutput] => [
        path,
        { content, plan },
      ]);
    }),
  );

  const failures = validateManifests(
    generated,
    changed ? changedScope(changedPaths(generated)) : undefined,
  );
  if (failures.length) {
    for (const failure of failures) {
      console.error(failure);
    }
    console.error(`${failures.length} validation failure(s), nothing written`);
    return 1;
  }

  const stale = staleFiles(undefined, force);
  for (const { path, wanted } of stale) {
    console.log(`${check ? 'stale' : 'wrote'} ${path}`);
    if (!check) {
      fs.mkdir(path.slice(0, path.lastIndexOf('/')));
      fs.write(path, wanted);
    }
  }

  const removed = [...staleTerraform(world.apps), ...staleWorkflows(world.apps)];
  for (const path of removed) {
    console.log(`${check ? 'stale' : 'removed'} ${path}`);
    if (!check) {
      fs.remove(path);
    }
  }

  if (!stale.length && !removed.length && !migrated.length) {
    console.log('everything is up to date');
  }
  if (commit) {
    commitAll([...migrated, ...stale.map(({ path }) => path), ...removed], world.apps);
  }

  return check && stale.length + removed.length + migrated.length > 0 ? 1 : 0;
}

function publish(names: string[]): number {
  const versions = parse(fs.readText(VERSIONS)).charts;
  const apps = discover();
  const world = worldOf(apps.map(({ app }) => app));

  const unknown = names.filter(
    (name) => !apps.some(({ app }) => app.name === name && app.artifact),
  );
  if (unknown.length) {
    throw new UsageError(`no app with an artifact named ${unknown.join(', ')}`);
  }

  const dirs = [
    ...apps.map(({ app, target }) => `${target.location}/${app.name}`),
    APPS_BUNDLE,
    CLUSTER_ROOT,
  ];
  if (capture('git', ['status', '--porcelain', '--', ...dirs]).trim()) {
    console.error(`uncommitted changes in ${dirs.join(', ')}; commit before publishing`);
    return 1;
  }

  const revision = capture('git', ['rev-parse', 'HEAD']).trim();
  for (const { app, target } of apps) {
    if (app.artifact && (!names.length || names.includes(app.name))) {
      publishArtifact(planApp(app, versions, target, world), revision);
    }
  }

  const { artifact, bootstrap } = parse(fs.readText(APPS_BUNDLE)) as {
    artifact: { repo: string; tag: string };
    bootstrap: string[];
  };
  const missing = bootstrap.filter((name) => !apps.some(({ app }) => app.name === name));
  if (missing.length) {
    throw new Error(`${APPS_BUNDLE}: bootstrap names unknown apps ${missing.join(', ')}`);
  }

  publishCluster(
    {
      ...artifact,
      root: CLUSTER_ROOT,
      apps: apps
        .filter(({ app }) => app.artifact && !bootstrap.includes(app.name))
        .map(({ app, target }) => {
          const plan = planApp(app, versions, target, world);
          return {
            name: app.name,
            path: appPath(plan),
            revision: capture('git', [
              'log',
              '-1',
              '--format=%H',
              '--',
              plan.directory,
              `${CLUSTER_ROOT}/${appPath(plan)}`,
            ]).trim(),
          };
        }),
    },
    revision,
  );

  return 0;
}

function diff(only?: string): number {
  const stale = staleFiles(only);
  for (const { path, wanted } of stale) {
    const old = fs.exists(path) ? path : '/dev/null';
    exec('diff', ['-u', '-L', `a/${path}`, '-L', `b/${path}`, old, '-'], {
      stdin: wanted,
      check: false,
    });
  }

  return stale.length > 0 ? 1 : 0;
}

export const run = cli({
  usage: USAGE,
  flags: {
    check: { type: 'boolean' },
    commit: { type: 'boolean' },
    changed: { type: 'boolean' },
    force: { type: 'boolean' },
    format: { type: 'string' },
    context: { type: 'string' },
    all: { type: 'boolean' },
    latest: { type: 'boolean' },
    image: { type: 'string', multiple: true },
    path: { type: 'string', multiple: true },
    platform: { type: 'string', default: 'linux/amd64' },
    schema: { type: 'string' },
    documents: { type: 'string' },
    yes: { type: 'boolean' },
    wait: { type: 'boolean' },
    timeout: { type: 'string' },
  },
  setup: (values): Ctx => {
    let kube: Kube | undefined;
    return { kube: () => (kube ??= Kube.connect(values.context)) };
  },
  commands: {
    render: (_ctx, values) => render(values.check, values.commit, values.changed, values.force),
    diff: (_ctx, _values, rest) => diff(rest[0]),
    fmt: (_ctx, values, rest) => fmt(rest.length ? rest : appPaths(), values.check),
    publish: (_ctx, _values, rest) => publish(rest),
    supergraph: (_ctx, values, rest) =>
      rest[0] === 'check'
        ? supergraphCheck(discover(), rest[1], values.schema, values.documents)
        : supergraph(discover(), values.check),
    list: () => {
      const count = (block?: object) => {
        const size = block ? Object.keys(block).length : 0;
        return size ? String(size) : '-';
      };

      const rows = discover().map(({ app, target }) => [
        app.name,
        target.location,
        count(appWorkloads(app)),
        count(appRoutes(app)),
        count(appOidc(app)),
        count(appDatabases(app)),
        count(appSecrets(app)),
        String(appMetrics(app).length || '-'),
        app.network.ingress.join(',') || '-',
      ]);

      console.log(
        columns(
          [
            'APP',
            'LOCATION',
            'WORKLOADS',
            'ROUTES',
            'OIDC',
            'DATABASES',
            'SECRETS',
            'METRICS',
            'INGRESS',
          ],
          rows,
        ),
      );

      return 0;
    },
    graph: (_ctx, values, [name]) => {
      if (values.format && values.format !== 'mermaid' && values.format !== 'dot') {
        throw new UsageError('--format must be mermaid or dot');
      }

      const apps = discover().map(({ app }) => app);
      if (name && !apps.some((app) => app.name === name)) {
        throw new Error(`no app named ${name}`);
      }

      const graph = buildGraph(apps, name);
      console.log(values.format === 'dot' ? toDot(graph) : toMermaid(graph));

      return 0;
    },
    version: (_ctx, values) => version(values.path),
    status: (ctx, values, [name]) => status(ctx.kube(), name, values.all, values.latest),
    env: (ctx, _, [name, workload]) => {
      const { app, world } = loadApp(name, 'env');
      return env(ctx.kube(), app, world, workload);
    },
    doctor: (ctx, _, [name]) => {
      const { app, world } = loadApp(name, 'doctor');
      return doctor(ctx.kube(), app, world);
    },
    history: (ctx, _, [name]) => history(ctx.kube(), requireApp(name, 'history')),
    build: (_ctx, values, [name]) => {
      const app = requireApp(name, 'build');
      const found = discover().find(({ app: candidate }) => candidate.name === app);
      if (!found) {
        throw new Error(`no app named ${app}`);
      }
      return buildApp(found.app, found.target, { platform: values.platform as string });
    },
    rollback: (ctx, values, [name, tag]) =>
      rollback(ctx.kube(), requireApp(name, 'rollback'), {
        tag,
        image: values.image[0],
        yes: values.yes,
        wait: values.wait,
        timeoutMinutes: parseMinutes(values.timeout, DEFAULT_TIMEOUT_MINUTES),
      }),
    heal: (ctx, values, [app, value]) => {
      const { name, state } = requireOnOff('heal', app, value);
      return heal(ctx.kube(), name, state, values.yes);
    },
    autosync: (ctx, values, [app, value]) => {
      const { name, state } = requireOnOff('autosync', app, value);
      return autosync(ctx.kube(), name, state, values.yes, {
        wait: values.wait,
        timeoutMinutes: parseMinutes(values.timeout, DEFAULT_TIMEOUT_MINUTES),
      });
    },
  },
  completers: {
    graph: appNames,
    status: appNames,
    env: appNames,
    doctor: appNames,
    history: appNames,
    build: appNames,
    rollback: appNames,
    heal: appThenOnOff,
    autosync: appThenOnOff,
  },
});
