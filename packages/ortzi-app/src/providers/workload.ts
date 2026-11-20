import type { Schema } from 'jsonschema/lib/index.js';
import type { App, NamedWorkload, Sidecar, Workload } from '../types.ts';
import { declared } from '../app.ts';
import {
  declaredPorts,
  expandWorkload,
  overrideConflicts,
  type Refs,
  SECRET_KINDS,
} from '../workload.ts';
import type { Contribution, Fail, PlanContext } from './provider.ts';

const schema: Schema = {
  type: 'array',
  description: 'What the app runs.',
  items: {
    type: 'object',
    additionalProperties: false,
    required: ['name', 'image'],
    properties: {
      name: {
        type: 'string',
      },
      image: {
        type: 'string',
      },
      release: {
        enum: ['deploy'],
        description:
          "Run whatever build of the image is promoted to the registry's deploy tag (`ortzi app promote`) instead of a tag in image.values.yaml.",
      },
      kind: {
        enum: ['Deployment', 'DaemonSet', 'CronJob', 'Job'],
      },
      replicas: {
        type: 'integer',
      },
      autoscale: {
        type: 'object',
        additionalProperties: false,
        required: ['min', 'max', 'cpu'],
        description:
          'Run a HorizontalPodAutoscaler instead of fixed replicas. cpu is the target average utilisation, as a percent of requests. Deployments only.',
        properties: {
          min: {
            type: 'integer',
            minimum: 1,
          },
          max: {
            type: 'integer',
            minimum: 1,
          },
          cpu: {
            type: 'integer',
            minimum: 1,
            maximum: 100,
          },
        },
      },
      schedule: {
        type: 'string',
      },
      phase: {
        enum: ['preSync', 'postSync'],
        description:
          'Run the Job as a Helm hook: preSync before anything else applies, postSync once everything is applied and healthy. Required with kind Job.',
      },
      service: {
        type: 'object',
        additionalProperties: false,
        properties: {
          name: {
            type: 'string',
          },
          trafficDistribution: {
            type: 'string',
          },
        },
        required: ['name'],
      },
      resources: {
        type: 'object',
        additionalProperties: false,
        properties: {
          cpu: {
            type: 'string',
          },
          memory: {
            type: 'string',
          },
          memoryLimit: {
            type: 'string',
          },
        },
      },
      env: {
        description:
          'Map of variable to a string, {resource, part}, {service, port}, {secret|database|bucket|oidc, key} or {field}. A list is the raw form.',
        oneOf: [
          {
            type: 'array',
            items: {
              type: 'object',
            },
          },
          {
            type: 'object',
            additionalProperties: {
              oneOf: [
                {
                  type: ['string', 'number', 'boolean'],
                },
                {
                  type: 'object',
                  additionalProperties: false,
                  required: ['resource'],
                  properties: {
                    resource: {
                      type: 'string',
                    },
                    part: {
                      enum: ['url', 'host', 'port'],
                      description: 'Part of the resource URL. Defaults to url.',
                    },
                  },
                },
                {
                  type: 'object',
                  additionalProperties: false,
                  required: ['service', 'port'],
                  properties: {
                    service: {
                      type: 'string',
                      description: 'A service of this app.',
                    },
                    port: {
                      type: ['string', 'integer'],
                    },
                    path: {
                      type: 'string',
                    },
                  },
                },
                {
                  type: 'object',
                  additionalProperties: false,
                  required: ['field'],
                  properties: {
                    field: {
                      type: 'string',
                    },
                  },
                },
                {
                  type: 'object',
                  additionalProperties: false,
                  required: ['secret', 'key'],
                  properties: {
                    secret: {
                      type: 'string',
                    },
                    key: {
                      type: 'string',
                    },
                    optional: {
                      type: 'boolean',
                    },
                  },
                },
                {
                  type: 'object',
                  additionalProperties: false,
                  required: ['database', 'key'],
                  properties: {
                    database: {
                      type: 'string',
                    },
                    key: {
                      type: 'string',
                    },
                    optional: {
                      type: 'boolean',
                    },
                  },
                },
                {
                  type: 'object',
                  additionalProperties: false,
                  required: ['bucket', 'key'],
                  properties: {
                    bucket: {
                      type: 'string',
                    },
                    key: {
                      type: 'string',
                    },
                    optional: {
                      type: 'boolean',
                    },
                  },
                },
                {
                  type: 'object',
                  additionalProperties: false,
                  required: ['oidc', 'key'],
                  properties: {
                    oidc: {
                      type: 'string',
                    },
                    key: {
                      type: 'string',
                    },
                    optional: {
                      type: 'boolean',
                    },
                  },
                },
              ],
            },
          },
        ],
      },
      envFrom: {
        type: 'array',
        description: '{secret|database|bucket|oidc: resource name}, or a raw secretRef object.',
        items: {
          type: 'object',
          additionalProperties: false,
          properties: {
            secret: {
              type: 'string',
            },
            database: {
              type: 'string',
            },
            bucket: {
              type: 'string',
            },
            oidc: {
              type: 'string',
            },
            optional: {
              type: 'boolean',
            },
            prefix: {
              type: 'string',
            },
            secretRef: {
              type: 'object',
            },
            configMapRef: {
              type: 'object',
            },
          },
        },
      },
      readOnlyRootFilesystem: {
        type: 'boolean',
      },
      runAsNonRoot: {
        type: 'boolean',
      },
      probes: {
        type: 'object',
        additionalProperties: false,
        properties: {
          http: {
            type: 'object',
            additionalProperties: false,
            required: ['path', 'port'],
            properties: {
              path: {
                type: 'string',
              },
              port: {
                type: ['integer', 'string'],
              },
            },
          },
          grpc: {
            type: 'object',
            additionalProperties: false,
            required: ['port'],
            properties: {
              port: {
                type: 'integer',
              },
            },
          },
          exec: {
            type: 'object',
            additionalProperties: false,
            required: ['command'],
            properties: {
              command: {
                type: 'array',
                minItems: 1,
                items: {
                  type: 'string',
                },
              },
            },
          },
          startup: {
            type: 'object',
            additionalProperties: false,
            properties: {
              delay: {
                type: 'integer',
              },
              every: {
                type: 'integer',
              },
              timeout: {
                type: 'integer',
              },
              failures: {
                type: 'integer',
              },
            },
          },
          readiness: {
            type: 'object',
            additionalProperties: false,
            properties: {
              delay: {
                type: 'integer',
              },
              every: {
                type: 'integer',
              },
              timeout: {
                type: 'integer',
              },
              failures: {
                type: 'integer',
              },
            },
          },
          liveness: {
            type: 'object',
            additionalProperties: false,
            properties: {
              delay: {
                type: 'integer',
              },
              every: {
                type: 'integer',
              },
              timeout: {
                type: 'integer',
              },
              failures: {
                type: 'integer',
              },
            },
          },
        },
      },
      mounts: {
        type: 'object',
        description: 'Keyed by volume name. ConfigMaps mount read-only.',
        additionalProperties: {
          type: 'object',
          additionalProperties: false,
          required: ['path'],
          properties: {
            path: {
              type: 'string',
            },
            configMap: {
              type: 'string',
            },
            emptyDir: {
              type: 'boolean',
            },
            config: {
              type: 'string',
              description: 'A config resource.',
            },
            volume: {
              type: 'string',
              description: 'A volume resource.',
            },
          },
        },
      },
      ports: {
        type: 'array',
        description: "{name, container, service}. service adds the port to the workload's Service.",
        items: {
          type: 'object',
          additionalProperties: false,
          required: ['name', 'container'],
          properties: {
            name: {
              type: 'string',
            },
            container: {
              type: 'integer',
            },
            service: {
              type: 'integer',
            },
            appProtocol: {
              type: 'string',
            },
          },
        },
      },
      overrides: {
        type: 'object',
        description: 'Written directly to the k8s-workload values for this workload.',
      },
      command: {
        type: 'array',
        items: {
          type: 'string',
        },
      },
      args: {
        type: 'array',
        items: {
          type: 'string',
        },
      },
      backoffLimit: {
        type: 'integer',
      },
      initContainers: {
        type: 'array',
        items: {
          type: 'object',
        },
      },
      sidecars: {
        type: 'array',
        items: {
          type: 'object',
          properties: {
            inherit: {
              type: 'array',
              description:
                "Copy these from the workload's main container. The sidecar's own env entries win over inherited ones.",
              uniqueItems: true,
              items: {
                enum: ['env', 'envFrom', 'mounts'],
              },
            },
            ports: {
              type: 'array',
              description:
                "{name, container, service}. service adds the port to the workload's Service.",
              items: {
                type: 'object',
                additionalProperties: false,
                required: ['name', 'container'],
                properties: {
                  name: {
                    type: 'string',
                  },
                  container: {
                    type: 'integer',
                  },
                  service: {
                    type: 'integer',
                  },
                  appProtocol: {
                    type: 'string',
                  },
                },
              },
            },
          },
        },
      },
      serviceAccount: {
        type: 'string',
        description: 'A serviceAccount resource.',
      },
    },
  },
};

