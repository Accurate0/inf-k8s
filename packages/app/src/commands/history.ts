import { columns, type Kube, minuteTimestamp, short } from '@ortzi/kit';
import { getApp, historyNewestFirst } from '../argo';
import { gitRevision, tagsAt } from '../git';

function describeTags(tags: Map<string, string>): string {
  if (tags.size === 0) return '-';
  const entries = [...tags];
  if (new Set(tags.values()).size === 1) return short(entries[0][1]);
  return entries.map(([image, tag]) => `${image.split('/').pop()}=${short(tag)}`).join(' ');
}

export function history(kube: Kube, name: string): number {
  const app = getApp(kube, name);
  const entries = historyNewestFirst(app);
  if (entries.length === 0) {
    console.log(`${name} has no sync history`);
    return 0;
  }
  const rows = entries.map((entry) => {
    const revision = gitRevision(app, entry);
    return [
      minuteTimestamp(entry.deployedAt ?? '-'),
      short(revision ?? '-'),
      describeTags(tagsAt(app, revision)),
    ];
  });
  console.log(columns(['DEPLOYED (UTC)', 'REVISION', 'IMAGES'], rows));
  return 0;
}
