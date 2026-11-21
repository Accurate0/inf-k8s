import type { Plan } from '../plan.ts';
import type { App } from '../types.ts';
import { renderFlux } from './flux.ts';

export interface Output {
  name: string;
  render(plan: Plan, app: App): Map<string, string>;
}

const flux: Output = {
  name: 'flux',
  render: (plan, app) => renderFlux(plan, app.overrides?.flux),
};

export const OUTPUTS: Output[] = [flux];

export const renderOutputs = (plan: Plan, app: App) =>
  OUTPUTS.flatMap((output) => [...output.render(plan, app)]);
