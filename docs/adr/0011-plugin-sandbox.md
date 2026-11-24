# 0011. WASM plugin sandbox and its run result

Status: Accepted. Source: `utopia-plugin/src/lib.rs`, `tests/sandbox.rs` (PRD §16, §4.2 P2,
rule 13, MC-8; `docs/state-machines/plugin-run-lifecycle.md`).

## Decision

- A plugin is a directory with `plugin.json` and a `.wasm` or `.wat` module (which must stay
  inside that directory). It runs in an interpreter (wasmi) with only two imports, so it has
  no files, network or clock. A module that imports anything else does not instantiate.
- Limits per run: `FUEL_PER_RUN` instructions, `MEMORY_LIMIT` (16 MiB), `MAX_HOST_CALLS`
  (1,000) and 4 MiB per JSON message. A buggy or hostile plugin can only fail.
- What a plugin can do to the graph is exactly the automation API (`utopia_api::Op`),
  filtered by its manifest's `read`/`write` permissions through the same `Op::kind()` the
  MCP bridge and the HTTP API use, so a new write variant cannot slip past.
- ABI: the module exports `memory`, `utopia_alloc(len) -> ptr` and
  `utopia_run(ptr, len) -> i64` (packed `(ptr << 32) | len`, UTF-8 JSON). Imports in module
  `utopia`: `call(ptr, len) -> i64` (runs one API op, returns `{"ok"}` or `{"error"}`) and
  `log(ptr, len)`. `utopia_run` gets `{command, page, block, text}` and returns
  `{"insert"}`, `{"replace"}`, `{"message"}` or `{}`.
- A host write commits inside the call (rule 13). So `RunResult` carries the pages written
  and the lines logged on every path, success or failure (trap, fuel, oversized or non-JSON
  output, and a start function that calls the host).
- The host reaches the caller's `&mut Store` through a raw pointer for the length of
  `Plugin::run`; the `// SAFETY:` comment at the dereference states why that is sound.
