import assert from 'node:assert/strict';
import { test } from 'node:test';
import { collect, isName, quote } from './env.ts';

test('names', () => {
  assert.ok(isName('DATABASE_URL'));
  assert.ok(isName('_x1'));
  assert.ok(!isName('1ABC'));
  assert.ok(!isName('client-id'));
  assert.ok(!isName(''));
});

test('quoting survives the shell', () => {
  assert.equal(quote('plain'), "'plain'");
  assert.equal(quote("it's"), `'it'\\''s'`);
  assert.equal(quote('$HOME `x` "y"'), `'$HOME \`x\` "y"'`);
});

test('later paths win and bad names are skipped', () => {
  const vars = collect(
    [
      ['a', { A: '1', B: '1', 'bad-key': 'x' }],
      ['b', { B: '2', N: 3 }],
    ],
    '',
  );

  assert.equal(vars.get('A'), '1');
  assert.equal(vars.get('B'), '2');
  assert.equal(vars.get('N'), '3');
  assert.ok(!vars.has('bad-key'));
});
