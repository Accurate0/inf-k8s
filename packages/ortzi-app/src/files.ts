import type { App, Target, World } from './types.ts';
import type { GeneratedFile, ResourceType } from './providers/provider.ts';
import { declared } from './app.ts';
import { RESOURCES } from './providers/index.ts';
import { kustomizationPath, renderKustomization } from './manifests.ts';
import {
  appTerraformPath,
  renderAppTerraform,
  renderTerraformIndex,
  TERRAFORM_APPS,
} from './terraform.ts';

export function generatedFiles(
  world: World,
  selected: { app: App; target: Target }[],
  read: (path: string) => string,
): GeneratedFile[] {
  const perApp = selected.flatMap(({ app, target }) => {
    const kustomization = renderKustomization(app, target);
    return [
      ...RESOURCES.flatMap(
        (provider) =>
          provider.generate?.(app, target, declared(app, provider.type as ResourceType)) ?? [],
      ),
      ...(kustomization ? [{ path: kustomizationPath(app, target), content: kustomization }] : []),
    ];
  });

  const terraform = world.apps.flatMap((app) => {
    const content = renderAppTerraform(app);
    return content ? [{ path: appTerraformPath(app), content }] : [];
  });

  return [
    ...perApp,
    ...RESOURCES.flatMap((provider) => provider.generateWorld?.(world, read) ?? []),
    { path: `${TERRAFORM_APPS}/_index.tf.json`, content: renderTerraformIndex(world.apps) },
    ...terraform,
  ];
}
