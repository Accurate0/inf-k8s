import {
  Document,
  isMap,
  isPair,
  isScalar,
  isSeq,
  parseDocument,
  type Pair,
  type YAMLMap,
  type YAMLSeq,
} from 'yaml';
import { RESOURCES } from './providers/index.ts';

const WIDTH = 120;

const OPTIONS = { lineWidth: 0, flowCollectionPadding: false } as const;

const TOP = [
  'name',
  'repository',
  'chartRegistry',
  'charts',
  'namespace',
  'network',
  'overrides',
  'exports',
  'resources',
  'workloads',
  'build',
  'sources',
  'artifact',
];

const WORKLOAD = [
  'name',
  'image',
  'kind',
  'schedule',
  'phase',
  'replicas',
  'autoscale',
  'backoffLimit',
  'command',
  'args',
  'resources',
  'ports',
  'probes',
  'inherit',
  'env',
  'envFrom',
  'mounts',
  'initContainers',
  'sidecars',
  'serviceAccount',
  'runAsNonRoot',
  'readOnlyRootFilesystem',
  'service',
  'overrides',
];

const REQUESTS = ['cpu', 'memory', 'memoryLimit'];

// renovate's regex managers read sources and build.test.db line by line
const BLOCK_ITEMS = new Set(['workloads', 'sidecars', 'initContainers', 'sources']);
const BLOCK_VALUES = new Set(['env', 'db']);
const RAW = 'overrides';

type Collection = YAMLMap | YAMLSeq;

const isCollection = (node: unknown): node is Collection => isMap(node) || isSeq(node);

const keyOf = (pair: Pair) => (isScalar(pair.key) ? String(pair.key.value) : '');

function sortPairs(map: YAMLMap, order: string[]) {
  const rank = (pair: Pair) => {
    const index = order.indexOf(keyOf(pair));
    return index === -1 ? order.length : index;
  };
  map.items.sort((a, b) => rank(a) - rank(b));
}

function orderWorkload(workload: unknown) {
  if (!isMap(workload)) {
    return;
  }

  sortPairs(workload, WORKLOAD);
  const requests = workload.get('resources', true);
  if (isMap(requests)) {
    sortPairs(requests, REQUESTS);
  }

  for (const key of ['initContainers', 'sidecars']) {
    const containers = workload.get(key, true);
    if (isSeq(containers)) {
      containers.items.forEach(orderWorkload);
    }
  }
}

function order(root: YAMLMap) {
  sortPairs(root, TOP);

  const resources = root.get('resources', true);
  if (isSeq(resources)) {
    const types: string[] = RESOURCES.map((provider) => provider.type);
    const rank = (item: unknown) => {
      const index = isMap(item) ? types.indexOf(String(item.get('type'))) : -1;
      return index === -1 ? types.length : index;
    };
    resources.items.sort((a, b) => rank(a) - rank(b));

    for (const item of resources.items) {
      if (isMap(item)) {
        sortPairs(item, ['name', 'type']);
      }
    }
  }

  const workloads = root.get('workloads', true);
  if (isSeq(workloads)) {
    workloads.items.forEach(orderWorkload);
  }
}

function flowSafe(node: unknown): boolean {
  if (node === null || node === undefined) {
    return true;
  }

  if (isScalar(node)) {
    const multiline = typeof node.value === 'string' && node.value.includes('\n');
    const block = node.type === 'BLOCK_LITERAL' || node.type === 'BLOCK_FOLDED';
    return !multiline && !block && !node.comment && !node.commentBefore;
  }

  if (isCollection(node)) {
    return (
      !node.comment &&
      !node.commentBefore &&
      (node.items as unknown[]).every((item) =>
        isPair(item) ? flowSafe(item.key) && flowSafe(item.value) : flowSafe(item),
      )
    );
  }

  return false;
}

function setFlow(node: unknown, flow: boolean) {
  if (!isCollection(node)) {
    return;
  }

  node.flow = flow;
  for (const item of node.items as unknown[]) {
    if (isPair(item)) {
      setFlow(item.value, flow);
    } else {
      setFlow(item, flow);
    }
  }
}

function inline(node: unknown): string {
  const copy = isCollection(node) || isScalar(node) ? node.clone() : node;
  setFlow(copy, true);
  return new Document(copy).toString(OPTIONS).trimEnd();
}

const isContainer = (map: YAMLMap) =>
  map.items.length > 0 && map.items.every((pair) => isCollection(pair.value));

function mayFlow(node: Collection, depth: number, key: string, item: boolean): boolean {
  if (item) {
    return !BLOCK_ITEMS.has(key);
  }

  if (BLOCK_VALUES.has(key)) {
    return false;
  }

  if (isSeq(node)) {
    return node.items.every((entry) => isScalar(entry));
  }

  return depth >= 3 && !isContainer(node);
}

function place(
  node: unknown,
  column: number,
  prefix: number,
  flowable: boolean,
  depth: number,
  key = '',
) {
  if (!isCollection(node)) {
    return;
  }

  if (node.items.length === 0) {
    node.flow = true;
    return;
  }

  if (flowable && flowSafe(node) && prefix + inline(node).length <= WIDTH) {
    setFlow(node, true);
    return;
  }

  node.flow = false;
  if (isMap(node)) {
    for (const pair of node.items) {
      const value = pair.value;
      if (!isCollection(value)) {
        continue;
      }

      const child = keyOf(pair);
      if (child === RAW) {
        setFlow(value, false);
        continue;
      }

      const prefixed = column + inline(pair.key).length + 2;
      place(value, column + 2, prefixed, mayFlow(value, depth + 1, child, false), depth + 1, child);
    }
    return;
  }

  for (const item of node.items) {
    if (isCollection(item)) {
      place(item, column + 2, column + 2, mayFlow(item, depth + 1, key, true), depth + 1, key);
    }
  }
}

export function formatApp(text: string): string {
  const doc = parseDocument(text);
  if (doc.errors.length) {
    throw doc.errors[0];
  }

  const root = doc.contents;
  if (!isMap(root)) {
    throw new Error('expected a mapping');
  }

  order(root);
  for (const pair of root.items) {
    if (isScalar(pair.key)) {
      pair.key.spaceBefore = false;
    }
  }
  place(root, 0, 0, false, 0);

  return doc.toString(OPTIONS);
}
