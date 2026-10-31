import { age, columns, type Kube, short } from '@ortzi/kit';
import { type Application, getApp, listApps } from '../argo';
import { type Report, buildReport, displayImage, symbol } from '../report';
import { listWorkloads } from '../workloads';

function lastDeployed(app: Application): string {
  const at = app.status?.history?.at(-1)?.deployedAt;
  return at ? age(at, Date.now()) : '-';
}

function printSummary(reports: Report[]): void {
  const bad = reports.filter((r) => r.problems.length > 0);
  if (bad.length === 0) {
    console.log(`✓ All ${reports.length} apps are running what git says.`);
    return;
  }
  const worst = symbol(bad.flatMap((r) => r.problems));
  console.log(`${worst} ${bad.length} of ${reports.length} apps need attention\n`);
  for (const { app, problems } of bad) {
    console.log(`${symbol(problems)} ${app.metadata.name}`);
    for (const problem of problems) console.log(`    ${problem.text}`);
    console.log('');
  }
  console.log(`✓ ${reports.length - bad.length} apps ok`);
}

function printAll(reports: Report[], latest: boolean): void {
  const header = ['NAME', 'READY', 'SYNC', 'HEALTH', 'AUTOSYNC', ...(latest ? ['LATEST'] : []), 'AGE'];
  const rows = reports.map(({ app, images }) => [
    app.metadata.name,
    images.length > 0 ? `${images.filter((i) => i.running === i.git).length}/${images.length}` : '-',
    app.status?.sync?.status ?? '-',
    app.status?.health?.status ?? '-',
    app.spec.syncPolicy?.automated ? 'on' : 'OFF',
    ...(latest ? [`${images.filter((i) => i.latest === 'behind').length} behind`] : []),
    lastDeployed(app),
  ]);
  console.log(columns(header, rows));
}

function printApp({ app, images, problems }: Report): void {
  const automated = app.spec.syncPolicy?.automated;
  console.log(`${symbol(problems)} ${app.metadata.name}`);
  console.log(`  sync       ${app.status?.sync?.status ?? '-'}`);
  console.log(`  health     ${app.status?.health?.status ?? '-'}`);
  console.log(`  auto-sync  ${automated ? 'on' : 'off'}`);
  console.log(`  self-heal  ${automated?.selfHeal ? 'on' : 'off'}`);
  console.log(`  deployed   ${lastDeployed(app)} ago`);
  if (problems.length > 0) {
    console.log('');
    for (const problem of problems) console.log(`  ${problem.text}`);
  }
  if (images.length > 0) {
    const rows = images.map((i) => [
      displayImage(i.image),
      short(i.running ?? '-'),
      short(i.git),
      i.latest ?? '-',
    ]);
    console.log(`\n${columns(['IMAGE', 'RUNNING', 'GIT', 'LATEST'], rows)}`);
  }
}

export function status(kube: Kube, name: string | undefined, all: boolean, latest: boolean): number {
  const workloads = listWorkloads(kube);
  if (name) {
    printApp(buildReport(getApp(kube, name), workloads, true));
    return 0;
  }
  const reports = listApps(kube).map((app) => buildReport(app, workloads, latest));
  if (all) printAll(reports, latest);
  else printSummary(reports);
  return 0;
}
