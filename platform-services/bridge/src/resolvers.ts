import { type Build, delivery } from './delivery.ts';
import {
  type App,
  appNames,
  assemble,
  type Cluster,
  type ConfigMap,
  containerImages,
  type FluxObject,
  health,
  type Image,
  lastDeployed,
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
import type { Resolvers } from './schema.ts';

export interface Sources {
  kubernetes: Kubernetes;
  forgejo: Forgejo;
  // the repository in-repo apps are built from, by their app-<name>.yaml workflow
  buildRepository: string;
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

const RUN_PAGE = 50;

export function resolvers(sources: Sources): Resolvers {
  const { kubernetes, forgejo } = sources;

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
      paths.workloads(namespace).map(async ({ kind, path }) =>
        (await kubernetes.list<Workload>(path)).map((workload) => ({
          ...workload,
          kind,
        })),
      ),
    );
    return lists.flat();
  };

  const withPods = async (list: Workload[]): Promise<WorkloadModel[]> => {
    const namespaces = [...new Set(list.map((w) => w.metadata.namespace ?? 'default'))];
    const pods = new Map(
      await Promise.all(
        namespaces.map(async (ns) => [ns, await kubernetes.list<Pod>(paths.pods(ns))] as const),
      ),
    );
    return list.map((workload) => ({
      workload,
      pods: podsOf(workload, pods.get(workload.metadata.namespace ?? 'default') ?? []),
    }));
  };

  const appWorkloads = async (app: App) => withPods(workloadsFor(app, await workloads()));

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
          build && ['failure', 'cancelled'].includes(build.json.status)
            ? (await forgejo.jobs(build.repository, build.json.id)).find((job) =>
                ['failure', 'cancelled'].includes(job.status),
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

  const entity = async (representation: { __typename?: string; id?: string }) => {
    const [service, node, ...rest] = (representation.id ?? '').split(':');
    const key = rest.join(':');
    if (representation.__typename === 'FluxApp' && service === 'flux' && node === 'app') {
      const found = assemble(key, await cluster());
      return found ? app(found) : null;
    }

    const split = key.lastIndexOf(':');
    const repository = key.slice(0, split);
    const id = key.slice(split + 1);
    if (representation.__typename === 'ForgejoCommit' && node === 'commit') {
      return (await commit(repository, id)) ?? null;
    }
    if (representation.__typename === 'ForgejoRun' && node === 'run') {
      const json = await forgejo.run(repository, Number(id));
      return json ? run(repository)(json) : null;
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
      fluxApp: async (_, { name }: { name: string }) => {
        const found = assemble(name, await cluster());
        return found ? app(found) : null;
      },
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
          .slice(0, Math.min(Math.max(limit, 1), RUN_PAGE))
          .map(run(repository)),
      _entities: (
        _,
        { representations }: { representations: { __typename?: string; id?: string }[] },
      ) => Promise.all(representations.map(entity)),
      _service: () => ({ sdl: '' }),
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
