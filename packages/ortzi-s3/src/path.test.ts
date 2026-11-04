import assert from 'node:assert/strict';
import { test } from 'node:test';
import { basename, join, parseDuration, parseRemote, size } from './path.ts';

test('parses s3 locations with and without the scheme', () => {
  assert.deepEqual(parseRemote('s3://b/a/c.txt'), { bucket: 'b', key: 'a/c.txt' });
  assert.deepEqual(parseRemote('b'), { bucket: 'b', key: '' });
  assert.deepEqual(parseRemote('s3://b/'), { bucket: 'b', key: '' });
  assert.throws(() => parseRemote('s3://'));
});

test('joins and splits paths', () => {
  assert.equal(basename('a/b/c.txt'), 'c.txt');
  assert.equal(basename('a/b/'), 'b');
  assert.equal(join('dir/', 'x'), 'dir/x');
  assert.equal(join('dir', 'x'), 'dir/x');
  assert.equal(join('', 'x'), 'x');
});

test('parses durations and formats sizes', () => {
  assert.equal(parseDuration('1h'), 3600);
  assert.equal(parseDuration('7d'), 604800);
  assert.throws(() => parseDuration('1w'));
  assert.equal(size(512), '512 B');
  assert.equal(size(1536), '1.5 KiB');
});
