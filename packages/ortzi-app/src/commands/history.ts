import { columns, type Kube, minuteTimestamp, short } from '@ortzi/kit';
import { getApp } from '../flux';
import { promotions } from '../registry';
import { RELEASE_REGISTRY } from '../providers/workload';
import { releaseRows } from '../promotion';
import { desiredTags } from '../flux';

export function history(kube: Kube, name: string): number {
  const app = getApp(kube, name);
  const rows = app.releases.flatMap((release) =>
    (release.status?.history ?? []).map((entry) => [
      minuteTimestamp(entry.lastDeployed ?? '-'),
      release.metadata.name,
      `v${entry.version ?? '?'}`,
      entry.status ?? '-',
      short(entry.chartVersion ?? '-'),
    ]),
  );
  rows.sort((a, b) => b[0].localeCompare(a[0]));
  if (!rows.length) {
    console.log(`${name} has no release history`);
  } else {
    console.log(columns(['DEPLOYED (UTC)', 'RELEASE', 'REVISION', 'STATUS', 'CHART'], rows));
  }

  const released = [...desiredTags(app).keys()].filter((i) => i.startsWith(RELEASE_REGISTRY));
  const promoted = new Map([...promotions(released, 10)].filter(([, list]) => list.length));
  if (promoted.size) {
    console.log(
      `\n${columns(['#', 'IMAGE', 'SHA', 'PROMOTED (UTC)', 'LIVE FOR', 'BY'], releaseRows(promoted, new Date().toISOString()))}`,
    );
  }
  return 0;
}
