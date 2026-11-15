import { capture, fs, pushDir } from '@ortzi/host';
import { parseDocument } from 'yaml';
import type { Plan } from './plan';

const ARTIFACT_ROOT = '.cache/ortzi-app/artifacts';
const REVISION = 'inf-k8s.net/revision';

export interface Bundle {
  repo: string;
  tag: string;
  applications: { location: string; name: string; path: string; revision: string }[];
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

export function stampRevision(application: string, revision: string): string {
  const document = parseDocument(application);
  document.setIn(['metadata', 'annotations', REVISION], revision);
  return document.toString({ lineWidth: 0 });
}

export function publishBundle(bundle: Bundle, revision: string) {
  const dir = `${ARTIFACT_ROOT}/apps`;
  freshDir(dir);

  for (const { location, name, path, revision: appRevision } of bundle.applications) {
    fs.mkdir(`${dir}/${location}`);
    fs.write(`${dir}/${location}/${name}.yaml`, stampRevision(fs.readText(path), appRevision));
  }

  const pushed = pushDir(dir, bundle.repo, { tags: [bundle.tag], revision, sign: true });
  console.log(
    `${pushed.changed ? 'pushed' : 'unchanged'} ${bundle.repo}:${bundle.tag} ${pushed.digest}`,
  );
}
