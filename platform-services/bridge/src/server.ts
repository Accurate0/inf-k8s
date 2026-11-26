import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http';
import {
  execute,
  type GraphQLSchema,
  Kind,
  type OperationDefinitionNode,
  parse,
  specifiedRules,
  validate,
} from 'graphql';
import { AuthError, type Authenticator } from './auth.ts';
import { failed, SpanKind, traced, withParent } from './tracing.ts';

export const GRAPHQL_PATH = '/v1/graphql';
const MAX_BODY = 1 << 20;

async function body(request: IncomingMessage): Promise<string> {
  let size = 0;
  const chunks: Buffer[] = [];
  for await (const chunk of request) {
    size += (chunk as Buffer).length;
    if (size > MAX_BODY) {
      throw new AuthError(413, 'BAD_REQUEST', 'request body too large');
    }
    chunks.push(chunk as Buffer);
  }
  return Buffer.concat(chunks).toString('utf8');
}

const errors = (message: string, code: string) =>
  JSON.stringify({ errors: [{ message, extensions: { code } }] });

export function server(schema: GraphQLSchema, auth: Authenticator): Server {
  return createServer((request, response) => {
    if (request.method === 'POST' && request.url?.split('?')[0] === GRAPHQL_PATH) {
      withParent(request.headers, () =>
        traced(
          `POST ${GRAPHQL_PATH}`,
          { 'http.request.method': 'POST', 'url.path': GRAPHQL_PATH },
          () => handle(schema, auth, request, response),
          SpanKind.SERVER,
        ),
      ).catch((error) => console.error(error));
      return;
    }
    void handle(schema, auth, request, response);
  });
}

async function handle(
  schema: GraphQLSchema,
  auth: Authenticator,
  request: IncomingMessage,
  response: ServerResponse,
): Promise<void> {
  {
    const json = (status: number, payload: string) => {
      if (status >= 400) {
        failed(`${status}`);
      }
      response.writeHead(status, { 'content-type': 'application/json' });
      response.end(payload);
    };

    if (request.method === 'GET' && request.url === '/health') {
      response.writeHead(200, { 'content-type': 'text/plain' });
      response.end('ok');
      return;
    }
    if (request.method !== 'POST' || request.url?.split('?')[0] !== GRAPHQL_PATH) {
      json(404, errors('not found', 'NOT_FOUND'));
      return;
    }

    try {
      const principal = await auth.authenticate(request.headers.authorization);
      const { query, variables, operationName } = JSON.parse(await body(request)) as {
        query?: string;
        variables?: Record<string, unknown>;
        operationName?: string;
      };
      if (!query) {
        json(400, errors('missing query', 'BAD_REQUEST'));
        return;
      }

      const document = await traced('graphql.parse', {}, () => parse(query));
      const invalid = await traced('graphql.validate', {}, () =>
        validate(schema, document, specifiedRules),
      );
      if (invalid.length) {
        json(400, JSON.stringify({ errors: invalid.map((error) => error.toJSON()) }));
        return;
      }

      const name =
        operationName ??
        document.definitions.find(
          (definition): definition is OperationDefinitionNode =>
            definition.kind === Kind.OPERATION_DEFINITION,
        )?.name?.value;
      const result = await traced(
        `graphql.${name ?? 'anonymous'}`,
        { 'graphql.operation.name': name ?? '' },
        () =>
          execute({
            schema,
            document,
            variableValues: variables,
            operationName,
            contextValue: { principal },
          }),
      );
      for (const error of result.errors ?? []) {
        if (error.originalError && !error.extensions?.code) {
          console.error(`${error.path?.join('.') ?? 'query'}: ${error.originalError.message}`);
        }
      }
      json(200, JSON.stringify(result));
    } catch (error) {
      if (error instanceof AuthError) {
        json(error.status, errors(error.message, error.code));
        return;
      }
      if (error instanceof SyntaxError) {
        json(400, errors('invalid json body', 'BAD_REQUEST'));
        return;
      }
      console.error(error);
      json(500, errors('internal error', 'INTERNAL_SERVER_ERROR'));
    }
  }
}
