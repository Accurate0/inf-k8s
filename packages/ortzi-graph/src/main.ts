import { authToken, env, fs, httpSend, readStdin, serve, stdinIsTerminal } from '@ortzi/host';
import { UsageError, cli } from '@ortzi/kit';
import * as graphql from './graphql';
import { LAB_PORT, type Upstream, handle } from './lab';

const DEFAULT_URL = 'https://graph.ortzi.cloud/v1/graphql';

const USAGE = `usage: ortzi graph <command> [flags]

commands:
  query [DOCUMENT]         run a query or mutation against the graph
  token                    print your access token, e.g. for curl or another GraphQL client
  lab                      run the Hive lab on localhost, signed in as you (no subscriptions)

flags:
  --url URL                GraphQL endpoint (default: $GRAPH_URL or ${DEFAULT_URL})

query:
  --file FILE, -f FILE     read the document from a file instead of the argument or stdin
  --var NAME=VALUE         set a variable; VALUE is parsed as JSON when it can be (repeatable)
  --variables JSON         set variables from a JSON object
  --operation NAME         operation to run when the document has several

lab:
  --port PORT              port to serve on (default: ${LAB_PORT})

log in with \`ortzi login graph\``;

function readDocument(argument: string | undefined, file: string | undefined): string {
  if (file !== undefined) return fs.readText(file);
  if (argument !== undefined) return argument;
  if (!stdinIsTerminal()) return new TextDecoder().decode(readStdin());

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
    return { url: values.url ?? vars.GRAPH_URL ?? DEFAULT_URL, token: vars.GRAPH_TOKEN };
  },
  commands: {
    query: (ctx, values, [argument]) => {
      const document = readDocument(argument, values.file).trim();
      if (!document) throw new UsageError('the document is empty');

      const body = graphql.body(document, values.variables, values.var, values.operation);
      const token = ctx.token ?? authToken();

      const response = httpSend({
        method: 'POST',
        url: ctx.url,
        headers: {
          authorization: `Bearer ${token}`,
          'content-type': 'application/json',
          accept: 'application/json',
        },
        body: JSON.stringify(body),
        timeoutMs: 60_000,
      });

      let parsed: graphql.Response | undefined;
      try {
        parsed = JSON.parse(response.body);
        console.log(JSON.stringify(parsed, null, 2));
      } catch {
        console.log(response.body);
      }

      return graphql.failed(response.status, parsed) ? 1 : 0;
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

      let page: string | undefined;
      const upstream: Upstream = {
        page: () => {
          if (page !== undefined) return page;

          const response = httpSend({
            url: ctx.url,
            headers: { authorization: `Bearer ${authToken()}`, accept: 'text/html' },
            timeoutMs: 60_000,
          });
          if (response.status !== 200) {
            throw new Error(`the router returned ${response.status} for its lab page`);
          }

          page = response.body;
          return page;
        },

        forward: (body, accept) => {
          const response = httpSend({
            method: 'POST',
            url: ctx.url,
            headers: {
              authorization: `Bearer ${authToken()}`,
              'content-type': 'application/json',
              accept: accept ?? 'application/graphql-response+json, application/json',
            },
            body,
            timeoutMs: 60_000,
          });

          return {
            status: response.status,
            headers: { 'content-type': response.headers['content-type'] ?? 'application/json' },
            body: response.body,
          };
        },
      };

      serve.listen(port);
      console.error(`lab for ${ctx.url} on http://localhost:${port}, using your graph login`);
      console.error('subscriptions are not forwarded; stop with ctrl-c');

      for (;;) {
        const request = serve.accept();

        let response;
        try {
          response = handle(request, port, upstream);
        } catch (e) {
          console.error(`${request.method} ${request.path}: ${e instanceof Error ? e.message : e}`);
          response = {
            status: 502,
            body: 'could not reach the graph; check `ortzi graph token`\n',
          };
        }

        serve.respond(response);
      }
    },
  },
});
