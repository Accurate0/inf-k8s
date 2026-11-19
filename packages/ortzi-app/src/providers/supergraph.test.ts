import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parse } from 'yaml';
import { parse as parseGraphql } from 'graphql';
import {
  compose,
  mergeDocuments,
  routerConfig,
  unprefixedRootFields,
  unprefixedTypes,
} from '../supergraph.ts';
import { resolveSubgraphs, validateResources } from '../render.ts';
import { platform, subgraphApp, routerApp } from '../test-fixtures.ts';

test('router config adds websocket paths for subgraphs with subscriptions', () => {
  const config = parse(
    routerConfig(
      resolveSubgraphs([
        subgraphApp('spotify'),
        subgraphApp('home-gateway', { websocket: '/v1/graphql/ws' }),
      ]),
      'http:\n  port: 4000\nsubscriptions:\n  enabled: true\n',
      [],
    ),
  );

  assert.deepEqual(config, {
    http: { port: 4000 },
    subscriptions: {
      enabled: true,
      websocket: { subgraphs: { 'home-gateway': { path: '/v1/graphql/ws' } } },
    },
  });

  const renamed = parse(
    routerConfig(
      resolveSubgraphs([
        subgraphApp('home-gateway', { name: 'home', websocket: '/v1/graphql/ws' }),
      ]),
      '{}',
      [],
    ),
  );
  assert.deepEqual(renamed.subscriptions, {
    enabled: true,
    websocket: { subgraphs: { home: { path: '/v1/graphql/ws' } } },
  });
});

test('router config gates subgraphs that declare auth', () => {
  const config = parse(
    routerConfig(
      resolveSubgraphs([
        subgraphApp('spotify', {
          auth: { clients: ['spotify', 'graph'], groups: ['spotify_users'] },
        }),
        subgraphApp('home-gateway', {
          name: 'home',
          auth: { clients: ['home-gateway', 'graph'], groups: ['home_gateway_readers'] },
        }),
        subgraphApp('maccas-api', { name: 'maccas' }),
      ]),
      '{}',
      [],
    ),
  );

  const issuers = ['graph', 'home-gateway', 'spotify'].map(
    (client) => `https://idm.anurag.sh/oauth2/openid/${client}`,
  );
  assert.deepEqual(config.jwt, {
    enabled: true,
    require_authentication: false,
    jwks_providers: issuers.map((issuer) => ({
      source: 'remote',
      url: `${issuer}/public_key.jwk`,
      refresh_on_unknown_kid: '30s',
    })),
    issuers,
  });
  assert.deepEqual(config.plugins, {
    access: {
      enabled: true,
      config: {
        subgraphs: {
          spotify: { issuers: [issuers[2], issuers[0]], groups: ['spotify_users'] },
          home: { issuers: [issuers[1], issuers[0]], groups: ['home_gateway_readers'] },
        },
      },
    },
  });

  const open = parse(routerConfig(resolveSubgraphs([subgraphApp('spotify')]), '{}', []));
  assert.equal(open.jwt, undefined);
  assert.equal(open.plugins, undefined);

  assert.throws(
    () =>
      resolveSubgraphs([subgraphApp('spotify', { auth: { clients: ['spotify'], groups: [] } })]),
    /spotify: resources.spotify: auth.groups must list at least one non-empty name/,
  );
});

test('every root field of a subgraph starts with its prefix', () => {
  const sdl = (s: string) => parseGraphql(s);

  assert.deepEqual(
    unprefixedRootFields(
      sdl(
        'type Query { homeEntities: Int homeassistant: Int entities: Int _entities: Int _service: Int } type Mutation { homeLight: Int } type Subscription { events: Int } type Other { x: Int }',
      ),
      'home',
    ),
    ['Query.homeassistant', 'Query.entities', 'Subscription.events'],
  );
  assert.deepEqual(
    unprefixedRootFields(
      sdl('schema { query: Root } type Root { stats: Int } extend type Root { spotifyStats: Int }'),
      'spotify',
    ),
    ['Root.stats'],
  );
  assert.deepEqual(
    unprefixedRootFields(
      sdl('type Query { featureFlags: Int featureFlag: Int flags: Int featureFlagsSegments: Int }'),
      'featureFlags',
      ['featureFlags', 'featureFlag'],
    ),
    ['Query.flags'],
  );
});

test('every type of a subgraph starts with its prefix unless it references another subgraph entity', () => {
  const sdl = parseGraphql(`
    extend schema @link(url: "https://specs.apollo.dev/federation/v2.3", import: ["@key"])
    scalar DateTime
    type Query { homeEntities: [HomeEntity!]! }
    interface HomeEntity { id: ID! }
    type HomeLight implements HomeEntity { id: ID! artist: SpotifyArtist }
    type Light { id: ID! }
    type Homely { id: ID! }
    enum Mode { A }
    input HomeInput { a: Int }
    union HomeUpdate = HomeLight
    type SpotifyArtist @key(fields: "name", resolvable: false) { name: String! }
    type SpotifyPlay { name: String! }
    type _Service { sdl: String }
  `);

  assert.deepEqual(unprefixedTypes(sdl, 'home', ['spotify']), [
    'Homely',
    'Light',
    'Mode',
    'SpotifyPlay',
  ]);
  assert.deepEqual(
    unprefixedTypes(
      parseGraphql('type Artist @key(fields: "name") { name: String! }'),
      'spotify',
      [],
    ),
    ['Artist'],
  );
});

