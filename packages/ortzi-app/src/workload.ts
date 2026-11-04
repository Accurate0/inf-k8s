type Values = Record<string, any>;

export type SecretKind = 'secret' | 'database' | 'bucket' | 'oidc';

export interface Refs {
  resource: (resource: string, env: string) => { name: string; value: string };
  secret: (kind: SecretKind, name: string) => string;
  config: (name: string) => { configMap: string; files: string[] };
  volume: (name: string) => string;
  service: (service: string, port: string | number, path?: string) => string;
}

const SECRET_KINDS: SecretKind[] = ['secret', 'database', 'bucket', 'oidc'];

const secretKind = (value: Values) => SECRET_KINDS.find((kind) => value[kind]);

const PROBES = {
  startup: 'startupProbe',
  readiness: 'readinessProbe',
  liveness: 'livenessProbe',
} as const;

const TIMINGS = {
  delay: 'initialDelaySeconds',
  every: 'periodSeconds',
  timeout: 'timeoutSeconds',
  failures: 'failureThreshold',
} as const;

function expandEnv(env: Values, refs: Refs) {
  return Object.entries(env).map(([name, value]) => {
    if (typeof value !== 'object' || value === null) return { name, value: String(value) };
    if (value.resource) return refs.resource(value.resource, name);
    if (value.service) return { name, value: refs.service(value.service, value.port, value.path) };

    const kind = secretKind(value);
    if (kind) {
      const optional = value.optional === undefined ? {} : { optional: value.optional };
      return {
        name,
        valueFrom: {
          secretKeyRef: { name: refs.secret(kind, value[kind]), key: value.key, ...optional },
        },
      };
    }

    if (value.field) return { name, valueFrom: { fieldRef: { fieldPath: value.field } } };
    return { name, ...value };
  });
}

export type EnvSource =
  | { name: string; from: string; value: string }
  | { name: string; from: string; value?: string; secret: SecretKey };

export interface SecretKey {
  name: string;
  key?: string;
  optional: boolean;
}

export function describeEnv(env: Values, refs: Refs): EnvSource[] {
  return Object.entries(env).map(([name, value]): EnvSource => {
    if (typeof value !== 'object' || value === null) {
      return { name, from: 'literal', value: String(value) };
    }

    if (value.resource) {
      return {
        name,
        from: `resource ${value.resource}`,
        value: refs.resource(value.resource, name).value,
      };
    }

    if (value.service) {
      return {
        name,
        from: `service ${value.service}:${value.port}${value.path ?? ''}`,
        value: refs.service(value.service, value.port, value.path),
      };
    }

    const kind = secretKind(value);
    if (kind) {
      return {
        name,
        from: `${kind} ${value[kind]}`,
        secret: {
          name: refs.secret(kind, value[kind]),
          key: value.key,
          optional: !!value.optional,
        },
      };
    }

    if (value.field) return { name, from: 'field', value: value.field };
    return { name, from: 'raw', value: JSON.stringify(value) };
  });
}

export function describeEnvFrom(from: unknown, refs: Refs): EnvSource {
  const value = from as Values;
  const kind = secretKind(value);
  if (!kind) return { name: 'envFrom', from: 'raw', value: JSON.stringify(value) };

  return {
    name: value.prefix ? `envFrom ${value.prefix}*` : 'envFrom',
    from: `${kind} ${value[kind]}`,
    secret: { name: refs.secret(kind, value[kind]), optional: !!value.optional },
  };
}

function expandProbes({ http, grpc, exec, ...probes }: Values) {
  const target = http ? { httpGet: http } : grpc ? { grpc } : { exec };
  return Object.fromEntries(
    Object.entries(probes).map(([probe, timings]) => [
      PROBES[probe as keyof typeof PROBES],
      {
        ...target,
        ...Object.fromEntries(
          Object.entries(timings as Values).map(([key, value]) => [
            TIMINGS[key as keyof typeof TIMINGS],
            value,
          ]),
        ),
      },
    ]),
  );
}

