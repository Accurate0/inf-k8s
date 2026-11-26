import { trace } from '@opentelemetry/api';
import { GraphQLError } from 'graphql';
import { type Build, delivery } from './delivery.ts';
import {
  type App,
  appNames,
  assemble,
  type Cluster,
  type ConfigMap,
  type ContainerStatus,
  containerImages,
  type FluxObject,
  health,
  type Image,
  imageDigest,
  lastDeployed,
  type Metadata,
  objectsOf,
  type Pod,
  podsOf,
  ready,
  readyMessage,
  replicas,
  revision,
  splitImage,
  tagSha,
  type Workload,
  workloadsFor,
} from './flux.ts';
import type { Forgejo, ForgejoCommitJson, ForgejoJobJson, ForgejoRunJson } from './forgejo.ts';
import { type Kubernetes, paths } from './kubernetes.ts';
import type { Context, Resolvers } from './schema.ts';
import { changed, updates } from './updates.ts';

export interface Sources {
  kubernetes: Kubernetes;
  forgejo: Forgejo;
  // the repository in-repo apps are built from, by their app-<name>.yaml workflow
  buildRepository: string;
  // images on this host are in the registry subgraph's ledger
  registryHost?: string;
  debounceMs?: number;
}

interface AppModel {
  __typename: 'FluxApp';
  app: App;
}

interface ImageModel {
  app: App;
  image: Image;
}

interface WorkloadModel {
  workload: Workload;
  pods: Pod[];
}

interface ContainerModel {
  pod: Pod;
  name: string;
  image: string;
  status?: ContainerStatus;
}

interface CommitModel {
  __typename: 'ForgejoCommit';
  repository: string;
  json: ForgejoCommitJson;
}

interface RunModel {
  __typename: 'ForgejoRun';
  repository: string;
  json: ForgejoRunJson;
}

interface JobModel {
  repository: string;
  json: ForgejoJobJson;
}

interface KubeEvent {
  metadata: Metadata & { creationTimestamp?: string };
  type?: string;
  reason?: string;
  message?: string;
  count?: number;
  lastTimestamp?: string;
  eventTime?: string;
  series?: { count?: number; lastObservedTime?: string };
  involvedObject: { kind: string; name: string };
}

const RUN_PAGE = 50;
const BUILD_POLL_MS = 15_000;
const RESTARTABLE = new Set(['Deployment', 'StatefulSet', 'DaemonSet']);
const FAILED = ['failure', 'cancelled'];

const clamp = (value: number, min: number, max: number) => Math.min(Math.max(value, min), max);

const userError = (message: string, code = 'BAD_USER_INPUT') =>
  new GraphQLError(message, { extensions: { code } });

function audit({ principal }: Context, mutation: string, args: Record<string, unknown>) {
  trace.getActiveSpan()?.setAttribute('enduser.id', principal.subject);
  console.log(`${principal.subject} ${mutation} ${JSON.stringify(args)}`);
}

// forgejo:<node>:<owner>/<repo>:<id>
function forgejoKey(id: string): { node: string; repository: string; key: string } | undefined {
  const [service, node, ...rest] = id.split(':');
  const joined = rest.join(':');
  const split = joined.lastIndexOf(':');
  if (service !== 'forgejo' || !node || split < 0) {
    return undefined;
  }
  return { node, repository: joined.slice(0, split), key: joined.slice(split + 1) };
}

const eventSeenAt = (event: KubeEvent) =>
  event.lastTimestamp ??
  event.series?.lastObservedTime ??
  event.eventTime ??
  event.metadata.creationTimestamp;

