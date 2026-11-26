import { Authenticator, parseConfig } from './auth.ts';
import { Forgejo } from './forgejo.ts';
import { Kubernetes, watched } from './kubernetes.ts';
import { resolvers } from './resolvers.ts';
import { executableSchema } from './schema.ts';
import { server } from './server.ts';
import { startTracing } from './tracing.ts';

const env = (name: string, fallback?: string) => {
  const value = process.env[name] ?? fallback;
  if (value === undefined) {
    throw new Error(`${name} is not set`);
  }
  return value;
};

const stopTracing = startTracing(process.env.OTEL_TRACING_URL);

const kubernetes = new Kubernetes();
await kubernetes.watch(watched);

const schema = executableSchema(
  resolvers({
    kubernetes,
    forgejo: new Forgejo(
      env('FORGEJO_URL'),
      env('FORGEJO_TOKEN'),
      env('FORGEJO_PUBLIC_URL', 'https://git.anurag.sh'),
    ),
    buildRepository: env('BUILD_REPOSITORY', 'anurag/k8s'),
  }),
);
const auth = new Authenticator(parseConfig(process.env.SUBGRAPH_AUTH));
const port = Number(env('PORT', '4000'));

const app = server(schema, auth);
await app.listen({ port, host: '0.0.0.0' });
console.log(`listening on :${port}`);

for (const signal of ['SIGINT', 'SIGTERM'] as const) {
  process.on(signal, () => {
    kubernetes.stop();
    void app
      .close()
      .then(stopTracing)
      .finally(() => process.exit(0));
  });
}
