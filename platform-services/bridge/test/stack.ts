import type { AddressInfo } from 'node:net';
import { Authenticator } from '../src/auth.ts';
import { Forgejo } from '../src/forgejo.ts';
import { Kubernetes, watched } from '../src/kubernetes.ts';
import { resolvers } from '../src/resolvers.ts';
import { executableSchema } from '../src/schema.ts';
import { server } from '../src/server.ts';
import { fetcher, JWKS, keypair, passport } from './helpers.ts';
import { FakeKube } from './kube.ts';

export const APP = { annotations: { 'event.toolkit.fluxcd.io/app': 'graph' } };
export const READY = {
  observedGeneration: 1,
  conditions: [{ type: 'Ready', status: 'True' }],
};
export const DIGEST = `sha256:${'a'.repeat(64)}`;

export const PATHS = {
  kustomizations: '/apis/kustomize.toolkit.fluxcd.io/v1/namespaces/flux-system/kustomizations',
  helmReleases: '/apis/helm.toolkit.fluxcd.io/v2/namespaces/flux-system/helmreleases',
  deployments: '/apis/apps/v1/deployments',
  pods: '/api/v1/pods',
};

export const ROUTER = {
  metadata: {
    name: 'router',
    namespace: 'graph',
    annotations: {
      'meta.helm.sh/release-name': 'graph-k8s-workload',
      'meta.helm.sh/release-namespace': 'graph',
    },
  },
  spec: {
    replicas: 2,
    selector: { matchLabels: { app: 'router' } },
    template: {
      spec: {
        containers: [
          { name: 'router', image: 'registry.anurag.sh/containers/graph-router:v1.0.69-20740f6' },
        ],
      },
    },
  },
  status: { readyReplicas: 2 },
};

export const ROUTER_POD = {
  metadata: {
    name: 'router-1',
    namespace: 'graph',
    labels: { app: 'router' },
    ownerReferences: [{ kind: 'ReplicaSet', name: 'router-6f9c' }],
  },
  spec: {
    nodeName: 'k8s-1',
    containers: [
      { name: 'router', image: 'registry.anurag.sh/containers/graph-router:v1.0.69-20740f6' },
    ],
  },
  status: {
    phase: 'Running',
    startTime: '2026-11-25T13:30:00Z',
    conditions: [{ type: 'Ready', status: 'True' }],
    containerStatuses: [
      {
        name: 'router',
        ready: true,
        restartCount: 1,
        imageID: `registry.anurag.sh/containers/graph-router@${DIGEST}`,
        lastState: {
          terminated: { reason: 'OOMKilled', exitCode: 137, finishedAt: '2026-11-25T13:29:00Z' },
        },
      },
    ],
  },
};

const lists = () => ({
  [PATHS.kustomizations]: [
    {
      metadata: { name: 'graph', generation: 1, ...APP },
      spec: {},
      status: { ...READY, lastAppliedRevision: 'main@sha1:dc81672' },
    },
    {
      metadata: { name: 'graph-manifests', generation: 1, ...APP },
      spec: {},
      status: READY,
    },
    {
      metadata: {
        name: 'flux-system',
        generation: 1,
        annotations: { 'event.toolkit.fluxcd.io/app': 'flux-system' },
      },
      spec: {},
      status: READY,
    },
  ],
  [PATHS.helmReleases]: [
    {
      metadata: { name: 'graph-k8s-workload', generation: 1, ...APP },
      spec: { releaseName: 'graph-k8s-workload', targetNamespace: 'graph' },
      status: {
        ...READY,
        history: [{ chartVersion: '1.2.0', lastDeployed: '2026-11-25T13:20:00Z' }],
      },
    },
  ],
  '/api/v1/namespaces/flux-system/configmaps': [
    {
      metadata: { name: 'graph-image-values' },
      data: {
        'values.yaml':
          'images:\n  curlimages/curl: 8.18.0\n  registry.anurag.sh/containers/graph-router: v1.0.69-20740f6 # {"$imagepolicy": "flux-system:graph-graph-router:tag"}\n',
      },
    },
  ],
  '/apis/image.toolkit.fluxcd.io/v1/namespaces/flux-system/imagepolicies': [
    {
      metadata: { name: 'graph-graph-router', ...APP },
      spec: { imageRepositoryRef: { name: 'graph-graph-router' } },
      status: {
        latestRef: {
          name: 'registry.anurag.sh/containers/graph-router',
          tag: 'v1.0.69-20740f6',
        },
      },
    },
  ],
  '/apis/image.toolkit.fluxcd.io/v1/namespaces/flux-system/imagerepositories': [
    {
      metadata: { name: 'graph-graph-router', ...APP },
      spec: { image: 'registry.anurag.sh/containers/graph-router' },
      status: { lastScanResult: { scanTime: '2026-11-25T13:06:27Z' } },
    },
  ],
  [PATHS.deployments]: [ROUTER],
  '/apis/apps/v1/statefulsets': [],
  '/apis/apps/v1/daemonsets': [],
  '/apis/batch/v1/cronjobs': [],
  '/apis/batch/v1/jobs': [
    {
      metadata: {
        name: 'supergraph-upload',
        namespace: 'graph',
        annotations: {
          'meta.helm.sh/release-name': 'graph-k8s-workload',
          'meta.helm.sh/release-namespace': 'graph',
        },
      },
      spec: {
        template: { spec: { containers: [{ name: 'upload', image: 'curlimages/curl:8.18.0' }] } },
      },
    },
  ],
  [PATHS.pods]: [
    ROUTER_POD,
    {
      metadata: { name: 'router-1', namespace: 'other', labels: { app: 'router' } },
      status: { containerStatuses: [{ name: 'router', restartCount: 9 }] },
    },
  ],
  '/apis/apps/v1/namespaces/graph/replicasets': [
    {
      metadata: {
        name: 'router-6f9c',
        namespace: 'graph',
        ownerReferences: [{ kind: 'Deployment', name: 'router' }],
      },
    },
  ],
  '/api/v1/namespaces/graph/events': [
    {
      metadata: { name: 'e1', namespace: 'graph', creationTimestamp: '2026-11-25T13:00:00Z' },
      type: 'Normal',
      reason: 'ScalingReplicaSet',
      message: 'Scaled up replica set router-6f9c to 2',
      count: 1,
      lastTimestamp: '2026-11-25T13:28:00Z',
      involvedObject: { kind: 'Deployment', name: 'router' },
    },
    {
      metadata: { name: 'e2', namespace: 'graph' },
      type: 'Warning',
      reason: 'BackOff',
      message: 'Back-off restarting failed container',
      series: { count: 4, lastObservedTime: '2026-11-25T13:29:30Z' },
      involvedObject: { kind: 'Pod', name: 'router-1' },
    },
    {
      metadata: { name: 'e3', namespace: 'graph' },
      type: 'Normal',
      reason: 'Pulled',
      count: 1,
      lastTimestamp: '2026-11-25T13:29:00Z',
      involvedObject: { kind: 'Pod', name: 'someone-else' },
    },
  ],
});

