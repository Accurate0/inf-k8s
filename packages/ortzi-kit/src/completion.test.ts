import assert from 'node:assert/strict';
import { test } from 'node:test';
import { completionCandidates, formatCompletions, positionalWords } from './completion.ts';

const spec = {
  commands: ['status', 'rollback'],
  flags: { context: { type: 'string' as const }, yes: { type: 'boolean' as const } },
  completers: { rollback: (apps: string[], position: number) => (position === 0 ? apps : []) },
};
const complete = (...words: string[]) => completionCandidates(spec, words, () => ['api', 'web']);

test('completes commands and filters by prefix', () => {
  assert.deepEqual(complete(''), ['status', 'rollback']);
  assert.deepEqual(complete('ro'), ['rollback']);
});

test('completes flags', () => {
  assert.deepEqual(complete('--'), ['--context', '--yes']);
  assert.deepEqual(complete('rollback', '--y'), ['--yes']);
});

test('uses a command completer by position', () => {
  assert.deepEqual(complete('rollback', ''), ['api', 'web']);
  assert.deepEqual(complete('rollback', 'w'), ['web']);
  assert.deepEqual(complete('rollback', 'api', ''), []);
  assert.deepEqual(complete('status', ''), []);
});

test('skips flags and their values when finding positionals', () => {
  assert.deepEqual(positionalWords(['--context', 'prod', 'rollback', '--yes', 'api'], spec.flags), [
    'rollback',
    'api',
  ]);
  assert.deepEqual(complete('--context', 'prod', 'rollback', ''), ['api', 'web']);
});

test('does not complete a string flag value', () => {
  assert.deepEqual(complete('--context', ''), []);
});

test('formats output for the cobra protocol', () => {
  assert.equal(formatCompletions(['a', 'b']), 'a\nb\n:4');
});
