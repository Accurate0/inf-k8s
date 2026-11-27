import type { Schema } from 'jsonschema/lib/index.js';
import type { App } from '../types.ts';
import { appNamespace, declared, specs } from '../app.ts';
import { appServices } from '../services.ts';
import {
  type Condition,
  fail,
  type KubeObject,
  ok,
  type ParentRef,
  sameParent,
  unmet,
} from '../checks.ts';
import type { ResourceProvider } from './provider.ts';

export type RouteAuth =
  | { mode: 'jwt'; clients: string[] }
  | {
      mode: 'oidc';
      client: string;
      scopes?: string[];
      forwardAccessToken?: boolean;
      logoutPath?: string;
      jwt?: Record<string, unknown>;
    }
  | { mode: 'app' | 'none'; reason: string };

export type RouteDns = { provider: 'cloudflare'; proxied: boolean } | { provider: 'unifi' };

export interface RouteRatelimit {
  requests: number;
  per: string;
  header: string;
}

export interface RouteLoadBalancer {
  type: 'roundRobin' | 'leastRequest' | 'random' | 'consistentHash';
  headers?: string[];
}

export interface RouteGateway {
  gateway: string;
  hosts?: string[];
  dns?: RouteDns;
  listener?: string;
}

export interface RouteSpec {
  protocol?: 'http' | 'tcp';
  gateways: RouteGateway[];
  auth: RouteAuth;
  backend?: { name: string; port: number };
  paths?: (string | { type: string; value: string })[];
  rules?: {
    paths: (string | { type: string; value: string })[];
    backend: { name: string; port: number };
  }[];
  ratelimit?: RouteRatelimit;
  timeouts?: Record<string, unknown>;
  loadBalancer?: RouteLoadBalancer;
}

export type Route = Omit<RouteSpec, 'gateways'> & RouteGateway & { hosts: string[] };

declare module './provider.ts' {
  interface ResourceTypes {
    route: { name: string; type: 'route' } & RouteSpec;
  }
}

const PATHS: Schema = {
  type: 'array',
  items: {
    oneOf: [
      {
        type: 'string',
      },
      {
        type: 'object',
        required: ['type', 'value'],
        properties: {
          type: {
            type: 'string',
          },
          value: {
            type: 'string',
          },
        },
      },
    ],
  },
};

const BACKEND: Schema = {
  type: 'object',
  additionalProperties: false,
  required: ['name', 'port'],
  properties: {
    name: {
      type: 'string',
    },
    port: {
      type: 'integer',
    },
  },
};

const schema: Schema = {
  type: 'object',
  additionalProperties: false,
  required: ['name', 'type', 'auth', 'gateways'],
  properties: {
    name: {
      type: 'string',
    },
    type: {
      enum: ['route'],
    },
    protocol: {
      enum: ['http', 'tcp'],
      description:
        'Defaults to http. tcp attaches a TCPRoute to a gateway listener and takes only gateways, backend and auth.',
    },
    gateways: {
      type: 'array',
      minItems: 1,
      description:
        'Gateways the route attaches to. Each gets its own HTTPRoute: named after the route with one gateway, <route>-<gateway without -gateway> with several.',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['gateway'],
        properties: {
          gateway: {
            type: 'string',
          },
          hosts: {
            type: 'array',
            items: {
              type: 'string',
            },
          },
          listener: {
            type: 'string',
            description: 'TCP listener on the gateway, for tcp.',
          },
          dns: {
            description: "Publish this gateway's hosts in DNS.",
            oneOf: [
              {
                type: 'object',
                additionalProperties: false,
                required: ['provider', 'proxied'],
                properties: {
                  provider: {
                    enum: ['cloudflare'],
                  },
                  proxied: {
                    type: 'boolean',
                  },
                },
              },
              {
                type: 'object',
                additionalProperties: false,
                required: ['provider'],
                properties: {
                  provider: {
                    enum: ['unifi'],
                  },
                },
              },
            ],
          },
        },
      },
    },
    auth: {
      type: 'object',
      description:
        'How this route authenticates. jwt and oidc are enforced at the gateway; app and none need a reason.',
      additionalProperties: false,
      required: ['mode'],
      properties: {
        mode: {
          enum: ['jwt', 'oidc', 'app', 'none'],
        },
        clients: {
          type: 'array',
          description: 'oidc clients whose tokens are accepted, for jwt.',
          items: {
            type: 'string',
          },
        },
        client: {
          type: 'string',
          description: 'oidc client with a securityPolicy targeting this route, for oidc.',
        },
        reason: {
          type: 'string',
          description: 'Why the gateway does not authenticate, for app and none.',
        },
        scopes: {
          type: 'array',
          description: 'Login scopes, for oidc.',
          items: {
            type: 'string',
          },
        },
        forwardAccessToken: {
          type: 'boolean',
          description: 'For oidc.',
        },
        logoutPath: {
          type: 'string',
          description: 'For oidc.',
        },
        jwt: {
          type: 'object',
          description: 'Verify the login token at the gateway, for oidc.',
        },
      },
    },
    backend: BACKEND,
    paths: PATHS,
    ratelimit: {
      type: 'object',
      additionalProperties: false,
      required: ['requests', 'per', 'header'],
      properties: {
        requests: {
          type: 'integer',
          minimum: 1,
        },
        per: {
          enum: ['Second', 'Minute', 'Hour', 'Day'],
        },
        header: {
          type: 'string',
          description: 'Limit each distinct value of this request header.',
        },
      },
    },
    loadBalancer: {
      type: 'object',
      additionalProperties: false,
      required: ['type'],
      description:
        'How the gateway spreads requests across the backend. headers picks the hash inputs for consistentHash.',
      properties: {
        type: {
          enum: ['roundRobin', 'leastRequest', 'random', 'consistentHash'],
        },
        headers: {
          type: 'array',
          minItems: 1,
          items: {
            type: 'string',
          },
        },
      },
    },
    timeouts: {
      type: 'object',
    },
    rules: {
      type: 'array',
      description: 'Send paths to different backends. Replaces paths and backend.',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['paths', 'backend'],
        properties: {
          paths: PATHS,
          backend: BACKEND,
        },
      },
    },
  },
};

