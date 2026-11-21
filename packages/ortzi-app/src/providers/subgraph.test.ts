import assert from 'node:assert/strict';
import { test } from 'node:test';
import { type App, rawFileUrl, resolveSubgraphs, validateResources } from '../render.ts';
import {
  target,
  platform,
  render,
  subgraphApp,
  routerApp,
  releaseValues,
  readValues,
} from '../test-fixtures.ts';

test('a subgraph lets the supergraph app in', () => {
  const spotify = subgraphApp('spotify');
  const rendered = render(spotify, target, [spotify, routerApp]);

  assert.deepEqual(releaseValues(rendered, 'k8s-network').ingress, ['graph']);
});

test('subgraphs resolve to their in-cluster url and raw schema', () => {
  const [spotify, home] = resolveSubgraphs([
    subgraphApp('spotify'),
    subgraphApp('home-gateway', {
      path: '/v1/graphql',
      websocket: '/v1/graphql/ws',
      repository: 'https://git.anurag.sh/anurag/home-gateway',
      ref: 'v2',
      schema: 'graphql/subgraph.graphql',
    }),
    routerApp,
  ]);

  assert.deepEqual(spotify, {
    name: 'spotify',
    app: 'spotify',
    prefix: 'spotify',
    url: 'http://api.spotify.svc.cluster.local/graphql',
    schemaUrl: 'https://raw.githubusercontent.com/Accurate0/spotify/main/subgraph.graphql',
  });
  assert.equal(home?.websocket, '/v1/graphql/ws');
  assert.equal(
    home?.schemaUrl,
    'https://git.anurag.sh/anurag/home-gateway/raw/branch/v2/graphql/subgraph.graphql',
  );
});

test('subgraphs without a repository read their schema from the app directory', () => {
  const [registry] = resolveSubgraphs([
    subgraphApp('registry', { repository: undefined, ref: undefined }),
  ]);

  assert.equal(registry?.schemaPath, 'subgraph.graphql');
  assert.equal(registry?.schemaUrl, undefined);

  assert.throws(
    () => resolveSubgraphs([subgraphApp('registry', { ref: undefined })]),
    /registry: resources.registry: repository and ref go together/,
  );
  assert.throws(
    () =>
      resolveSubgraphs([
        subgraphApp('registry', { repository: undefined, ref: undefined, schema: '../x' }),
      ]),
    /must stay inside the app directory/,
  );
});

test('raw file urls need an https owner/repo url', () => {
  assert.throws(() => rawFileUrl('https://github.com/Accurate0', 'main', 'a'), /owner/);
  assert.throws(() => rawFileUrl('ssh://git@git.anurag.sh/a/b.git', 'main', 'a'), /https/);
});

test('subgraph validation', () => {
  const world = (apps: App[]) => ({ platform, apps, readValues });

  assert.throws(
    () => validateResources(world([subgraphApp('spotify')])),
    /spotify: subgraphs need an app declaring a supergraph/,
  );
  assert.throws(
    () => validateResources(world([routerApp, { ...routerApp, name: 'graph2' }])),
    /only one app may declare a supergraph, found graph, graph2/,
  );
  assert.throws(
    () => validateResources(world([routerApp, subgraphApp('spotify', { workload: 'nope' })])),
    /spotify: resources.spotify: no workload named nope/,
  );
  assert.throws(
    () => validateResources(world([routerApp, subgraphApp('spotify', { port: 'grpc' })])),
    /no matching port/,
  );
  assert.throws(
    () => validateResources(world([routerApp, subgraphApp('spotify', { path: 'graphql' })])),
    /graphql must start with \//,
  );
  assert.throws(
    () => validateResources(world([routerApp, subgraphApp('spotify', { prefix: 'Spotify' })])),
    /prefix Spotify must be lowerCamelCase/,
  );
  assert.throws(
    () =>
      validateResources(
        world([routerApp, subgraphApp('spotify'), subgraphApp('music', { name: 'spotify' })]),
      ),
    /subgraph name spotify is declared by both spotify and music/,
  );
  assert.throws(
    () =>
      validateResources(
        world([routerApp, subgraphApp('spotify'), subgraphApp('music', { prefix: 'spotify' })]),
      ),
    /subgraph prefix spotify is declared by both spotify and music/,
  );
  validateResources(world([routerApp, subgraphApp('spotify')]));
});

test('subgraph auth clients must be oidc resources', () => {
  const world = (apps: App[]) => ({ platform, apps, readValues });
  const spotify = subgraphApp('spotify', { auth: { clients: ['spotify'], groups: ['g'] } });

  assert.throws(
    () => validateResources(world([routerApp, spotify])),
    /spotify: resources.spotify: auth.clients has no oidc resource named spotify/,
  );
  validateResources(
    world([
      routerApp,
      {
        ...spotify,
        resources: [
          ...(spotify.resources ?? []),
          {
            name: 'spotify',
            type: 'oidc',
            displayName: 'spotify',
            preferShortUsername: true,
            host: 'spotify.sh',
            secretName: 's',
          },
        ],
      },
    ]),
  );
});

test('persisted documents resolve beside the schema', () => {
  const [spotify, registry] = resolveSubgraphs([
    subgraphApp('spotify', { persistedDocuments: 'web/persisted-documents.json' }),
    subgraphApp('registry', {
      repository: undefined,
      ref: undefined,
      persistedDocuments: 'documents.json',
    }),
  ]);

  assert.equal(
    spotify?.documentsUrl,
    'https://raw.githubusercontent.com/Accurate0/spotify/main/web/persisted-documents.json',
  );
  assert.equal(registry?.documentsPath, 'documents.json');
  assert.throws(
    () =>
      resolveSubgraphs([
        subgraphApp('registry', {
          repository: undefined,
          ref: undefined,
          persistedDocuments: '/etc/documents.json',
        }),
      ]),
    /persistedDocuments \/etc\/documents.json must stay inside the app directory/,
  );
});
