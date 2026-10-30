import assert from 'node:assert/strict';
import { test } from 'node:test';
import { age, columns, minuteTimestamp, parseMinutes, short } from './format.ts';

test('aligns columns', () => {
  assert.equal(
    columns(['NAME', 'AGE'], [['api', '5h'], ['home-gateway', '3d']]),
    'NAME          AGE\napi           5h\nhome-gateway  3d',
  );
});

test('formats ages', () => {
  const start = Date.parse('2026-01-01T00:00:00Z');
  assert.equal(age('2026-01-01T00:00:00Z', start + 5 * 60_000), '5m');
  assert.equal(age('2026-01-01T00:00:00Z', start + 5 * 3_600_000), '5h');
  assert.equal(age('2026-01-01T00:00:00Z', start + 48 * 3_600_000), '2d');
});

test('shortens only full shas', () => {
  assert.equal(short('a'.repeat(40)), 'a'.repeat(8));
  assert.equal(short('v1.2.3'), 'v1.2.3');
});

test('trims timestamps to the minute', () => {
  assert.equal(minuteTimestamp('2026-10-29T16:06:20Z'), '2026-10-29 16:06');
});

test('parses minutes', () => {
  assert.equal(parseMinutes(undefined, 5), 5);
  assert.equal(parseMinutes('2.5', 5), 2.5);
  assert.throws(() => parseMinutes('soon', 5));
  assert.throws(() => parseMinutes('0', 5));
});
