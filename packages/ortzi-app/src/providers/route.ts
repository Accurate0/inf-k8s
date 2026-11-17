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

export interface Route {
  protocol?: 'http' | 'tcp';
  listener?: string;
  hosts: string[];
  gateway: string;
  auth: RouteAuth;
  backend?: { name: string; port: number };
  paths?: (string | { type: string; value: string })[];
  rules?: {
    paths: (string | { type: string; value: string })[];
    backend: { name: string; port: number };
  }[];
  dns?: RouteDns;
  ratelimit?: RouteRatelimit;
  timeouts?: Record<string, unknown>;
  loadBalancer?: RouteLoadBalancer;
}

declare module './provider.ts' {
  interface ResourceTypes {
    route: { name: string; type: 'route' } & Route;
  }
}

const schema: Schema = {
  type: 'object',
  additionalProperties: false,
  required: ['name', 'type', 'auth', 'gateway'],
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
        'Defaults to http. tcp attaches a TCPRoute to a gateway listener and takes only gateway, listener, backend and auth.',
    },
    listener: {
      type: 'string',
      description: 'TCP listener on the gateway, for tcp.',
    },
    gateway: {
      type: 'string',
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
    hosts: {
      type: 'array',
      items: {
        type: 'string',
      },
    },
    backend: {
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
    },
    paths: {
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
    },
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
          paths: {
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
          },
          backend: {
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
          },
        },
      },
    },
    dns: {
      description: "Publish the route's hosts in DNS.",
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
};

const HTTP_ONLY = [
  'hosts',
  'paths',
  'rules',
  'dns',
  'ratelimit',
  'timeouts',
  'loadBalancer',
] as const;

const LOAD_BALANCERS = {
  roundRobin: 'RoundRobin',
  leastRequest: 'LeastRequest',
  random: 'Random',
  consistentHash: 'ConsistentHash',
} as const;

export function appRoutes(app: App): Record<string, Route> {
  return Object.fromEntries(
    specs(declared(app, 'route').filter((route) => route.protocol !== 'tcp')),
  ) as Record<string, Route>;
}

export function appTcpRoutes(app: App): Record<string, Route> {
  return Object.fromEntries(
    specs(declared(app, 'route').filter((route) => route.protocol === 'tcp')),
  ) as Record<string, Route>;
}

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

export function validateGatewayAccess(apps: App[], listeners: Listener[]) {
  for (const app of apps) {
    const labels: Record<string, string> = {
      ...app.namespace?.labels,
      'kubernetes.io/metadata.name': appNamespace(app),
    };
    for (const [name, route] of Object.entries(appTcpRoutes(app))) {
      const gateway = route.gateway;
      const listener = listeners.find((l) => l.gateway === gateway && l.name === route.listener);
      if (!listener?.labels || listener.protocol !== 'TCP') {
        throw new Error(
          `${app.name}: routes.${name}: no TCP listener ${route.listener} on ${gateway}`,
        );
      }
      for (const [key, value] of Object.entries(listener.labels)) {
        if (labels[key] !== value) {
          throw new Error(
            `${app.name}: routes.${name}: ${route.listener} needs namespace.labels ${key}: "${value}"`,
          );
        }
      }
    }

    for (const [name, route] of Object.entries(appRoutes(app))) {
      const gateway = route.gateway;
      for (const host of route.hosts) {
        const matching = listeners.filter(
          (l) => l.gateway === gateway && l.labels && listenerMatches(l.hostname, host),
        );
        if (!matching.length) {
          throw new Error(`${app.name}: routes.${name}: no ${gateway} listener accepts ${host}`);
        }
        const listener = matching.find((l) => l.hostname === host) ?? matching[0];
        for (const [key, value] of Object.entries(listener.labels ?? {})) {
          if (labels[key] !== value) {
            throw new Error(
              `${app.name}: routes.${name}: ${host} needs namespace.labels ${key}: "${value}"`,
            );
          }
        }
      }
    }
  }
}

export const route: ResourceProvider<'route'> = {
  type: 'route',
  schema,

  validate(app, _routes, fail) {
    const routes = Object.entries(appRoutes(app));
    const clients = new Set(declared(app, 'oidc').map((client) => client.name));
    for (const [name, route] of routes) {
      if (route.listener) {
        fail(`routes.${name}.listener is only for protocol tcp`);
      }
      if (!route.hosts?.length) {
        fail(`routes.${name}.hosts is required`);
      }
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
      if (!route.listener) {
        fail(`routes.${name}.listener is required for protocol tcp`);
      }
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
