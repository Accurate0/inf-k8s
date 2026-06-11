import type { RequestHandler } from "./$types";
import { client } from "$lib/server/client";

export const GET: RequestHandler = ({ request }) => {
  const controller = new AbortController();
  request.signal.addEventListener("abort", () => controller.abort());

  const stream = new ReadableStream({
    async start(ctrl) {
      const encoder = new TextEncoder();
      try {
        for await (const snapshot of client.streamSnapshot(controller.signal)) {
          const payload = JSON.stringify({
            version: snapshot.version,
            flags: snapshot.flags,
            segments: snapshot.segments,
          });
          ctrl.enqueue(encoder.encode(`data: ${payload}\n\n`));
        }
      } catch {
        // stream cancelled or backend dropped; close the SSE response.
      } finally {
        ctrl.close();
      }
    },
    cancel() {
      controller.abort();
    },
  });

  return new Response(stream, {
    headers: {
      "content-type": "text/event-stream",
      "cache-control": "no-cache",
      connection: "keep-alive",
    },
  });
};
