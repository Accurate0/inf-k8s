const UNPKG = 'https://unpkg.com';

export const labPage = (upstream: string): string => `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>graph lab · ${upstream}</title>
    <link rel="stylesheet" href="${UNPKG}/graphiql@3.8.3/graphiql.min.css" />
    <style>
      html, body, #lab { height: 100%; margin: 0; }
    </style>
  </head>
  <body>
    <div id="lab"></div>
    <script crossorigin src="${UNPKG}/react@18.3.1/umd/react.production.min.js"></script>
    <script crossorigin src="${UNPKG}/react-dom@18.3.1/umd/react-dom.production.min.js"></script>
    <script crossorigin src="${UNPKG}/graphql-ws@5.16.0/umd/graphql-ws.min.js"></script>
    <script crossorigin src="${UNPKG}/graphiql@3.8.3/graphiql.min.js"></script>
    <script>
      const fetcher = GraphiQL.createFetcher({
        url: location.origin + '/',
        wsClient: graphqlWs.createClient({ url: 'ws://' + location.host + '/' }),
      });

      ReactDOM.createRoot(document.getElementById('lab')).render(
        React.createElement(GraphiQL, { fetcher, shouldPersistHeaders: false }),
      );
    </script>
  </body>
</html>
`;
