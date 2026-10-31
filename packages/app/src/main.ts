import { fs } from '@ortzi/host';
import { cli } from '@ortzi/kit';
import { parse, stringify } from 'yaml';
import { type App, imageValuesPath, renderApplication, type Target, validate } from './render';

const LOCATIONS = ['projects', 'platform-services'];
const VERSIONS = 'platform/versions.yaml';

const USAGE = `usage: ortzi app <command> [flags]

run from the root of the k8s repo

commands:
  render           write application.yaml for every app.yaml
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
        target: { location, hasManifests: fs.exists(`${location}/${entry.name}/manifests`) },
      });
    }
  }
  return found;
}

function render(check: boolean): number {
  const versions = parse(fs.readText(VERSIONS)).charts;
  let stale = 0;
  const sync = (path: string, wanted: string) => {
    if (fs.exists(path) && fs.readText(path) === wanted) return;
    stale++;
    console.log(`${check ? 'stale' : 'wrote'} ${path}`);
    if (!check) fs.write(path, wanted);
  };

  for (const { app, target } of discover()) {
    sync(
      `${target.location}/${app.name}/application.yaml`,
      renderApplication(app, versions, target),
    );
    if (app.workloads) {
      const path = imageValuesPath(app, target);
      const current = fs.exists(path) ? parse(fs.readText(path)) : {};
      const images = { ...current?.images };
      for (const { image } of Object.values(app.workloads)) images[image] ??= 'latest';
      const wanted = stringify({ images });
      if (!fs.exists(path) || stringify({ images: current?.images ?? {} }) !== wanted) sync(path, wanted);
    }
  }
  return check && stale > 0 ? 1 : 0;
}

export const run = cli({
  usage: USAGE,
  flags: { check: { type: 'boolean' } },
  setup: () => undefined,
  commands: {
    render: (_ctx, values) => render(values.check),
    list: () => {
      for (const { app, target } of discover()) {
        const blocks = Object.keys(app).filter((k) => k !== 'name');
        console.log(`${target.location}/${app.name}\t${blocks.join(' ')}`);
      }
      return 0;
    },
  },
});
