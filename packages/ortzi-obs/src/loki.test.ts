import assert from 'node:assert/strict';
import { test } from 'node:test';
import { buildQuery, flatten, source } from './loki.ts';

test('builds selectors from flags', () => {
  assert.equal(
    buildQuery({ ns: 'waf-manager', grep: 'say "hi"' }),
    String.raw`{namespace="waf-manager"} |= "say \"hi\""`,
  );
  assert.equal(
    buildQuery({ ns: 'waf-manager', app: 'api', pod: 'api-7f', grep: 'say "hi"' }),
    String.raw`{namespace="waf-manager",app="api",pod=~"api-7f.*"} |= "say \"hi\""`,
  );
});

test('explicit query wins and empty is rejected', () => {
  assert.throws(() => buildQuery({}));
  assert.equal(buildQuery({ query: '{job="x"}', ns: 'ignored' }), '{job="x"}');
});

test('streams flatten oldest first', () => {
  const entries = flatten({
    data: {
      result: [
        {
          stream: { namespace: 'a' },
          values: [
            ['30', 'c'],
            ['10', 'a'],
          ],
        },
        { stream: { namespace: 'b' }, values: [['20', 'b']] },
      ],
    },
  });

  assert.deepEqual(
    entries.map((e) => e.line),
    ['a', 'b', 'c'],
  );
});

test('keeps nanosecond precision', () => {
  const [entry] = flatten({
    data: { result: [{ stream: {}, values: [['1762156800123456789', 'x']] }] },
  });

  assert.equal(entry.ts, 1762156800123456789n);
});

test('labels the source', () => {
  assert.equal(source({ namespace: 'a', pod: 'p' }), 'a/p');
  assert.equal(source({ namespace: 'a' }), 'a');
  assert.equal(source({ job: 'j' }), 'j');
  assert.equal(source({}), '-');
});
