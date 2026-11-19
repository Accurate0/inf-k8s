import { capture, dockerLogin, fs, run, signImage } from '@ortzi/host';
import type { App, Target } from '../types';
import { promote } from '../registry';
import { releaseImages } from '../providers/workload';
import { describe } from './promote';
import { buildArgs, localTag, METADATA } from '../docker';

export interface BuildOptions {
  promote: boolean;
  force: boolean;
  platform: string;
}

export function buildApp(app: App, target: Target, options: BuildOptions): number {
  const build = app.build;
  if (!build) {
    throw new Error(`${app.name} has no build block`);
  }
  const dir = `${target.location}/${app.name}`;
  const project = build.image.split('/')[1];

  const sha = capture('git', ['rev-parse', 'HEAD']).trim();
  const dirty = capture('git', ['status', '--porcelain']).trim() !== '';
  const tag = localTag(sha, dirty, new Date());
  const committedAt = dirty
    ? Math.floor(Date.now() / 1000)
    : Number(capture('git', ['show', '-s', '--format=%ct', sha]).trim());
  if (dirty) {
    console.log(`working tree has changes; tagging ${tag}`);
  }

  fs.mkdir('.cache/ortzi-app');
  dockerLogin([project]);
  run('docker', buildArgs(build, dir, tag, committedAt, options.platform));

  const digest = JSON.parse(fs.readText(METADATA))['containerimage.digest'] as string | undefined;
  if (!digest) {
    throw new Error(`docker reported no digest for ${build.image}:${tag}`);
  }
  signImage(`${build.image}@${digest}`);
  console.log(`pushed ${build.image}:${tag} (${digest})`);

  if (!options.promote) {
    return 0;
  }
  if (!releaseImages(app).includes(build.image)) {
    console.log(`${app.name} doesn't run ${build.image} with release: deploy; nothing promoted`);
    return 0;
  }
  console.log(describe(build.image, promote(app.name, build.image, digest, options.force)));
  return 0;
}
