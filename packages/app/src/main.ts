import { fs, run as exec } from '@ortzi/host';
import { cli, columns, Kube, parseMinutes, UsageError } from '@ortzi/kit';
import { listApps } from './argo';
import { history } from './commands/history';
import { autosync, heal } from './commands/policy';
import { rollback } from './commands/rollback';
import { status } from './commands/status';
import { parse, stringify } from 'yaml';
import {
  type App,
  imageValuesPath,
  isCurrent,
  renderApplication,
  renderWorkflow,
  stamped,
  type Target,
  validate,
  validateIngress,
  workflowPath,
  workloadImages,
} from './render';
import { validateManifests } from './validate';

const LOCATIONS = ['projects', 'platform-services'];
const VERSIONS = 'platform/versions.yaml';

const USAGE = `usage: ortzi app <command> [flags]

definition (run from the root of the k8s repo):
  render                   validate manifests, then write application.yaml for every app.yaml
  diff [app]               show what render would change, exit 1 if anything would
  list                     list apps and what they declare

cluster:
  status                   what needs attention (--all lists every app)
  status <app>             one app's state and images
  history <app>            previous argo syncs and their image tags
  rollback <app> [tag]     edit images on the cluster, turning auto-sync off first
                           (default tag: the most recent different one in history)
  heal <app> on|off        set self-heal; auto-sync still applies new git commits
  autosync <app> on|off    turn argo auto-sync off, or restore it (returns the app to git)

flags:
  --check          render: write nothing, exit 1 if invalid or out of date
  --context CTX    kube context (default: current)
  --all            status: list every app
  --latest         status: also compare against ghcr's latest tag
  --image NAME     rollback: only this image (repeat by running again)
  --yes            apply; writes are a dry run without this
  --wait           rollback, autosync on: follow the change and fail if it doesn't finish
  --timeout MIN    with --wait: minutes to allow for each step (default 5)`;

interface Ctx {
  kube: () => Kube;
}

const DEFAULT_TIMEOUT_MINUTES = 5;

const appNames = (ctx: Ctx, position: number) =>
  position === 0 ? listApps(ctx.kube()).map((app) => app.metadata.name) : [];

const appThenOnOff = (ctx: Ctx, position: number) =>
  position === 1 ? ['on', 'off'] : appNames(ctx, position);

function requireApp(name: string | undefined, command: string): string {
  if (!name) throw new UsageError(`usage: ortzi app ${command} <app>`);
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


function discover(): { app: App; target: Target }[] {
  const found = [];
  for (const location of LOCATIONS) {
    for (const entry of fs.readDir(location)) {
      if (!entry.dir) continue;
      const path = `${location}/${entry.name}/app.yaml`;
      if (!fs.exists(path)) continue;
      const app = validate(parse(fs.readText(path)), path);
      if (app.name !== entry.name) throw new Error(`${path}: name must match its directory`);
      found.push({
        app,
        target: {
          location,
          hasManifests: fs.exists(`${location}/${entry.name}/manifests`),
          rootKustomization: fs.exists(`${location}/${entry.name}/kustomization.yaml`),
        },
      });
    }
  }
  validateIngress(found.map(({ app }) => app));
  return found;
}

function staleFiles(only?: string): { path: string; wanted: string }[] {
  const versions = parse(fs.readText(VERSIONS)).charts;
  const stale = [];
  const apps = discover();
  if (only && !apps.some(({ app }) => app.name === only)) throw new Error(`no app named ${only}`);
  for (const { app, target } of apps) {
    if (only && app.name !== only) continue;
    const path = `${target.location}/${app.name}/application.yaml`;
    const wanted = renderApplication(app, versions, target);
    if (!fs.exists(path) || !isCurrent(fs.readText(path), wanted)) {
      stale.push({ path, wanted: stamped(wanted, new Date()) });
    }
    if (app.build) {
      const workflow = workflowPath(app);
      const rendered = renderWorkflow(app, target);
      if (!fs.exists(workflow) || !isCurrent(fs.readText(workflow), rendered)) {
        stale.push({ path: workflow, wanted: stamped(rendered, new Date()) });
      }
    }
    if (app.workloads) {
      const values = imageValuesPath(app, target);
      const current = fs.exists(values) ? parse(fs.readText(values)) : {};
      const images = { ...current?.images };
      for (const image of workloadImages(app)) images[image] ??= 'latest';
      const output = stringify({ images });
      if (!fs.exists(values) || stringify({ images: current?.images ?? {} }) !== output) {
        stale.push({ path: values, wanted: output });
      }
    }
  }
  return stale;
}

function render(check: boolean): number {
  const versions = parse(fs.readText(VERSIONS)).charts;
  const generated = new Map(
    discover().map(({ app, target }) => [
      `${target.location}/${app.name}/application.yaml`,
      renderApplication(app, versions, target),
    ]),
  );
  const failures = validateManifests(generated);
  if (failures.length) {
    for (const failure of failures) console.error(failure);
    console.error(`${failures.length} validation failure(s), nothing written`);
    return 1;
  }
  const stale = staleFiles();
  for (const { path, wanted } of stale) {
    console.log(`${check ? 'stale' : 'wrote'} ${path}`);
    if (!check) fs.write(path, wanted);
  }
  return check && stale.length > 0 ? 1 : 0;
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
    context: { type: 'string' },
    all: { type: 'boolean' },
    latest: { type: 'boolean' },
    image: { type: 'string' },
    yes: { type: 'boolean' },
    wait: { type: 'boolean' },
    timeout: { type: 'string' },
  },
  setup: (values): Ctx => {
    let kube: Kube | undefined;
    return { kube: () => (kube ??= Kube.connect(values.context)) };
  },
  commands: {
    render: (_ctx, values) => render(values.check),
    diff: (_ctx, _values, rest) => diff(rest[0]),
    list: () => {
      const count = (block?: object) => {
        const size = block ? Object.keys(block).length : 0;
        return size ? String(size) : '-';
      };
      const rows = discover().map(({ app, target }) => [
        app.name,
        target.location,
        count(app.workloads),
        count(app.routes),
        count(app.oidc),
        count(app.databases),
        count(app.secrets),
        app.metrics ? 'yes' : '-',
        app.network.ingress.join(',') || '-',
      ]);
      console.log(
        columns(
          ['APP', 'LOCATION', 'WORKLOADS', 'ROUTES', 'OIDC', 'DATABASES', 'SECRETS', 'METRICS', 'INGRESS'],
          rows,
        ),
      );
      return 0;
    },
    status: (ctx, values, [name]) => status(ctx.kube(), name, values.all, values.latest),
    history: (ctx, _, [name]) => history(ctx.kube(), requireApp(name, 'history')),
    rollback: (ctx, values, [name, tag]) =>
      rollback(ctx.kube(), requireApp(name, 'rollback'), {
        tag,
        image: values.image,
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
    status: appNames,
    history: appNames,
    rollback: appNames,
    heal: appThenOnOff,
    autosync: appThenOnOff,
  },
});
