import { capture, fs, pullConfig, pushDir } from '@ortzi/host';
import { CHART_REGISTRY, helmTemplates, type Kubernetes, type Plan } from './plan';

const ARTIFACT_ROOT = '.cache/ortzi-app/artifacts';

export interface Bundle {
  repo: string;
  tag: string;
  applications: { location: string; name: string; path: string }[];
}

function freshDir(dir: string) {
  if (fs.exists(dir)) {
    fs.remove(dir);
  }
  fs.mkdir(dir);
}

let helmConfig: string | undefined;

export function hydrate(plan: Plan, kubernetes: Kubernetes, dir = `${ARTIFACT_ROOT}/${plan.name}`) {
  freshDir(dir);

  const templates = helmTemplates(plan, kubernetes);
  if (templates.some(({ args }) => args.some((arg) => arg.startsWith(CHART_REGISTRY)))) {
    helmConfig ??= pullConfig(['charts']);
  }

  for (const { name, args, values } of templates) {
    fs.write(
      `${dir}/${name}.yaml`,
      capture('helm', args, {
        stdin: values,
        ...(helmConfig ? { env: { HELM_REGISTRY_CONFIG: helmConfig } } : {}),
      }),
    );
  }

  for (const source of plan.sources) {
    if (source.type === 'manifests') {
      fs.write(`${dir}/kustomize.yaml`, capture('kustomize', ['build', source.path]));
    }
  }

  return dir;
}

const contents = (dir: string) =>
  Object.fromEntries(
    fs
      .readDir(dir)
      .filter((entry) => !entry.dir)
      .map((entry) => [entry.name, fs.readText(`${dir}/${entry.name}`)]),
  );

export function unstableFiles(plan: Plan, kubernetes: Kubernetes): string[] {
  const first = contents(hydrate(plan, kubernetes, `${ARTIFACT_ROOT}/${plan.name}.check`));
  const second = contents(hydrate(plan, kubernetes));
  fs.remove(`${ARTIFACT_ROOT}/${plan.name}.check`);

  const names = [...new Set([...Object.keys(first), ...Object.keys(second)])];
  return names.filter((name) => first[name] !== second[name]).sort();
}

export function publishArtifact(plan: Plan, kubernetes: Kubernetes, revision: string) {
  if (!plan.artifact) {
    throw new Error(`${plan.name}: no artifact declared`);
  }

  const { repo, tag } = plan.artifact;
  const pushed = pushDir(hydrate(plan, kubernetes), repo, { tags: [tag], revision, sign: true });
  console.log(`${pushed.changed ? 'pushed' : 'unchanged'} ${repo}:${tag} ${pushed.digest}`);
}

export function bundleApplications(bundle: Bundle): string {
  const dir = `${ARTIFACT_ROOT}/apps`;
  freshDir(dir);

  for (const { location, name, path } of bundle.applications) {
    fs.mkdir(`${dir}/${location}`);
    fs.write(`${dir}/${location}/${name}.yaml`, fs.readText(path));
  }

  return dir;
}

export function publishBundle(bundle: Bundle, revision: string) {
  const pushed = pushDir(bundleApplications(bundle), bundle.repo, {
    tags: [bundle.tag],
    revision,
    sign: true,
  });
  console.log(
    `${pushed.changed ? 'pushed' : 'unchanged'} ${bundle.repo}:${bundle.tag} ${pushed.digest}`,
  );
}
