import assert from 'node:assert/strict';
import { test } from 'node:test';
import { collect, issuer, isName, namespaceOf, quote } from './env.ts';

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

test('issuer reads the unverified iss claim', () => {
  const payload = btoa(JSON.stringify({ iss: 'https://git.anurag.sh/api/actions', sub: '?>?' }))
    .replace(/\+/g, '-')
    .replace(/\//g, '_')
    .replace(/=+$/, '');

  assert.equal(issuer(`e30.${payload}.sig`), 'https://git.anurag.sh/api/actions');
  assert.equal(issuer('not-a-jwt'), undefined);
});

test('namespace is the first path segment', () => {
  assert.equal(namespaceOf('k8s-schemas/resource-schemas'), 'k8s-schemas');
  assert.equal(namespaceOf('/forgejo'), 'forgejo');
});
