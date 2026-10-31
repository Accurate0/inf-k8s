import { fs, run as exec } from '@ortzi/host';
import { cli } from '@ortzi/kit';
import { parse, stringify } from 'yaml';
import {
  type App,
  imageValuesPath,
  isCurrent,
  renderApplication,
  stamped,
  type Target,
  validate,
  workloadImages,
} from './render';

const LOCATIONS = ['projects', 'platform-services'];
const VERSIONS = 'platform/versions.yaml';

const USAGE = `usage: ortzi app <command> [flags]

run from the root of the k8s repo

commands:
  render           write application.yaml for every app.yaml
  diff [app]       show what render would change, exit 1 if anything would
  list             list apps and the blocks they use

flags:
  --check          render: write nothing, exit 1 if anything is out of date`;

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
  flags: { check: { type: 'boolean' } },
  setup: () => undefined,
  commands: {
    render: (_ctx, values) => render(values.check),
    diff: (_ctx, _values, rest) => diff(rest[0]),
    list: () => {
      for (const { app, target } of discover()) {
        const blocks = Object.keys(app).filter((k) => k !== 'name');
        console.log(`${target.location}/${app.name}\t${blocks.join(' ')}`);
      }
      return 0;
    },
  },
});