function expandMounts(mounts: Values, refs: Refs) {
  const entries = Object.entries<Values>(mounts).map(([name, mount]): [string, Values] => {
    if (mount.volume) return [name, { ...mount, claimName: refs.volume(mount.volume) }];
    if (!mount.config) return [name, mount];
    const { configMap, files } = refs.config(mount.config);
    return [
      name,
      { ...mount, configMap, items: files.map((path) => ({ key: path.split('/').pop(), path })) },
    ];
  });

  return {
    volumes: entries.map(([name, { claimName, configMap, items }]) => {
      if (claimName) return { name, persistentVolumeClaim: { claimName } };
      return configMap
        ? {
            name,
            configMap: {
              name: configMap,
              ...(items ? { items } : {}),
            },
          }
        : { name, emptyDir: {} };
    }),
    volumeMounts: entries.map(([name, { path, configMap }]) => ({
      name,
      mountPath: path,
      ...(configMap ? { readOnly: true } : {}),
    })),
  };
}

function expandResources({ cpu, memory, memoryLimit }: Values) {
  const requests = { ...(cpu ? { cpu } : {}), ...(memory ? { memory } : {}) };
  const limits = memoryLimit ? { memory: memoryLimit } : {};
  return {
    ...(Object.keys(requests).length ? { requests } : {}),
    ...(Object.keys(limits).length ? { limits } : {}),
  };
}

export const declaredPorts = (spec: Values): Values[] =>
  [spec, ...(spec.sidecars ?? [])].flatMap((container: Values) => container.ports ?? []);

function expandEnvFrom(from: unknown, refs: Refs) {
  const value = from as Values;
  const kind = secretKind(value);
  if (!kind) return value;

  const { [kind]: name, optional, prefix } = value;
  return {
    ...(prefix ? { prefix } : {}),
    secretRef: { name: refs.secret(kind, name), ...(optional === undefined ? {} : { optional }) },
  };
}

export function expandContainer(spec: Values, refs: Refs): Values {
  const { probes, mounts, ...out } = spec;

  if (out.env && !Array.isArray(out.env)) out.env = expandEnv(out.env, refs);
  if (out.envFrom) {
    out.envFrom = out.envFrom.map((from: unknown) => expandEnvFrom(from, refs));
  }
  if (out.resources) out.resources = expandResources(out.resources);
  if (out.ports) {
    out.ports = out.ports.map((port: Values) => ({
      name: port.name,
      containerPort: port.container,
    }));
  }

  return {
    ...out,
    ...(probes ? expandProbes(probes) : {}),
    ...(mounts ? expandMounts(mounts, refs) : {}),
  };
}

export function expandWorkload(spec: Values, refs: Refs): Values {
  const { overrides, serviceAccount, ...rest } = spec;
  const out = expandContainer(rest, refs);
  if (serviceAccount) out.serviceAccountName = serviceAccount;

  const exposed = declaredPorts(spec).filter((port) => port.service);
  if (exposed.length) {
    out.service = {
      ...out.service,
      ports: exposed.map(({ name, service, container, appProtocol }) => ({
        name,
        port: service,
        targetPort: container,
        ...(appProtocol ? { appProtocol } : {}),
        protocol: 'TCP',
      })),
    };
  }

  const volumes: Values[] = out.volumes ?? [];
  for (const key of ['initContainers', 'sidecars']) {
    if (!out[key]) continue;
    out[key] = out[key].map((container: Values) => {
      const { volumes: own = [], ...expanded } = expandContainer(container, refs);
      for (const volume of own) {
        const existing = volumes.find((v) => v.name === volume.name);
        if (!existing) volumes.push(volume);
        else if (JSON.stringify(existing) !== JSON.stringify(volume))
          throw new Error(`volume ${volume.name} is mounted from different sources`);
      }
      return expanded;
    });
  }
  if (volumes.length) out.volumes = volumes;

  return { ...out, ...overrides };
}

export function overrideConflicts(spec: Values): string[] {
  const set = new Set(Object.keys(spec));
  if (spec.probes) for (const probe of Object.values(PROBES)) set.add(probe);
  if (spec.mounts) ['volumes', 'volumeMounts'].forEach((key) => set.add(key));
  return Object.keys(spec.overrides ?? {}).filter((key) => set.has(key));
}
