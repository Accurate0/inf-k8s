import type { App, Target, Versions, World } from './types.ts';
import type {
  Contribution,
  Plan,
  PlanContext,
  PlanSource,
  PlatformChart,
  ResourceType,
} from './providers/provider.ts';
import { appNamespace, declared } from './app.ts';
import { refsFor } from './refs.ts';
import { releaseImages, workloads } from './providers/workload.ts';
import { RESOURCES, sourceProvider } from './providers/index.ts';
import { HARBOR } from './flux-objects.ts';

export type { Plan, PlanSource, PlatformChart } from './providers/provider.ts';

export const CHART_REGISTRY = 'oci://registry.anurag.sh/charts';

// Charts render in this order; any chart not listed goes after them, in registry order.
const CHART_ORDER = [
  'k8s-workload',
  'k8s-network',
  'k8s-http-routes',
  'k8s-oidc',
  'k8s-postgres',
  'k8s-secrets',
  'k8s-storage',
  'k8s-observability',
];

const rank = (chart: string) => {
  const index = CHART_ORDER.indexOf(chart);
  return index === -1 ? CHART_ORDER.length : index;
};

export function planApp(app: App, versions: Versions, target: Target, world: World): Plan {
  const { name } = app;
  const namespace = appNamespace(app);
  const directory = `${target.location}/${name}`;

  const readValues = (path: string) => {
    if (!world.readValues) {
      throw new Error(`${name}: values are inlined, pass readValues`);
    }
    return world.readValues(path);
  };

  const chart: PlanContext['chart'] = (
    chartName,
    values,
    { valueFiles, valuesFrom, retain } = {},
  ) => {
    const version = app.charts?.[chartName] ?? versions[chartName];
    if (!version) {
      throw new Error(`${app.name}: no version for chart ${chartName}`);
    }
    if (valueFiles) {
      values = Object.assign({}, ...valueFiles.map(readValues), values);
    }

    return {
      type: 'chart',
      chart: chartName,
      registry: app.chartRegistry ?? CHART_REGISTRY,
      version,
      values,
      ...(retain ? { retain } : {}),
      ...(valuesFrom
        ? { valuesFrom: { configMap: valuesFrom.configMap, values: readValues(valuesFrom.path) } }
        : {}),
    };
  };

  const ctx: PlanContext = {
    app,
    namespace,
    directory,
    target,
    world,
    annotations: {
      'inf-k8s.net/app': name,
      'inf-k8s.net/repository': app.repository ?? 'https://github.com/Accurate0/inf-k8s',
    },
    refs: refsFor(app, world),
    chart,
    readValues,
  };

  const contributions: Contribution[] = [
    workloads.plan(ctx),
    ...RESOURCES.map(
      (provider) => provider.plan?.(ctx, declared(app, provider.type as ResourceType)) ?? {},
    ),
  ];
  const charts = contributions.flatMap((contribution) => contribution.charts ?? []);

  if (app.network.policy !== false) {
    const ingress = [
      ...contributions.flatMap((contribution) => contribution.ingress ?? []),
      ...app.network.ingress,
      ...contributions.flatMap((contribution) => contribution.appIngress ?? []),
    ];
    charts.push(chart('k8s-network', { namespace, ingress: [...new Set(ingress)] }));
  }

  const secrets = contributions.flatMap((contribution) => contribution.secrets ?? []);
  const synced = secrets.flatMap((s) => s.synced ?? []);
  const generated = secrets.flatMap((s) => s.generated ?? []);
  const pushed = secrets.flatMap((s) => s.pushed ?? []);
  const templates = secrets.flatMap((s) => s.templates ?? []);
  if (synced.length || generated.length || pushed.length || templates.length) {
    charts.push(
      chart('k8s-secrets', {
        defaults: {
          namespace,
          provider: 'openbao',
          resyncInterval: 900,
          annotations: ctx.annotations,
        },
        secrets: synced,
        ...(generated.length ? { generated } : {}),
        ...(pushed.length ? { pushed } : {}),
        ...(templates.length ? { templates } : {}),
      }),
    );
  }

  const sources: PlanSource[] = [
    ...charts
      .map((source, index) => ({ source, index }))
      .sort((a, b) => rank(a.source.chart) - rank(b.source.chart) || a.index - b.index)
      .map(({ source }) => source),
    ...(app.sources ?? []).map((source) => sourceProvider(source.type).plan(ctx, source as never)),
  ];

  return {
    name,
    namespace,
    location: target.location,
    directory,
    ...(app.namespace?.labels ? { namespaceLabels: app.namespace.labels } : {}),
    ownsNamespace: (app.workloads ?? []).length > 0,
    sources,
    ...(app.artifact ? { artifact: app.artifact } : {}),
    ...(releaseImages(app).length ? { releases: releaseImages(app) } : {}),
  };
}

const RETAIN = { 'helm.sh/resource-policy': 'keep' };

export const chartValues = (source: PlatformChart) => ({
  ...source.values,
  ...(source.retain ? { retainAnnotations: RETAIN } : {}),
});

export function helmTemplates(plan: Plan): { args: string[]; values: string }[] {
  return plan.sources.flatMap((source) => {
    const template =
      source.type === 'chart'
        ? {
            release: plan.name,
            location: [`charts/${source.chart}`],
            values: { ...source.valuesFrom?.values, ...chartValues(source) },
          }
        : sourceProvider(source.type).helmTemplate?.(plan, source as never);
    if (!template) {
      return [];
    }

    return [
      {
        args: [
          'template',
          template.release,
          ...template.location,
          '--namespace',
          plan.namespace,
          '-f',
          '-',
        ],
        values: JSON.stringify(template.values),
      },
    ];
  });
}

export function harborProjects(plans: Plan[]): string[] {
  const projects = plans.flatMap((plan) =>
    plan.sources.flatMap((source) =>
      source.type === 'helm' && source.repo.startsWith(HARBOR)
        ? [source.repo.slice(HARBOR.length).split('/')[0]!]
        : [],
    ),
  );

  return [...new Set(projects)].sort();
}
