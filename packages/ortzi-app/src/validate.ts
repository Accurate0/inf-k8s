import { fs, httpSend, pullConfig, type Run, runAll } from '@ortzi/host';
import { harborProjects, helmTemplates, type Plan } from './plan';

export interface RenderedOutput {
  content: string;
  plan: Plan;
}

const SCHEMA_BASE = 'https://k8s-schemas.anurag.sh';
const CACHE_ROOT = '.cache/kubeconform';
const MANIFEST_ROOTS = ['platform-services', 'projects', 'selfhosted', 'kubernetes'];
const SKIPPED_DIRS = new Set(['node_modules', 'target', 'dist', '.git']);

function schemaVersion(): string | undefined {
  try {
    const response = httpSend({ url: `${SCHEMA_BASE}/version.json`, timeoutMs: 5000 });
    return response.status === 200 ? response.body.match(/[0-9a-f]{64}/)?.[0] : undefined;
  } catch {
    return undefined;
  }
}

function schemaCache(): string {
  const version = schemaVersion();

  fs.mkdir(CACHE_ROOT);
  const cached = fs
    .readDir(CACHE_ROOT)
    .filter((entry) => entry.dir)
    .map((entry) => entry.name);
  const current = version ?? cached[0] ?? 'unversioned';
  for (const old of cached) {
    if (old !== current) {
      fs.remove(`${CACHE_ROOT}/${old}`);
    }
  }

  fs.mkdir(`${CACHE_ROOT}/${current}`);
  return `${CACHE_ROOT}/${current}`;
}

function walk(dir: string, visit: (dir: string, file: string) => void) {
  for (const entry of fs.readDir(dir)) {
    if (!entry.dir) {
      visit(dir, entry.name);
    } else if (!SKIPPED_DIRS.has(entry.name)) {
      walk(`${dir}/${entry.name}`, visit);
    }
  }
}

export function validateManifests(
  generated: Map<string, RenderedOutput>,
  selected: (path: string) => boolean = () => true,
): string[] {
  const piped = { stdout: 'piped', stderr: 'piped', check: false } as const;

  const apps = new Map<Plan, string[]>();
  const wanted = new Set<Plan>();
  for (const [path, { content, plan }] of generated) {
    apps.set(plan, [...(apps.get(plan) ?? []), content]);
    if (selected(path)) {
      wanted.add(plan);
    }
  }

  const selectedApps = [...apps]
    .filter(([plan]) => wanted.has(plan))
    .map(([plan, contents]) => ({ plan, contents, templates: helmTemplates(plan) }));

  const projects = harborProjects(selectedApps.map(({ plan }) => plan));
  let helmEnv: Record<string, string> = {};
  if (projects.length) {
    try {
      helmEnv = { HELM_REGISTRY_CONFIG: pullConfig(projects) };
    } catch (error) {
      return [`registry: could not get pull access to ${projects.join(', ')}: ${error}`];
    }
  }

  const kustomizations: string[] = [];
  for (const root of MANIFEST_ROOTS) {
    walk(root, (dir, file) => {
      if (!selected(dir)) {
        return;
      }
      if (file === 'kustomization.yaml' || file === 'kustomization.yml') {
        kustomizations.push(dir);
      }
    });
  }

  const builds = runAll([
    ...selectedApps.flatMap(({ templates }) =>
      templates.map(({ args, values }) => ({
        program: 'helm',
        args,
        options: { ...piped, stdin: values, env: helmEnv },
      })),
    ),
    ...kustomizations.map((dir) => ({
      program: 'kustomize',
      args: ['build', dir],
      options: piped,
    })),
  ]);

  const cache = selectedApps.length || kustomizations.length ? schemaCache() : '';
  const kubeconform = (stdin: string, flags: string[] = []): Run => ({
    program: 'kubeconform',
    args: [
      '-strict',
      '-ignore-missing-schemas',
      '-cache',
      cache,
      '-schema-location',
      `${SCHEMA_BASE}/{{.Group}}/{{.ResourceKind}}_{{.ResourceAPIVersion}}.json`,
      '-schema-location',
      'default',
      ...flags,
      '-',
    ],
    options: { ...piped, stdin },
  });

  interface Check {
    where: string;
    failures: string[];
    validate?: Run;
  }

  let next = 0;
  const appChecks = selectedApps.map(({ plan, contents, templates }): Check => {
    const failures: string[] = [];
    const documents = [...contents];
    for (const { args } of templates) {
      const output = builds[next++]!;
      if (output.ok) {
        documents.push(output.stdout);
      } else {
        failures.push(`${plan.name}: helm ${args[2]}\n${output.stderr}`);
      }
    }
    return { where: plan.name, failures, validate: kubeconform(documents.join('\n---\n')) };
  });

  const kustomizeChecks = kustomizations.map((dir): Check => {
    const output = builds[next++]!;
    if (!output.ok) {
      return { where: dir, failures: [`${dir}: kustomize\n${output.stderr}`] };
    }

    const flagsPath = `${dir}/.kubeconform-flags`;
    const flags = fs.exists(flagsPath) ? fs.readText(flagsPath).split(/\s+/).filter(Boolean) : [];
    return { where: dir, failures: [], validate: kubeconform(output.stdout, flags) };
  });

  const checks = [...appChecks, ...kustomizeChecks];
  const validations = checks.flatMap(({ validate }) => (validate ? [validate] : []));
  if (cache && !fs.readDir(cache).length) {
    const all = validations.map(({ options }) => options?.stdin).join('\n---\n');
    runAll([kubeconform(all)]);
  }
  const results = runAll(validations);
  let result = 0;
  for (const check of checks) {
    if (!check.validate) {
      continue;
    }
    const output = results[result++]!;
    if (!output.ok) {
      check.failures.push(`${check.where}\n${output.stdout}${output.stderr}`);
    }
  }

  const report = (heading: string, group: Check[]) => {
    console.error(heading);
    for (const { where, failures } of group) {
      console.error(`  ${failures.length ? '✗' : '✓'} ${where}`);
    }
  };
  report(`validating ${appChecks.length} apps`, appChecks);
  report(`validating ${kustomizeChecks.length} kustomizations`, kustomizeChecks);

  return checks.flatMap(({ failures }) => failures);
}
