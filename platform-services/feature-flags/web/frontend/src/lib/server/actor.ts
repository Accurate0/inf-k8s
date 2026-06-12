import { env } from "$env/dynamic/private";

/**
 * Kanidm's OIDC userinfo endpoint. The gateway forwards the access token
 * (SecurityPolicy `forwardAccessToken`), but Kanidm's access token carries no
 * profile claims, so the username is resolved from userinfo on demand.
 */
const USERINFO_ENDPOINT =
  env.OIDC_USERINFO_ENDPOINT ??
  "https://idm.anurag.sh/oauth2/openid/feature-flags/userinfo";

type CacheEntry = { actor: string; expiresAt: number };
const cache = new Map<string, CacheEntry>();

function bearer(request: Request): string | undefined {
  const header = request.headers.get("authorization");
  const token = header?.replace(/^Bearer\s+/i, "");
  return token && token !== header ? token : undefined;
}

/** `{ jti, exp }` from a JWT payload, without verifying — the gateway already validated it. */
function tokenMeta(token: string): { jti: string; exp: number } | undefined {
  const payload = token.split(".")[1];
  if (!payload) return undefined;
  try {
    const claims = JSON.parse(Buffer.from(payload, "base64url").toString("utf8"));
    if (typeof claims.jti !== "string") return undefined;
    return { jti: claims.jti, exp: typeof claims.exp === "number" ? claims.exp : 0 };
  } catch {
    return undefined;
  }
}

/**
 * The acting user for audit logging, resolved from the OIDC userinfo endpoint using the
 * access token the gateway forwarded. Returns undefined when no identity is available, so
 * the backend falls back to recording `unknown` rather than failing the mutation.
 */
export async function actorFromRequest(request: Request): Promise<string | undefined> {
  const token = bearer(request);
  if (!token) return undefined;

  const meta = tokenMeta(token);
  const now = Date.now();
  if (meta) {
    const hit = cache.get(meta.jti);
    if (hit && hit.expiresAt > now) return hit.actor;
  }

  let actor: string | undefined;
  try {
    const response = await fetch(USERINFO_ENDPOINT, {
      headers: { authorization: `Bearer ${token}` },
    });
    if (response.ok) {
      const claims = await response.json();
      actor = claims.preferred_username ?? claims.email ?? claims.sub ?? undefined;
    }
  } catch {
    return undefined;
  }

  if (actor && meta) {
    const ttl = meta.exp ? meta.exp * 1000 : now + 60_000;
    cache.set(meta.jti, { actor, expiresAt: Math.min(ttl, now + 300_000) });
  }
  return actor;
}
