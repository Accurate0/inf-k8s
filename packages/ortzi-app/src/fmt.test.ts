import assert from 'node:assert/strict';
import { test } from 'node:test';
import { formatApp } from './fmt.ts';

test('puts top-level blocks in canonical order and drops blank lines between them', () => {
  const text = `artifact:
  repo: oci://registry.anurag.sh/manifests/foo
  tag: main

workloads: []
network:
  ingress: []
name: foo
`;

  assert.equal(
    formatApp(text),
    `name: foo
network:
  ingress: []
workloads: []
artifact:
  repo: oci://registry.anurag.sh/manifests/foo
  tag: main
`,
  );
});

test('groups resources by type, consumed before exposed, keeping order within a type', () => {
  const text = `name: foo
network:
  ingress: []
resources:
  - {name: b, type: route, auth: {mode: none, reason: public}, hosts: [b.example], backend: {name: web, port: 80}}
  - {type: secret, name: s, secretName: foo-secrets}
  - {name: a, type: route, auth: {mode: none, reason: public}, hosts: [a.example], backend: {name: web, port: 80}}
  - {name: cache, type: cache}
`;

  const names = [...formatApp(text).matchAll(/^ {2}- \{name: (\w+), type: (\w+)/gm)].map(
    ([, name, type]) => `${type}/${name}`,
  );

  assert.deepEqual(names, ['secret/s', 'cache/cache', 'route/b', 'route/a']);
});

test('orders workload keys, sidecar keys and resource requests', () => {
  const text = `name: foo
network:
  ingress: []
workloads:
  - resources: {memoryLimit: 1Gi, memory: 64Mi}
    image: foo
    name: foo
    sidecars:
      - image: bar
        name: bar
`;

  assert.equal(
    formatApp(text),
    `name: foo
network:
  ingress: []
workloads:
  - name: foo
    image: foo
    resources: {memory: 64Mi, memoryLimit: 1Gi}
    sidecars:
      - name: bar
        image: bar
`,
  );
});

test('keeps comments and the schema modeline', () => {
  const text = `# yaml-language-server: $schema=https://k8s-schemas.anurag.sh/app.json
name: foo
# charts stay on ghcr
chartRegistry: oci://ghcr.io/accurate0/charts
network:
  ingress: []
`;

  const formatted = formatApp(text);

  assert.match(formatted, /^# yaml-language-server/);
  assert.match(formatted, /# charts stay on ghcr\nchartRegistry:/);
});

test('flows what fits in 120 columns and keeps the rest in block style', () => {
  const long = Array.from({ length: 12 }, (_, i) => `rules/rule-number-${i}.yaml`);
  const text = `name: foo
network:
  ingress: [argocd]
resources:
  - name: rules
    type: config
    configMap: foo-rules
    files: [${long.join(', ')}]
  - name: s
    type: secret
    secretName: foo-secrets
`;

  const formatted = formatApp(text);

  assert.match(formatted, /\n {2}- {name: s, type: secret, secretName: foo-secrets}\n/);
  assert.match(formatted, /\n {4}files:\n {6}- rules\/rule-number-0.yaml\n/);
  for (const line of formatted.split('\n')) {
    assert.ok(line.length <= 120, line);
  }
});

test('keeps sources, test databases, env and overrides in block style', () => {
  const text = `name: foo
network:
  ingress: []
workloads:
  - {name: foo, image: foo, env: {A: "1"}, overrides: {strategy: {type: Recreate}}}
build:
  image: foo
  test:
    run: cargo test
    db: {image: "postgres:18", databaseName: foo}
sources:
  - {type: manifests, path: .}
`;

  assert.equal(
    formatApp(text),
    `name: foo
network:
  ingress: []
workloads:
  - name: foo
    image: foo
    env:
      A: "1"
    overrides:
      strategy:
        type: Recreate
build:
  image: foo
  test:
    run: cargo test
    db:
      image: "postgres:18"
      databaseName: foo
sources:
  - type: manifests
    path: .
`,
  );
});

test('a second run changes nothing', () => {
  const text = `name: foo
network:
  ingress: []
resources:
  - name: web
    type: route
    auth:
      mode: app
      reason: the app checks its own sessions, with a comma
    hosts:
      - foo.example
    backend:
      name: web
      port: 80
`;

  const once = formatApp(text);

  assert.equal(formatApp(once), once);
});
