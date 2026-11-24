import assert from 'node:assert/strict';
import { test } from 'node:test';
import { pathspecs, releaseTag } from './release-tag.ts';
import { RELEASE_TAG } from './flux-objects.ts';

test('translates workflow trigger paths to git pathspecs', () => {
  assert.deepEqual(pathspecs(['projects/foo/**', '!projects/foo/app.yaml']), [
    ':(glob)projects/foo/**',
    ':(exclude,glob)projects/foo/app.yaml',
  ]);
});

test('defaults to the whole repo', () => {
  assert.deepEqual(pathspecs([]), [':(glob).']);
});

test('release tags match the image policy filter', () => {
  const tag = releaseTag(42, 'abc1234');
  assert.equal(tag, 'v1.0.42-abc1234');
  assert.match(tag, new RegExp(RELEASE_TAG.replace('(?P<', '(?<')));
});
