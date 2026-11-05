import { fs, httpSend } from '@ortzi/host';
import { type App, declared, resolveSubgraphs, supergraphApp, type Target } from '../render.ts';
import { compose, NODE_SUBGRAPH } from '../supergraph.ts';

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
  const dir = `${target.location}/${router.name}`;
  const [{ base }] = declared(router, 'supergraph');
  const subgraphs = resolveSubgraphs(apps);

  const sdls = Object.fromEntries(
    subgraphs.map((subgraph) => [subgraph.name, fetchSchema(subgraph.schemaUrl)]),
  );

  const composed = compose(subgraphs, sdls, fs.readText(`${dir}/${base}`));
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

  const stale = wanted.filter(
    ([path, contents]) => !fs.exists(path) || fs.readText(path) !== contents,
  );
  if (!check) fs.mkdir(`${dir}/subgraphs`);
  for (const [path, contents] of stale) {
    console.log(`${check ? 'stale' : 'wrote'} ${path}`);
    if (!check) fs.write(path, contents);
  }
  if (!stale.length) console.log('supergraph is up to date');

  return check && stale.length ? 1 : 0;
}
