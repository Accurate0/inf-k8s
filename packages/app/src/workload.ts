type Values = Record<string, any>;

export type SecretKind = 'secret' | 'database' | 'bucket' | 'oidc';

export interface Refs {
  resource: (resource: string, env: string) => { name: string; value: string };
  secret: (kind: SecretKind, name: string) => string;
}

const SECRET_KINDS: SecretKind[] = ['secret', 'database', 'bucket', 'oidc'];

const secretKind = (value: Values) => SECRET_KINDS.find((kind) => value[kind]);

const PROBES = { startup: 'startupProbe', readiness: 'readinessProbe', liveness: 'livenessProbe' } as const;
const TIMINGS = { delay: 'initialDelaySeconds', every: 'periodSeconds', timeout: 'timeoutSeconds', failures: 'failureThreshold' } as const;

function expandEnv(env: Values, refs: Refs) {
  return Object.entries(env).map(([name, value]) => {
    if (typeof value !== 'object' || value === null) return { name, value: String(value) };
    if (value.resource) return refs.resource(value.resource, name);
    const kind = secretKind(value);
    if (kind) {
      const optional = value.optional === undefined ? {} : { optional: value.optional };
      return { name, valueFrom: { secretKeyRef: { name: refs.secret(kind, value[kind]), key: value.key, ...optional } } };
    }
    if (value.field) return { name, valueFrom: { fieldRef: { fieldPath: value.field } } };
    return { name, ...value };
  });
}

function expandProbes({ http, grpc, ...probes }: Values) {
  const target = http ? { httpGet: http } : { grpc };
  return Object.fromEntries(
    Object.entries(probes).map(([probe, timings]) => [
      PROBES[probe as keyof typeof PROBES],
      {
        ...target,
        ...Object.fromEntries(
          Object.entries(timings as Values).map(([key, value]) => [TIMINGS[key as keyof typeof TIMINGS], value]),
        ),
      },
    ]),
  );
}

function expandMounts(mounts: Values) {
  const entries = Object.entries<Values>(mounts);
  return {
    volumes: entries.map(([name, { configMap, items }]) =>
      configMap
        ? {
            name,
            configMap: {
              name: configMap,
              ...(items
                ? { items: items.map((path: string) => ({ key: path.split('/').pop(), path })) }
                : {}),
            },
          }
        : { name, emptyDir: {} },
    ),
    volumeMounts: entries.map(([name, { path, configMap }]) => ({
      name,
      mountPath: path,
      ...(configMap ? { readOnly: true } : {}),
    })),
  };
}

function expandResources({ cpu, memory, memoryLimit, ...raw }: Values) {
  const requests = { ...(cpu ? { cpu } : {}), ...(memory ? { memory } : {}), ...raw.requests };
  const limits = { ...(memoryLimit ? { memory: memoryLimit } : {}), ...raw.limits };
  return {
    ...(Object.keys(requests).length ? { requests } : {}),
    ...(Object.keys(limits).length ? { limits } : {}),
  };
}

function expandEnvFrom(from: unknown, refs: Refs) {
  if (typeof from === 'string') return { secretRef: { name: from } };
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
    out.ports = out.ports.map((port: unknown) => (typeof port === 'number' ? { containerPort: port } : port));
  }
  return { ...out, ...(probes ? expandProbes(probes) : {}), ...(mounts ? expandMounts(mounts) : {}) };
}

export function expandWorkload(spec: Values, refs: Refs): Values {
  const { overrides, ...rest } = spec;
  const out = expandContainer(rest, refs);
  for (const key of ['initContainers', 'sidecars']) {
    if (out[key]) out[key] = out[key].map((container: Values) => expandContainer(container, refs));
  }
  if (out.service?.ports) {
    out.service = {
      ...out.service,
      ports: out.service.ports.map((port: Values) => ({ ...port, protocol: port.protocol ?? 'TCP' })),
    };
  }
  return { ...out, ...overrides };
}

export function overrideConflicts(spec: Values): string[] {
  const set = new Set(Object.keys(spec));
  if (spec.probes) for (const probe of Object.values(PROBES)) set.add(probe);
  if (spec.mounts) ['volumes', 'volumeMounts'].forEach((key) => set.add(key));
  return Object.keys(spec.overrides ?? {}).filter((key) => set.has(key));
}
