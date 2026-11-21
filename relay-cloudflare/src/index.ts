//! Worker entry point: the default `fetch` handler and the graph Durable Object class.
//!
//! Only these two are exported from the entry module. `wrangler dev` (and any deploy)
//! turns every other value export into a binding, so the routing, auth and validation
//! code lives in `relay.ts` — which is where the tests import it from.

import { GraphObject, type Env } from "./graph";
import { handle } from "./relay";

export { GraphObject };

export default {
  async fetch(
    request: Request,
    env: Env,
    _ctx: ExecutionContext,
  ): Promise<Response> {
    const response = await handle(request, env);
    // `get` handlers answer HEAD as well, as axum's router does.
    if (request.method !== "HEAD") return response;
    return new Response(null, { status: response.status, headers: response.headers });
  },
} satisfies ExportedHandler<Env>;
