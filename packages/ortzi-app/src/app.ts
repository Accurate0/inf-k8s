import type { App } from './types.ts';
import type { ResourceType, ResourceTypes, SourceType, SourceTypes } from './providers/provider.ts';

export const appNamespace = (app: App) => app.namespace?.name ?? app.name;

export const sourcesOf = <T extends SourceType>(app: App, type: T) =>
  (app.sources ?? []).filter((source): source is SourceTypes[T]['source'] => source.type === type);

export const declared = <T extends ResourceType>(app: App, type: T) =>
  (app.resources ?? []).filter((resource): resource is ResourceTypes[T] => resource.type === type);

export const specs = <S extends { name: string; type: string }>(resources: S[]) =>
  resources.map(({ name, type: _type, ...spec }) => [name, spec] as const);

export function secretRef(app: App, kind: ResourceType, name: string): string {
  const found = (app.resources ?? []).find((r) => r.type === kind && r.name === name) as
    { secretName?: string } | undefined;
  if (!found) {
    throw new Error(`${app.name}: no ${kind} resource named ${name}`);
  }
  return found.secretName as string;
}
