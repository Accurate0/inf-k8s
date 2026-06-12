import type { Actions, PageServerLoad } from "./$types";
import { client, type Constraint } from "$lib/server/client";
import { actorFromRequest } from "$lib/server/actor";
import { operatorLabels } from "$lib/labels";
import { fail } from "@sveltejs/kit";

const operatorByLabel = new Map(
  Object.entries(operatorLabels).map(([value, label]) => [label, Number(value)]),
);

export const load: PageServerLoad = async () => {
  const { segments } = await client.listSegments();
  return { segments };
};

export const actions: Actions = {
  upsert: async ({ request }) => {
    const data = await request.formData();
    const key = String(data.get("key")).trim();
    const name = String(data.get("name")).trim();
    const constraintsRaw = String(data.get("constraints"));

    let constraints: Constraint[];
    try {
      const parsed = JSON.parse(constraintsRaw);
      if (!Array.isArray(parsed)) throw new Error("constraints must be a JSON array");
      constraints = parsed.map((c) => {
        const operator = operatorByLabel.get(String(c.operator));
        if (operator === undefined) throw new Error(`unknown operator \`${c.operator}\``);
        return {
          attribute: String(c.attribute),
          operator,
          values: Array.isArray(c.values) ? c.values : [c.values],
        };
      });
    } catch (e) {
      return fail(400, { message: `invalid constraints: ${(e as Error).message}`, values: { key, name, constraintsRaw } });
    }

    try {
      await client.upsertSegment({ key, name, constraints }, await actorFromRequest(request));
    } catch (e) {
      return fail(400, { message: (e as Error).message, values: { key, name, constraintsRaw } });
    }
    return { success: true };
  },

  delete: async ({ request }) => {
    const data = await request.formData();
    try {
      await client.deleteSegment(String(data.get("key")), await actorFromRequest(request));
    } catch (e) {
      return fail(400, { message: (e as Error).message });
    }
    return { success: true };
  },
};