function inherit(app: App, workload: string, spec: Workload): Workload {
  if (!spec.sidecars) {
    return spec;
  }

  const sidecars = spec.sidecars.map(({ inherit: keys = [], ...sidecar }) => {
    const merged: Sidecar = { ...sidecar };
    for (const key of keys) {
      if (spec[key] === undefined) {
        throw new Error(
          `${app.name}: workloads.${workload}: sidecar ${sidecar.name} inherits ${key}, but the workload has none`,
        );
      }

      if (key === 'envFrom') {
        merged.envFrom = [...(spec.envFrom ?? []), ...(sidecar.envFrom ?? [])];
      } else {
        merged[key] = { ...spec[key], ...sidecar[key] };
      }
    }
    return merged;
  });

  return { ...spec, sidecars };
}

export function appWorkloads(app: App): Record<string, Workload> {
  return Object.fromEntries(
    (app.workloads ?? []).map(({ name, ...spec }) => [name, inherit(app, name, spec)]),
  );
}

export const RELEASE_REGISTRY = 'registry.anurag.sh/';

export const releaseImages = (app: App): string[] => [
  ...new Set(
    Object.values(appWorkloads(app))
      .filter((w) => w.release)
      .map((w) => w.image),
  ),
];

export const workloadImages = (app: App) =>
  Object.values(appWorkloads(app)).flatMap((w) => [
    w.image,
    ...(w.initContainers ?? []).map((c) => c.image),
    ...(w.sidecars ?? []).map((c) => c.image),
  ]);

