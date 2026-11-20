import type { Kube } from '@ortzi/kit';
import type { Schema } from 'jsonschema/lib/index.js';
import type { App, Target, World } from '../types.ts';
import type { Check } from '../checks.ts';
import type { Refs } from '../workload.ts';
import type { FluxObject } from '../flux-objects.ts';

// Each provider file adds itself here with `declare module './provider.ts'`.
export interface ResourceTypes {}
export interface SourceTypes {}

export type ResourceType = keyof ResourceTypes;
export type Resource = ResourceTypes[ResourceType];
export type SourceType = keyof SourceTypes;
export type Source = { [K in SourceType]: SourceTypes[K]['source'] }[SourceType];

export interface PlatformChart {
  type: 'chart';
  chart: string;
  registry: string;
  version: string;
  valueFiles?: string[];
  values: Record<string, unknown>;
  retain?: boolean;
}

export type PlanSource = PlatformChart | { [K in SourceType]: SourceTypes[K]['plan'] }[SourceType];

export interface Plan {
  name: string;
  namespace: string;
  location: string;
  directory: string;
  namespaceLabels?: Record<string, string>;
  ownsNamespace: boolean;
  sources: PlanSource[];
  artifact?: { repo: string; tag: string };
  releases?: string[];
}

export interface PlanContext {
  app: App;
  namespace: string;
  directory: string;
  target: Target;
  world: World;
  annotations: Record<string, string>;
  refs: Refs;
  chart(
    name: string,
    values: Record<string, unknown>,
    options?: { valueFiles?: string[]; retain?: boolean },
  ): PlatformChart;
  readValues(path: string): Record<string, unknown>;
}

export interface Secrets {
  synced?: Record<string, unknown>[];
  generated?: Record<string, unknown>[];
  templates?: Record<string, unknown>[];
}

export interface Contribution {
  charts?: PlatformChart[];
  ingress?: string[];
  appIngress?: string[];
  secrets?: Secrets;
}

export interface Kustomize {
  generators?: Record<string, unknown>[];
  resources?: string[];
}

export interface GeneratedFile {
  path: string;
  content: string;
  stamp?: boolean;
}

export type Fail = (message: string) => never;

export interface CheckContext {
  world: World;
  external: boolean;
  refs: Refs;
  uses: Record<string, unknown>[];
  fail: Fail;
}

export interface Graph {
  apps: App[];
  node(id: string, label: string, kind: 'app' | 'database' | 'platform' | 'namespace'): string;
  edge(edge: { from: string; to: string; label?: string; dashed?: boolean }): void;
}

export interface DoctorContext {
  kube: Kube;
  ns: Kube;
  app: App;
}

export type Block = Record<string, Record<string, unknown>>;

export const tfRef = (expr: string) => `\${${expr}}`;

export interface ResourceProvider<T extends ResourceType = ResourceType> {
  type: T;
  ref?: 'value' | 'secret' | 'volume';
  schema: Schema;
  validate?(app: App, resources: ResourceTypes[T][], fail: Fail): void;
  unused?(
    app: App,
    resource: ResourceTypes[T],
    uses: Record<string, unknown>[],
  ): string | false | undefined;
  uses?(app: App, resources: ResourceTypes[T][]): Record<string, unknown>[];
  check?(app: App, resources: ResourceTypes[T][], ctx: CheckContext): void;
  checkWorld?(world: World): void;
  value?(app: App, resource: ResourceTypes[T], world: World): string;
  plan?(ctx: PlanContext, resources: ResourceTypes[T][]): Contribution;
  kustomize?(app: App, resources: ResourceTypes[T][]): Kustomize;
  files?(app: App, resources: ResourceTypes[T][]): string[];
  generate?(app: App, target: Target, resources: ResourceTypes[T][]): GeneratedFile[];
  generateWorld?(world: World, read: (path: string) => string): GeneratedFile[];
  terraform?(app: App, resources: ResourceTypes[T][]): Block[];
  terraformOutputs?(apps: App[]): Record<string, unknown>;
  graph?(app: App, resource: ResourceTypes[T], graph: Graph): void;
  doctor?: { label: string; run(ctx: DoctorContext, resources: ResourceTypes[T][]): Check[] };
}

export interface ValidateSourceContext {
  app: App;
  kustomized: boolean;
  fail: Fail;
}

export interface SourceProvider<T extends SourceType = SourceType> {
  type: T;
  schema: Schema;
  validate?(ctx: ValidateSourceContext, sources: SourceTypes[T]['source'][]): void;
  files?(app: App, source: SourceTypes[T]['source']): string[];
  plan(ctx: PlanContext, source: SourceTypes[T]['source']): SourceTypes[T]['plan'];
  application(plan: Plan, source: SourceTypes[T]['plan']): Record<string, unknown>;
  flux(plan: Plan, source: SourceTypes[T]['plan']): FluxObject[];
  helmTemplate?(
    plan: Plan,
    source: SourceTypes[T]['plan'],
  ): { release: string; location: string[]; values: Record<string, unknown> };
}
