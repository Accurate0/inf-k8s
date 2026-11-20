import { capture, fs, pushDir } from '@ortzi/host';
import { stringify } from 'yaml';
import type { Plan } from './plan';
import { stampFlux, stampRevision } from './stamp';

export { stampRevision } from './stamp';

const ARTIFACT_ROOT = '.cache/ortzi-app/artifacts';

export interface Bundle {
  repo: string;
  tag: string;
  applications: {
    location: string;
    name: string;
    path: string;
    flux?: string;
    revision: string;
    releases: string[];
  }[];
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

export function publishBundle(
  bundle: Bundle,
  revision: string,
  resolve: (images: string[]) => Map<string, string>,
) {
  const dir = `${ARTIFACT_ROOT}/apps`;
  freshDir(dir);

  const tags = resolve([...new Set(bundle.applications.flatMap((a) => a.releases))]);
  const fluxApps = new Map<string, string[]>();
  for (const application of bundle.applications) {
    const { location, name, path, flux, revision: appRevision, releases } = application;
    const images = Object.fromEntries(
      releases.map((image) => {
        const tag = tags.get(image);
        if (!tag) {
          throw new Error(`${name}: ${image} has never been promoted; run ortzi app promote`);
        }
        return [image, tag];
      }),
    );

    if (flux) {
      const rendered = new Map(files(flux).map((file) => [file, fs.readText(`${flux}/${file}`)]));
      for (const [file, content] of stampFlux(rendered, appRevision, images)) {
        const target = `${dir}/flux/${location}/${name}/${file}`;
        fs.mkdir(target.slice(0, target.lastIndexOf('/')));
        fs.write(target, content);
      }
      fluxApps.set(location, [...(fluxApps.get(location) ?? []), name]);
      continue;
    }

    fs.mkdir(`${dir}/${location}`);
    fs.write(
      `${dir}/${location}/${name}.yaml`,
      stampRevision(fs.readText(path), appRevision, images),
    );
  }

  for (const location of new Set(bundle.applications.map((a) => a.location))) {
    fs.mkdir(`${dir}/flux/${location}`);
    fs.write(
      `${dir}/flux/${location}/kustomization.yaml`,
      stringify({
        apiVersion: 'kustomize.config.k8s.io/v1beta1',
        kind: 'Kustomization',
        resources: (fluxApps.get(location) ?? []).sort().map((name) => `${name}/ks.yaml`),
      }),
    );
  }

  const pushed = pushDir(dir, bundle.repo, { tags: [bundle.tag], revision, sign: true });
  console.log(
    `${pushed.changed ? 'pushed' : 'unchanged'} ${bundle.repo}:${bundle.tag} ${pushed.digest}`,
  );
}
