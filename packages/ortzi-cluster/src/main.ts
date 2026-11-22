import { cli, Kube, parseMinutes } from '@ortzi/kit';
import { status } from './status';
import { sync } from './sync';

const USAGE = `usage: ortzi cluster <command> [flags]

commands:
  sync [SOURCE...]   make flux refetch every source's artifact (or just SOURCE, as name or
                     kind/name), then reconcile the root flux-system kustomization
  status             flux version and controllers, the fetched vs applied cluster revision,
                     and every flux object that isn't ready

flags:
  --context CTX        kube context (default: current)
  --detach             sync: request the refetch and return
  --sources-only       sync: don't reconcile the root kustomization afterwards
  --timeout MINUTES    sync: give up waiting after this long (default 10)
  --all                status: also count every flux kind`;

export const run = cli({
  usage: USAGE,
  flags: {
    context: { type: 'string' },
    detach: { type: 'boolean' },
    'sources-only': { type: 'boolean' },
    timeout: { type: 'string' },
    all: { type: 'boolean' },
  },
  setup: (values) => Kube.connect(values.context),
  commands: {
    sync: (kube, values, rest) =>
      sync(kube, {
        filters: rest,
        detach: values.detach,
        sourcesOnly: values['sources-only'],
        timeoutMinutes: parseMinutes(values.timeout, 10),
      }),
    status: (kube, values) => status(kube, { all: values.all }),
  },
});