const containersOf = (spec: Workload) => [
  spec,
  ...((spec.initContainers ?? []) as Workload[]),
  ...((spec.sidecars ?? []) as Workload[]),
];

export const workloadRefs = (app: App): Record<string, unknown>[] =>
  Object.values(appWorkloads(app))
    .flatMap(containersOf)
    .flatMap((c) => [
      ...(c.env && !Array.isArray(c.env) ? Object.values(c.env) : []),
      ...(c.envFrom ?? []),
    ])
    .filter((ref): ref is Record<string, unknown> => typeof ref === 'object' && ref !== null);

export const mountedVolumes = (app: App) =>
  Object.values(appWorkloads(app)).flatMap((spec) =>
    containersOf(spec).flatMap((container) =>
      Object.values((container.mounts ?? {}) as Record<string, { volume?: string }>).flatMap(
        (mount) => mount.volume ?? [],
      ),
    ),
  );

function validate(app: App, fail: Fail) {
  const workloadNames = (app.workloads ?? []).map((workload) => workload.name);
  for (const name of workloadNames) {
    if (workloadNames.indexOf(name) !== workloadNames.lastIndexOf(name)) {
      fail(`workloads: duplicate name ${name}`);
    }
  }

  const released = releaseImages(app);
  if (released.length && !app.artifact) {
    fail('workloads: release needs an artifact app, whose images are injected at publish');
  }
  for (const [name, workload] of Object.entries(appWorkloads(app))) {
    if (workload.release && !workload.image.startsWith(RELEASE_REGISTRY)) {
      fail(`workloads.${name}.release: only ${RELEASE_REGISTRY} images are promoted`);
    }
    if (!workload.release && released.includes(workload.image)) {
      fail(`workloads.${name}: ${workload.image} is released elsewhere; set release here too`);
    }
    if (workload.kind === 'Job' && !workload.phase) {
      fail(`workloads.${name}.phase is required for a Job`);
    }
    if (workload.phase && workload.kind !== 'Job') {
      fail(`workloads.${name}.phase is only for a Job`);
    }
    if (workload.kind === 'Job' && declaredPorts(workload).length) {
      fail(`workloads.${name}: a Job cannot expose ports`);
    }
    if (declaredPorts(workload).length && !workload.service?.name) {
      fail(`workloads.${name}.service.name is required with ports`);
    }
    for (const key of overrideConflicts(workload)) {
      fail(`workloads.${name}.overrides.${key} is already set on the workload`);
    }
    if (workload.autoscale) {
      const { min, max } = workload.autoscale;
      if ((workload.kind ?? 'Deployment') !== 'Deployment') {
        fail(`workloads.${name}.autoscale is only for a Deployment`);
      }
      if (workload.replicas !== undefined) {
        fail(`workloads.${name}: set either replicas or autoscale, not both`);
      }
      if (min > max) {
        fail(`workloads.${name}.autoscale.min is more than max`);
      }
      for (const container of [workload, ...((workload.sidecars ?? []) as Workload[])]) {
        if (!container.resources?.cpu) {
          fail(`workloads.${name}.autoscale needs resources.cpu on every container`);
        }
      }
    }
    for (const container of containersOf(workload)) {
      const targets = ['http', 'grpc', 'exec'].filter((target) => container.probes?.[target]);
      if (targets.length > 1) {
        fail(`workloads.${name}.probes: use one of ${targets.join(', ')}, not several`);
      }
      for (const [env, value] of Object.entries(
        !container.env || Array.isArray(container.env) ? {} : container.env,
      )) {
        if (typeof value === 'string' && value.includes('.svc.cluster.local')) {
          fail(
            `workloads.${name}.env.${env}: use {service} or {resource} instead of a cluster address`,
          );
        }
      }
    }
  }
}

