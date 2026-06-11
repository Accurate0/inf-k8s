import type { Actions } from "./$types";
import { client, type Variant } from "$lib/server/client";
import { fail, redirect } from "@sveltejs/kit";

export const actions: Actions = {
  default: async ({ request }) => {
    const data = await request.formData();
    const key = String(data.get("key")).trim();
    const valueType = Number(data.get("valueType"));
    const enabled = data.get("enabled") === "on";
    const defaultVariantKey = String(data.get("defaultVariantKey")).trim();
    const variantsRaw = String(data.get("variants"));

    let variants: Variant[];
    try {
      const parsed = JSON.parse(variantsRaw);
      if (!Array.isArray(parsed)) throw new Error("variants must be a JSON array");
      variants = parsed.map((v) => ({ key: String(v.key), value: v.value }));
    } catch (e) {
      return fail(400, { message: `invalid variants: ${(e as Error).message}`, values: { key, defaultVariantKey, variantsRaw } });
    }

    try {
      await client.createFlag({ key, valueType, enabled, defaultVariantKey, variants });
    } catch (e) {
      return fail(400, { message: (e as Error).message, values: { key, defaultVariantKey, variantsRaw } });
    }
    redirect(303, `/flags/${encodeURIComponent(key)}`);
  },
};
