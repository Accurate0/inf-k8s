import { cli, Kube } from '@ortzi/kit';
import { logs, status, sync } from './commands';

const NAMESPACE = 'k8s-schemas';

const USAGE = `usage: ortzi schemas <command> [flags]

commands:
  sync     run the k8s-schemas job now and follow it
  status   show the schedule, recent runs and the published version
  logs     follow the latest run (or --job NAME)

flags:
  --context CTX        kube context (default: current)
  --detach             sync: start the job and return
  --force              sync: start even if a run is already active
  --timeout MINUTES    sync: give up following after this long (default 20)
  --k8s-version V      sync: override K8S_VERSION for this run, e.g. v1.36.0
  --job NAME           logs: the job to follow`;

export const run = cli({
  usage: USAGE,
  flags: {
    context: { type: 'string' },
    detach: { type: 'boolean' },
    force: { type: 'boolean' },
    timeout: { type: 'string' },
    'k8s-version': { type: 'string' },
    job: { type: 'string' },
  },
  setup: (values) => Kube.connect(values.context).namespace(NAMESPACE),
  commands: {
    sync: (kube, values) =>
      sync(kube, NAMESPACE, {
        detach: values.detach,
        force: values.force,
        timeoutMinutes: Number(values.timeout ?? '20'),
        k8sVersion: values['k8s-version'],
      }),
    status: (kube) => status(kube),
    logs: (kube, values) => logs(kube, values.job),
  },
});