const HTTP_ONLY = ['paths', 'rules', 'ratelimit', 'timeouts', 'loadBalancer'] as const;

const LOAD_BALANCERS = {
  roundRobin: 'RoundRobin',
  leastRequest: 'LeastRequest',
  random: 'Random',
  consistentHash: 'ConsistentHash',
} as const;

export const routeName = (name: string, gateway: string, count: number) =>
  count === 1 ? name : `${name}-${gateway.replace(/-gateway$/, '')}`;

function expand(app: App, tcp: boolean): Record<string, Route> {
  return Object.fromEntries(
    specs(declared(app, 'route').filter((route) => (route.protocol === 'tcp') === tcp)).flatMap(
      ([name, { gateways, ...route }]) =>
        (gateways ?? []).map((entry) => [
          routeName(name, entry.gateway, gateways.length),
          { ...route, ...entry, hosts: entry.hosts ?? [] },
        ]),
    ),
  );
}

export const appRoutes = (app: App): Record<string, Route> => expand(app, false);

export const appTcpRoutes = (app: App): Record<string, Route> => expand(app, true);

function dnsAnnotations(dns: RouteDns): Record<string, string> {
  if (dns.provider === 'unifi') {
    return { 'external-dns.kubernetes.io/managed-unifi': 'true' };
  }
  return {
    'external-dns.kubernetes.io/managed': 'true',
    'external-dns.kubernetes.io/cloudflare-proxied': String(dns.proxied),
  };
}

function ratelimitRules({ requests, per, header }: RouteRatelimit) {
  return {
    rules: [
      {
        clientSelectors: [{ headers: [{ name: header, invert: false, type: 'Distinct' }] }],
        limit: { requests, unit: per },
      },
    ],
  };
}

function loadBalancer({ type, headers }: RouteLoadBalancer) {
  return {
    type: LOAD_BALANCERS[type],
    ...(headers
      ? { consistentHash: { type: 'Headers', headers: headers.map((name) => ({ name })) } }
      : {}),
  };
}

export interface Listener {
  gateway: string;
  name?: string;
  protocol?: string;
  hostname?: string;
  labels?: Record<string, string>;
}

const listenerMatches = (hostname: string | undefined, host: string) =>
  hostname === host || (hostname?.startsWith('*.') && host.endsWith(hostname.slice(1)));

function requiredLabels(app: App, listeners: Listener[]) {
  const required: { key: string; value: string; route: string; why: string }[] = [];
  for (const [name, route] of Object.entries(appTcpRoutes(app))) {
    const listener = listeners.find(
      (l) => l.gateway === route.gateway && l.name === route.listener,
    );
    if (!listener?.labels || listener.protocol !== 'TCP') {
      throw new Error(
        `${app.name}: routes.${name}: no TCP listener ${route.listener} on ${route.gateway}`,
      );
    }
    for (const [key, value] of Object.entries(listener.labels)) {
      required.push({ key, value, route: name, why: route.listener as string });
    }
  }

  for (const [name, route] of Object.entries(appRoutes(app))) {
    for (const host of route.hosts) {
      const matching = listeners.filter(
        (l) => l.gateway === route.gateway && l.labels && listenerMatches(l.hostname, host),
      );
      if (!matching.length) {
        throw new Error(
          `${app.name}: routes.${name}: no ${route.gateway} listener accepts ${host}`,
        );
      }
      const listener = matching.find((l) => l.hostname === host) ?? matching[0];
      for (const [key, value] of Object.entries(listener.labels ?? {})) {
        required.push({ key, value, route: name, why: host });
      }
    }
  }

  return required;
}

