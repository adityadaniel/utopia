# Keymap overrides: editor copy, live gpui bindings, keymap.json

## Status

The machine is explicit. Per action the override is
`Override::{Valid(String), Invalid { raw, why }, SameAsDefault}` (`actions.rs:928`), computed
once when the file is read or an override is set (`Override::parse :943`). `Overrides`
(`actions.rs:984`) is a private map of those, so no reader can form a fourth opinion:
`install` (`:1189`) binds only `Valid`; `keys_for` (`:1045`) answers with the default for
`Invalid` and for an absent entry; `to_json` writes `Valid` and keeps the raw text of
`Invalid` but never writes `SameAsDefault`; the editor, the cheat sheet, the clash line and
the filter go through `keys_for`/`entry`; Quick Jump reads the live keymap (`active_keys`).
All five listed combinations are now dealt with:

1. **Fixed** — `from_json` (`:989`) runs the same check `binding` does (`keys_problem :803`)
   and carries an entry gpui cannot bind as `Invalid`, warning once (startup and Settings both
   show it). It is never in force, so the editor, the Shortcuts sheet and Quick Jump all
   report the default, and the row shows the text the user wrote in amber with
   "Not a shortcut: …; ⇧⌘T is in force." (`settings.rs:1179`). Tests:
   `unusable_keys_are_invalid_and_stay_out_of_force` (`actions.rs:1352`),
   `an_unusable_entry_is_marked_and_the_default_is_in_force` (`tests/settings.rs:814`).
2. **Fixed** — a failed write is its own state: `SettingsView.save_error` (`settings.rs:146`),
   set and cleared by `apply` (`:357`), drawn as the `settings-unsaved` footer (`:739`) and
   read by `unsaved()` (`:381`). It used to be written into `status`, which every caller
   overwrote one line later, so it was never actually shown; the chip still reports the
   rebind, and the two lines sit side by side. Test:
   `a_failed_save_is_reported_after_the_rebind` (`tests/settings.rs:849`).
3. **Fixed, with a refinement of the sketch** — `conflicts` (`actions.rs:1098`) compares
   scopes instead of context strings. The sketch says "scope = the set of contexts the binding
   is live in"; taken literally that reports `Indent` (`BlockEditor`, Tab) against
   `AcceptCompletion` (`BlockEditor && autocomplete`, Tab), which is the deliberate layering
   the catalog documents, and would put a permanent clash line on two default rows
   (`defaults_do_not_conflict`). So the scope is each predicate's disjunctive normal form over
   identifiers (`clauses :1136`): two bindings clash when they name the same place, a clause
   with extra identifiers is a narrower layer of that place, and `None` is the whole window,
   live everywhere (`scopes_touch :1122`). A predicate this code does not model (`!`, `==`,
   `>`) falls back to string equality. Tests:
   `a_clash_across_two_overlapping_contexts_is_reported` (`actions.rs:1406`),
   `a_narrower_context_is_a_layer_not_a_clash` (`actions.rs:1423`),
   `a_clash_across_contexts_is_reported` (`tests/settings.rs:892`).
4. **Fixed by construction** — `Override::parse` returns `SameAsDefault` when the canonical
   keys equal the platform default, and both `from_json` (`:989`) and `set` (`:1074`) drop
   that state, so the map never holds an entry that binds what the app already binds: no
   changed marker, no identical binding, nothing written back. `rebind` (`settings.rs:295`)
   now only calls `set`, which makes "rebind to the chord it already has" and "reset it" the
   same state. Test: `an_entry_equal_to_the_default_is_not_an_override` (`actions.rs:1383`).
5. **Not fixed** — there is still no file watcher: both copies are read at open, so an
   external edit while Settings is open waits for the next launch. A watcher is a side task
   with its own lifecycle (the app-root machine owns those), and re-reading the file whenever
   the window is raised (`open_settings_now` raises the existing window rather than making a
   new one) would silently discard the unsaved state issue 2 now reports. Left as it was.

Verified by eye with `UTOPIA_SHOT_SCRIPT` against a scratch graph and config dir, shots in
`docs/verification/keymap-overrides-three-copies/`.

## What it controls