const texts = {
  '/api/v1/namespaces/graph/pods/router-1/log?container=router&tailLines=2&previous=false':
    'listening\nserving\n',
};

export const FORGEJO_ROUTES = {
  '/actions/runs?page=1&limit=50': {
    workflow_runs: [
      {
        id: 3703,
        index_in_repo: 2682,
        workflow_id: 'app-graph.yaml',
        commit_sha: '204a20cbb3b0bf88',
        status: 'success',
        title: 'graph: me plugin',
        stopped: '2026-11-25T13:08:00Z',
        html_url: 'https://git/runs/2682',
        prettyref: 'main',
      },
    ],
  },
  '/actions/runs/3703': {
    id: 3703,
    index_in_repo: 2682,
    workflow_id: 'app-graph.yaml',
    commit_sha: '204a20cbb3b0bf88',
    status: 'success',
    title: 'graph: me plugin',
    html_url: 'https://git/runs/2682',
  },
  '/actions/runs/3703/rerun': {},
  '/actions/runs/3703/cancel': {},
  '/actions/runs/3703/jobs': [
    {
      id: 6108,
      name: 'release',
      status: 'success',
      html_url: 'https://git/jobs/2',
    },
  ],
  '/actions/jobs/6108/logs': 'line 1\nline 2\npushed v1.0.75-204a20c\n',
  '/git/commits/20740f6': {
    sha: '20740f6aaa',
    commit: {
      message: 'update schemas\n\nbody',
      author: { name: 'Anurag', date: '2026-11-25T11:05:00Z' },
    },
  },
};

export const READ =
  'flux:app:read forgejo:run:read forgejo:log:read kubernetes:workload:read kubernetes:log:read';
export const WRITE = 'flux:app:write kubernetes:workload:write forgejo:run:write';

export async function stack() {
  const key = keypair('graph-1');
  const kube = new FakeKube(lists(), texts);
  const forgejoCalls: { method: string; url: string }[] = [];
  const forgejoFetch = fetcher(FORGEJO_ROUTES);
  const kubernetes = new Kubernetes({
    url: 'https://kube',
    token: async () => 't',
    fetcher: kube.fetcher,
  });
  await kubernetes.watch(watched);

  const schema = executableSchema(
    resolvers({
      kubernetes,
      forgejo: new Forgejo('https://git', 't', 'https://git.example.test', (async (
        input: string | URL | Request,
        init?: RequestInit,
      ) => {
        forgejoCalls.push({ method: init?.method ?? 'GET', url: String(input) });
        return forgejoFetch(input, init);
      }) as typeof fetch),
      buildRepository: 'anurag/k8s',
      debounceMs: 10,
    }),
  );
  const auth = new Authenticator(
    { audience: 'bridge', jwks: JWKS },
    fetcher({ '/.well-known/passport.jwks': { keys: [key.jwk] } }),
  );
  const app = server(schema, auth);
  await app.listen({ port: 0, host: '127.0.0.1' });
  const url = `http://127.0.0.1:${(app.server.address() as AddressInfo).port}`;

  const token = (scope: string) => key.token(passport(scope.split(' ')));

  const post = async (
    scope: string,
    query: string,
    extra: Record<string, string> = {},
    variables?: Record<string, unknown>,
  ) => {
    const response = await fetch(`${url}/v1/graphql`, {
      method: 'POST',
      headers: {
        ...extra,
        'content-type': 'application/json',
        'x-graph-passport': token(scope),
      },
      body: JSON.stringify({ query, variables }),
    });
    return { status: response.status, body: (await response.json()) as any };
  };

  return {
    url,
    kube,
    forgejoCalls,
    token,
    post,
    close: async () => {
      kubernetes.stop();
      await app.close();
    },
  };
}
