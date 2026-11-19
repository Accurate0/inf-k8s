import { parseImage, short } from '@ortzi/kit';
import { type Promoted, promote as promoteImage } from '../registry';

export interface PromoteOptions {
  images: string[];
  sha?: string;
  force: boolean;
}

function reference(image: string): { image: string; reference: string } {
  const at = image.indexOf('@');
  if (at >= 0) {
    return { image: image.slice(0, at), reference: image.slice(at + 1) };
  }
  const { repository, tag } = parseImage(image);
  if (tag === 'latest' && !image.endsWith(':latest')) {
    throw new Error(`${image}: pass IMAGE@DIGEST or IMAGE:SHA`);
  }
  return { image: repository, reference: tag };
}

export function describe(image: string, { changed, previous, promotion }: Promoted): string {
  const now = `${short(promotion.sha ?? '-')} (${promotion.digest.slice(7, 19)})`;
  if (!changed) {
    return `${image}: ${now} is already deployed`;
  }
  return `${image}: ${previous ? previous.slice(7, 19) : 'nothing'} -> ${now}`;
}

export function promote(app: string, released: string[], options: PromoteOptions): number {
  const targets = options.images.length
    ? options.images.map(reference)
    : released.map((image) => {
        if (!options.sha) {
          throw new Error(`pass --sha to promote every released image of ${app}, or --image`);
        }
        return { image, reference: options.sha };
      });
  if (!targets.length) {
    throw new Error(`${app} has no workloads with release: deploy; pass --image`);
  }

  for (const { image, reference } of targets) {
    console.log(describe(image, promoteImage(app, image, reference, options.force)));
  }
  return 0;
}
