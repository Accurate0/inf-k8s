import { fs, httpSend } from '@ortzi/host';
import {
  type App,
  declared,
  isCurrent,
  resolveSubgraphs,
  stamped,
  supergraphApp,
  type Target,
} from '../render.ts';
import { compose, NODE_SUBGRAPH, SUPERGRAPH_HEADER_TEXT } from '../supergraph.ts';

function fetchSchema(url: string): string {
  const response = httpSend({ url, timeoutMs: 30_000 });
  if (response.status !== 200) throw new Error(`${url}: ${response.status}`);
  return response.body;
}

export function supergraph(found: { app: App; target: Target }[], check: boolean): number {
  const apps = found.map(({ app }) => app);
  const router = supergraphApp(apps);
  if (!router) {
    console.log('no app declares a supergraph');
    return 0;
  }

  const target = found.find(({ app }) => app === router)?.target as Target;
  const appDir = `${target.location}/${router.name}`;
  const [{ base }] = declared(router, 'supergraph');
  const dir = `${appDir}/${base}`.replace(/\/[^/]*$/, '');
  const subgraphs = resolveSubgraphs(apps);

  const sdls = Object.fromEntries(
    subgraphs.map((subgraph) => [subgraph.name, fetchSchema(subgraph.schemaUrl)]),
  );

  const composed = compose(subgraphs, sdls, fs.readText(`${appDir}/${base}`));
  const wanted = [
    ...subgraphs.map((subgraph) => [
      `${dir}/subgraphs/${subgraph.name}.graphql`,
      sdls[subgraph.name],
    ]),
    ...(composed.node ? [[`${dir}/subgraphs/${NODE_SUBGRAPH}.graphql`, composed.node]] : []),
    [`${dir}/supergraph.graphql`, composed.supergraph],
    [`${dir}/schema.graphql`, composed.schema],
    [`${dir}/router.yaml`, composed.router],
  ] as [string, string][];

  const header = `# ${SUPERGRAPH_HEADER_TEXT}`;
  const generated = (contents: string) => contents.startsWith(`${header}\n`);
  const current = (path: string, contents: string) => {
    if (!fs.exists(path)) return false;
    const existing = fs.readText(path);

    return generated(contents) ? isCurrent(existing, contents) : existing === contents;
  };

  const now = new Date();
  const stale = wanted.filter(([path, contents]) => !current(path, contents));
  if (!check) fs.mkdir(`${dir}/subgraphs`);
  for (const [path, contents] of stale) {
    console.log(`${check ? 'stale' : 'wrote'} ${path}`);
    if (!check) fs.write(path, generated(contents) ? stamped(contents, now, header) : contents);
  }
  if (!stale.length) console.log('supergraph is up to date');

  return check && stale.length ? 1 : 0;
}
