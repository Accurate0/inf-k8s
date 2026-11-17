import type { Schema } from 'jsonschema/lib/index.js';
import type { App } from '../types.ts';
import { declared, specs } from '../app.ts';
import { appServices } from '../services.ts';
import type { ResourceProvider } from './provider.ts';

export interface Metrics {
  path?: string;
  port: string;
  interval?: string;
  workload: string;
}

declare module './provider.ts' {
  interface ResourceTypes {
    metrics: { name: string; type: 'metrics' } & Metrics;
  }
}

const schema: Schema = {
  type: 'object',
  additionalProperties: false,
  properties: {
    name: {
      type: 'string',
    },
    type: {
      enum: ['metrics'],
    },
    path: {
      type: 'string',
    },
    interval: {
      type: 'string',
    },
    workload: {
      type: 'string',
    },
    port: {
      type: 'string',
    },
  },
  required: ['name', 'type', 'workload', 'port'],
};

export function appMetrics(app: App): Metrics[] {
  return specs(declared(app, 'metrics')).map(([, spec]) => spec as Metrics);
}

export const metrics: ResourceProvider<'metrics'> = {
  type: 'metrics',
  schema,

  validate(app, _metrics, fail) {
    const scraped = appMetrics(app).map((scrape) => scrape.workload);
    for (const workload of scraped) {
      if (scraped.indexOf(workload) !== scraped.lastIndexOf(workload)) {
        fail(`metrics: ${workload} is listed twice`);
      }
    }
  },

  check(app, _metrics, { fail }) {
    const services = appServices(app);
    for (const { workload, port } of appMetrics(app)) {
      const service = services.find((s) => s.workload === workload);
      if (!service?.ports.length) {
        fail(`metrics: workload ${workload} has no service ports`);
      }
      if (!service?.ports.some((p) => p.name === port)) {
        fail(`metrics: workload ${workload} has no service port named ${port}`);
      }
    }
  },

  plan({ app, namespace, chart }) {
    const scrapes = appMetrics(app);
    return {
      ingress: scrapes.length ? ['monitoring'] : [],
      charts: scrapes.map(({ workload, ...scrape }) =>
        chart('k8s-observability', { name: workload, namespace, ...scrape }),
      ),
    };
  },
};
