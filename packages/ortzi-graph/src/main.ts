import { authToken, env, fs, readStdin, serve, stdinIsTerminal } from '@ortzi/host';
import { GRAPH_URL, UsageError, cli, graphqlBody, graphqlFailed, sendGraphQL } from '@ortzi/kit';
import { labPage } from './lab';

const LAB_PORT = 4280;

const USAGE = `usage: ortzi graph <command> [flags]

commands:
  query [DOCUMENT]         run a query or mutation against the graph
  token                    print your access token, e.g. for curl or another GraphQL client
  lab                      run GraphiQL on localhost, signed in as you

flags:
  --url URL                GraphQL endpoint (default: $GRAPH_URL or ${GRAPH_URL})

query:
  --file FILE, -f FILE     read the document from a file instead of the argument or stdin
  --var NAME=VALUE         set a variable; VALUE is parsed as JSON when it can be (repeatable)
  --variables JSON         set variables from a JSON object
  --operation NAME         operation to run when the document has several

lab:
  --port PORT              port to serve on, 4280-4283 (default: ${LAB_PORT})

log in with \`ortzi login graph\``;

function readDocument(argument: string | undefined, file: string | undefined): string {
  if (file !== undefined) {
    return fs.readText(file);
  }
  if (argument !== undefined) {
    return argument;
  }
  if (!stdinIsTerminal()) {
    return new TextDecoder().decode(readStdin());
  }

  throw new UsageError(`missing DOCUMENT\n\n${USAGE}`);
}

export const run = cli({
  usage: USAGE,
  flags: {
    url: { type: 'string' },
    file: { type: 'string', short: 'f' },
    var: { type: 'string', multiple: true },
    variables: { type: 'string' },
    operation: { type: 'string' },
    port: { type: 'string' },
  },
  setup: (values) => {
    const vars = env();
    return { url: values.url ?? vars.GRAPH_URL ?? GRAPH_URL, token: vars.GRAPH_TOKEN };
  },
  commands: {
    query: (ctx, values, [argument]) => {
      const document = readDocument(argument, values.file).trim();
      if (!document) {
        throw new UsageError('the document is empty');
      }

      const body = graphqlBody(document, values.variables, values.var, values.operation);
      const sent = sendGraphQL(body, { url: ctx.url, token: ctx.token ?? authToken() });
      console.log(sent.response ? JSON.stringify(sent.response, null, 2) : sent.raw);

      return graphqlFailed(sent.status, sent.response) ? 1 : 0;
    },

    token: () => {
      console.log(authToken());
      return 0;
    },

    lab: (ctx, values) => {
      const port = values.port === undefined ? LAB_PORT : Number(values.port);
      if (!Number.isInteger(port) || port <= 0 || port > 65535) {
        throw new UsageError(`invalid --port ${values.port}`);
      }

      console.error(`lab for ${ctx.url} on http://localhost:${port}, using your graph login`);
      console.error('stop with ctrl-c');

      serve.proxy(port, ctx.url, labPage(ctx.url));
      return 0;
    },
  },
});
