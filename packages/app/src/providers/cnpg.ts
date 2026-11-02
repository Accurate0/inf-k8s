import { parseDocument, stringify } from 'yaml';
import { databaseRole, type DatabaseSpec, HEADER } from '../render.ts';

export const databasePath = (database: string) => `system-components/postgres-cluster/databases/${database}.yaml`;

export function renderDatabase(database: string, spec?: Pick<DatabaseSpec, 'role' | 'extensions'>): string {
  const name = databaseRole(database, spec);
  const manifest = {
    apiVersion: 'postgresql.cnpg.io/v1',
    kind: 'Database',
    metadata: { name: database, namespace: 'cnpg-system' },
    spec: {
      name,
      owner: name,
      cluster: { name: 'cloudnative-pg-cluster' },
      databaseReclaimPolicy: 'retain',
      ...(spec?.extensions ? { extensions: spec.extensions.map((extension) => ({ name: extension, ensure: 'present' })) } : {}),
    },
  };
  return `${HEADER}\n# yaml-language-server: $schema=https://k8s-schemas.anurag.sh/postgresql.cnpg.io/database_v1.json\n${stringify(manifest)}`;
}

export function addRoles(cluster: string, roles: string[]): string {
  const doc = parseDocument(cluster);
  const existing = (doc.toJS().spec.managed.roles as { name: string }[]).map((role) => role.name);
  const missing = roles.filter((role) => !existing.includes(role));
  if (!missing.length) return cluster;
  for (const name of missing) {
    doc.addIn(['spec', 'managed', 'roles'], { name, ensure: 'present', connectionLimit: -1, inherit: true, login: true });
  }
  return doc.toString({ lineWidth: 0 });
}
