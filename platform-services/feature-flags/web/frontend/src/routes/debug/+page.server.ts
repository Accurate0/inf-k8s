import type { Actions, PageServerLoad } from "./$types";
import { client } from "$lib/server/client";
import { fail } from "@sveltejs/kit";

/** Decode a JWT payload's claim names without verifying — diagnostics only. */
function jwtClaimKeys(authorization: string | null): string[] | null {
  const token = authorization?.replace(/^Bearer\s+/i, "");
  const payload = token?.split(".")[1];
  if (!payload) return null;
  try {
    const json = Buffer.from(payload, "base64url").toString("utf8");
    return Object.keys(JSON.parse(json)).sort();
  } catch {
    return null;
  }
}

export const load: PageServerLoad = async ({ request }) => {
  const { flags } = await client.listFlags();
  const auth = {
    forwardedUser: request.headers.get("x-forwarded-user"),
    hasAuthorization: request.headers.has("authorization"),
    accessTokenClaims: jwtClaimKeys(request.headers.get("authorization")),
  };
  return { flags, auth };
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
