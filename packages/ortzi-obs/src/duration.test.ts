import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parseDuration } from './duration.ts';

test('durations', () => {
  assert.equal(parseDuration('30m'), 30 * 60_000);
  assert.equal(parseDuration('1h'), 3_600_000);
  assert.equal(parseDuration('7d'), 7 * 86_400_000);
  assert.throws(() => parseDuration('10'));
  assert.throws(() => parseDuration('5y'));
  assert.throws(() => parseDuration('h'));
});
