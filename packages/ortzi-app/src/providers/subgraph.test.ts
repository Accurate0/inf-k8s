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
import { type Schema, Validator } from 'jsonschema/lib/index.js';
import { subgraph, subgraphAuthValue } from './subgraph.ts';
import { supplementaryScopeMaps } from './oidc.ts';

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
    prefixes: ['spotify'],
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

  const forgejo = {
    name: 'forgejo',
    issuer: 'https://git.anurag.sh/api/actions',
    jwks: 'https://git.anurag.sh/api/actions/.well-known/keys',
    audience: 'graph.ortzi.cloud',
  };
  const trusting: App = {
    ...routerApp,
    resources: [{ ...routerApp.resources![0], externalIssuers: [forgejo] } as never],
  };
  const external = (issuer: string) =>
    subgraphApp('spotify', {
      auth: {
        clients: ['spotify'],
        groups: ['spotify_users'],
        external: [{ issuer, claims: { repository: 'anurag/*' } }],
      },
    });
  assert.throws(
    () => validateResources(world([routerApp, external('forgejo')])),
    /auth names no external issuer forgejo; declare it in the supergraph's externalIssuers/,
  );
  assert.throws(
    () => validateResources(world([trusting, external('github')])),
    /auth names no external issuer github/,
  );
  assert.throws(
    () =>
      validateResources(
        world([
          {
            ...routerApp,
            resources: [
              { ...routerApp.resources![0], externalIssuers: [forgejo, forgejo] } as never,
            ],
          },
        ]),
      ),
    /external issuer forgejo is declared twice/,
  );

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
    () => validateResources(world([routerApp, subgraphApp('spotify', { prefixes: ['Spotify'] })])),
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
        world([routerApp, subgraphApp('spotify'), subgraphApp('music', { prefixes: ['spotify'] })]),
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

const scoped = (name: string, clients: string[], scopes: unknown[]) =>
  subgraphApp(name, { auth: { clients, scopes } });

const forgejoRouter: App = {
  ...routerApp,
  resources: [
    {
      name: 'graph',
      type: 'supergraph',
      base: 'router.base.yaml',
      externalIssuers: [
        {
          name: 'forgejo',
          issuer: 'https://git.example/api/actions',
          jwks: 'https://git.example/keys',
          audience: 'graph.example',
        },
      ],
    },
  ],
};

test('scopes become supplementary scope maps on every client allowed to call the subgraph', () => {
  const apps = [
    scoped(
      'registry',
      ['graph', 'registry'],
      [
        { name: 'registry:package:read', description: 'read', groups: ['registry_users'] },
        { name: 'registry:token:create', description: 'mint', groups: ['registry_admins'] },
      ],
    ),
    scoped(
      'waf',
      ['graph'],
      [
        {
          name: 'waf:block:write',
          description: 'block',
          groups: ['waf_admins', 'registry_admins'],
        },
      ],
    ),
  ];

  assert.deepEqual(supplementaryScopeMaps(apps, 'graph'), {
    registry_admins: ['registry:token:create', 'waf:block:write'],
    registry_users: ['registry:package:read'],
    waf_admins: ['waf:block:write'],
  });
  assert.deepEqual(supplementaryScopeMaps(apps, 'registry'), {
    registry_admins: ['registry:token:create'],
    registry_users: ['registry:package:read'],
  });
  assert.equal(supplementaryScopeMaps(apps, 'spotify'), undefined);
});

test('a subgraph hands its workload the providers, scopes and external grants it enforces', () => {
  const registry = scoped(
    'registry',
    ['graph'],
    [
      { name: 'registry:package:read', description: 'read', groups: ['registry_users'] },
      {
        name: 'registry:package:publish',
        description: 'publish',
        groups: ['registry_admins'],
        external: [{ issuer: 'forgejo', claims: { repository: 'anurag/*' } }],
      },
    ],
  );
  const resource = registry.resources![0] as Parameters<typeof subgraphAuthValue>[0];

  assert.deepEqual(subgraphAuthValue(resource, [registry, forgejoRouter]), {
    providers: [{ issuer: 'https://idm.anurag.sh/oauth2/openid/graph', audience: 'graph' }],
    scopes: ['registry:package:read', 'registry:package:publish'],
    groups: [],
    external: [
      {
        name: 'forgejo',
        issuer: 'https://git.example/api/actions',
        jwks: 'https://git.example/keys',
        audience: 'graph.example',
        grants: [{ scope: 'registry:package:publish', claims: { repository: 'anurag/*' } }],
      },
    ],
  });
});

test('scope names are <subgraph>:<resource>:<action> and declared once', () => {
  const check = (apps: App[]) => validateResources({ platform, apps: [forgejoRouter, ...apps] });
  const scope = (name: string) => ({ name, description: 'd', groups: ['g'] });

  assert.throws(
    () => check([scoped('registry', ['graph'], [scope('registry:admin')])]),
    /scope registry:admin must be <subgraph>:<resource>:<action>/,
  );
  assert.throws(
    () => check([scoped('registry', ['graph'], [scope('waf:block:write')])]),
    /scope waf:block:write must start with registry:/,
  );
  assert.throws(
    () =>
      check([
        scoped(
          'registry',
          ['graph'],
          [scope('registry:package:read'), scope('registry:package:read')],
        ),
      ]),
    /scope registry:package:read is already declared/,
  );
  assert.throws(
    () =>
      check([
        scoped(
          'registry',
          ['graph'],
          [
            {
              ...scope('registry:package:read'),
              external: [{ issuer: 'github', claims: { a: 'b' } }],
            },
          ],
        ),
      ]),
    /auth names no external issuer github/,
  );
});

