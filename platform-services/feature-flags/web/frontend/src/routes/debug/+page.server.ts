import type { Actions } from "./$types";
import { client } from "$lib/server/client";
import { fail } from "@sveltejs/kit";

export const actions: Actions = {
  evaluate: async ({ request }) => {
    const data = await request.formData();
    const targetingKey = String(data.get("targetingKey"));
    const attributesRaw = String(data.get("attributes") || "{}");

    let attributes: Record<string, unknown>;
    try {
      attributes = JSON.parse(attributesRaw);
      if (typeof attributes !== "object" || attributes === null || Array.isArray(attributes)) {
        throw new Error("attributes must be a JSON object");
      }
    } catch (e) {
      return fail(400, { message: `invalid attributes: ${(e as Error).message}`, values: { targetingKey, attributesRaw } });
    }

    try {
      const { flags } = await client.resolveAll(targetingKey, attributes);
      return { flags, values: { targetingKey, attributesRaw } };
    } catch (e) {
      return fail(400, { message: (e as Error).message, values: { targetingKey, attributesRaw } });
    }
  },
};
