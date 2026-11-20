import type { Schema } from 'jsonschema/lib/index.js';
import { RESOURCES, SOURCES } from './providers/index.ts';
import { workloads } from './providers/workload.ts';

export const APP_SCHEMA: Schema = {
  $schema: 'http://json-schema.org/draft-07/schema#',
  $id: 'https://k8s-schemas.anurag.sh/app.json',
  title: 'app.yaml',
  type: 'object',
  additionalProperties: false,
  required: ['name', 'network'],
  properties: {
    name: {
      type: 'string',
      pattern: '^[a-z0-9]([a-z0-9-]*[a-z0-9])?$',
    },
    repository: {
      type: 'string',
    },
    charts: {
      type: 'object',
      description: 'Pin chart versions for this app.',
      additionalProperties: {
        type: 'string',
      },
    },
    chartRegistry: {
      type: 'string',
      description:
        'OCI registry the k8s-* platform charts are pulled from. Defaults to oci://registry.anurag.sh/charts.',
    },
    network: {
      type: 'object',
      additionalProperties: false,
      required: ['ingress'],
      properties: {
        ingress: {
          type: 'array',
          description: 'Namespaces allowed to reach this app.',
          items: {
            type: 'string',
          },
        },
        policy: {
          type: 'boolean',
          description:
            "Render the namespace's default-deny network policies. Defaults to true; must be set explicitly with namespace.name.",
        },
      },
    },
    workloads: workloads.schema,
    resources: {
      type: 'array',
      description: 'Named dependencies and what the app exposes.',
      items: {
        oneOf: RESOURCES.map((provider) => provider.schema),
      },
    },
    exports: {
      type: 'array',
      description: 'Workloads other apps can use as a resource.',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['name', 'workload', 'port'],
        properties: {
          name: {
            type: 'string',
          },
          workload: {
            type: 'string',
          },
          port: {
            type: ['string', 'integer'],
            description: 'Service port name or number.',
          },
        },
      },
    },
    artifact: {
      description:
        "Deliver the app to Argo CD without git: charts come from their registries with values and image tags inline in the Application, and `ortzi app publish` pushes the kustomization's build to this repository. git and artifact sources stay live sources next to it.",
      type: 'object',
      additionalProperties: false,
      required: ['repo', 'tag'],
      properties: {
        repo: {
          type: 'string',
          pattern: '^oci://[^/]+/[^/]+/[^/]+$',
          description:
            "Repository the kustomization's build is pushed to and synced from, e.g. oci://registry.anurag.sh/manifests/<app>.",
        },
        tag: {
          type: 'string',
          description: 'Tag publish moves and Argo CD syncs.',
        },
      },
    },
    sources: {
      type: 'array',
      description: 'What the app deploys beyond its workloads and resources.',
      items: {
        oneOf: SOURCES.map((provider) => provider.schema),
      },
    },
    overrides: {
      description:
        'Change what one output generates. Keyed by output, then generated object. Every block is validated, even for outputs that are not active.',
      type: 'object',
      additionalProperties: false,
      properties: {
        argocd: {
          type: 'object',
          additionalProperties: false,
          properties: {
            application: {
              description: 'Change the rendered Argo Application.',
              type: 'object',
              additionalProperties: false,
              properties: {
                options: {
                  type: 'array',
                  description: 'Replaces the default syncOptions.',
                  items: {
                    type: 'string',
                  },
                },
                automated: {
                  type: 'boolean',
                  description: 'Sync automatically with prune and self-heal. Defaults to true.',
                },
                ignoreDifferences: {
                  type: 'array',
                  items: {
                    type: 'object',
                  },
                },
              },
            },
          },
        },
        flux: {
          type: 'object',
          additionalProperties: false,
          properties: {
            kustomization: {
              description: 'Change the rendered per-app Flux Kustomization.',
              type: 'object',
              additionalProperties: false,
              properties: {
                dependsOn: {
                  type: 'array',
                  description: 'Apps whose Kustomization must be ready before this one applies.',
                  items: {
                    type: 'string',
                  },
                },
                timeout: {
                  type: 'string',
                  description: 'How long to wait for the app to become ready. Defaults to 5m.',
                },
              },
            },
            helmRelease: {
              description: 'Change every rendered HelmRelease.',
              type: 'object',
              additionalProperties: false,
              properties: {
                driftIgnore: {
                  type: 'array',
                  description: 'driftDetection.ignore rules: JSON pointers Flux does not correct.',
                  items: {
                    type: 'object',
                  },
                },
              },
            },
          },
        },
      },
    },
    build: {
      type: 'object',
      additionalProperties: false,
      required: ['image', 'ci'],
      properties: {
        image: {
          type: 'string',
        },
        ci: {
          type: 'string',
          enum: ['github', 'forgejo'],
          description:
            'Where the image is built and pushed from: github, or forgejo, which pushes with a short-lived robot from `ortzi registry login` and cosign-signs the image with `ortzi registry sign`.',
        },
        context: {
          type: 'string',
          description: 'Defaults to the app directory.',
        },
        dockerfile: {
          type: 'string',
          description: 'Defaults to <app directory>/Dockerfile.',
        },
        args: {
          type: 'object',
          additionalProperties: {
            type: 'string',
          },
        },
        paths: {
          type: 'array',
          description: 'Extra paths that trigger a build.',
          items: {
            type: 'string',
          },
        },
        test: {
          type: 'object',
          additionalProperties: false,
          required: ['run'],
          properties: {
            run: {
              type: 'string',
              description: 'Runs in the app directory.',
            },
            db: {
              type: 'object',
              additionalProperties: false,
              required: ['image'],
              properties: {
                image: {
                  type: 'string',
                  description: 'Postgres image, pinned by digest.',
                },
                databaseName: {
                  type: 'string',
                  description: 'Defaults to postgres.',
                },
              },
            },
            packages: {
              type: 'array',
              items: {
                type: 'string',
              },
            },
            setup: {
              type: 'string',
              description:
                'Extra shell run before the tests, after packages and the toolchain are installed.',
            },
            toolchain: {
              type: 'object',
              additionalProperties: false,
              required: ['rust'],
              properties: {
                rust: {
                  type: 'string',
                  pattern: '^\\d+\\.\\d+\\.\\d+$',
                  description:
                    'Rust toolchain installed with rustup before the tests, e.g. 1.99.1.',
                },
              },
            },
          },
        },
      },
    },
    namespace: {
      type: 'object',
      additionalProperties: false,
      properties: {
        name: {
          type: 'string',
          description: 'Deploy into this existing namespace instead of one named after the app.',
        },
        labels: {
          type: 'object',
          additionalProperties: {
            type: 'string',
          },
          description:
            'Applied through k8s-workload, or managedNamespaceMetadata for an app with no workloads.',
        },
      },
    },
  },
};

export const schemaJson = () => `${JSON.stringify(APP_SCHEMA, null, 2)}\n`;
