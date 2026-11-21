import { parseAllDocuments } from 'yaml';
import { renderFlux } from './outputs/flux.ts';
import { planApp } from './plan.ts';
import { type App, validate, validateResources } from './render.ts';

export const versions = {
  'k8s-workload': '0.1.0',
  'k8s-network': '0.1.0',
  'k8s-observability': '0.1.0',
  'k8s-http-routes': '2.3.1',
  'k8s-oidc': '0.4.0',
  'k8s-postgres': '1.0.1',
  'k8s-secrets': '0.6.0',
};

export const target = { location: 'projects', hasManifests: false };

export const platform = [
  { name: 'pooled', type: 'database', host: 'pooler.db' },
  { name: 'direct', type: 'database', host: 'direct.db' },
  { name: 'tracing', type: 'platform', url: 'http://tempo/traces' },
  { name: 'replicated', type: 'cache', url: 'redis://replicated' },
  { name: 'perth', type: 'cache', url: 'redis://perth' },
  { name: 'cache', type: 'cache', url: 'redis://cache' },
];

// values files are inlined from disk at render; tests start from empty ones
export const readValues = () => ({});

export interface Rendered {
  files: Map<string, string>;
  objects: Record<string, any>[];
  releases: Record<string, any>[];
}

export function render(app: App, t = target, apps: App[] = []): Rendered {
  const files = renderFlux(
    planApp(app, versions, t, { platform, apps, readValues }),
    app.overrides?.flux,
  );
  const objects = [...files.values()].flatMap((content) =>
    parseAllDocuments(content).map((document) => document.toJS()),
  );
  return { files, objects, releases: objects.filter((o) => o.kind === 'HelmRelease') };
}

export const release = (rendered: Rendered, chart: string) =>
  rendered.releases.find((r) => r.metadata.name.endsWith(`-${chart}`));

export const releaseValues = (rendered: Rendered, chart: string) =>
  release(rendered, chart)?.spec.values;

export const provider = (port: unknown = 'http', extra: Record<string, unknown> = {}) =>
  validate(
    {
      name: 'gw',
      network: { ingress: [] },
      exports: [{ name: 'api', workload: 'gw', port }],
      workloads: [
        {
          name: 'gw',
          image: 'i',
          service: { name: 'api' },
          ports: [
            { name: 'http', container: 3000, service: 80 },
            { name: 'grpc', container: 50051, service: 50051 },
          ],
        },
      ],
      ...extra,
    },
    'test',
  );

export const base = { name: 'foo', network: { ingress: [] } };

export const checkRefs = (app: Record<string, unknown>) =>
  validateResources({ platform, apps: [validate({ ...base, ...app }, 'test')] });

export const overridden: App = {
  name: 'foo',
  namespace: { name: 'kube-system' },
  network: { ingress: [], policy: false },
  resources: [
    {
      name: 'images',
      type: 'bucket',
      provider: 'r2',
      bucket: 'foo-images',
      secretName: 'images-bucket-secrets',
      secretTemplate: { type: 'example.io/config', data: { bucket: '{{ .S3_BUCKET }}' } },
    },
  ],
};

export const subgraphApp = (name: string, subgraph: Record<string, unknown> = {}): App => ({
  name,
  workloads: [
    {
      name: `${name}-api`,
      image: 'i',
      ports: [{ name: 'http', container: 8000, service: 80 }],
      service: { name: 'api' },
    },
  ],
  resources: [
    {
      name,
      type: 'subgraph',
      prefix: name.replace(/-./g, (c) => c[1]!.toUpperCase()),
      workload: `${name}-api`,
      port: 'http',
      path: '/graphql',
      repository: `https://github.com/Accurate0/${name}`,
      ref: 'main',
      schema: 'subgraph.graphql',
      ...subgraph,
    } as App['resources'] extends (infer R)[] | undefined ? R : never,
  ],
  network: { ingress: [] },
});

export const routerApp: App = {
  name: 'graph',
  resources: [{ name: 'graph', type: 'supergraph', base: 'router.base.yaml' }],
  network: { ingress: [] },
};