// Routes need their namespace to carry the labels the gateway listener selects on, so the
// labels come from the routes rather than being repeated in namespace.labels.
export function withGatewayAccess(apps: App[], listeners: Listener[]): App[] {
  return apps.map((app) => {
    const namespace = appNamespace(app);
    const explicit = app.namespace?.labels ?? {};
    const derived: Record<string, string> = {};
    for (const { key, value, route, why } of requiredLabels(app, listeners)) {
      const at = `${app.name}: routes.${route}: ${why}`;
      if (key === 'kubernetes.io/metadata.name') {
        if (value !== namespace) {
          throw new Error(`${at} only admits namespace ${value}`);
        }
        continue;
      }
      if (app.namespace?.name) {
        throw new Error(
          `${at} needs label ${key}: "${value}" on namespace ${namespace}, which this app does not own`,
        );
      }
      if (key in explicit) {
        throw new Error(
          `${at}: namespace.labels.${key} comes from this route; remove it from namespace.labels`,
        );
      }
      if (key in derived && derived[key] !== value) {
        throw new Error(`${at} needs ${key}: "${value}", another route needs "${derived[key]}"`);
      }
      derived[key] = value;
    }

    if (!Object.keys(derived).length) {
      return app;
    }
    const labels = Object.fromEntries(
      Object.entries({ ...explicit, ...derived }).sort(([a], [b]) => a.localeCompare(b)),
    );
    return { ...app, namespace: { ...app.namespace, labels } };
  });
}

