import type { Plan } from '../plan.ts';
import type { App } from '../render.ts';
import { renderApplication } from './argocd.ts';

export interface Output {
  name: string;
  render(plan: Plan, app: App): Map<string, string>;
}

const argocd: Output = {
  name: 'argocd',
  render: (plan, app) => new Map([[`${plan.directory}/application.yaml`, renderApplication(plan, app.argocd?.overrides)]]),
};

export const OUTPUTS: Output[] = [argocd];

export const renderOutputs = (plan: Plan, app: App) => OUTPUTS.flatMap((output) => [...output.render(plan, app)]);
