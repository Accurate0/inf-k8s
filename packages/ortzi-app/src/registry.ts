import { authToken, env, httpSend } from '@ortzi/host';
import { RELEASE_REGISTRY } from './providers/workload.ts';
import type { Promotion } from './promotion.ts';

export { type Promotion, releaseTag } from './promotion.ts';

const GRAPH_URL = 'https://graph.ortzi.cloud/v1/graphql';

export interface Promoted {
  promotion: Promotion;
  previous: string | null;
  changed: boolean;
}

const PROMOTION = 'app repository digest sha committedAt promotedAt liveUntil actor forced';

function graphql<T>(query: string, variables: Record<string, unknown>): T {
  const response = httpSend({
    method: 'POST',
    url: env().GRAPH_URL ?? GRAPH_URL,
    headers: {
      authorization: `Bearer ${authToken()}`,
      'content-type': 'application/json',
      accept: 'application/json',
    },
    body: JSON.stringify({ query, variables }),
    timeoutMs: 60_000,
  });

  let parsed: { data?: T; errors?: { message: string }[] };
  try {
    parsed = JSON.parse(response.body);
  } catch {
    throw new Error(`graph answered ${response.status}: ${response.body.slice(0, 200)}`);
  }
  if (parsed.errors?.length) {
    throw new Error(parsed.errors.map((e) => e.message).join('; '));
  }
  if (!parsed.data) {
    throw new Error(`graph answered ${response.status} without data`);
  }
  return parsed.data;
}

export function harborRepository(image: string): string {
  if (!image.startsWith(RELEASE_REGISTRY)) {
    throw new Error(`${image} is not in ${RELEASE_REGISTRY}`);
  }
  return image.slice(RELEASE_REGISTRY.length);
}

export function promotions(images: string[], limit: number): Map<string, Promotion[]> {
  const byRepository = new Map(images.map((image) => [harborRepository(image), image]));
  const found = new Map<string, Promotion[]>(images.map((image) => [image, []]));
  if (!images.length) {
    return found;
  }

  const { registryPromotions } = graphql<{ registryPromotions: Promotion[] }>(
    `query($repositories: [String!]!, $limit: Int!) {
      registryPromotions(repositories: $repositories, limit: $limit) { ${PROMOTION} }
    }`,
    { repositories: [...byRepository.keys()], limit },
  );
  for (const promotion of registryPromotions) {
    const image = byRepository.get(promotion.repository);
    if (image) {
      found.get(image)?.push(promotion);
    }
  }
  return found;
}

export function promote(app: string, image: string, reference: string, force: boolean): Promoted {
  const { registryPromote } = graphql<{ registryPromote: Promoted }>(
    `mutation($input: RegistryPromoteInput!) {
      registryPromote(input: $input) { changed previous promotion { ${PROMOTION} } }
    }`,
    { input: { app, repository: harborRepository(image), reference, force } },
  );
  return registryPromote;
}
