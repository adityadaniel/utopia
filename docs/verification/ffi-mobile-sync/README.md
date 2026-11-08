# Verifying the FFI sync round (docs/state-machines/ffi-mobile-sync.md)

The change is in the Rust core behind the mobile surface (`crates/utopia-ffi`,
`crates/utopia-sync`) plus the strings the SwiftUI shell renders for a round's outcome. The
Rust side is covered by the unit and acceptance tests listed in the machine doc; these shots
are for the visible half — the status line under today's page.

## How they were taken

The mobile shell is the one in `mobile/ios/UtopiaMobile`, built for macOS exactly as
`xtask mobile` step 2 does it:

```bash
cargo build -p utopia-ffi
cargo run -q -p utopia-ffi --bin uniffi-bindgen -- generate \
  --library "$CARGO_TARGET_DIR/debug/libutopia_ffi.dylib" --language swift --out-dir /tmp/ffigen
xcrun --sdk macosx swiftc -swift-version 5 -parse-as-library \
  mobile/ios/UtopiaMobile/*.swift /tmp/ffigen/utopia_ffi.swift \
  -o /tmp/UtopiaMobile-macos \
  -Xcc -fmodule-map-file=/tmp/ffigen/utopia_ffiFFI.modulemap -I /tmp/ffigen \
  "$CARGO_TARGET_DIR/debug/libutopia_ffi.a" \
  -framework Security -framework CoreFoundation -framework SystemConfiguration
UTOPIA_MOBILE_GRAPH=/tmp/s1.utopia UTOPIA_TODAY=2026-11-02 UTOPIA_SHOT=/tmp/offline.png \
  /tmp/UtopiaMobile-macos
```

Two throwaway patches were applied to a **copy** of the shell in `/tmp` for the shots (nothing
in the repo): `UTOPIA_AUTOSYNC=<url>` fires one `GraphModel.sync` at launch so the line is
reachable without tapping the Sync sheet, and the three `Keychain.get` lookups return `nil`
so taking a screenshot never touches the user's keychain or shows a prompt. The strings
rendered are the repo's `GraphModel.describe`; only the trigger and the secrets are patched.

## sync-offline.png

`UTOPIA_AUTOSYNC=http://127.0.0.1:1` (a closed port). The line reads
**"Offline — 2 changes queued"**: the `SyncOutcome::Offline { queued }` arm, with the count
from the round, instead of the old `"Offline: sync: transport: … (os error 61)"` that every
failure used to produce. The two queued changes are today's page and its first block, which is
what the store has not sent.

## sync-unsupported.png

`UTOPIA_AUTOSYNC=http://127.0.0.1:8791` served by a throwaway Python HTTP server that answers
every manifest with `{"protocol_version": 99, …}`. The line reads
**"Sync: relay speaks protocol 99; update the app"** — `SyncOutcome::Unsupported { version }`,
the answer §12.6 says must stop the client from pushing. On `main` this was the same
"Offline: …" as a dead network.

## What these do not show

- `NeedsAuth` and `Busy` were not screenshotted (they need a relay that answers 401 and a tap
  during a round, neither of which the shot hook can do); they are covered by
  `ac_12_6_a_refusal_is_not_offline` and `ac_12_6_a_second_tap_during_a_round_is_busy`, and
  the Swift `switch` is exhaustive, so a new variant would not compile.
- The iOS Simulator build: the shell type-checks against the iOS SDK (as `xtask mobile`
  does), but it has not run on a simulator or device here.
- The grey rules across "Nothing yet today" and the status line are SwiftUI `List` row
  separators in the captured window, present in the preview before this change too.
