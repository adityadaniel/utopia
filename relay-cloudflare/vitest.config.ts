import { cloudflareTest } from "@cloudflare/vitest-pool-workers";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [
    // The real workerd runtime, with the Durable Object (SQLite classes), the R2 bucket
    // and the migrations from wrangler.jsonc — not a mock of them.
    cloudflareTest({
      wrangler: { configPath: "./wrangler.jsonc" },
      // Tests that need another token list or a tighter quota drive the Durable Object
      // directly; this is what the end-to-end requests see.
      miniflare: {
        bindings: {
          RELAY_TOKENS: "alpha,beta",
          QUOTA_BYTES: "1073741824",
        },
      },
    }),
  ],
  test: {
    include: ["test/**/*.test.ts"],
  },
});
