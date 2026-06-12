import type { PageServerLoad } from "./$types";
import { client } from "$lib/server/client";

export const load: PageServerLoad = async () => {
  const { changes } = await client.listChanges();
  return { changes };
};
