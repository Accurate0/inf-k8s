type Values = Record<string, any>;

export type ResolveResource = (resource: string, env: string) => { name: string; value: string };

const PROBES = { startup: 'startupProbe', readiness: 'readinessProbe', liveness: 'livenessProbe' } as const;
const TIMINGS = { delay: 'initialDelaySeconds', every: 'periodSeconds', timeout: 'timeoutSeconds', failures: 'failureThreshold' } as const;

function expandEnv(env: Values, resolve: ResolveResource) {
  return Object.entries(env).map(([name, value]) => {
    if (typeof value !== 'object' || value === null) return { name, value: String(value) };
    if (value.resource) return resolve(value.resource, name);
    if (value.secret) {
      const optional = value.optional === undefined ? {} : { optional: value.optional };
      return { name, valueFrom: { secretKeyRef: { name: value.secret, key: value.key, ...optional } } };
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

export function expandContainer(spec: Values, resolve: ResolveResource): Values {
  const { probes, mounts, ...out } = spec;
  if (out.env && !Array.isArray(out.env)) out.env = expandEnv(out.env, resolve);
  if (out.envFrom) {
    out.envFrom = out.envFrom.map((from: unknown) => (typeof from === 'string' ? { secretRef: { name: from } } : from));
  }
  if (out.resources) out.resources = expandResources(out.resources);
  if (out.ports) {
    out.ports = out.ports.map((port: unknown) => (typeof port === 'number' ? { containerPort: port } : port));
  }
  return { ...out, ...(probes ? expandProbes(probes) : {}), ...(mounts ? expandMounts(mounts) : {}) };
}

export function expandWorkload(spec: Values, resolve: ResolveResource): Values {
  const { overrides, ...rest } = spec;
  const out = expandContainer(rest, resolve);
  for (const key of ['initContainers', 'sidecars']) {
    if (out[key]) out[key] = out[key].map((container: Values) => expandContainer(container, resolve));
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
