import type { FastifyInstance } from 'fastify';
import { execute, type GraphQLSchema, subscribe, validate } from 'graphql';
import { useServer } from 'graphql-ws/use/ws';
import { WebSocketServer } from 'ws';
import { AuthError, type Authenticator, PASSPORT_HEADER, type Principal } from './auth.ts';

export const WEBSOCKET_PATH = '/v1/graphql/ws';

// graphql-transport-ws as the router speaks it: the caller's headers arrive as the connection_init payload
export function socket(app: FastifyInstance, schema: GraphQLSchema, auth: Authenticator): void {
  const wss = new WebSocketServer({ server: app.server, path: WEBSOCKET_PATH });
  const principals = new WeakMap<object, Principal>();

  const server = useServer(
    {
      schema,
      execute,
      subscribe,
      validate,
      onConnect: async (ctx) => {
        // the router sends its fetch headers, the passport among them, as connectionParams
        const params = ctx.connectionParams ?? {};
        const passport = Object.entries(params).find(
          ([key]) => key.toLowerCase() === PASSPORT_HEADER,
        )?.[1];
        try {
          principals.set(
            ctx,
            await auth.authenticate(typeof passport === 'string' ? passport : undefined),
          );
          return true;
        } catch (error) {
          if (error instanceof AuthError) {
            return false;
          }
          throw error;
        }
      },
      context: (ctx) => ({ principal: principals.get(ctx) }),
    },
    wss,
  );

  app.addHook('onClose', async () => {
    await server.dispose();
    await new Promise<void>((resolve) => wss.close(() => resolve()));
  });
}
