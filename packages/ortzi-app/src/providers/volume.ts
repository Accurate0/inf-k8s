import type { Schema } from 'jsonschema/lib/index.js';
import type { App, PlatformExport } from '../types.ts';
import { fail, type KubeObject, ok } from '../checks.ts';
import { mountedVolumes } from './workload.ts';
import type { ResourceProvider } from './provider.ts';

export interface Volume {
  name: string;
  type: 'volume';
  claimName: string;
  class: string;
  size: string;
  access: string;
  backup?: 'daily';
}

declare module './provider.ts' {
  interface ResourceTypes {
    volume: Volume;
  }
}

const schema: Schema = {
  type: 'object',
  additionalProperties: false,
  required: ['name', 'type', 'claimName', 'class', 'size', 'access'],
  properties: {
    name: {
      type: 'string',
    },
    type: {
      enum: ['volume'],
    },
    claimName: {
      type: 'string',
      description: 'Name of the persistent volume claim.',
    },
    class: {
      type: 'string',
      description: 'A storage export from platform/exports.',
    },
    size: {
      type: 'string',
    },
    access: {
      enum: ['ReadWriteOnce', 'ReadWriteMany', 'ReadOnlyMany', 'ReadWriteOncePod'],
    },
    backup: {
      enum: ['daily'],
    },
  },
};

export function storageClass(app: App, volume: Volume, platform: PlatformExport[]) {
  const found = platform.find((e) => e.type === 'storage' && e.name === volume.class);
  if (!found?.storageClass) {
    throw new Error(
      `${app.name}: resources.${volume.name}: no storage export named ${volume.class}`,
    );
  }

  if (volume.backup && !found.backupLabels) {
    throw new Error(
      `${app.name}: resources.${volume.name}: storage ${volume.class} has no backups`,
    );
  }

  return {
    storageClass: found.storageClass,
    labels: volume.backup ? found.backupLabels : undefined,
  };
}

export const volume: ResourceProvider<'volume'> = {
  type: 'volume',
  ref: 'volume',
  schema,

  unused: (app, resource) =>
    mountedVolumes(app).includes(resource.name) ? false : 'no workload mounts this volume',

  check(app, volumes, { world }) {
    for (const resource of volumes) {
      storageClass(app, resource, world.platform);
    }
  },

  plan({ app, namespace, annotations, world, chart }, volumes) {
    if (!volumes.length) {
      return {};
    }
    return {
      charts: [
        chart(
          'k8s-storage',
          {
            volumes: volumes.map((resource) => {
              const { storageClass: className, labels } = storageClass(
                app,
                resource,
                world.platform,
              );
              return {
                name: resource.claimName,
                namespace,
                storageClass: className,
                size: resource.size,
                access: resource.access,
                ...(labels ? { labels } : {}),
                annotations,
              };
            }),
          },
          { retain: true },
        ),
      ],
    };
  },

  doctor: {
    label: 'volumes',
    run: ({ ns }, volumes) =>
      volumes.map(({ claimName }) => {
        const phase = ns.get<KubeObject>('persistentvolumeclaims', claimName).status?.phase;
        return phase === 'Bound'
          ? ok(`volume ${claimName} bound`)
          : fail(`volume ${claimName} ${phase}`);
      }),
  },
};
