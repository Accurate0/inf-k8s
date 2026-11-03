import assert from 'node:assert/strict';
import { test } from 'node:test';
import { labels, number, summarise } from './prometheus.ts';

test('label sets render like promql', () => {
  assert.equal(labels({}), '{}');
  assert.equal(labels({ __name__: 'up' }), 'up');
  assert.equal(labels({ __name__: 'up', job: 'api' }), 'up{job="api"}');
  assert.equal(labels({ job: 'api', instance: 'a' }), '{instance="a", job="api"}');
});

test('summaries skip NaN', () => {
  const s = summarise([
    [1, '2'],
    [2, 'NaN'],
    [3, '4'],
    [4, '0'],
  ]);

  assert.deepEqual(s, { last: 0, min: 0, max: 4, avg: 2 });
  assert.equal(summarise([]), undefined);
});

test('numbers drop needless decimals', () => {
  assert.equal(number(3), '3');
  assert.equal(number(0.5), '0.5000');
});
