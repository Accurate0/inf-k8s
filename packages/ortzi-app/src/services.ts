import type { App, Export, Workload } from './types.ts';
import { appNamespace } from './app.ts';
import { declaredPorts } from './workload.ts';
import { appWorkloads } from './providers/workload.ts';

function servicePorts(workload: string, spec: Workload) {
  const ports = declaredPorts(spec)
    .filter((p) => p.service)
    .map((p) => ({ name: p.name as string, port: p.service as number }));
  return { workload, service: spec.service?.name ?? workload, ports };
}

export const appServices = (app: App) =>
  Object.entries(appWorkloads(app)).map(([workload, spec]) => servicePorts(workload, spec));

export function exportPort(provider: App, exported: Export): number {
  const spec = appWorkloads(provider)[exported.workload];
  const ports = spec ? servicePorts(exported.workload, spec).ports : [];
  const found = ports.find((p) => p.name === exported.port || p.port === exported.port)?.port;
  if (!found) {
    throw new Error(
      `${provider.name}: exports.${exported.name}: no matching port on workload ${exported.workload}`,
    );
  }

  return found;
}

export const serviceUrl = (service: string, namespace: string, port: number, path = '') =>
  `http://${service}.${namespace}.svc.cluster.local${port === 80 ? '' : `:${port}`}${path}`;

export function ownServiceUrl(
  app: App,
  service: string,
  port: string | number,
  path?: string,
): string {
  const found = appServices(app).find((s) => s.service === service);
  if (!found) {
    throw new Error(`${app.name}: env: no service named ${service}`);
  }

  const exposed = found.ports.find((p) => p.name === port || p.port === port);
  if (!exposed) {
    throw new Error(`${app.name}: env: service ${service} has no port ${port}`);
  }

  return serviceUrl(service, appNamespace(app), exposed.port, path);
}
