import { Authenticator, parseConfig } from './auth.ts';
import { Forgejo } from './forgejo.ts';
import { Kubernetes } from './kubernetes.ts';
import { resolvers } from './resolvers.ts';
import { executableSchema } from './schema.ts';
import { server } from './server.ts';

const env = (name: string, fallback?: string) => {
  const value = process.env[name] ?? fallback;
  if (value === undefined) {
    throw new Error(`${name} is not set`);
  }
  return value;
};

const schema = executableSchema(
  resolvers({
    kubernetes: new Kubernetes(),
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

const http = server(schema, auth).listen(port, () => console.log(`listening on :${port}`));
for (const signal of ['SIGINT', 'SIGTERM'] as const) {
  process.on(signal, () => http.close(() => process.exit(0)));
}
