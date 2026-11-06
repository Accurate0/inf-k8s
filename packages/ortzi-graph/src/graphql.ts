export interface Body {
  query: string;
  variables?: Record<string, unknown>;
  operationName?: string;
}

export interface Response {
  data?: unknown;
  errors?: { message: string }[];
}

/** `--var name=value` takes JSON when the value parses as JSON, the raw string otherwise. */
export function parseVars(pairs: string[]): Record<string, unknown> {
  return Object.fromEntries(
    pairs.map((pair) => {
      const at = pair.indexOf('=');
      if (at <= 0) throw new Error(`--var ${pair}: expected name=value`);

      const raw = pair.slice(at + 1);
      let value: unknown = raw;
      try {
        value = JSON.parse(raw);
      } catch {}

      return [pair.slice(0, at), value];
    }),
  );
}

export function body(
  query: string,
  variables?: string,
  vars: string[] = [],
  operation?: string,
): Body {
  let parsed: Record<string, unknown> = {};
  if (variables !== undefined) {
    try {
      parsed = JSON.parse(variables);
    } catch {
      throw new Error('--variables must be a JSON object');
    }
    if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
      throw new Error('--variables must be a JSON object');
    }
  }

  const merged = { ...parsed, ...parseVars(vars) };

  return {
    query,
    ...(Object.keys(merged).length ? { variables: merged } : {}),
    ...(operation ? { operationName: operation } : {}),
  };
}

export const failed = (status: number, response: Response | undefined) =>
  status < 200 || status >= 300 || !!response?.errors?.length;
