# App root: startup state, side tasks, Settings window handle, quit paths

## Status

The machine is explicit: `RootState::{Onboarding, Locked, Ready(Ready), Failed(String)}`
(`main.rs:170`), with the side tasks inside `Ready` (`main.rs:195`), the run decided once by
`RunMode` (`main.rs:226`), the open graph held at app level by `Running` (`main.rs:210`), and
the Settings window as `SettingsWindow::{Closed, Open(handle)}` (`main.rs:640`). All seven
listed combinations are dealt with:

1. **Fixed** — a `UTOPIA_SHOT_SCRIPT` that does not parse is returned from `RunMode::from_env`
   (`main.rs:236`) before a window exists, and `run_app` exits with the parse error
   (`main.rs:786`). A scripted run can no longer look like a normal launch that nothing drives
   or quits. Tests: `a_malformed_shot_script_is_an_error`,
   `one_mode_is_parsed_for_the_whole_run` (`main.rs` tests).
2. **Fixed** — one quit path: `quit_app` (`main.rs:762`) finishes the edited block
   (`Root::end_edit :313` → `Workspace::finish_edit`, `workspace.rs:1527`) and then calls
   `AppState::flush_for_quit` (`app_state.rs:97`), which reports a writer failure instead of
   swallowing it. Every route reaches it: Cmd+Q and the Quit menu item from any window
   (`main.rs:801`; the per-workspace `Quit` handler is gone, so the action bubbles to the app),
   and the last window closing (`main.rs:838`). The app-level `on_app_quit` backstop
   (`main.rs:805`) covers a quit the app is never asked about (a logout, the system terminating
   it); `Running` keeps the store alive after its window has closed, so that flush is still
   possible. Tests: `ac_1_1_the_last_keystroke_survives_a_quit` (`main.rs`),
   `a_failed_quit_flush_is_reported` (`app_state.rs`).
3. **Fixed by construction** — `_hotkey`/`_sync`/`_assistants`/`_inbox` live in `Ready`
   (`main.rs:195`), and the inbox task is not an `Option` (it exists whenever a graph is open).
   Dropping `Ready` drops the tasks, so "Ready but discovery still running" is a field, and no
   side task can outlive its graph.
4. **Fixed by construction** — there is no placeholder state: `Root::new` (`main.rs:266`)
   computes the first state from `Root::onboarding` (`main.rs:319`) or `Root::load_graph`
   (`main.rs:412`) before the `Root` value exists. `Failed` only ever holds a real open error.
5. **Fixed** — `SettingsWindow::forget_if_closed` (`main.rs:649`) runs from the app's
   window-closed observer (`main.rs:838`), so a closed Settings window leaves `Closed`, not a
   stale handle. Test: `closing_the_settings_window_forgets_its_handle` (`main.rs`).
6. **Fixed** — `SettingsView::new` observes its own window appearance and sets the theme
   global, like the main window does (`settings.rs:258`), so Settings keeps following the
   system appearance after the main window closes. Not covered by a test: GPUI's test platform
   always reports `Light` and cannot simulate an appearance change (`platform/test/window.rs`),
   so this one is only verified by reading the code.
7. **Fixed** — one writer for the graph (MC-4). `load_graph` takes `OwnerLock` before
   `Store::open_with_key` (`main.rs:413-431`), `Running._owner` (`main.rs:217`) keeps it past
   the window closing, and `RootState::Locked.owner` (`main.rs:182`) carries the same lock
   across a passphrase attempt instead of taking a second one in this process. A graph
   another process owns is `Failed(cli::ALREADY_OPEN)` with no store and no lock; the window
   stays open with the message, as it does for every other failure to open a graph. See
   [`owner-lock.md`](owner-lock.md) and `docs/verification/feat-mcp-owner-lock/2-already-open.png`.

Two places where this differs from the sketch below, deliberately:

- `Locked { attempts: u32 }` was **not** added. A wrong passphrase already re-shows the error
  and there is no lockout to model, so a counter would be dead state; the bad states the
  document lists do not include one.
- `Mode` is spelled `RunMode`: `Mode` already means something else in `page_view` and the
  palettes, and the run mode is read once for the whole process rather than switched.
- Issue 1's fix is a hard error, not a fallback to `Interactive`. The only thing that sets a
  shot script is an unattended run; silently ignoring it would leave that run hanging at a
  window with no step to quit it.

## What it controls

The desktop binary's lifecycle (`crates/utopia-app/src/main.rs`): choosing a graph, the
onboarding/unlock/ready/failed screens, the side tasks that only exist in the Ready state,
the single Settings window, and how the app quits.

## Where the state lives

