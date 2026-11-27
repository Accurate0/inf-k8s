import type { App } from './types.ts';
import type { Block, ResourceType } from './providers/provider.ts';
import { HEADER_TEXT } from './generated.ts';
import { declared } from './app.ts';
import { RESOURCES } from './providers/index.ts';
import { ciTerraform } from './ci.ts';

export const TERRAFORM_APPS = 'terraform/apps';

const merge = (blocks: Block[]): Block => {
  const out: Block = {};
  for (const block of blocks) {
    for (const [type, resources] of Object.entries(block)) {
      out[type] = { ...out[type], ...resources };
    }
  }
  return out;
};

const json = (value: Record<string, unknown>) =>
  `${JSON.stringify({ '//': HEADER_TEXT, ...value }, null, 2)}\n`;

export const appTerraformPath = (app: App) => `${TERRAFORM_APPS}/${app.name}.tf.json`;

export function renderAppTerraform(app: App): string | undefined {
  const blocks = [
    ...RESOURCES.flatMap(
      (provider) => provider.terraform?.(app, declared(app, provider.type as ResourceType)) ?? [],
    ),
    ...ciTerraform(app),
  ];
  return blocks.length ? json({ resource: merge(blocks) }) : undefined;
}

export function renderTerraformIndex(apps: App[]): string {
  return json({
    output: Object.assign({}, ...RESOURCES.map((provider) => provider.terraformOutputs?.(apps))),
  });
}
