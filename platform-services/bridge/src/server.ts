import Fastify, { type FastifyInstance, type FastifyReply } from 'fastify';
import {
  execute,
  type GraphQLSchema,
  Kind,
  type OperationDefinitionNode,
  parse,
  specifiedRules,
  validate,
} from 'graphql';
import { AuthError, type Authenticator, PASSPORT_HEADER } from './auth.ts';
import { socket } from './socket.ts';
import { failed, SpanKind, traced, withParent } from './tracing.ts';

export const GRAPHQL_PATH = '/v1/graphql';

interface GraphQLBody {
  query?: string;
  variables?: Record<string, unknown>;
  operationName?: string;
}

const errors = (message: string, code: string) => ({ errors: [{ message, extensions: { code } }] });

function send(reply: FastifyReply, status: number, payload: unknown): FastifyReply {
  if (status >= 400) {
    failed(`${status}`);
  }
  return reply.code(status).type('application/json').send(payload);
}

export function server(schema: GraphQLSchema, auth: Authenticator): FastifyInstance {
  const app = Fastify({ bodyLimit: 1 << 20 });

  app.setErrorHandler((error: Error & { statusCode?: number }, _request, reply) => {
    if (error.statusCode === 413) {
      return send(reply, 413, errors('request body too large', 'BAD_REQUEST'));
    }
    if (error.statusCode && error.statusCode < 500) {
      return send(reply, error.statusCode, errors(error.message, 'BAD_REQUEST'));
    }
    console.error(error);
    return send(reply, 500, errors('internal error', 'INTERNAL_SERVER_ERROR'));
  });

  app.setNotFoundHandler((_request, reply) => send(reply, 404, errors('not found', 'NOT_FOUND')));

  app.get('/health', (_request, reply) => reply.type('text/plain').send('ok'));

  app.post<{ Body: GraphQLBody }>(GRAPHQL_PATH, (request, reply) =>
    withParent(request.headers, () =>
      traced(
        `POST ${GRAPHQL_PATH}`,
        { 'http.request.method': 'POST', 'url.path': GRAPHQL_PATH },
        async () => {
          let principal;
          try {
            const passport = request.headers[PASSPORT_HEADER];
            principal = await auth.authenticate(
              typeof passport === 'string' ? passport : undefined,
            );
          } catch (error) {
            if (error instanceof AuthError) {
              return send(reply, error.status, errors(error.message, error.code));
            }
            throw error;
          }

          const { query, variables, operationName } = request.body ?? {};
          if (!query) {
            return send(reply, 400, errors('missing query', 'BAD_REQUEST'));
          }

          let document;
          try {
            document = await traced('graphql.parse', {}, () => parse(query));
          } catch (error) {
            return send(reply, 400, errors((error as Error).message, 'GRAPHQL_PARSE_FAILED'));
          }
          const invalid = await traced('graphql.validate', {}, () =>
            validate(schema, document, specifiedRules),
          );
          if (invalid.length) {
            return send(reply, 400, { errors: invalid.map((error) => error.toJSON()) });
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
          return send(reply, 200, result);
        },
        SpanKind.SERVER,
      ),
    ),
  );

  socket(app, schema, auth);
  return app;
}
