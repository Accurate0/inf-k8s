import assert from 'node:assert/strict';
import { test } from 'node:test';
import { buildGraph, toDot, toMermaid } from './graph.ts';
import { validate } from './render.ts';

const graphApps = [
  validate(
    {
      name: 'gw',
      network: { ingress: ['caller', 'monitoring', 'other'] },
      exports: [{ name: 'api', workload: 'gw', port: 80 }],
      workloads: [
        {
          name: 'gw',
          image: 'i',
          ports: [{ name: 'http', container: 80, service: 80 }],
          service: { name: 'gw' },
        },
      ],
    },
    'test',
  ),
  validate(
    {
      name: 'caller',
      network: { ingress: [] },
      resources: [
        { name: 'llm', type: 'app', app: 'gw', export: 'api' },
        { name: 'traces', type: 'platform', export: 'tracing' },
        { name: 'foo-bar', type: 'database', secretName: 'foo-bar-database-secret' },
      ],
    },
    'test',
  ),
  validate({ name: 'other', network: { ingress: [] } }, 'test'),
];

test('graph shows resources, databases and hand written ingress', () => {
  const { edges } = buildGraph(graphApps);

  assert.deepEqual(edges, [
    { from: 'caller', to: 'gw', label: 'api' },
    { from: 'caller', to: 'platform/tracing' },
    { from: 'caller', to: 'database/foo_bar' },
    { from: 'monitoring', to: 'gw', label: 'ingress', dashed: true },
    { from: 'other', to: 'gw', label: 'ingress', dashed: true },
  ]);
});

test('graph can focus on one app', () => {
  const { nodes, edges } = buildGraph(graphApps, 'other');

  assert.deepEqual(nodes.map((n) => n.id).sort(), ['gw', 'other']);
  assert.equal(edges.length, 1);
});

test('graph renders mermaid and dot', () => {
  const graph = buildGraph(graphApps);
  const mermaid = toMermaid(graph);

  assert.match(mermaid, /^graph LR/);
  assert.match(mermaid, /caller -->\|api\| gw/);
  assert.match(mermaid, /other -\.->\|ingress\| gw/);
  assert.match(mermaid, /database_foo_bar\[\("foo_bar"\)\]/);
  const dot = toDot(graph);

  assert.match(dot, /"caller" -> "gw" \[label="api"\];/);
  assert.match(dot, /"other" -> "gw" \[label="ingress", style=dashed\];/);
});