test('a subgraph declaring scopes must pass them to its workload', () => {
  const router: App = {
    ...forgejoRouter,
    resources: [
      ...forgejoRouter.resources!,
      {
        name: 'graph',
        type: 'oidc',
        displayName: 'Graph',
        preferShortUsername: true,
        host: 'graph.example',
        secretName: 'graph-oidc',
      },
    ],
  };
  const registry = (env: Record<string, unknown>) => {
    const app = scoped(
      'registry',
      ['graph'],
      [{ name: 'registry:package:read', description: 'read', groups: ['registry_users'] }],
    );
    return { ...app, workloads: [{ ...app.workloads![0], env }] } as App;
  };

  assert.throws(
    () => validateResources({ platform, apps: [router, registry({})] }),
    /declares scopes, so its workload needs env SUBGRAPH_AUTH: \{subgraph: registry\}/,
  );
  const wired = registry({ SUBGRAPH_AUTH: { subgraph: 'registry' } });
  validateResources({ platform, apps: [router, wired] });

  const env = releaseValues(render(wired, target, [router, wired]), 'k8s-workload').workloads[0]
    .env;
  assert.deepEqual(
    JSON.parse(env.find((e: { name: string }) => e.name === 'SUBGRAPH_AUTH').value),
    {
      providers: [{ issuer: 'https://idm.anurag.sh/oauth2/openid/graph', audience: 'graph' }],
      scopes: ['registry:package:read'],
      groups: [],
      external: [],
    },
  );
});

test('scopes may start with any of the subgraph prefixes', () => {
  const app = (scope: string) =>
    subgraphApp('bridge', {
      prefixes: ['forgejo', 'flux'],
      auth: { clients: ['graph'], scopes: [{ name: scope, description: 'd', groups: ['g'] }] },
    });
  assert.throws(
    () => validateResources({ platform, apps: [forgejoRouter, app('waf:block:read')] }),
    /scope waf:block:read must start with bridge: or forgejo: or flux:/,
  );
  assert.throws(
    () => validateResources({ platform, apps: [forgejoRouter, app('flux:app:read')] }),
    /auth.clients has no oidc resource named graph/,
  );
});

test('grantedBy renders the same grants as groups and external did', () => {
  const legacy = scoped(
    'registry',
    ['graph'],
    [
      {
        name: 'registry:package:publish',
        description: 'publish',
        groups: ['registry_admins', 'registry_users'],
        external: [
          { issuer: 'forgejo', claims: { repository: 'anurag/*' } },
          { issuer: 'forgejo', claims: { repository: 'accurate0/*' } },
        ],
      },
    ],
  );
  const granted = scoped(
    'registry',
    ['graph'],
    [
      {
        name: 'registry:package:publish',
        description: 'publish',
        grantedBy: [
          { groups: ['registry_admins'] },
          { external: { issuer: 'forgejo', claims: { repository: 'anurag/*' } } },
          { groups: ['registry_users', 'registry_admins'] },
          { external: { issuer: 'forgejo', claims: { repository: 'accurate0/*' } } },
        ],
      },
    ],
  );
  const auth = (app: App) =>
    subgraphAuthValue(app.resources![0] as Parameters<typeof subgraphAuthValue>[0], [
      app,
      forgejoRouter,
    ]);

  assert.deepEqual(auth(granted), auth(legacy));
  assert.deepEqual(
    supplementaryScopeMaps([granted], 'graph'),
    supplementaryScopeMaps([legacy], 'graph'),
  );
});

test('a scope declares its grants in grantedBy or the deprecated fields, not both', () => {
  const check = (scope: Record<string, unknown>) => () =>
    validateResources({
      platform,
      apps: [
        forgejoRouter,
        scoped(
          'registry',
          ['graph'],
          [{ name: 'registry:package:read', description: 'd', ...scope }],
        ),
      ],
    });

  assert.throws(
    check({ grantedBy: [{ groups: ['a'] }], groups: ['b'] }),
    /scope registry:package:read uses grantedBy, move groups and external into it/,
  );
  assert.throws(check({}), /scope registry:package:read needs grantedBy/);
  assert.throws(
    check({ grantedBy: [{ external: { issuer: 'github', claims: { repository: '*' } } }] }),
    /auth names no external issuer github/,
  );
});

test('the scope schema takes one source per grantedBy entry', () => {
  const scopeSchema = (
    subgraph.schema as {
      properties: { auth: { properties: { scopes: { items: Schema } } } };
    }
  ).properties.auth.properties.scopes.items;
  const valid = (grantedBy: unknown) =>
    new Validator().validate(
      { name: 'registry:package:read', description: 'd', grantedBy },
      scopeSchema,
    ).valid;
  const pattern = { issuer: 'forgejo', claims: { repository: '*' } };

  assert.ok(valid([{ groups: ['a'] }, { external: pattern }, { external: pattern }]));
  assert.ok(!valid([]));
  assert.ok(!valid([{ groups: ['a'], external: pattern }]));
  assert.ok(!valid([{ external: [pattern] }]));
  assert.ok(!valid([{ groups: [] }]));
  assert.ok(
    !new Validator().validate(
      {
        name: 'registry:package:read',
        description: 'd',
        grantedBy: [{ groups: ['a'] }],
        groups: ['a'],
      },
      scopeSchema,
    ).valid,
  );
});
