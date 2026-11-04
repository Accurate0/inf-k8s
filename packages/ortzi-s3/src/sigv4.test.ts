import assert from 'node:assert/strict';
import { test } from 'node:test';
import { encodeQuery, hashHex, presign, sign } from './sigv4.ts';

const creds = {
  accessKeyId: 'AKIAIOSFODNN7EXAMPLE',
  secretAccessKey: 'wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY',
  region: 'us-east-1',
};
const now = new Date('2013-05-24T00:00:00Z');

test('signs the AWS GET object example', () => {
  const headers = sign(
    {
      method: 'GET',
      host: 'examplebucket.s3.amazonaws.com',
      path: '/test.txt',
      headers: { Range: 'bytes=0-9' },
      payloadHash: hashHex(''),
    },
    creds,
    now,
  );

  assert.equal(
    headers.authorization,
    'AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41',
  );
  assert.equal(headers.host, undefined);
});

test('presigns the AWS GET object example', () => {
  const query = presign(
    { method: 'GET', host: 'examplebucket.s3.amazonaws.com', path: '/test.txt' },
    creds,
    now,
    86400,
  );

  assert.match(
    query,
    /X-Amz-Signature=aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404$/,
  );
});

test('query encoding sorts and escapes reserved characters', () => {
  assert.equal(
    encodeQuery([
      ['prefix', 'a b/c'],
      ['list-type', '2'],
      ['delimiter', '/'],
    ]),
    'delimiter=%2F&list-type=2&prefix=a%20b%2Fc',
  );
});
