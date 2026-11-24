import { capture } from '@ortzi/host';
import { pathspecs, releaseTag } from '../release-tag';

export function version(paths: string[]): number {
  if (capture('git', ['rev-parse', '--is-shallow-repository']).trim() === 'true') {
    throw new Error('shallow clone; fetch full history (fetch-depth: 0) to count commits');
  }

  const count = Number(
    capture('git', ['rev-list', '--count', 'HEAD', '--', ...pathspecs(paths)]).trim(),
  );
  const sha = capture('git', ['rev-parse', '--short=7', 'HEAD']).trim();

  console.log(releaseTag(count, sha));

  return 0;
}
