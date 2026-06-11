import type { Actions, PageServerLoad } from "./$types";
import { client } from "$lib/server/client";
import { fail } from "@sveltejs/kit";

export const load: PageServerLoad = async () => {
  const { flags } = await client.listFlags();
  return { flags };
};

export const actions: Actions = {
  evaluate: async ({ request }) => {
    const data = await request.formData();
    const targetingKey = String(data.get("targetingKey"));
    const flagKey = String(data.get("flagKey") || "");
    const attributesRaw = String(data.get("attributes") || "{}");
    const values = { targetingKey, flagKey, attributesRaw };

    let attributes: Record<string, unknown>;
    try {
      attributes = JSON.parse(attributesRaw);
      if (typeof attributes !== "object" || attributes === null || Array.isArray(attributes)) {
        throw new Error("attributes must be a JSON object");
      }
    } catch (e) {
      return fail(400, { message: `invalid attributes: ${(e as Error).message}`, values });
    }

    const context = { targetingKey, attributes };

    try {
      if (flagKey) {
        const flag = await client.getFlag(flagKey);
        const evaluated = await client.resolve(flagKey, flag.valueType, targetingKey, attributes);
        return { flags: [evaluated], single: true, context, values };
      }

      const { flags } = await client.resolveAll(targetingKey, attributes);
      return { flags, single: false, context, values };
    } catch (e) {
      return fail(400, { message: (e as Error).message, values });
    }
  },
};
