// Bindings typed for the tests: `cloudflare:test`'s `env` is `Cloudflare.Env`, which
// `wrangler types` would normally generate from wrangler.jsonc.
import type { Env as RelayEnv } from "../src/graph";

declare global {
  namespace Cloudflare {
    interface Env extends RelayEnv {}
  }
}

export {};
