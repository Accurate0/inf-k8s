import { type Flags, args, main, parseArgs } from '@ortzi/host';
import { completionCandidates, formatCompletions } from './completion';

export class UsageError extends Error {}

type Values<F extends Flags> = ReturnType<typeof parseArgs<F>>['values'];

export type Command<C, F extends Flags> = (ctx: C, values: Values<F>, rest: string[]) => number;

export interface CliSpec<C, F extends Flags> {
  usage: string;
  flags: F;
  setup: (values: Values<F>) => C;
  commands: Record<string, Command<C, F>>;
  completers?: Record<string, (ctx: C, position: number) => string[]>;
}

export function cli<C, F extends Flags>(spec: CliSpec<C, F>): { run(): void } {
  return main(() => {
    const argv = args();
    if (argv[0] === '__complete') {
      const { values } = parseArgs([], spec.flags);
      const candidates = completionCandidates(
        { commands: Object.keys(spec.commands), flags: spec.flags, completers: spec.completers },
        argv.slice(1),
        () => spec.setup(values),
      );
      console.log(formatCompletions(candidates));
      return 0;
    }
    if (argv.includes('--help') || argv.includes('-h')) {
      console.log(spec.usage);
      return 0;
    }
    const { values, positionals } = parseArgs(argv, spec.flags);
    const [name, ...rest] = positionals;
    if (!name) {
      console.log(spec.usage);
      return 2;
    }
    const command = Object.hasOwn(spec.commands, name) ? spec.commands[name] : undefined;
    if (!command) {
      console.error(`unknown command ${name}\n\n${spec.usage}`);
      return 2;
    }
    try {
      return command(spec.setup(values), values, rest);
    } catch (e) {
      if (!(e instanceof UsageError)) throw e;
      console.error(e.message);
      return 2;
    }
  });
}

export function applying(yes: boolean): boolean {
  if (!yes) console.log('dry run; pass --yes to apply');
  return yes;
}
