import assert from 'node:assert/strict';
import { test } from 'node:test';
import type { Promotion } from './promotion.ts';
import { duration, releaseRows } from './promotion.ts';

const promotion = (sha: string, promotedAt: string, liveUntil: string | null, forced = false) =>
  ({
    app: 'spotify',
    repository: 'containers/spotify',
    digest: `sha256:${sha}`,
    sha,
    committedAt: null,
    promotedAt,
    liveUntil,
    actor: 'forgejo:anurag/spotify',
    forced,
  }) satisfies Promotion;

test('durations read in the largest two units', () => {
  assert.equal(duration('2026-11-19T00:00:00Z', '2026-11-19T00:45:00Z'), '45m');
  assert.equal(duration('2026-11-19T00:00:00Z', '2026-11-19T03:12:00Z'), '3h12m');
  assert.equal(duration('2026-11-17T00:00:00Z', '2026-11-19T04:30:00Z'), '2d4h');
});

test('the rollback table lists every image newest first with how long each build was live', () => {
  const history = new Map([
    [
      'registry.anurag.sh/containers/spotify',
      [
        promotion('b'.repeat(40), '2026-11-19T06:25:00.123Z', null, true),
        promotion('a'.repeat(40), '2026-11-18T06:26:00Z', '2026-11-19T06:25:00.123Z'),
      ],
    ],
    [
      'registry.anurag.sh/containers/spotify-web',
      [promotion('c'.repeat(40), '2026-11-18T13:04:00Z', null)],
    ],
  ]);

  assert.deepEqual(releaseRows(history, '2026-11-19T09:37:00Z'), [
    [
      '0',
      'spotify',
      'bbbbbbbb',
      '2026-11-19 06:25',
      'live now, 3h12m',
      'forgejo:anurag/spotify (forced)',
    ],
    [
      '1',
      'spotify-web',
      'cccccccc',
      '2026-11-18 13:04',
      'live now, 20h33m',
      'forgejo:anurag/spotify',
    ],
    ['2', 'spotify', 'aaaaaaaa', '2026-11-18 06:26', '23h59m', 'forgejo:anurag/spotify'],
  ]);
});
