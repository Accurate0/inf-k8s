import type { Actions, PageServerLoad } from "./$types";
import { client } from "$lib/server/client";
import { actorFromRequest } from "$lib/server/actor";
import { fail } from "@sveltejs/kit";

export const load: PageServerLoad = async ({ url }) => {
  const includeArchived = url.searchParams.get("archived") === "1";
  const { flags } = await client.listFlags(includeArchived);
  return { flags, includeArchived };
};

export const actions: Actions = {
  toggle: async ({ request }) => {
    const data = await request.formData();
    const key = String(data.get("key"));
    const defaultVariantKey = String(data.get("defaultVariantKey"));
    const enabled = data.get("enabled") === "true";
    try {
      await client.updateFlag(key, enabled, defaultVariantKey, actorFromRequest(request));
    } catch (e) {
      return fail(400, { message: (e as Error).message });
    }
    return { success: true };
  },
};
