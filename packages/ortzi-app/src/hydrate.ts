import { capture, fs, pushDir } from '@ortzi/host';
import type { Plan } from './plan';
import { stampFlux } from './stamp';

const ARTIFACT_ROOT = '.cache/ortzi-app/artifacts';

export interface Cluster {
  repo: string;
  tag: string;
  root: string;
  // path is the app's directory under root
  apps: { name: string; path: string; revision: string; releases: string[] }[];
}

// Every file under dir, relative to it.
function files(dir: string, prefix = ''): string[] {
  return fs
    .readDir(`${dir}${prefix}`)
    .flatMap((entry) =>
      entry.dir ? files(dir, `${prefix}/${entry.name}`) : [`${prefix}/${entry.name}`.slice(1)],
    );
}

function freshDir(dir: string) {
  if (fs.exists(dir)) {
    fs.remove(dir);
  }
  fs.mkdir(dir);
}

export function publishArtifact(plan: Plan, revision: string) {
  const manifests = plan.sources.find((source) => source.type === 'manifests');
  if (!plan.artifact || !manifests) {
    return;
  }

  const dir = `${ARTIFACT_ROOT}/${plan.name}`;
  freshDir(dir);
  fs.write(`${dir}/kustomize.yaml`, capture('kustomize', ['build', manifests.path]));

  const { repo, tag } = plan.artifact;
  const pushed = pushDir(dir, repo, { tags: [tag], revision, sign: true });
  console.log(`${pushed.changed ? 'pushed' : 'unchanged'} ${repo}:${tag} ${pushed.digest}`);
}

export function publishCluster(
  cluster: Cluster,
  revision: string,
  resolve: (images: string[]) => Map<string, string>,
) {
  const dir = `${ARTIFACT_ROOT}/cluster`;
  freshDir(dir);

  const tree = new Map(
    files(cluster.root).map((file) => [file, fs.readText(`${cluster.root}/${file}`)]),
  );

  const tags = resolve([...new Set(cluster.apps.flatMap((app) => app.releases))]);
  for (const { name, path, revision: appRevision, releases } of cluster.apps) {
    const images = Object.fromEntries(
      releases.map((image) => {
        const tag = tags.get(image);
        if (!tag) {
          throw new Error(`${name}: ${image} has never been promoted; run ortzi app promote`);
        }
        return [image, tag];
      }),
    );

    const prefix = `${path}/`;
    const own = new Map([...tree].filter(([file]) => file.startsWith(prefix)));
    if (!own.size) {
      throw new Error(
        `${name}: nothing rendered under ${cluster.root}/${path}; run ortzi app render`,
      );
    }
    for (const [file, content] of stampFlux(own, appRevision, images)) {
      tree.set(file, content);
    }
  }

  for (const [file, content] of tree) {
    const target = `${dir}/${file}`;
    fs.mkdir(target.slice(0, target.lastIndexOf('/')));
    fs.write(target, content);
  }

  const pushed = pushDir(dir, cluster.repo, { tags: [cluster.tag], revision, sign: true });
  console.log(
    `${pushed.changed ? 'pushed' : 'unchanged'} ${cluster.repo}:${cluster.tag} ${pushed.digest}`,
  );
}