test('composition rejects unprefixed root fields', () => {
  const subgraphs = resolveSubgraphs([subgraphApp('spotify')]);
  assert.throws(
    () =>
      compose(
        subgraphs,
        { spotify: 'type Query { spotifyStats: Stats accounts: Int } type Stats { a: Int }' },
        '{}',
      ),
    /spotify: root fields must start with spotify: Query\.accounts\n {2}spotify: types must start with Spotify, or another subgraph's prefix for an entity it references: Stats/,
  );
});

test('composes subgraphs into a supergraph routed in cluster', () => {
  const link =
    'extend schema @link(url: "https://specs.apollo.dev/federation/v2.3", import: ["@key"])';
  const subgraphs = resolveSubgraphs([subgraphApp('spotify'), subgraphApp('home')]);
  const composed = compose(
    subgraphs,
    {
      spotify: `${link}\ntype Query { spotifyArtist(name: String!): SpotifyArtist }\ntype SpotifyArtist @key(fields: "name") { name: String! plays: Int! }`,
      home: `${link}\ntype Query { homePlayer: HomePlayer }\ntype HomePlayer { artist: SpotifyArtist }\ntype SpotifyArtist @key(fields: "name", resolvable: false) { name: String! }`,
    },
    '{}',
  );

  assert.match(composed.supergraph, /url: "http:\/\/api\.spotify\.svc\.cluster\.local\/graphql"/);
  assert.match(composed.schema, /type SpotifyArtist \{\s+name: String!\s+plays: Int!/);
  assert.match(composed.schema, /homePlayer: HomePlayer/);
  assert.throws(
    () =>
      compose(
        subgraphs,
        {
          spotify: `${link}\ntype Query { spotifyA: SpotifyShared }\ntype SpotifyShared @key(fields: "a") { a: Int }`,
          home: `${link}\ntype Query { homeA: SpotifyShared }\ntype SpotifyShared @key(fields: "a", resolvable: false) { a: String }`,
        },
        '{}',
      ),
    /supergraph composition failed/,
  );
});

test('the node plugin serves node for every Node type keyed by id, and only when there is one', () => {
  const link =
    'extend schema @link(url: "https://specs.apollo.dev/federation/v2.3", import: ["@key", "@shareable"])';
  const spotify = `${link}
    type Query { spotifyArtists(first: Int): SpotifyArtistConnection! }
    interface Node { id: ID! }
    type PageInfo @shareable { hasNextPage: Boolean! }
    type SpotifyArtistConnection @shareable { pageInfo: PageInfo! edges: [SpotifyArtist!]! }
    type SpotifyArtist implements Node @key(fields: "id") @key(fields: "name") { id: ID! name: String! }`;
  const subgraphs = resolveSubgraphs([subgraphApp('spotify')]);
  const composed = compose(subgraphs, { spotify }, '{}');

  assert.match(
    composed.node!,
    /type SpotifyArtist implements Node @key\(fields: "id", resolvable: false\)/,
  );
  assert.match(composed.schema, /node\(id: ID!\): Node/);
  assert.match(
    composed.supergraph,
    /NODE @join__graph\(name: "node", url: "http:\/\/node-plugin\.invalid\/graphql"\)/,
  );
  assert.deepEqual(parse(composed.router).plugins, {
    node: { enabled: true, config: { subgraph: 'node', types: ['SpotifyArtist'] } },
  });

  assert.throws(
    () =>
      compose(
        subgraphs,
        {
          spotify: spotify.replace(
            '@key(fields: "id") @key(fields: "name")',
            '@key(fields: "name")',
          ),
        },
        '{}',
      ),
    /spotify: SpotifyArtist implements Node without @key\(fields: "id"\)/,
  );
  assert.throws(
    () =>
      validateResources({ platform, apps: [routerApp, subgraphApp('spotify', { name: 'node' })] }),
    /subgraph name node is reserved/,
  );
});

test('persisted documents merge sorted, and the same id must mean the same document', () => {
  assert.equal(
    mergeDocuments({
      spotify: '{"b": "{ b }", "a": "{ a }"}',
      home: '{"a": "{ a }"}',
    }),
    '{\n  "a": "{ a }",\n  "b": "{ b }"\n}\n',
  );

  assert.throws(
    () => mergeDocuments({ spotify: '{"a": "{ a }"}', home: '{"a": "{ b }"}' }),
    /persisted document a differs between spotify and home/,
  );
  assert.throws(() => mergeDocuments({ spotify: '["{ a }"]' }), /JSON object of id to document/);
});

test('router config loads persisted documents beside the supergraph', () => {
  const base =
    'supergraph:\n  source: storage\n  storage_id: supergraph\n  location: supergraph.graphql\n  poll_interval: 30s\n';
  const config = parse(routerConfig([], base, [], true));

  assert.deepEqual(config.persisted_documents, {
    enabled: true,
    require_id: true,
    storage: {
      type: 'storage',
      storage_id: 'supergraph',
      location: 'persisted-documents.json',
      poll_interval: '30s',
    },
  });
  assert.equal(parse(routerConfig([], base, [])).persisted_documents, undefined);
  assert.throws(
    () => routerConfig([], 'supergraph:\n  source: file\n  path: x\n', [], true),
    /must load from storage/,
  );
});
