import assert from 'node:assert/strict';
import { test } from 'node:test';
import { blocks, escape, text } from './xml.ts';

const listing = `<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
  <Name>b</Name><IsTruncated>true</IsTruncated>
  <NextContinuationToken>tok</NextContinuationToken>
  <Contents><Key>a &amp; b.txt</Key><Size>3</Size></Contents>
  <Contents><Key>c.txt</Key><Size>10</Size></Contents>
  <CommonPrefixes><Prefix>dir/</Prefix></CommonPrefixes>
</ListBucketResult>`;

test('reads repeated blocks and unescapes text', () => {
  const contents = blocks(listing, 'Contents');
  assert.equal(contents.length, 2);
  assert.equal(text(contents[0], 'Key'), 'a & b.txt');
  assert.equal(text(listing, 'NextContinuationToken'), 'tok');
  assert.equal(text(blocks(listing, 'CommonPrefixes')[0], 'Prefix'), 'dir/');
  assert.equal(text(listing, 'Missing'), undefined);
});

test('escapes keys for request bodies', () => {
  assert.equal(escape('a<b>&c'), 'a&lt;b&gt;&amp;c');
});
