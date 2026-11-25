import { fs, httpSend } from '@ortzi/host';
import { buildSchema, findBreakingChanges } from 'graphql';
import {
  type App,
  declared,
  isCurrent,
  resolveSubgraphs,
  stamped,
  supergraphApp,
  type Target,
} from '../render.ts';
import {
  compose,
  NODE_SUBGRAPH,
  PERSISTED_DOCUMENTS,
  SUPERGRAPH_HEADER_TEXT,
} from '../supergraph.ts';

function fetchText(url: string): string {
  const response = httpSend({ url, timeoutMs: 30_000 });
  if (response.status !== 200) {
    throw new Error(`${url}: ${response.status}`);
  }
  return response.body;
}

interface Inputs {
  dir: string;
  base: string;
  externalIssuers: Parameters<typeof compose>[4];
  subgraphs: ReturnType<typeof resolveSubgraphs>;
  sdls: Record<string, string>;
  manifests: Record<string, string>;
}

function gather(found: { app: App; target: Target }[], skip?: string): Inputs | undefined {
  const apps = found.map(({ app }) => app);
  const router = supergraphApp(apps);
  if (!router) {
    return undefined;
  }

  const target = found.find(({ app }) => app === router)?.target as Target;
  const appDir = `${target.location}/${router.name}`;
  const [{ base, externalIssuers }] = declared(router, 'supergraph');
  const dir = `${appDir}/${base}`.replace(/\/[^/]*$/, '');
  const subgraphs = resolveSubgraphs(apps);
  const fetched = subgraphs.filter((subgraph) => subgraph.name !== skip);

  const locations = new Map(found.map(({ app, target }) => [app.name, target.location]));
  const sdls = Object.fromEntries(
    fetched.map((subgraph) => [
      subgraph.name,
      subgraph.schemaPath
        ? fs.readText(`${locations.get(subgraph.app)}/${subgraph.app}/${subgraph.schemaPath}`)
        : fetchText(subgraph.schemaUrl as string),
    ]),
  );

  const manifests = Object.fromEntries(
    fetched.flatMap((subgraph) => {
      if (subgraph.documentsPath) {
        return [
          [
            subgraph.name,
            fs.readText(`${locations.get(subgraph.app)}/${subgraph.app}/${subgraph.documentsPath}`),
          ],
        ];
      }

      return subgraph.documentsUrl ? [[subgraph.name, fetchText(subgraph.documentsUrl)]] : [];
    }),
  );

  return {
    dir,
    base: fs.readText(`${appDir}/${base}`),
    externalIssuers,
    subgraphs,
    sdls,
    manifests,
  };
}

const withoutHeader = (sdl: string) => sdl.replace(/^(#[^\n]*\n)+/, '');

export function supergraphCheck(
  found: { app: App; target: Target }[],
  name: string | undefined,
  schema: string | undefined,
  documents: string | undefined,
): number {
  if (!name || !schema) {
    console.error('usage: supergraph check <subgraph> --schema FILE [--documents FILE]');
    return 2;
  }

  const inputs = gather(found, name);
  if (!inputs) {
    console.log('no app declares a supergraph');
    return 0;
  }
  if (!inputs.subgraphs.some((subgraph) => subgraph.name === name)) {
    console.error(`no subgraph named ${name}`);
    return 2;
  }

  const { dir, base, externalIssuers, subgraphs, sdls, manifests } = inputs;
  sdls[name] = fs.readText(schema);
  if (documents) {
    manifests[name] = fs.readText(documents);
  }

  let composed;
  try {
    composed = compose(subgraphs, sdls, base, manifests, externalIssuers);
  } catch (error) {
    console.error((error as Error).message);
    return 1;
  }

  const current = `${dir}/schema.graphql`;
  const changes = fs.exists(current)
    ? findBreakingChanges(
        buildSchema(withoutHeader(fs.readText(current))),
        buildSchema(withoutHeader(composed.schema)),
      )
    : [];
  for (const change of changes) {
    console.log(`breaking: ${change.description}`);
  }

  console.log(
    `${name} composes${changes.length ? `, with ${changes.length} breaking change(s) no persisted document uses` : ''}`,
  );
  return 0;
}

export function supergraph(found: { app: App; target: Target }[], check: boolean): number {
  const inputs = gather(found);
  if (!inputs) {
    console.log('no app declares a supergraph');
    return 0;
  }

  const { dir, base, externalIssuers, subgraphs, sdls, manifests } = inputs;
  const composed = compose(subgraphs, sdls, base, manifests, externalIssuers);
  const wanted = [
    ...subgraphs.map((subgraph) => [
      `${dir}/subgraphs/${subgraph.name}.graphql`,
      sdls[subgraph.name],
    ]),
    ...(composed.node ? [[`${dir}/subgraphs/${NODE_SUBGRAPH}.graphql`, composed.node]] : []),
    [`${dir}/supergraph.graphql`, composed.supergraph],
    [`${dir}/schema.graphql`, composed.schema],
    [`${dir}/router.yaml`, composed.router],
    ...(composed.documents ? [[`${dir}/${PERSISTED_DOCUMENTS}`, composed.documents]] : []),
  ] as [string, string][];

  const header = `# ${SUPERGRAPH_HEADER_TEXT}`;
  const generated = (contents: string) => contents.startsWith(`${header}\n`);
  const current = (path: string, contents: string) => {
    if (!fs.exists(path)) {
      return false;
    }
    const existing = fs.readText(path);

    return generated(contents) ? isCurrent(existing, contents) : existing === contents;
  };

  const now = new Date();
  const stale = wanted.filter(([path, contents]) => !current(path, contents));
  if (!check) {
    fs.mkdir(`${dir}/subgraphs`);
  }
  for (const [path, contents] of stale) {
    console.log(`${check ? 'stale' : 'wrote'} ${path}`);
    if (!check) {
      fs.write(path, generated(contents) ? stamped(contents, now, header) : contents);
    }
  }
  if (!stale.length) {
    console.log('supergraph is up to date');
  }

  return check && stale.length ? 1 : 0;
}
