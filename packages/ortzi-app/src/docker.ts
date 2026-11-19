import type { Build } from './types';

export const METADATA = '.cache/ortzi-app/build-metadata.json';

export function localTag(sha: string, dirty: boolean, now: Date): string {
  if (!dirty) {
    return sha;
  }
  const stamp = now.toISOString().replace(/[-:]/g, '').replace(/\..*$/, '').replace('T', '');
  return `${sha}-dirty-${stamp}`;
}

export function buildArgs(
  build: Build,
  dir: string,
  tag: string,
  committedAt: number,
  platform: string,
): string[] {
  return [
    'buildx',
    'build',
    '--platform',
    platform,
    '--file',
    build.dockerfile ?? `${dir}/Dockerfile`,
    '--tag',
    `${build.image}:${tag}`,
    '--label',
    `net.inf-k8s.commit-sha=${tag}`,
    '--label',
    `net.inf-k8s.committed-at=${committedAt}`,
    ...Object.entries(build.args ?? {}).flatMap(([k, v]) => ['--build-arg', `${k}=${v}`]),
    '--cache-from',
    `type=registry,ref=${build.image}:buildcache`,
    '--metadata-file',
    METADATA,
    '--push',
    build.context ?? dir,
  ];
}
