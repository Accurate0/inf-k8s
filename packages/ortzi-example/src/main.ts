import { args, capture, env, fs, main, parseArgs, run as exec } from '@ortzi/host';

export const run = main(() => {
  const { values, positionals } = parseArgs(args(), {
    help: { type: 'boolean', short: 'h' },
    write: { type: 'string' },
    run: { type: 'string' },
    exit: { type: 'string' },
  });
  if (values.help) {
    console.log('usage: ortzi example [--write FILE] [words...]');
    return;
  }

  if (values.run) {
    exec(values.run);
    return;
  }
  if (values.exit) return Number(values.exit);

  console.log(`hello ${positionals.join(' ') || 'world'} from wasm`);
  const vars = env();
  console.log(`HOME is ${vars.HOME ?? 'hidden'}, ORTZI_EXAMPLE is ${vars.ORTZI_EXAMPLE ?? 'unset'}`);
  console.log(`git says: ${capture('git', ['--version']).trim()}`);
  exec('echo', ['inherited stdout from echo']);

  if (values.write) {
    fs.write(values.write, `written by ortzi example\n`);
    console.log(`wrote ${values.write}: ${fs.readText(values.write).trim()}`);
  }

  return 0;
});
