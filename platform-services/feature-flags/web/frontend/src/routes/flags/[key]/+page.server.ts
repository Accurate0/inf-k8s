import type { Actions, PageServerLoad } from "./$types";
import { client, type Rule, type Constraint } from "$lib/server/client";
import { actorFromRequest } from "$lib/server/actor";
import { error, fail, redirect } from "@sveltejs/kit";

export const load: PageServerLoad = async ({ params }) => {
  try {
    const flag = await client.getFlag(params.key);
    const { segments } = await client.listSegments();
    return { flag, segments };
  } catch (e) {
    error(404, (e as Error).message);
  }
};

export const actions: Actions = {
  update: async ({ request, params }) => {
    const data = await request.formData();
    const enabled = data.get("enabled") === "on";
    const defaultVariantKey = String(data.get("defaultVariantKey"));
    try {
      await client.updateFlag(params.key, enabled, defaultVariantKey, actorFromRequest(request));
    } catch (e) {
      return fail(400, { message: (e as Error).message });
    }
    return { success: true };
  },

  archive: async ({ request, params }) => {
    const data = await request.formData();
    const archived = data.get("archived") === "true";
    try {
      await client.archiveFlag(params.key, archived, actorFromRequest(request));
    } catch (e) {
      return fail(400, { message: (e as Error).message });
    }
    return { success: true };
  },

  delete: async ({ request, params }) => {
    try {
      await client.deleteFlag(params.key, actorFromRequest(request));
    } catch (e) {
      return fail(400, { message: (e as Error).message });
    }
    redirect(303, "/");
  },

  upsertVariant: async ({ request, params }) => {
    const data = await request.formData();
    const key = String(data.get("variantKey")).trim();
    const valueRaw = String(data.get("value"));
    let value: unknown;
    try {
      value = JSON.parse(valueRaw);
    } catch (e) {
      return fail(400, { message: `invalid variant value JSON: ${(e as Error).message}` });
    }
    try {
      await client.upsertVariant(params.key, { key, value }, actorFromRequest(request));
    } catch (e) {
      return fail(400, { message: (e as Error).message });
    }
    return { success: true };
  },

  deleteVariant: async ({ request, params }) => {
    const data = await request.formData();
    try {
      await client.deleteVariant(params.key, String(data.get("variantKey")), actorFromRequest(request));
    } catch (e) {
      return fail(400, { message: (e as Error).message });
    }
    return { success: true };
  },

  setRules: async ({ request, params }) => {
    const data = await request.formData();
    const rulesRaw = String(data.get("rules"));
    let rules: Rule[];
    try {
      const parsed = JSON.parse(rulesRaw);
      if (!Array.isArray(parsed)) throw new Error("rules must be a JSON array");
      rules = parsed.map((r, i) => ({
        rank: i,
        segmentKey: String(r.segmentKey ?? ""),
        variantKey: String(r.variantKey ?? ""),
        bucketSalt: String(r.bucketSalt ?? ""),
        distributions: Array.isArray(r.distributions)
          ? r.distributions.map((d: { variantKey: unknown; weight: unknown }) => ({
              variantKey: String(d.variantKey),
              weight: Number(d.weight),
            }))
          : [],
        constraintGroups: Array.isArray(r.constraintGroups)
          ? r.constraintGroups.map((g: { constraints: Constraint[] }) => ({
              constraints: Array.isArray(g.constraints)
                ? g.constraints.map((c: Constraint) => ({
                    attribute: String(c.attribute),
                    operator: Number(c.operator),
                    values: Array.isArray(c.values) ? c.values : [],
                  }))
                : [],
            }))
          : [],
      }));
    } catch (e) {
      return fail(400, { message: `invalid rules: ${(e as Error).message}` });
    }
    try {
      await client.setFlagRules(params.key, rules, actorFromRequest(request));
    } catch (e) {
      return fail(400, { message: (e as Error).message });
    }
    return { success: true };
  },
};
