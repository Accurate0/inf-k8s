import assert from 'node:assert/strict';
import { test } from 'node:test';
import { body, failed, parseVars } from './graphql.ts';

test('vars parse as JSON when they can', () => {
  assert.deepEqual(parseVars(['id=registry:package:app', 'limit=5', 'on=true', 'eq=a=b']), {
    id: 'registry:package:app',
    limit: 5,
    on: true,
    eq: 'a=b',
  });

  assert.throws(() => parseVars(['novalue']), /expected name=value/);
  assert.throws(() => parseVars(['=x']), /expected name=value/);
});

test('--var overrides --variables', () => {
  assert.deepEqual(body('{ a }', '{"x": 1, "y": 2}', ['y=3'], 'Op'), {
    query: '{ a }',
    variables: { x: 1, y: 3 },
    operationName: 'Op',
  });

  assert.deepEqual(body('{ a }'), { query: '{ a }' });
  assert.throws(() => body('{ a }', '[1]'), /JSON object/);
  assert.throws(() => body('{ a }', 'nope'), /JSON object/);
});

test('errors or a bad status fail the command', () => {
  assert.equal(failed(200, { data: {} }), false);
  assert.equal(failed(200, { data: null, errors: [{ message: 'x' }] }), true);
  assert.equal(failed(403, undefined), true);
});
