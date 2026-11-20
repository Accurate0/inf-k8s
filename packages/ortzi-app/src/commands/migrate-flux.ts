import { applying, type Kube } from '@ortzi/kit';
import { type Application, assertChangeable, getApp, turnAutosyncOff } from '../argo';
import { APP_ANNOTATION, FLUX_NAMESPACE } from '../flux-objects';

interface Ready {
  metadata: {
    name: string;
    generation?: number;
    resourceVersion?: string;
    annotations?: Record<string, string>;
  };
  status?: {
    observedGeneration?: number;
    conditions?: { type: string; status: string; message?: string }[];
  };
}

const ready = (object: Ready) =>
  object.status?.observedGeneration === object.metadata.generation &&
  object.status?.conditions?.some((c) => c.type === 'Ready' && c.status === 'True') === true;

const message = (object: Ready) =>
  object.status?.conditions?.find((c) => c.type === 'Ready')?.message ?? 'not reconciled yet';

function find<T>(get: () => T): T | undefined {
  try {
    return get();
  } catch (error) {
    if (String(error).includes('not found')) {
      return undefined;
    }
    throw error;
  }
}

function detach(kube: Kube, app: Application, yes: boolean): void {
  console.log(`${yes ? 'turning' : 'would turn'} argo auto-sync off for ${app.metadata.name}`);
  console.log(
    `${yes ? 'dropping' : 'would drop'} its finalizer so deleting it keeps every resource`,
  );
  if (yes) {
    turnAutosyncOff(kube, app);
    kube.namespace('argocd').patch('application', app.metadata.name, {
      metadata: { finalizers: null },
    });
  }
}

function waitForFlux(kube: Kube, name: string, minutes: number): boolean {
  const flux = kube.namespace(FLUX_NAMESPACE);
  const objects: [string, string][] = [
    ['kustomization', name],
    ...flux
      .list<Ready>('helmreleases')
      .filter((release) => release.metadata.annotations?.[APP_ANNOTATION] === name)
      .map((release): [string, string] => ['helmrelease', release.metadata.name]),
  ];

  let ok = true;
  for (const [resource, object] of objects) {
    const done = flux.waitFor<Ready>(resource, object, ready, minutes);
    const current = flux.get<Ready>(resource, object);
    console.log(`${done ? '✓' : '✗'} ${resource} ${object}${done ? '' : `: ${message(current)}`}`);
    ok &&= done;
  }
  return ok;
}

// Stage one runs before the app joins the flux list, stage two once flux has its kustomization.
export function migrateFlux(kube: Kube, name: string, yes: boolean, minutes: number): number {
  assertChangeable(name);
  const app = find(() => getApp(kube, name));
  const ks = find(() => kube.namespace(FLUX_NAMESPACE).get<Ready>('kustomization', name));

  applying(yes);
  if (!app && !ks) {
    throw new Error(`${name} is neither an argo application nor a flux kustomization`);
  }

  if (app && !ks) {
    detach(kube, app, yes);
    console.log(
      `next: add ${name} to flux in platform/apps.yaml and push; once publish-apps has run, rerun this`,
    );
    return 0;
  }

  if (app) {
    detach(kube, app, yes);
    console.log(`${yes ? 'deleting' : 'would delete'} argo application ${name}; flux owns it now`);
    if (yes) {
      kube.namespace('argocd').delete('application', name);
    }
  }

  if (!yes) {
    return 0;
  }
  return waitForFlux(kube, name, minutes) ? 0 : 1;
}
