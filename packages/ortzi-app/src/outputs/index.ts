import type { Plan } from '../plan.ts';
import type { App } from '../types.ts';
import { renderApplication } from './argocd.ts';
import { renderFlux } from './flux.ts';

export interface Output {
  name: string;
  render(plan: Plan, app: App): Map<string, string>;
}

const argocd: Output = {
  name: 'argocd',
  render: (plan, app) =>
    new Map([
      [
        `${plan.directory}/application.yaml`,
        renderApplication(plan, app.overrides?.argocd?.application),
      ],
    ]),
};

const flux: Output = {
  name: 'flux',
  render: (plan, app) => (plan.artifact ? renderFlux(plan, app.overrides?.flux) : new Map()),
};

export const OUTPUTS: Output[] = [argocd, flux];

export const renderOutputs = (plan: Plan, app: App) =>
  OUTPUTS.flatMap((output) => [...output.render(plan, app)]);
