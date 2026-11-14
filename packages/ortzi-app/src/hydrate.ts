import { capture, fs, pushDir } from '@ortzi/host';
import { helmTemplates, type Plan } from './plan';

const ARTIFACT_ROOT = '.cache/ortzi-app/artifacts';

export function hydrate(plan: Plan): string {
  const dir = `${ARTIFACT_ROOT}/${plan.name}`;
  if (fs.exists(dir)) {
    fs.remove(dir);
  }
  fs.mkdir(dir);

  for (const { name, args, values } of helmTemplates(plan, true)) {
    fs.write(`${dir}/${name}.yaml`, capture('helm', args, { stdin: values }));
  }

  for (const source of plan.sources) {
    if (source.type === 'manifests') {
      fs.write(`${dir}/kustomize.yaml`, capture('kustomize', ['build', source.path]));
    }
  }

  return dir;
}

export function publishArtifact(plan: Plan, revision: string) {
  if (!plan.artifact) {
    throw new Error(`${plan.name}: no artifact declared`);
  }

  const { repo, tag } = plan.artifact;
  const pushed = pushDir(hydrate(plan), repo, { tags: [tag], revision, sign: true });
  console.log(`${pushed.changed ? 'pushed' : 'unchanged'} ${repo}:${tag} ${pushed.digest}`);
}
