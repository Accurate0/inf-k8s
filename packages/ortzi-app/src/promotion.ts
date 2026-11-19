const short = (sha: string) => (/^[0-9a-f]{40}$/.test(sha) ? sha.slice(0, 8) : sha);

const minute = (iso: string) => iso.replace('T', ' ').replace(/:\d\d(\.\d+)?Z$/, '');

export interface Promotion {
  app: string;
  repository: string;
  digest: string;
  sha: string | null;
  committedAt: string | null;
  promotedAt: string;
  liveUntil: string | null;
  actor: string;
  forced: boolean;
}

// The chart renders image:tag, so the digest rides after the sha as tag@digest.
export const releaseTag = (promotion: Promotion) =>
  `${promotion.sha ?? 'deploy'}@${promotion.digest}`;

export function duration(from: string, to: string): string {
  const minutes = Math.max(0, Math.round((Date.parse(to) - Date.parse(from)) / 60_000));
  const days = Math.floor(minutes / 1440);
  const hours = Math.floor((minutes % 1440) / 60);
  if (days) {
    return `${days}d${hours}h`;
  }
  return hours ? `${hours}h${minutes % 60}m` : `${minutes}m`;
}

export interface Row {
  image: string;
  promotion: Promotion;
}

export function releaseRows(history: Map<string, Promotion[]>, now: string): string[][] {
  return rowsOf(history).map(({ image, promotion }, index) => [
    String(index),
    image.split('/').pop() as string,
    short(promotion.sha ?? '-'),
    minute(promotion.promotedAt),
    promotion.liveUntil
      ? duration(promotion.promotedAt, promotion.liveUntil)
      : `live now, ${duration(promotion.promotedAt, now)}`,
    `${promotion.actor}${promotion.forced ? ' (forced)' : ''}`,
  ]);
}

export const rowsOf = (history: Map<string, Promotion[]>): Row[] =>
  [...history]
    .flatMap(([image, list]) => list.map((promotion) => ({ image, promotion })))
    .sort((a, b) => Date.parse(b.promotion.promotedAt) - Date.parse(a.promotion.promotedAt));
