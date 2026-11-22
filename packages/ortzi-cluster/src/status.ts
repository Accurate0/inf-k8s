import { age, columns, type Kube } from '@ortzi/kit';
import {
  FLUX_NAMESPACE,
  type FluxObject,
  ROOT,
  SOURCES,
  commitOf,
  ready,
  readyMessage,
  revisionOf,
  shortRevision,
  state,
} from './flux';

const WORKLOADS = ['kustomizations', 'helmreleases'];

interface FluxReport {
  spec: {
    distribution?: { version?: string; status?: string };
    operator?: { version?: string };
    components?: { name: string; ready: boolean; status?: string }[];
    reconcilers?: {
      kind: string;
      stats: { running: number; failing: number; suspended: number; totalSize?: string };
    }[];
    sync?: { source?: string; ready?: boolean; status?: string };
  };
}

function list(kube: Kube, resources: string[]): FluxObject[] {
  return resources.flatMap((resource) => {
    try {
      kube.resource(resource);
    } catch {
      return [];
    }
    return kube.list<FluxObject>(resource);
  });
}

function report(kube: Kube): FluxReport | undefined {
  try {
    return kube.namespace(FLUX_NAMESPACE).get<FluxReport>('fluxreports', 'flux');
  } catch {
    return undefined;
  }
}

export interface StatusOptions {
  all: boolean;
}

export function status(kube: Kube, options: StatusOptions): number {
  const now = Date.now();
  const flux = kube.namespace(FLUX_NAMESPACE);
  const fluxReport = report(kube)?.spec;
  let ok = true;

  if (fluxReport) {
    const components = fluxReport.components ?? [];
    const down = components.filter((c) => !c.ready);
    ok = down.length === 0;

    console.log(
      `flux         ${fluxReport.distribution?.version ?? '-'} (operator ${fluxReport.operator?.version ?? '-'})`,
    );
    console.log(`controllers  ${components.length - down.length}/${components.length} ready`);
    for (const component of down) {
      console.log(`  ✗ ${component.name}: ${component.status ?? 'not ready'}`);
    }
  }

  const source = flux.get<FluxObject>('ocirepository', ROOT);
  const root = flux.get<FluxObject>('kustomization', ROOT);
  const fetched = revisionOf(source);
  const applied = root.status?.lastAppliedRevision;
  const commit = commitOf(source);
  const updated = source.status?.artifact?.lastUpdateTime;

  console.log(`source       ${fluxReport?.sync?.source ?? ROOT}`);
  console.log(
    `fetched      ${shortRevision(fetched)}${commit ? `  commit ${commit.slice(0, 8)}` : ''}${updated ? `  ${age(updated, now)} ago` : ''}${ready(source) ? '' : `  ✗ ${readyMessage(source)}`}`,
  );
  console.log(
    `applied      ${shortRevision(applied)}${applied && applied !== fetched ? '  ⚠ not the fetched revision' : ''}${ready(root) ? '' : `  ✗ ${readyMessage(root)}`}`,
  );
  ok = ok && ready(source) && ready(root);

  if (options.all && fluxReport?.reconcilers) {
    const rows = fluxReport.reconcilers
      .filter((r) => r.stats.running + r.stats.failing + r.stats.suspended > 0)
      .map((r) => [
        r.kind,
        String(r.stats.running),
        String(r.stats.failing),
        String(r.stats.suspended),
        r.stats.totalSize ?? '',
      ]);
    console.log('');
    console.log(columns(['KIND', 'RUNNING', 'FAILING', 'SUSPENDED', 'SIZE'], rows));
  }

  const objects = list(kube, [...WORKLOADS, ...SOURCES]);
  const problems = objects
    .map((object) => ({ object, state: state(object) }))
    .filter(({ state }) => state !== 'ready');

  if (problems.length === 0) {
    console.log(`\n✓ all ${objects.length} flux objects are ready`);
    return ok ? 0 : 1;
  }

  ok = ok && problems.every(({ state }) => state !== 'failing');
  const rows = problems
    .sort((a, b) => a.state.localeCompare(b.state))
    .map(({ object, state }) => [
      `${object.kind?.toLowerCase()}/${object.metadata.name}`,
      object.metadata.namespace ?? '-',
      state,
      state === 'suspended' ? '' : readyMessage(object).split('\n')[0]!.slice(0, 100),
    ]);
  console.log('');
  console.log(columns(['OBJECT', 'NAMESPACE', 'STATE', 'MESSAGE'], rows));
  console.log(`\n${objects.length - problems.length}/${objects.length} flux objects ready`);
  return ok ? 0 : 1;
}
