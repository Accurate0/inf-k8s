import { type App, appDatabases } from './render.ts';
import { databasePath } from './providers/cnpg.ts';
import { TERRAFORM_APPS } from './terraform.ts';

const CI_PATHS = [
  '.github/workflows/',
  '.forgejo/workflows/',
  'terraform/',
  'system-components/postgres-cluster/',
];
const INDEX = `${TERRAFORM_APPS}/_index.tf.json`;

export interface PlannedCommit {
  message: string;
  paths: string[];
}

function owner(path: string, databases: Map<string, string>): string | undefined {
  if (path === INDEX) return undefined;
  const match =
    path.match(/^(?:projects|platform-services)\/([^/]+)\//) ??
    path.match(/^\.(?:github|forgejo)\/workflows\/app-(.+)\.yaml$/) ??
    path.match(new RegExp(`^${TERRAFORM_APPS}/(.+)\\.tf\\.json$`));
  return match?.[1] ?? databases.get(path);
}

export function planCommits(paths: string[], apps: App[]): PlannedCommit[] {
  const databases = new Map(
    apps.flatMap((app) =>
      Object.keys(appDatabases(app)).map((database) => [databasePath(database), app.name] as const),
    ),
  );

  const owned = new Map<string, string[]>();
  const shared: string[] = [];
  for (const path of paths) {
    const name = owner(path, databases);
    if (name) owned.set(name, [...(owned.get(name) ?? []), path]);
    else shared.push(path);
  }

  const suffix = paths.some((path) => CI_PATHS.some((prefix) => path.startsWith(prefix)))
    ? ''
    : ' [skip ci]';
  const commits = [...owned.keys()].sort().map((name) => ({
    message: `chore(render): update ${name}${suffix}`,
    paths: owned.get(name) as string[],
  }));

  if (shared.length)
    commits.push({
      message: `chore(render): update shared generated files${suffix}`,
      paths: shared,
    });
  return commits;
}