export const route: ResourceProvider<'route'> = {
  type: 'route',
  schema,

  validate(app, declaredRoutes, fail) {
    const names = new Set<string>();
    for (const route of declaredRoutes) {
      const gateways = route.gateways ?? [];
      const tcp = route.protocol === 'tcp';
      if (new Set(gateways.map((entry) => entry.gateway)).size !== gateways.length) {
        fail(`routes.${route.name}.gateways lists a gateway twice`);
      }
      if (route.auth?.mode === 'oidc' && gateways.length > 1) {
        fail(`routes.${route.name}: an oidc route takes one gateway`);
      }
      gateways.forEach((entry, index) => {
        const at = `routes.${route.name}.gateways[${index}]`;
        const generated = routeName(route.name, entry.gateway, gateways.length);
        if (names.has(generated)) {
          fail(`${at}: another route also renders an HTTPRoute named ${generated}`);
        }
        names.add(generated);
        if (tcp) {
          if (!entry.listener) {
            fail(`${at}.listener is required for protocol tcp`);
          }
          for (const field of ['hosts', 'dns'] as const) {
            if (entry[field] !== undefined) {
              fail(`${at}.${field} is only for protocol http`);
            }
          }
        } else {
          if (entry.listener) {
            fail(`${at}.listener is only for protocol tcp`);
          }
          if (!entry.hosts?.length) {
            fail(`${at}.hosts is required`);
          }
        }
      });
    }

    const routes = Object.entries(appRoutes(app));
    const clients = new Set(declared(app, 'oidc').map((client) => client.name));
    for (const [name, route] of routes) {
      if (!route.backend === !route.rules) {
        fail(`routes.${name} needs either backend or rules`);
      }
      if (route.backend && !route.paths) {
        fail(`routes.${name}.paths is required with backend`);
      }
      if (route.loadBalancer) {
        const { type, headers } = route.loadBalancer;
        if (type === 'consistentHash' && !headers?.length) {
          fail(`routes.${name}.loadBalancer.headers is required for consistentHash`);
        }
        if (type !== 'consistentHash' && headers) {
          fail(`routes.${name}.loadBalancer.headers is only for consistentHash`);
        }
      }
      const auth = route.auth;
      if (auth.mode === 'jwt') {
        if (!auth.clients?.length) {
          fail(`routes.${name}.auth.clients is required for jwt`);
        }
        for (const client of auth.clients ?? []) {
          if (!clients.has(client)) {
            fail(`routes.${name}.auth: no oidc client named ${client}`);
          }
        }
      } else if (auth.mode === 'oidc') {
        if (!clients.has(auth.client)) {
          fail(`routes.${name}.auth: no oidc client named ${auth.client}`);
        }
        if (
          routes.some(
            ([other, r]) =>
              other !== name && r.auth?.mode === 'oidc' && r.auth.client === auth.client,
          )
        ) {
          fail(`routes.${name}.auth: oidc client ${auth.client} is used by another oidc route`);
        }
      } else if (!auth.reason?.trim()) {
        fail(`routes.${name}.auth.reason is required for ${auth.mode}`);
      }
    }

    for (const [name, route] of Object.entries(appTcpRoutes(app))) {
      if (!route.backend) {
        fail(`routes.${name}.backend is required for protocol tcp`);
      }
      for (const field of HTTP_ONLY) {
        if (route[field] !== undefined) {
          fail(`routes.${name}.${field} is only for protocol http`);
        }
      }
      const auth = route.auth;
      if (auth.mode !== 'app' && auth.mode !== 'none') {
        fail(`routes.${name}.auth.mode must be app or none for protocol tcp`);
      } else if (!auth.reason?.trim()) {
        fail(`routes.${name}.auth.reason is required for ${auth.mode}`);
      }
    }
  },

  check(app, _routes, { external, fail }) {
    const services = appServices(app);
    for (const [name, route] of [
      ...Object.entries(appRoutes(app)),
      ...Object.entries(appTcpRoutes(app)),
    ]) {
      for (const backend of [
        ...(route.backend ? [route.backend] : []),
        ...(route.rules ?? []).map((rule) => rule.backend),
      ]) {
        const service = services.find((s) => s.service === backend.name);
        if (!service && external) {
          continue;
        }
        if (!service) {
          fail(`routes.${name}.backend: no service named ${backend.name}`);
        }
        if (!service?.ports.some((p) => p.port === backend.port)) {
          fail(`routes.${name}.backend: service ${backend.name} has no port ${backend.port}`);
        }
      }
    }
  },

  plan({ app, chart }) {
    const routes = Object.entries(appRoutes(app));
    const tcpRoutes = Object.entries(appTcpRoutes(app));
    if (!routes.length && !tcpRoutes.length) {
      return {};
    }

    return {
      ingress: ['envoy-gateway-system'],
      charts: [
        chart('k8s-http-routes', {
          ...(tcpRoutes.length
            ? {
                tcpRoutes: tcpRoutes.map(([routeName, route]) => ({
                  name: routeName,
                  gateway: {
                    name: route.gateway,
                    namespace: 'envoy-gateway-system',
                  },
                  listener: route.listener,
                  backend: route.backend,
                })),
              }
            : {}),
          routes: routes.map(([routeName, route]) => {
            return {
              name: routeName,
              ...(route.auth?.mode === 'jwt' ? { jwtClients: route.auth.clients } : {}),
              gateway: {
                name: route.gateway,
                namespace: 'envoy-gateway-system',
              },
              ...(route.dns ? { annotations: dnsAnnotations(route.dns) } : {}),
              hostnames: route.hosts,
              ...(route.ratelimit ? { ratelimit: ratelimitRules(route.ratelimit) } : {}),
              ...(route.loadBalancer ? { loadBalancer: loadBalancer(route.loadBalancer) } : {}),
              rules: (
                route.rules ?? [
                  {
                    paths: route.paths as NonNullable<Route['paths']>,
                    backend: route.backend as { name: string; port: number },
                  },
                ]
              ).map((rule) => ({
                backendRefs: [{ group: '', kind: 'Service', ...rule.backend, weight: 1 }],
                matches: rule.paths.map((match) => ({
                  path: typeof match === 'string' ? { type: 'PathPrefix', value: match } : match,
                })),
                ...(route.timeouts ? { timeouts: route.timeouts } : {}),
              })),
            };
          }),
        }),
      ],
    };
  },

  doctor: {
    label: 'routes',
    run({ app, ns }) {
      const checks = Object.keys(appRoutes(app)).map((name) => {
        const route = ns.get<KubeObject>('httproutes', name);
        const refs: ParentRef[] = route.spec?.parentRefs ?? [];
        const parents = (route.status?.parents ?? []).filter((parent: { parentRef: ParentRef }) =>
          refs.some((ref) => sameParent(ref, parent.parentRef)),
        );
        const problems = parents.length
          ? parents.flatMap((parent: { conditions?: Condition[] }) =>
              unmet(parent.conditions, ['Accepted', 'ResolvedRefs']),
            )
          : ['no gateway has picked it up'];
        return problems.length
          ? fail(`route ${name}: ${problems.join('; ')}`)
          : ok(`route ${name} accepted`);
      });

      for (const kind of ['backendtrafficpolicies', 'securitypolicies']) {
        for (const policy of ns.list<KubeObject>(kind)) {
          const problems = (policy.status?.ancestors ?? []).flatMap(
            (ancestor: { conditions?: Condition[] }) => unmet(ancestor.conditions, ['Accepted']),
          );
          const text = policy.metadata.name;
          checks.push(
            problems.length ? fail(`${text}: ${problems.join('; ')}`) : ok(`${text} accepted`),
          );
        }
      }

      return checks;
    },
  },
};
