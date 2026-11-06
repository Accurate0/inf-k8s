import assert from 'node:assert/strict';
import { test } from 'node:test';
import { isTraceId, ms, spans, tree } from './tempo.ts';

const span = (spanId: string, name: string, start: number, end: number, parentSpanId?: string) => ({
  spanId,
  ...(parentSpanId ? { parentSpanId } : {}),
  name,
  startTimeUnixNano: String(1_000_000_000n + BigInt(start) * 1_000_000n),
  endTimeUnixNano: String(1_000_000_000n + BigInt(end) * 1_000_000n),
});

const response = {
  batches: [
    {
      resource: { attributes: [{ key: 'service.name', value: { stringValue: 'graph-router' } }] },
      scopeSpans: [{ spans: [span('a', 'query', 0, 40)] }],
    },
    {
      resource: { attributes: [{ key: 'service.name', value: { stringValue: 'registry' } }] },
      scopeSpans: [
        {
          spans: [
            {
              ...span('c', 'graphql.registryPackages', 7, 20, 'b'),
              status: { code: 2, message: 'boom' },
            },
            span('b', 'POST /v1/graphql', 5, 30, 'a'),
          ],
        },
      ],
    },
  ],
};

test('trace ids are hex', () => {
  assert.ok(isTraceId('6d0ecb1679533e3bbb5383e51be15424'));
  assert.ok(!isTraceId('{ resource.service.name = "registry" }'));
});

test('spans flatten across services, oldest first', () => {
  const all = spans(response);
  assert.deepEqual(
    all.map((s) => [s.service, s.name]),
    [
      ['graph-router', 'query'],
      ['registry', 'POST /v1/graphql'],
      ['registry', 'graphql.registryPackages'],
    ],
  );
  assert.equal(all[2]!.error, 'boom');
});

test('v2 responses nest resourceSpans under trace', () => {
  assert.equal(spans({ trace: { resourceSpans: response.batches } }).length, 3);
});

test('tree indents children under their parent', () => {
  assert.deepEqual(tree(spans(response)), [
    '   +0.00ms    40.0ms  graph-router: query',
    '   +5.00ms    25.0ms    registry: POST /v1/graphql',
    '   +7.00ms    13.0ms      registry: graphql.registryPackages  ERROR boom',
  ]);
});

test('spans with a missing parent become roots', () => {
  const [, orphan] = spans({
    batches: [{ scopeSpans: [{ spans: [span('a', 'x', 0, 1), span('b', 'y', 2, 3, 'gone')] }] }],
  });
  assert.equal(orphan!.parent, 'gone');
  assert.deepEqual(
    tree(spans({ batches: [{ scopeSpans: [{ spans: [span('b', 'y', 2, 3, 'gone')] }] }] })),
    ['   +0.00ms    1.00ms  -: y'],
  );
});

test('durations', () => {
  assert.equal(ms(1_500_000n), '1.50ms');
  assert.equal(ms(25_000_000n), '25.0ms');
  assert.equal(ms(2_345_000_000n), '2.35s');
});