function check(app: App, refs: Refs) {
  for (const spec of Object.values(appWorkloads(app))) {
    expandWorkload(spec, refs);
  }
}

function plan({ app, namespace, directory, chart, refs }: PlanContext): Contribution {
  const workloads = Object.entries(appWorkloads(app));
  if (!workloads.length) {
    return {};
  }

  const serviceAccounts = declared(app, 'serviceAccount').map((account) => account.name);
  return {
    charts: [
      chart(
        'k8s-workload',
        {
          namespace,
          ...(app.namespace?.labels ? { namespaceLabels: app.namespace.labels } : {}),
          ...(serviceAccounts.length ? { serviceAccounts } : {}),
          workloads: workloads.map(([workload, spec]) => {
            return { name: workload, ...expandWorkload(spec, refs) };
          }),
        },
        { valueFiles: [`${directory}/image.values.yaml`] },
      ),
    ],
  };
}

export function passthroughWarnings(app: App): string[] {
  return Object.entries(appWorkloads(app)).flatMap(([workload, spec]) =>
    [
      [workload, spec] as const,
      ...((spec.initContainers ?? []) as NamedWorkload[]).map(
        (c) => [`${workload}/${c.name}`, c] as const,
      ),
      ...((spec.sidecars ?? []) as NamedWorkload[]).map(
        (c) => [`${workload}/${c.name}`, c] as const,
      ),
    ].flatMap(([container, c]) => {
      const where = `${app.name}: workloads.${container}`;
      return [
        ...(Array.isArray(c.env) ? [`${where}.env is a raw list; use a map with refs`] : []),
        ...(c.envFrom ?? [])
          .filter((from) => !SECRET_KINDS.some((kind) => (from as Record<string, unknown>)[kind]))
          .map(
            () =>
              `${where}.envFrom has a raw entry; reference a secret, database, bucket or oidc resource`,
          ),
        ...Object.entries((c.mounts ?? {}) as Record<string, { configMap?: string }>)
          .filter(([, mount]) => mount.configMap)
          .map(
            ([name]) => `${where}.mounts.${name} mounts a raw configMap; declare a config resource`,
          ),
      ];
    }),
  );
}

export const workloads = { schema, validate, check, plan };