Which keys fire which action, what the Keybindings editor and the Shortcuts sheet show, what
Quick Jump prints next to a command, and what survives a restart. AGENTS.md says
`utopia_ui::actions::CATALOG` is the one source of truth; the *overrides* on top of it live
in three places. `crates/utopia-ui/src/actions.rs`, `settings.rs`, `quick_jump.rs`.

## Where the state lives

| Copy | Location | Written by |
|---|---|---|
| Editor's view | `settings.rs:134` `SettingsView.overrides` (`keys_for :1022`, `is_changed :1023`) | load `:167`, rebind, reset |
| Live bindings | gpui `cx.key_bindings()` via `install`/`reinstall` (`actions.rs:1037-1059`); `active_keys :1009-1026` is what Quick Jump shows (`quick_jump.rs:307, 1151`) | `apply :346` |
| Disk | `keymap.json` via `Overrides::save` (`actions.rs:958-961`) and `Overrides::load` at startup (`main.rs:670, 678` → `actions.rs:1063-1075`) | `apply :348` |
| Clash rule | `actions.rs:992` `a.context == b.context` (string equality) | – |

## States

Per action: **Default** / **Overridden (valid)** / **Overridden (invalid keys)** /
**Overridden equal to default** — and for each, whether disk, live and editor agree.

## Events and transitions

- `rebind` (`settings.rs:283-316`): set or (if equal to default, `:285`) remove the override;
  `apply` → `reinstall` then `save`; clash computed afterwards (`:292-304`).
- `reset_one :319`, `reset_all :338` → same `apply`.
- Startup: `install_keymap` reads the file; `SettingsView::new` reads it again (`:167`).

## Impossible or unhandled combinations (evidence)

1. **Invalid keys in the file: editor shows the override, gpui uses the default.**
   `Overrides::from_json` keeps any entry whose *action* is known without validating the keys
   (`actions.rs:924-925`); `install` drops it and keeps the default (`:1037-1042`, `:1047`,
   `eprintln!("ignoring keybinding …")`). So the editor shows e.g. `nonsense-key` with the
   changed marker, the Shortcuts tab agrees, Quick Jump shows `⇧⌘T`, and `⇧⌘T` fires. Test
   `an_unusable_override_keeps_the_default` (`tests/settings.rs:781-806`) asserts only the
   gpui side.
2. **Save fails: live and editor are new, disk is old.** `apply :346-351` reinstalls first,
   then saves; on `Err` the status says so (`:349`) but the chip shows no "unsaved" state and
   the next launch silently reverts. Intended per the comment at `:344-345`.
3. **Clash detection compares contexts by string.** `Some("BlockEditor")`,
   `Some("BlockEditor || LineInput")` and `None` are treated as unrelated (`:992`), yet all
   three apply inside the editor. Rebinding `Bold` (EDITOR, `:319-326`) onto `cmd-c` (Copy is
   TEXT, `:360-367`) produces no "Also used by" line; gpui resolves one of them. The doc
   comment (`:986-987`) describes only "editor vs whole window".
4. **An override equal to the default** (hand-edited file) shows the changed marker
   (`is_changed :971-973`) and installs an identical binding; it is only dropped when the
   chord is re-entered (`settings.rs:285-287`). Cosmetic.
5. **No file watcher**: both copies are read at open; an external edit while Settings is
   open is not reflected (uncertain whether that matters).

## Diagram

```mermaid
stateDiagram-v2
    [*] --> Default
    Default --> OverriddenValid : rebind (reinstall + save)
    OverriddenValid --> Default : reset | rebind to default chord
    OverriddenValid --> LiveOnly : save fails (status only; reverts at restart)
    [*] --> OverriddenInvalid : keymap.json has unparsable keys (editor: shown, gpui: default)
    OverriddenInvalid --> OverriddenValid : user re-enters a chord
    Default --> OverriddenEqualsDefault : hand-edited file (marker shown, no effect)
```

## Would an explicit state type help?

Yes: validate on load and carry the verdict, so every reader asks the same value.

```rust
enum Override { Valid(String), Invalid { raw: String, why: String }, SameAsDefault }
struct Overrides(BTreeMap<&'static str, Override>);
// install(): only Valid installs; the editor renders Invalid with a warning chip
// clash(): compare by *scope*, where scope = the set of contexts the binding is live in
```
