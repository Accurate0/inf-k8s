import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { schemaJson } from './schema.ts';

test('app.schema.json is generated from the providers, run pnpm schema', () => {
  const committed = readFileSync(new URL('../app.schema.json', import.meta.url), 'utf8');
  assert.equal(committed, schemaJson());
});
