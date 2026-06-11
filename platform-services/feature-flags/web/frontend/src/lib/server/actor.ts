import { env } from "$env/dynamic/private";

/**
 * The header the gateway sets to the authenticated user's username after OIDC. Envoy
 * does not inject this by default; a SecurityPolicy maps the `preferred_username`
 * claim to it (see manifests/securitypolicy.yaml). Configurable so the header name
 * can change without a code change.
 */
const ACTOR_HEADER = env.ACTOR_HEADER ?? "x-forwarded-user";

/** The acting user for audit logging, or undefined when the gateway sent no identity. */
export function actorFromRequest(request: Request): string | undefined {
  return request.headers.get(ACTOR_HEADER) ?? undefined;
}
