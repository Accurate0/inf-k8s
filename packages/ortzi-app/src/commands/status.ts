import { age, columns, type Kube, short } from '@ortzi/kit';
import {
  type App,
  appReady,
  driftCorrected,
  getApp,
  lastDeployed as deployedAt,
  listApps,
  revision,
  suspended,
} from '../flux';
import { type Report, buildReport, displayImage, symbol } from '../report';
import { listWorkloads } from '../workloads';

function lastDeployed(app: App): string {
  const at = deployedAt(app);
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
    console.log(`${symbol(problems)} ${app.name}`);
    for (const problem of problems) {
      console.log(`    ${problem.text}`);
    }
    console.log('');
  }
  console.log(`✓ ${reports.length - bad.length} apps ok`);
}

function printAll(reports: Report[], latest: boolean): void {
  const header = ['NAME', 'IMAGES', 'READY', 'AUTOSYNC', ...(latest ? ['LATEST'] : []), 'AGE'];
  const rows = reports.map(({ app, images }) => [
    app.name,
    images.length > 0
      ? `${images.filter((i) => i.running === i.git).length}/${images.length}`
      : '-',
    appReady(app) ? 'yes' : 'NO',
    suspended(app) ? 'OFF' : 'on',
    ...(latest ? [`${images.filter((i) => i.latest === 'behind').length} behind`] : []),
    lastDeployed(app),
  ]);
  console.log(columns(header, rows));
}

function printApp({ app, images, problems }: Report): void {
  console.log(`${symbol(problems)} ${app.name}`);
  console.log(`  ready      ${appReady(app) ? 'yes' : 'no'}`);
  console.log(`  auto-sync  ${suspended(app) ? 'off' : 'on'}`);
  console.log(`  self-heal  ${driftCorrected(app) ? 'on' : 'off'}`);
  console.log(`  revision   ${short(revision(app) ?? '-')}`);
  console.log(`  deployed   ${lastDeployed(app)} ago`);

  if (problems.length > 0) {
    console.log('');
    for (const problem of problems) {
      console.log(`  ${problem.text}`);
    }
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

export function status(
  kube: Kube,
  name: string | undefined,
  all: boolean,
  latest: boolean,
): number {
  const workloads = listWorkloads(kube);
  if (name) {
    printApp(buildReport(getApp(kube, name), workloads, true));
    return 0;
  }

  const reports = listApps(kube).map((app) => buildReport(app, workloads, latest));
  if (all) {
    printAll(reports, latest);
  } else {
    printSummary(reports);
  }
  return 0;
}
