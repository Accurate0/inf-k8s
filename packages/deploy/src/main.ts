import { cli, Kube, UsageError } from '@ortzi/kit';
import { history } from './commands/history';
import { autosync, heal } from './commands/policy';
import { rollback } from './commands/rollback';
import { status } from './commands/status';

const USAGE = `usage: ortzi deploy <command> [flags]

commands:
  status                   what needs attention (--all lists every app)
  status <app>             one app's state and images
  history <app>            previous argo syncs and their image tags
  rollback <app> [tag]     edit images on the cluster, turning auto-sync off first
                           (default tag: the most recent different one in history)
  heal <app> on|off        set self-heal; auto-sync still applies new git commits
  autosync <app> on|off    turn argo auto-sync off, or restore it (returns the app to git)

flags:
  --context CTX    kube context (default: current)
  --all            status: list every app
  --latest         status: also compare against ghcr's latest tag
  --image NAME     rollback: only this image (repeat by running again)
  --yes            apply; writes are a dry run without this`;

function requireApp(name: string | undefined, command: string): string {
  if (!name) throw new UsageError(`usage: ortzi deploy ${command} <app>`);
  return name;
}

function requireOnOff(
  command: string,
  name: string | undefined,
  state: string | undefined,
): { name: string; state: 'on' | 'off' } {
  if (!name || (state !== 'on' && state !== 'off')) {
    throw new UsageError(`usage: ortzi deploy ${command} <app> on|off`);
  }
  return { name, state };
}

export const run = cli({
  usage: USAGE,
  flags: {
    context: { type: 'string' },
    all: { type: 'boolean' },
    latest: { type: 'boolean' },
    image: { type: 'string' },
    yes: { type: 'boolean' },
  },
  setup: (values) => Kube.connect(values.context),
  commands: {
    status: (kube, values, [name]) => status(kube, name, values.all, values.latest),
    history: (kube, _, [name]) => history(kube, requireApp(name, 'history')),
    rollback: (kube, values, [name, tag]) =>
      rollback(kube, requireApp(name, 'rollback'), tag, values.image, values.yes),
    heal: (kube, values, [app, value]) => {
      const { name, state } = requireOnOff('heal', app, value);
      return heal(kube, name, state, values.yes);
    },
    autosync: (kube, values, [app, value]) => {
      const { name, state } = requireOnOff('autosync', app, value);
      return autosync(kube, name, state, values.yes);
    },
  },
});
