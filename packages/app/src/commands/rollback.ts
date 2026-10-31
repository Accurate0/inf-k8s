import { applying, ghcrDigest, type Kube, parseImage, short } from '@ortzi/kit';
import {
  type Application,
  assertChangeable,
  getApp,
  historyNewestFirst,
  turnAutosyncOff,
} from '../argo';
import { gitRevision, syncedTags, tagsAt } from '../git';
import { previousTags } from '../images';
import {
  type Workload,
  containersOf,
  listWorkloads,
  runningTags,
  waitForRollouts,
  workloadRef,
  workloadsFor,
} from '../workloads';

interface Edit {
  workload: Workload;
  container: string;
  repository: string;
  from: string;
  to: string;
}

function chooseTargets(
  app: Application,
  workloads: Workload[],
  tag: string | undefined,
): Map<string, string> {
  if (tag) return new Map([...syncedTags(app).keys()].map((image) => [image, tag]));
  const previous = previousTags(
    runningTags(workloads),
    historyNewestFirst(app).map((entry) => tagsAt(app, gitRevision(app, entry))),
  );
  if (!previous) throw new Error(`no different tags in ${app.metadata.name}'s history; pass a tag`);
  return previous;
}

function onlyImage(targets: Map<string, string>, app: string, image: string): Map<string, string> {
  const matches = [...targets].filter(([name]) => name === image || name.endsWith(`/${image}`));
  if (matches.length === 0) {
    throw new Error(`${app} has no image ${image}; it has ${[...targets.keys()].join(', ')}`);
  }
  return new Map(matches);
}

function assertTagsExist(targets: Map<string, string>): void {
  const missing = [...targets].filter(
    ([image, tag]) => image.startsWith('ghcr.io/') && !ghcrDigest(image, tag),
  );
  if (missing.length === 0) return;
  const lines = missing.map(([image, tag]) => `${image}:${tag} does not exist in ghcr`);
  throw new Error([...lines, 'nothing changed; use --image to limit the rollback'].join('\n'));
}

function planEdits(workloads: Workload[], targets: Map<string, string>): Edit[] {
  return workloads.flatMap((workload) =>
    containersOf(workload).flatMap((container) => {
      const { repository, tag: from } = parseImage(container.image);
      const to = targets.get(repository);
      return to && to !== from ? [{ workload, container: container.name, repository, from, to }] : [];
    }),
  );
}

function applyEdit(kube: Kube, { workload, container, repository, to }: Edit): void {
  kube
    .namespace(workload.metadata.namespace)
    .capture(['set', 'image', workloadRef(workload), `${container}=${repository}:${to}`]);
}

export interface RollbackOptions {
  tag?: string;
  image?: string;
  yes: boolean;
  wait: boolean;
  timeoutMinutes: number;
}

export function rollback(kube: Kube, name: string, options: RollbackOptions): number {
  const { tag, image, yes } = options;
  assertChangeable(name);
  const app = getApp(kube, name);
  const workloads = workloadsFor(app, listWorkloads(kube));

  const chosen = chooseTargets(app, workloads, tag);
  const targets = image ? onlyImage(chosen, name, image) : chosen;
  assertTagsExist(targets);

  const edits = planEdits(workloads, targets);
  if (edits.length === 0) {
    console.log(`${name} is already running those tags`);
    return 0;
  }

  console.log(`${yes ? 'rolling back' : 'would roll back'} ${name}:`);
  for (const { workload, container, from, to } of edits) {
    console.log(`  ${workloadRef(workload)} ${container}: ${short(from)} -> ${short(to)}`);
  }
  if (!yes) console.log('auto-sync would be turned off');
  if (!applying(yes)) return 0;

  turnAutosyncOff(kube, app);
  for (const edit of edits) applyEdit(kube, edit);
  const undo = `auto-sync is off; to return to git: ortzi app autosync ${name} on --yes`;
  if (!options.wait) {
    console.log(undo);
    return 0;
  }

  const changed = [...new Set(edits.map((edit) => edit.workload))];
  if (!waitForRollouts(kube, changed, options.timeoutMinutes)) {
    console.error(`rollback of ${name} did not finish; check \`ortzi app status ${name}\``);
    console.error(undo);
    return 1;
  }
  console.log(`rolled back ${name}`);
  console.log(undo);
  return 0;
}
