import { capture, fs, pushDir } from '@ortzi/host';
import type { Plan } from './plan';
import { stampRevision } from './stamp';

export { stampRevision } from './stamp';

const ARTIFACT_ROOT = '.cache/ortzi-app/artifacts';

export interface Bundle {
  repo: string;
  tag: string;
  applications: {
    location: string;
    name: string;
    path: string;
    revision: string;
    releases: string[];
  }[];
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
  for (const { location, name, path, revision: appRevision, releases } of bundle.applications) {
    const images = Object.fromEntries(
      releases.map((image) => {
        const tag = tags.get(image);
        if (!tag) {
          throw new Error(`${name}: ${image} has never been promoted; run ortzi app promote`);
        }
        return [image, tag];
      }),
    );
    fs.mkdir(`${dir}/${location}`);
    fs.write(
      `${dir}/${location}/${name}.yaml`,
      stampRevision(fs.readText(path), appRevision, images),
    );
  }

  const pushed = pushDir(dir, bundle.repo, { tags: [bundle.tag], revision, sign: true });
  console.log(
    `${pushed.changed ? 'pushed' : 'unchanged'} ${bundle.repo}:${bundle.tag} ${pushed.digest}`,
  );
}
