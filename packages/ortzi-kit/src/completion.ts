const NO_FILE_COMPLETION = 4;

export interface CompletionSpec<C> {
  commands: string[];
  flags: Record<string, { type: 'boolean' | 'string' }>;
  completers?: Record<string, (ctx: C, position: number) => string[]>;
}

export function positionalWords(words: string[], flags: CompletionSpec<unknown>['flags']): string[] {
  const positionals: string[] = [];
  for (let i = 0; i < words.length; i++) {
    const word = words[i];
    if (!word.startsWith('-')) {
      positionals.push(word);
    } else if (flags[word.replace(/^--/, '')]?.type === 'string' && !word.includes('=')) {
      i++;
    }
  }
  return positionals;
}

export function completionCandidates<C>(
  spec: CompletionSpec<C>,
  words: string[],
  context: () => C,
): string[] {
  const previous = words.slice(0, -1);
  const current = words.at(-1) ?? '';
  const last = previous.at(-1) ?? '';
  if (last.startsWith('--') && spec.flags[last.slice(2)]?.type === 'string') return [];

  let candidates: string[];
  if (current.startsWith('-')) {
    candidates = Object.keys(spec.flags).map((flag) => `--${flag}`);
  } else {
    const [command, ...rest] = positionalWords(previous, spec.flags);
    if (!command) candidates = spec.commands;
    else candidates = spec.completers?.[command]?.(context(), rest.length) ?? [];
  }
  return candidates.filter((candidate) => candidate.startsWith(current));
}

export const formatCompletions = (candidates: string[]) =>
  [...candidates, `:${NO_FILE_COMPLETION}`].join('\n');
