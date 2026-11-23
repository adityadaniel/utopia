# AGENTS.md — the iPhone app

This file is for work in `mobile/ios`. The root `AGENTS.md` still applies: its architecture
rules, its commands and its "Verifying a change" steps. This file adds what is special to
the iPhone app.

**`docs/ios-prd.md` is the spec of the app.** §4 is the design. §6 has the acceptance
criteria (`AC-1.2`, `AC-4.5`, …). §8 (C1–C9) has the constraints. Read the section you
change, and write its id in tests and comments. A change to §4 goes through the owner (C8).

## Layout

| Path | What it is |
|---|---|
| `UtopiaMobile/` | The SwiftUI app. One graph, the Today screen, read-only pages, Find, Sync, first run. |
| `UtopiaMobile/Tokens.swift` | The colour tokens of `DESIGN.md` §2.1, light and dark, under the Mac names. |
| `UtopiaMobile/CoreClient.swift` | The one place that calls the generated bindings. It runs each call off the main actor. |
| `UtopiaMobile/Draft.swift` | The draft outline in the compose box, and its rules (§4.5). |
| `UtopiaMobile/Storage.swift` | Paths, relay settings (`UserDefaults`) and secrets (Keychain). |
| `Check/main.swift` | A Swift program that runs the bindings against the real core on macOS. |
| `crates/utopia-ffi` | The core the app calls (UniFFI). New calls go here, not in Swift. |

## Platform

- **iOS 27.0 is the deployment target.** The SDK is iOS 27.2, from Xcode 27.2. There is no
  code for older versions, so do not add `#available` gates for iOS 26 or older.
- **Swift 6 language mode with strict concurrency.** `AppModel` is `@MainActor` and
  `@Observable`; work on the graph and on the network runs off the main actor (C5).
- **SwiftUI first.** UIKit only where SwiftUI cannot do the job. Today the one case is the
  caret control in the lines of the draft (`LineEditor.swift`).
- iPhone only, portrait only (A2).

## Build and run

```bash
source ~/.cargo/env
export DEVELOPER_DIR="$(xcode-select -p)" CARGO_INCREMENTAL=0
cargo run -p xtask -- mobile          # bindings, Check, Swift 6 type-check against the iOS SDK
cargo run -p xtask -- mobile --ios    # also links target/mobile/UtopiaMobile.app for the simulator
cargo run -p xtask -- seed-phone target/phone/g.utopia   # the scratch graph the ACs name
```

- `--ios` needs `rustup target add aarch64-apple-ios-sim` (installed on this machine).
- There is no Xcode project. `xtask mobile --ios` compiles the sources with `swiftc` and
  links `libutopia_ffi.a`, then signs the bundle ad hoc. Every `.swift` file in
  `UtopiaMobile/` is part of the app.
- Install with `xcrun simctl install <udid> target/mobile/UtopiaMobile.app`.
- The app opens `Documents/graph.utopia` in its container. To use a seeded graph, copy it
  there (`xcrun simctl get_app_container <udid> app.utopia.mobile data`) while the app is
  not running. `UTOPIA_MOBILE_GRAPH`, `UTOPIA_MOBILE_CONFIG` and `UTOPIA_TODAY` override the
  graph, the draft folder and the day, for runs that can set the environment.
- The draft is `Library/Application Support/Utopia/draft.json`. Delete it for a clean run.
- `make check` does not build Swift. For a Swift change, `xtask mobile --ios` is the check.
- The linker warns that the Rust library is built for 27.2 and linked as 27.0. The warnings
  are harmless.
- Disk is tight. A worktree's `target/debug` grows to more than 10 GB; delete it when it
  does (root `AGENTS.md`).

## Look at it on the simulator

- Use the T3 device tools (`device_list`, `device_open`, `device_screenshot`) and the
  `agent-device` command that `device_open` returns, for every launch, tap, text entry and
  screenshot. Use `simctl` only to install, find the container and read logs.
- Simulators here: **iPhone 18 Pro** (iOS 27.2) is the reference phone. **iPhone Duo** puts
  the status bar and the toolbar buttons in a column at its side, so do not judge the layout
  from it.
- An `agent-device` session holds a simulator until it is closed. Do not take over or close
  a session that another agent opened; use another simulator or wait.
- Do the four steps of the root "Verifying a change": check, seed, drive, and read every
  PNG. Save shots downscaled (`sips -Z 1200`) under `docs/verification/<branch>/`.

## Rules for the app

1. **The core decides what is a link (C1).** No Swift code parses `[[`, `#`, `((` or titles.
   Title suggestions, stored link text and the display parts of a block come from
   `utopia-ffi`.
2. **Every write is a core call (C2).** The phone only adds to today's note and ticks to-dos
   (K6, K9). It never edits, moves or deletes a block that is in the graph.
3. **Tokens only (§4.7).** A view names a `Tokens` colour; no raw colour, no shadow.
   Sizes follow Dynamic Type (`@ScaledMetric` or text styles). Touch targets are 44 × 44 pt
   or larger (G5).
4. **Native chrome.** Each screen is in a `NavigationStack` with a `.navigationTitle` and
   `.toolbarTitleDisplayMode(.inlineLarge)`. Its actions are toolbar items: Find and sync status
   in `.topBarTrailing`; Close (`Button(role: .close)`) in `.cancellationAction`; confirm
   (`Button(role: .confirm)` or a named button) in `.confirmationAction`. Search uses
   `.searchable`. Do not draw a custom header, back button or close button. The one custom
   bar is the action bar inside the compose box, because K8 puts it there.
5. **Local state stays local (C7).** The draft, recents and scroll position never sync.
   Secrets go only to the Keychain (C6).
6. **VoiceOver.** Each control has a spoken label from §4.5.

## Skills

Read these before you write Swift:

- `swiftui-whats-new-27` — iOS 27 SwiftUI APIs. `@State` is a macro in SDK 27: read its
  references before you fix a `@State` init error.
- `swiftui-pro`, `swiftui-specialist` — SwiftUI review and patterns.
- `swift-concurrency-pro` — Swift 6 concurrency.
- `write-swift`, `mobile-native` (in `.agents/skills/`) — this repo's Swift and mobile rules.
- `xcodebuildmcp-cli` — build and run on the simulator.

## Known limits

- The app has run on the simulator only, not on an iPhone.
- Widgets and the Control Center and Action button control (IO-2) are not built.
- No sync round with a real relay has run from the phone (AC-9.2 to 9.5, AC-10.2, AC-10.3).
- The day log shows equal times for seeded blocks: a block's creation time is its ULID's
  timestamp, so a seed cannot fake it.
- Open review findings for the first build (Send and typing at the same time, a kill during
  Send, the first Send in two commits, link ranges after edits, and others) are listed in
  the pull request that added the app.
