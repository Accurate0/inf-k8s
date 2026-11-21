import { prompt, stdinIsTerminal } from '@ortzi/host';
import {
  applying,
  columns,
  ghcrDigest,
  type Kube,
  minuteTimestamp,
  parseImage,
  short,
} from '@ortzi/kit';
import { type App, assertChangeable, desiredTags, getApp, setSuspended } from '../flux';
import { RELEASE_REGISTRY } from '../providers/workload';
import { type Row, releaseRows, rowsOf } from '../promotion';
import { type Promotion, promote, promotions } from '../registry';
import { describe } from './promote';
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

function chooseTargets(app: App, tag: string | undefined): Map<string, string> {
  if (!tag) {
    throw new Error(`${app.name} has no released images; pass the tag to roll back to`);
  }
  return new Map([...desiredTags(app).keys()].map((image) => [image, tag]));
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
  if (missing.length === 0) {
    return;
  }
  const lines = missing.map(([image, tag]) => `${image}:${tag} does not exist in ghcr`);
  throw new Error([...lines, 'nothing changed; use --image to limit the rollback'].join('\n'));
}

function planEdits(workloads: Workload[], targets: Map<string, string>): Edit[] {
  return workloads
    .filter((workload) => workload.kind !== 'Job')
    .flatMap((workload) =>
      containersOf(workload).flatMap((container) => {
        const { repository, tag: from } = parseImage(container.image);
        const to = targets.get(repository);
        return to && to !== from
          ? [{ workload, container: container.name, repository, from, to }]
          : [];
      }),
    );
}

function applyEdit(kube: Kube, { workload, container, repository, to }: Edit): void {
  const pod =
    workload.kind === 'CronJob'
      ? workload.spec.jobTemplate?.spec?.template?.spec
      : workload.spec.template?.spec;
  const list = pod?.initContainers?.some((c) => c.name === container)
    ? 'initContainers'
    : 'containers';
  const podPatch = { spec: { [list]: [{ name: container, image: `${repository}:${to}` }] } };

  kube
    .namespace(workload.metadata.namespace)
    .patch(
      workload.kind.toLowerCase(),
      workload.metadata.name,
      workload.kind === 'CronJob'
        ? { spec: { jobTemplate: { spec: { template: podPatch } } } }
        : { spec: { template: podPatch } },
      'application/strategic-merge-patch+json',
    );
}

export interface RollbackOptions {
  tag?: string;
  image?: string;
  yes: boolean;
  wait: boolean;
  timeoutMinutes: number;
}

function chooseRows(rows: Row[], sha: string | undefined): Row[] | undefined {
  if (sha) {
    const picked = new Map<string, Row>();
    for (const row of rows) {
      if (row.promotion.sha?.startsWith(sha) && !picked.has(row.image)) {
        picked.set(row.image, row);
      }
    }
    if (!picked.size) {
      throw new Error(`no promotion of ${sha} in the history`);
    }
    return [...picked.values()];
  }
  if (!stdinIsTerminal()) {
    return undefined;
  }
  const answer = prompt('roll back to #: ')?.trim();
  if (!answer) {
    return [];
  }
  const row = rows[Number(answer)];
  if (!/^\d+$/.test(answer) || !row) {
    throw new Error(`${answer} is not a row number`);
  }
  return [row];
}

function rollbackRelease(
  name: string,
  history: Map<string, Promotion[]>,
  options: RollbackOptions,
) {
  const rows = rowsOf(history);
  console.log(
    columns(
      ['#', 'IMAGE', 'SHA', 'PROMOTED (UTC)', 'LIVE FOR', 'BY'],
      releaseRows(history, new Date().toISOString()),
    ),
  );

  const chosen = chooseRows(rows, options.tag);
  if (chosen === undefined) {
    console.error(
      `pass a sha to roll back non-interactively: ortzi app rollback ${name} <sha> --yes`,
    );
    return 2;
  }
  if (!chosen.length) {
    return 0;
  }
  if (options.tag && !applying(options.yes)) {
    for (const { image, promotion } of chosen) {
      console.log(`would promote ${image} back to ${short(promotion.sha ?? promotion.digest)}`);
    }
    return 0;
  }

  for (const { image, promotion } of chosen) {
    console.log(describe(image, promote(name, image, promotion.digest, true)));
  }
  console.log(`publish-apps rolls it out; follow with ortzi app status ${name}`);
  return 0;
}

export function rollback(kube: Kube, name: string, options: RollbackOptions): number {
  const { tag, image, yes } = options;
  assertChangeable(name);
  const app = getApp(kube, name);

  const owned = [...desiredTags(app).keys()].filter((i) => i.startsWith(RELEASE_REGISTRY));
  const history = new Map(
    [...promotions(owned, 20)].filter(
      ([released, list]) => list.length && (!image || released.endsWith(`/${image}`)),
    ),
  );
  if (history.size) {
    return rollbackRelease(name, history, options);
  }

  const workloads = workloadsFor(app, listWorkloads(kube));

  const chosen = chooseTargets(app, tag);
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
  if (!yes) {
    console.log(`${name} would be suspended so flux does not undo it`);
  }
  if (!applying(yes)) {
    return 0;
  }

  setSuspended(kube, app, true);
  for (const edit of edits) {
    applyEdit(kube, edit);
  }
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
