# Plugin run lifecycle — verification shots

Captured with `UTOPIA_SHOT_SCRIPT` against a scratch graph (`/tmp/shot-verify/g.utopia`) and a
throwaway plugin in `UTOPIA_CONFIG_DIR/plugins/trapper/`, a 20-line `.wat` module that calls
`{"op":"capture","text":"captured by a failed plugin"}` on the host and then executes
`unreachable`, so the write commits and the run fails.

Script (one line):

```
wait 1500; type Met [[Ana]]; keys enter; wait 300; type /; wait 400; type trapper;
wait 400; shot menu.png; keys enter; wait 1200; shot after.png
```

- `plugin-in-slash-menu.png` — the installed plugin's command (`/trapper: Go`) as offered in
  the slash palette.
- `failed-run-after.png` — after choosing it: the trap is reported in the status line
  (`trapper: run: wasm 'unreachable' instruction executed`) and the block it captured is on
  the page, i.e. the failed run's committed write reached the views.

The same script run against `run_plugin` reverted to announcing `changed` only on the success
arm produced a byte-identical `after.png`: accepting the slash command edits the block
afterwards, and that commit announces the page anyway, so this flow cannot distinguish the two
builds and the before shot is not kept as if it did. The distinguishing evidence is the test
`p2_a_failed_plugin_run_announces_the_pages_it_wrote` (`crates/utopia-ui/tests/workspace.rs`),
which asserts on the `Changed` event itself: on `main` the capture lands and the trap is
reported, but no `Changed` event reaches the views.