| Piece | Location | Notes |
|---|---|---|
| Root state | `main.rs:170` `RootState::{Onboarding, Locked, Ready(Ready), Failed(String)}` | the first state is computed before `Root` exists (`:266`); `Failed` only from a failed `Store::open` or a graph another process owns |
| Side tasks | `main.rs:195` `Ready { ws, _hotkey, _sync, _assistants, _inbox }` | built by `load_graph` (`:412`) only on the Ready path; dropped with `Ready` |
| Run mode | `main.rs:226` `RunMode::{Interactive, Scripted(Vec<Step>), Probe}` | read once in `run_app` (`:786`), carried by `Root`; a parse failure is an `Err` |
| Open graph | `main.rs:210` `Running { root: Option<WeakEntity<Root>>, app: Entity<AppState>, _owner: Option<OwnerLock> }` | strong `app` keeps the store alive after the window; `root` is `None` once it has gone; the lock lives as long as the store and drops after it |
| Owner lock | `main.rs:182` `Locked { owner: Option<OwnerLock> }`, `main.rs:413-431` in `load_graph`, `utopia_store::OwnerLock` | taken before the store opens, trying for 2 s; `None` in `Locked` only while an attempt has it. See [`owner-lock.md`](owner-lock.md) |
| Probe | `RunMode::Probe` (`main.rs:226`), `run_probe` called from `load_graph :462` | – |
| Hotkey latch | `hotkey.rs:10` `FIRED: AtomicBool`, polled every 150 ms (`main.rs:476`) | presses within one poll coalesce |
| Settings window | `main.rs:640` `SettingsWindow::{Closed, Open(WindowHandle<SettingsView>)}`; `open_settings` deferred (`:669`) | closed by the window-closed observer (`:838`) |
| Theme | global set for the main window (`main.rs:274`, `:830`) and by `SettingsView::new` (`settings.rs:259`) | every window follows its own appearance |
| Quit | `main.rs:762` `quit_app` from the app-level `Quit` handler (`:801`) and the last-window close (`:838`); `AppState::flush_for_quit` (`app_state.rs:97`); `on_app_quit` backstop (`main.rs:805`) | one flush, reported, never swallowed |

## States

**Onboarding** / **Locked** (wrong passphrase → `Locked` again with the error; the owner lock
is held the whole time) / **Ready** (+ the side tasks it owns, and the owner lock in `Running`)
/ **Failed** (terminal, no retry UI; `Failed(ALREADY_OPEN)` means another process owns the
graph, and no store was opened). There is no placeholder state; `Root::new` starts in whichever
of these the graph and run mode decide.

## Events and transitions

- `Root::new` (`main.rs:266`) → `Root::onboarding` (`:319`) or `Root::load_graph` (`:412`); the
  shot script is spawned from `RunMode::Scripted` (`Root::start_script`, `:301`).
- Ready → start sync (`sync_setup::Coordinator::install`, `:490`), API + link inbox, hotkey
  or probe — all owned by `Ready`; Settings → Assistants (`assistants::Coordinator`) starts,
  stops and re-levels the loopback API of the same slot
  ([`assistant-access-and-api-life.md`](assistant-access-and-api-life.md)).
- Cmd+, → `open_settings` (deferred) → raise `SettingsWindow::Open`, else open one.
- Cmd+Q (any window) / the Quit menu item / the last window closing → `quit_app` → `cx.quit()`;
  the app-level `on_app_quit` backstop flushes again if the OS asked without the app hearing.

## The combinations that were impossible or unhandled

1. **Script fails to parse → no script, no hotkey, no quit.** The script was parsed in
   `Root::new`, the failure only logged, and `open_graph_with` still saw `UTOPIA_SHOT_SCRIPT`
   set and skipped the hotkey. Now the parse happens once, before the window, and a failure
   ends the run with the error on stderr (`main.rs:223`, `main.rs:818`).
2. **Three quit paths with different behaviour.** `Workspace::quit` ran `sync_index`, Cmd+Q
   with Settings focused and closing the last window did not. The per-workspace handler and
   `AppState::install_quit_hook` are gone; the one `quit_app` owns `editor.end`/`finish_edit`,
   `sync_index`, the report and the compact, and `Running` lets it reach the store after the
   window has closed.
3. **Side tasks lived outside the enum**, as four `Root` fields that no transition cleared.
   They are `Ready`'s fields now.
4. **`Failed("")` was a representable nonsense value.** There is no placeholder to observe.
5. **The Settings window handle went stale silently.** `SettingsWindow` is `Closed | Open`,
   and the app's window-closed observer asks the handle whether it is still alive.
6. **Theme froze for Settings when the main window closed first.** Settings observes its own
   window appearance now, like the main window.

## Diagram

```mermaid
stateDiagram-v2
    [*] --> Onboarding : no graph
    [*] --> Locked : graph encrypted
    [*] --> Ready : graph opened (owner lock taken, side tasks in Ready)
    [*] --> Failed : open error (terminal)
    Locked --> Locked : wrong passphrase
    Locked --> Ready : passphrase ok
    Onboarding --> Ready : name chosen
    Ready --> [*] : quit_app (Cmd+Q anywhere, menu, last window closed)
    note right of Ready : side tasks are Ready's fields; a malformed UTOPIA_SHOT_SCRIPT fails before any window
```

## The explicit type

```rust
enum RootState { Onboarding { … }, Locked { path, owner: Option<OwnerLock>, … }, Ready(Ready), Failed(String) }
struct Ready { ws: Entity<Workspace>, _hotkey: Option<Task<()>>, _sync: Entity<sync_setup::Coordinator>,
               _assistants: Entity<assistants::Coordinator>, _inbox: Task<()> }
struct Running { root: Option<WeakEntity<Root>>, app: Entity<AppState>, _owner: Option<OwnerLock> }
enum RunMode { Interactive, Scripted(Vec<shot::Step>), Probe }
enum SettingsWindow { Closed, Open(WindowHandle<SettingsView>) }
```