export function resolvers(sources: Sources): Resolvers {
  const { kubernetes, forgejo } = sources;
  const registryHost = sources.registryHost ?? 'registry.anurag.sh';
  const debounceMs = sources.debounceMs ?? 1_000;

  const cluster = async (): Promise<Cluster> => {
    const [kustomizations, helmReleases, configMaps, imagePolicies, imageRepositories] =
      await Promise.all([
        kubernetes.list<FluxObject>(paths.kustomizations),
        kubernetes.list<FluxObject>(paths.helmReleases),
        kubernetes.list<ConfigMap>(paths.configMaps),
        kubernetes.list<FluxObject>(paths.imagePolicies),
        kubernetes.list<FluxObject>(paths.imageRepositories),
      ]);
    return {
      kustomizations,
      helmReleases,
      configMaps,
      imagePolicies,
      imageRepositories,
    };
  };

  const workloads = async (namespace?: string): Promise<Workload[]> => {
    const lists = await Promise.all(
      paths.workloads.map(async ({ kind, path }) =>
        (await kubernetes.list<Workload>(path))
          .filter((workload) => !namespace || workload.metadata.namespace === namespace)
          .map((workload) => ({ ...workload, kind })),
      ),
    );
    return lists.flat();
  };

  const withPods = async (list: Workload[]): Promise<WorkloadModel[]> => {
    const pods = await kubernetes.list<Pod>(paths.pods);
    return list.map((workload) => ({ workload, pods: podsOf(workload, pods) }));
  };

  const appWorkloads = async (app: App) => withPods(workloadsFor(app, await workloads()));

  const containers = (pod: Pod): ContainerModel[] =>
    (pod.spec?.containers ?? []).map(({ name, image }) => ({
      pod,
      name,
      image,
      status: pod.status?.containerStatuses?.find((status) => status.name === name),
    }));

  const registryPath = (repository: string) =>
    repository.startsWith(`${registryHost}/`)
      ? repository.slice(registryHost.length + 1)
      : undefined;

  const registryImage = (image: string, digest: string | undefined) => {
    const repository = registryPath(splitImage(image).repository);
    return repository && digest ? { repository, digest } : null;
  };

  const runningTags = async ({ app, image }: ImageModel) =>
    [
      ...new Set(
        workloadsFor(app, await workloads())
          .flatMap(containerImages)
          .map(splitImage)
          .filter((parsed) => parsed.repository === image.repository && parsed.tag)
          .map((parsed) => parsed.tag!),
      ),
    ].sort();

  const latestBuild = async (app: App): Promise<RunModel | undefined> => {
    const workflow = `app-${app.name}.yaml`;
    const json = (await forgejo.runs(sources.buildRepository, RUN_PAGE)).find(
      (run) => run.workflow_id === workflow && (run.prettyref ?? 'main') === 'main',
    );
    return (
      json && {
        __typename: 'ForgejoRun',
        repository: sources.buildRepository,
        json,
      }
    );
  };

  const commit = async (repository: string, sha: string): Promise<CommitModel | undefined> => {
    const json = await forgejo.commit(repository, sha);
    return json && { __typename: 'ForgejoCommit', repository, json };
  };

  const run =
    (repository: string) =>
    (json: ForgejoRunJson): RunModel => ({
      __typename: 'ForgejoRun',
      repository,
      json,
    });

  const stages = new WeakMap<
    ImageModel,
    ReturnType<typeof delivery> | Promise<ReturnType<typeof delivery>>
  >();
  const stageOf = (model: ImageModel) => {
    let result = stages.get(model);
    if (!result) {
      result = (async () => {
        const build = model.image.policy ? await latestBuild(model.app) : undefined;
        const failedJob =
          build && FAILED.includes(build.json.status)
            ? (await forgejo.jobs(build.repository, build.json.id)).find((job) =>
                FAILED.includes(job.status),
              )?.name
            : undefined;
        const buildInfo: Build | undefined = build && {
          number: build.json.index_in_repo,
          sha: build.json.commit_sha,
          status: build.json.status,
          stoppedAt: build.json.stopped ?? undefined,
          failedJob,
        };
        const models = await appWorkloads(model.app);
        return delivery({
          selectedTag: model.image.policy?.status?.latestRef?.tag,
          desiredTag: model.image.desiredTag,
          runningTags: await runningTags(model),
          lastScanAt: model.image.scan?.status?.lastScanResult?.scanTime,
          build: buildInfo,
          workloads: models.map(({ workload, pods }) => ({
            name: `${workload.kind.toLowerCase()}/${workload.metadata.name}`,
            ...replicas(workload),
            ...health(pods),
          })),
        });
      })();
      stages.set(model, result);
    }
    return result;
  };

  const app = (value: App): AppModel => ({ __typename: 'FluxApp', app: value });

  const findApp = async (name: string) => {
    const found = assemble(name, await cluster());
    return found ? app(found) : null;
  };

  const changeable = async (name: string) => {
    if (name === 'flux-system') {
      throw userError('flux-system is not changed through the graph', 'FORBIDDEN');
    }
    return assemble(name, await cluster());
  };

  const requestReconcile = (target: App) =>
    kubernetes.patch(paths.fluxObject('kustomizations', target.name), {
      metadata: { annotations: { 'reconcile.fluxcd.io/requestedAt': new Date().toISOString() } },
    });

  // as `ortzi app autosync`: every kustomization and helm release of the app
  const setSuspended = async (target: App, suspend: boolean) => {
    for (const ks of [target.kustomization, ...target.kustomizations]) {
      await kubernetes.patch(paths.fluxObject('kustomizations', ks.metadata.name), {
        spec: { suspend },
      });
    }
    for (const release of target.releases) {
      await kubernetes.patch(paths.fluxObject('helmreleases', release.metadata.name), {
        spec: { suspend },
      });
    }
  };

  const runById = async (id: string) => {
    const parsed = forgejoKey(id);
    if (parsed?.node !== 'run' || !/^\d+$/.test(parsed.key)) {
      throw userError(`${id} is not a ForgejoRun id`);
    }
    return { repository: parsed.repository, run: Number(parsed.key) };
  };

  const freshRun = async (repository: string, id: number) => {
    const json = await forgejo.run(repository, id);
    return json ? run(repository)(json) : null;
  };

  const appFingerprint = async (target: App) => {
    const models = await appWorkloads(target);
    const build = target.images.some((image) => image.policy)
      ? await latestBuild(target)
      : undefined;
    return JSON.stringify([
      objectsOf(target).map((object) => object.metadata.resourceVersion),
      target.images.map((image) => [
        image.desiredTag,
        image.policy?.metadata.resourceVersion,
        image.scan?.metadata.resourceVersion,
      ]),
      models.map(({ workload, pods }) => [
        workload.metadata.resourceVersion,
        pods.map((pod) => pod.metadata.resourceVersion),
      ]),
      build && [build.json.id, build.json.status],
    ]);
  };

  const entity = async (representation: { __typename?: string; id?: string }) => {
    const [service, node, ...rest] = (representation.id ?? '').split(':');
    const key = rest.join(':');
    if (representation.__typename === 'FluxApp' && service === 'flux' && node === 'app') {
      return findApp(key);
    }

    const parsed = forgejoKey(representation.id ?? '');
    if (representation.__typename === 'ForgejoCommit' && parsed?.node === 'commit') {
      return (await commit(parsed.repository, parsed.key)) ?? null;
    }
    if (representation.__typename === 'ForgejoRun' && parsed?.node === 'run') {
      return freshRun(parsed.repository, Number(parsed.key));
    }
    return null;
  };

  return {
    Query: {
      fluxApps: async () => {
        const state = await cluster();
        return appNames(state).flatMap((name) => {
          const found = assemble(name, state);
          return found ? [app(found)] : [];
        });
      },
      fluxApp: (_, { name }: { name: string }) => findApp(name),
      kubernetesWorkloads: async (_, { namespace }: { namespace: string }) =>
        withPods(await workloads(namespace)),
      forgejoCommit: (_, { repository, sha }: { repository: string; sha: string }) =>
        commit(repository, sha),
      forgejoRuns: async (
        _,
        { repository, workflow, limit }: { repository: string; workflow?: string; limit: number },
      ) =>
        (await forgejo.runs(repository, RUN_PAGE))
          .filter((json) => !workflow || json.workflow_id === workflow)
          .slice(0, clamp(limit, 1, RUN_PAGE))
          .map(run(repository)),
      _entities: (
        _,
        { representations }: { representations: { __typename?: string; id?: string }[] },
      ) => Promise.all(representations.map(entity)),
      _service: () => ({ sdl: '' }),
    },

    Mutation: {
      fluxReconcile: async (_, args: { name: string }, context: Context) => {
        audit(context, 'fluxReconcile', args);
        const found = await changeable(args.name);
        if (!found) {
          return null;
        }
        await requestReconcile(found);
        return findApp(args.name);
      },
      fluxSuspend: async (_, args: { name: string }, context: Context) => {
        audit(context, 'fluxSuspend', args);
        const found = await changeable(args.name);
        if (!found) {
          return null;
        }
        await setSuspended(found, true);
        return findApp(args.name);
      },
      fluxResume: async (_, args: { name: string }, context: Context) => {
        audit(context, 'fluxResume', args);
        const found = await changeable(args.name);
        if (!found) {
          return null;
        }
        await setSuspended(found, false);
        await requestReconcile(found);
        return findApp(args.name);
      },
      kubernetesRestart: async (
        _,
        args: { namespace: string; kind: string; name: string },
        context: Context,
      ) => {
        audit(context, 'kubernetesRestart', args);
        const path = paths.workload(args.namespace, args.kind, args.name);
        if (!path || !RESTARTABLE.has(args.kind)) {
          throw userError(`only ${[...RESTARTABLE].join(', ')} restart`);
        }
        await kubernetes.patch(path, {
          spec: {
            template: {
              metadata: {
                annotations: { 'kubectl.kubernetes.io/restartedAt': new Date().toISOString() },
              },
            },
          },
        });
        const found = (await workloads(args.namespace)).find(
          (workload) => workload.kind === args.kind && workload.metadata.name === args.name,
        );
        return found ? (await withPods([found]))[0] : null;
      },
      forgejoRerun: async (_, args: { id: string }, context: Context) => {
        audit(context, 'forgejoRerun', args);
        const { repository, run: id } = await runById(args.id);
        await forgejo.act(repository, id, 'rerun');
        return freshRun(repository, id);
      },
      forgejoCancel: async (_, args: { id: string }, context: Context) => {
        audit(context, 'forgejoCancel', args);
        const { repository, run: id } = await runById(args.id);
        await forgejo.act(repository, id, 'cancel');
        return freshRun(repository, id);
      },
    },

    Subscription: {
      fluxAppUpdates: (_, { name }: { name: string }) => {
        const fresh = changed<AppModel>();
        return updates(
          async () => {
            const found = assemble(name, await cluster());
            return found
              ? fresh([{ key: name, fingerprint: await appFingerprint(found), value: app(found) }])
              : [];
          },
          { source: kubernetes.changes, debounceMs, intervalMs: BUILD_POLL_MS },
        );
      },
      kubernetesWorkloadUpdates: (_, { namespace }: { namespace: string }) => {
        const fresh = changed<WorkloadModel>();
        return updates(
          async () =>
            fresh(
              (await withPods(await workloads(namespace))).map((model) => ({
                key: `${model.workload.kind}/${model.workload.metadata.name}`,
                fingerprint: [
                  model.workload.metadata.resourceVersion,
                  ...model.pods.map((pod) => pod.metadata.resourceVersion),
                ].join(),
                value: model,
              })),
            ),
          { source: kubernetes.changes, debounceMs },
        );
      },
    },

    FluxApp: {
      id: ({ app }: AppModel) => `flux:app:${app.name}`,
      name: ({ app }: AppModel) => app.name,
      ready: ({ app }: AppModel) => objectsOf(app).every(ready),
      message: ({ app }: AppModel) => {
        const stuck = objectsOf(app).find((object) => !ready(object));
        return stuck ? (readyMessage(stuck) ?? 'not reconciled yet') : null;
      },
      suspended: ({ app }: AppModel) => app.kustomization.spec.suspend === true,
      revision: ({ app }: AppModel) => revision(app) ?? null,
      lastDeployedAt: ({ app }: AppModel) => lastDeployed(app) ?? null,
      helmReleases: ({ app }: AppModel) => app.releases,
      images: ({ app }: AppModel): ImageModel[] => app.images.map((image) => ({ app, image })),
      workloads: ({ app }: AppModel) => appWorkloads(app),
    },

    FluxHelmRelease: {
      name: (release: FluxObject) => release.metadata.name,
      ready: (release: FluxObject) => ready(release),
      message: (release: FluxObject) => readyMessage(release) ?? null,
      chartVersion: (release: FluxObject) => release.status?.history?.[0]?.chartVersion ?? null,
      lastDeployedAt: (release: FluxObject) => release.status?.history?.[0]?.lastDeployed ?? null,
    },

    FluxImage: {
      repository: ({ image }: ImageModel) => image.repository,
      desiredTag: ({ image }: ImageModel) => image.desiredTag ?? null,
      selectedTag: ({ image }: ImageModel) => image.policy?.status?.latestRef?.tag ?? null,
      lastScanAt: ({ image }: ImageModel) => image.scan?.status?.lastScanResult?.scanTime ?? null,
      runningTags: (model: ImageModel) => runningTags(model),
      commit: async ({ image }: ImageModel) => {
        const sha = tagSha(image.policy?.status?.latestRef?.tag);
        return sha ? ((await commit(sources.buildRepository, sha)) ?? null) : null;
      },
      latestBuild: async ({ app, image }: ImageModel) =>
        image.policy ? ((await latestBuild(app)) ?? null) : null,
      stage: async (model: ImageModel) => (await stageOf(model)).stage,
      blocker: async (model: ImageModel) => (await stageOf(model)).blocker ?? null,
      registryRepository: ({ image }: ImageModel) => {
        const path = registryPath(image.repository);
        return path ? { id: `registry:imageRepository:${path}` } : null;
      },
      runningImages: async ({ app, image }: ImageModel) => {
        const path = registryPath(image.repository);
        if (!path) {
          return null;
        }
        const digests = (await appWorkloads(app))
          .flatMap(({ pods }) => pods.flatMap(containers))
          .filter((container) => splitImage(container.image).repository === image.repository)
          .map((container) => imageDigest(container.status))
          .filter((digest): digest is string => Boolean(digest));
        return [...new Set(digests)].sort().map((digest) => ({ repository: path, digest }));
      },
    },

    KubernetesWorkload: {
      kind: ({ workload }: WorkloadModel) => workload.kind,
      name: ({ workload }: WorkloadModel) => workload.metadata.name,
      namespace: ({ workload }: WorkloadModel) => workload.metadata.namespace ?? 'default',
      ready: ({ workload }: WorkloadModel) => replicas(workload).ready,
      desired: ({ workload }: WorkloadModel) => replicas(workload).desired,
      images: ({ workload }: WorkloadModel) => containerImages(workload),
      restarts: ({ pods }: WorkloadModel) => health(pods).restarts,
      waitingReason: ({ pods }: WorkloadModel) => health(pods).waitingReason ?? null,
      pods: ({ pods }: WorkloadModel) =>
        [...pods].sort((a, b) => a.metadata.name.localeCompare(b.metadata.name)),
      events: async ({ workload, pods }: WorkloadModel, { limit }: { limit: number }) => {
        const namespace = workload.metadata.namespace ?? 'default';
        const [events, replicaSets] = await Promise.all([
          kubernetes.list<KubeEvent>(paths.events(namespace)),
          workload.kind === 'Deployment'
            ? kubernetes.list<{ metadata: Metadata }>(paths.replicaSets(namespace))
            : [],
        ]);
        const owned = replicaSets.filter((rs) =>
          rs.metadata.ownerReferences?.some(
            (owner) => owner.kind === 'Deployment' && owner.name === workload.metadata.name,
          ),
        );
        const involved = new Set([
          `${workload.kind}/${workload.metadata.name}`,
          ...owned.map((rs) => `ReplicaSet/${rs.metadata.name}`),
          ...pods.map((pod) => `Pod/${pod.metadata.name}`),
        ]);
        return events
          .filter(({ involvedObject }) =>
            involved.has(`${involvedObject.kind}/${involvedObject.name}`),
          )
          .sort((a, b) => (eventSeenAt(b) ?? '').localeCompare(eventSeenAt(a) ?? ''))
          .slice(0, clamp(limit, 1, 100));
      },
    },

    KubernetesPod: {
      name: (pod: Pod) => pod.metadata.name,
      phase: (pod: Pod) => pod.status?.phase ?? null,
      node: (pod: Pod) => pod.spec?.nodeName ?? null,
      ready: (pod: Pod) =>
        pod.status?.conditions?.some((c) => c.type === 'Ready' && c.status === 'True') ?? false,
      restarts: (pod: Pod) => health([pod]).restarts,
      startedAt: (pod: Pod) => pod.status?.startTime ?? null,
      containers: (pod: Pod) => containers(pod),
    },

    KubernetesContainer: {
      name: ({ name }: ContainerModel) => name,
      image: ({ image }: ContainerModel) => image,
      imageDigest: ({ status }: ContainerModel) => imageDigest(status) ?? null,
      ready: ({ status }: ContainerModel) => status?.ready ?? false,
      restarts: ({ status }: ContainerModel) => status?.restartCount ?? 0,
      waitingReason: ({ status }: ContainerModel) => status?.state?.waiting?.reason ?? null,
      lastTermination: ({ status }: ContainerModel) => status?.lastState?.terminated ?? null,
      logTail: async (
        { pod, name }: ContainerModel,
        { lines, previous }: { lines: number; previous: boolean },
      ) => {
        const query = new URLSearchParams({
          container: name,
          tailLines: String(clamp(lines, 1, 500)),
          previous: String(previous),
        });
        const log = await kubernetes.text(
          `${paths.log(pod.metadata.namespace ?? 'default', pod.metadata.name)}?${query}`,
        );
        return log?.replace(/\s+$/, '') ?? null;
      },
      registryImage: ({ image, status }: ContainerModel) =>
        registryImage(image, imageDigest(status)),
    },

    KubernetesTermination: {
      reason: (terminated: { reason?: string }) => terminated.reason ?? null,
      exitCode: (terminated: { exitCode?: number }) => terminated.exitCode ?? null,
      finishedAt: (terminated: { finishedAt?: string }) => terminated.finishedAt ?? null,
    },

    KubernetesEvent: {
      type: (event: KubeEvent) => event.type ?? 'Normal',
      reason: (event: KubeEvent) => event.reason ?? null,
      message: (event: KubeEvent) => event.message ?? null,
      count: (event: KubeEvent) => event.count ?? event.series?.count ?? 1,
      object: ({ involvedObject }: KubeEvent) =>
        `${involvedObject.kind.toLowerCase()}/${involvedObject.name}`,
      lastSeenAt: (event: KubeEvent) => eventSeenAt(event) ?? null,
    },

    ForgejoCommit: {
      id: ({ repository, json }: CommitModel) => `forgejo:commit:${repository}:${json.sha}`,
      repository: ({ repository }: CommitModel) => repository,
      sha: ({ json }: CommitModel) => json.sha,
      title: ({ json }: CommitModel) => json.commit.message.split('\n')[0] ?? null,
      author: ({ json }: CommitModel) => json.commit.author?.name ?? null,
      committedAt: ({ json }: CommitModel) => json.commit.author?.date ?? null,
      url: ({ repository, json }: CommitModel) =>
        json.html_url ?? `${forgejo.publicUrl}/${repository}/commit/${json.sha}`,
      runs: async ({ repository, json }: CommitModel) =>
        (await forgejo.runs(repository, RUN_PAGE))
          .filter((candidate) => candidate.commit_sha === json.sha)
          .map(run(repository)),
    },

    ForgejoRun: {
      id: ({ repository, json }: RunModel) => `forgejo:run:${repository}:${json.id}`,
      repository: ({ repository }: RunModel) => repository,
      number: ({ json }: RunModel) => json.index_in_repo,
      workflow: ({ json }: RunModel) => json.workflow_id,
      sha: ({ json }: RunModel) => json.commit_sha,
      status: ({ json }: RunModel) => json.status,
      title: ({ json }: RunModel) => json.title,
      startedAt: ({ json }: RunModel) => json.started ?? null,
      stoppedAt: ({ json }: RunModel) => json.stopped ?? null,
      url: ({ json }: RunModel) => json.html_url,
      jobs: async ({ repository, json }: RunModel): Promise<JobModel[]> =>
        (await forgejo.jobs(repository, json.id)).map((job) => ({
          repository,
          json: job,
        })),
    },

    ForgejoJob: {
      id: ({ json }: JobModel) => String(json.id),
      name: ({ json }: JobModel) => json.name,
      status: ({ json }: JobModel) => json.status,
      url: ({ json }: JobModel) => json.html_url,
      logTail: ({ repository, json }: JobModel, { lines }: { lines: number }) =>
        forgejo.logTail(repository, json.id, lines),
    },
  };
}
