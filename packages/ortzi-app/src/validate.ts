import { fs, httpSend, run } from '@ortzi/host';
import { helmTemplates, type Plan } from './plan';

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
  const failures: string[] = [];
  const report = (where: string, before: number) =>
    console.error(`  ${failures.length > before ? '✗' : '✓'} ${where}`);

  let cache: string | undefined;
  const kubeconform = (where: string, paths: string[], stdin?: string, flags: string[] = []) => {
    cache ??= schemaCache();
    const output = run(
      'kubeconform',
      [
        '-strict',
        '-ignore-missing-schemas',
        '-cache',
        cache,
        '-schema-location',
        `${SCHEMA_BASE}/{{.Group}}/{{.ResourceKind}}_{{.ResourceAPIVersion}}.json`,
        '-schema-location',
        'default',
        ...flags,
        ...(stdin === undefined ? paths : ['-']),
      ],
      { stdin, stdout: 'piped', stderr: 'piped', check: false },
    );
    if (!output.ok) {
      failures.push(`${where}\n${output.stdout}${output.stderr}`);
    }
  };

  const apps = new Map<Plan, string[]>();
  const wanted = new Set<Plan>();
  for (const [path, { content, plan }] of generated) {
    apps.set(plan, [...(apps.get(plan) ?? []), content]);
    if (selected(path)) {
      wanted.add(plan);
    }
  }

  const selectedApps = [...apps].filter(([plan]) => wanted.has(plan));

  console.error(`validating ${selectedApps.length} apps`);
  for (const [plan, contents] of selectedApps) {
    const before = failures.length;
    const documents = [...contents];
    for (const { args, values } of helmTemplates(plan)) {
      const output = run('helm', args, {
        stdin: values,
        stdout: 'piped',
        stderr: 'piped',
        check: false,
      });
      if (output.ok) {
        documents.push(output.stdout);
      } else {
        failures.push(`${plan.name}: helm ${args[2]}\n${output.stderr}`);
      }
    }

    kubeconform(plan.name, [], documents.join('\n---\n'));
    report(plan.name, before);
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

  console.error(`validating ${kustomizations.length} kustomizations`);
  for (const dir of kustomizations) {
    const before = failures.length;
    const output = run('kustomize', ['build', dir], {
      stdout: 'piped',
      stderr: 'piped',
      check: false,
    });
    if (!output.ok) {
      failures.push(`${dir}: kustomize\n${output.stderr}`);
      report(dir, before);
      continue;
    }

    const flagsPath = `${dir}/.kubeconform-flags`;
    const flags = fs.exists(flagsPath) ? fs.readText(flagsPath).split(/\s+/).filter(Boolean) : [];
    kubeconform(dir, [], output.stdout, flags);
    report(dir, before);
  }
  return failures;
}
