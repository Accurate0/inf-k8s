import { authToken, env, httpSend } from '@ortzi/host';
import { GRAPH_URL, type GraphQLBody, type GraphQLResponse, graphqlFailed } from './graphql';

export interface GraphQLSent<T> {
  status: number;
  raw: string;
  response?: GraphQLResponse<T>;
}

/** Sends a document to the graph as the package's login ($GRAPH_URL overrides the endpoint). */
export function sendGraphQL<T = unknown>(
  body: GraphQLBody,
  options: { url?: string; token?: string } = {},
): GraphQLSent<T> {
  const url = options.url ?? env().GRAPH_URL ?? GRAPH_URL;
  const token = options.token ?? authToken();
  const sent = httpSend({
    method: 'POST',
    url,
    headers: {
      authorization: `Bearer ${token}`,
      'content-type': 'application/json',
      accept: 'application/json',
    },
    body: JSON.stringify(body),
    timeoutMs: 60_000,
  });

  try {
    return { status: sent.status, raw: sent.body, response: JSON.parse(sent.body) };
  } catch {
    return { status: sent.status, raw: sent.body };
  }
}

/** Runs a document and returns its data, failing on any GraphQL error. */
export function graphql<T = unknown>(
  query: string,
  variables?: Record<string, unknown>,
  options: { url?: string; token?: string } = {},
): T {
  const sent = sendGraphQL<T>({ query, ...(variables ? { variables } : {}) }, options);
  if (graphqlFailed(sent.status, sent.response) || sent.response?.data === undefined) {
    const error = sent.response?.errors?.[0];
    const code = error?.extensions?.code ? ` (${error.extensions.code})` : '';
    throw new Error(
      `graph request failed: ${error?.message ?? `${sent.status} ${sent.raw}`}${code}`,
    );
  }

  return sent.response.data;
}
