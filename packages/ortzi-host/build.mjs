#!/usr/bin/env node
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';

import { _setPreopens } from '@bytecodealliance/preview2-shim/filesystem';
import { componentize } from '@bytecodealliance/componentize-js';
import { build } from 'esbuild';

_setPreopens({ '/': '/' });

const { values } = parseArgs({
  options: {
    entry: { type: 'string', default: 'src/main.ts' },
    out: { type: 'string' },
  },
});
if (!values.out) {
  console.error('usage: ortzi-wasm-build --entry src/main.ts --out dist/name.wasm');
  process.exit(2);
}

const work = await mkdtemp(join(tmpdir(), 'ortzi-wasm-'));
try {
  const bundle = join(work, 'bundle.js');
  await build({
    entryPoints: [resolve(values.entry)],
    bundle: true,
    format: 'esm',
    platform: 'neutral',
    target: 'es2022',
    outfile: bundle,
    external: ['ortzi:*', 'wasi:*'],
    logLevel: 'warning',
  });

  const { component } = await componentize({
    sourcePath: bundle,
    witPath: fileURLToPath(new URL('./wit', import.meta.url)),
    worldName: 'cli',
    disableFeatures: ['http', 'fetch-event'],
  });

  const out = resolve(values.out);
  await mkdir(dirname(out), { recursive: true });
  await writeFile(out, component);
  console.error(`built ${out} (${component.byteLength} bytes)`);
} finally {
  await rm(work, { recursive: true, force: true });
}
