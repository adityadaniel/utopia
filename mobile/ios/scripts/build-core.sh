#!/bin/bash
# Builds the Rust core (`utopia-ffi`) for the platform Xcode builds for, and writes the UniFFI
# Swift bindings. Xcode runs it before it compiles the app (see project.yml); `make bindings`
# runs it with no Xcode settings, for the host only.
#
# Output:
#   mobile/ios/Generated/utopia_ffi.swift            the Swift bindings (compiled into the app)
#   mobile/ios/Generated/include/                    the C header and module.modulemap
#   target/xcode/<platform>-<configuration>/libutopia_ffi.a   the library the app links
set -euo pipefail

IOS_DIR="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="$(cd "$IOS_DIR/../.." && pwd)"
GEN="$IOS_DIR/Generated"
PLATFORM="${PLATFORM_NAME:-host}"
CONFIG="${CONFIGURATION:-Debug}"

# Xcode exports SDKROOT, ARCHS and many other settings. Cargo build scripts run on the Mac,
# and an iOS SDKROOT breaks them, so cargo runs in a clean environment.
run_cargo() {
    env -i ${CARGO_EXTRA_ENV:-} \
        HOME="$HOME" \
        USER="${USER:-$(id -un)}" \
        PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
        DEVELOPER_DIR="${DEVELOPER_DIR:-$(xcode-select -p)}" \
        IPHONEOS_DEPLOYMENT_TARGET=27.0 \
        CARGO_INCREMENTAL=0 \
        cargo "$@"
}

cd "$ROOT"

# 1. The bindings come from a host build of the library: the interface is the same on every
#    platform.
run_cargo build -p utopia-ffi
mkdir -p "$GEN/include"
run_cargo run -q -p utopia-ffi --bin uniffi-bindgen -- generate \
    --library "$ROOT/target/debug/libutopia_ffi.dylib" \
    --language swift --out-dir "$GEN/tmp"
# Write only what changed, so Xcode does not compile the bindings again on every build.
sync_file() { cmp -s "$1" "$2" || cp "$1" "$2"; }
sync_file "$GEN/tmp/utopia_ffi.swift" "$GEN/utopia_ffi.swift"
sync_file "$GEN/tmp/utopia_ffiFFI.h" "$GEN/include/utopia_ffiFFI.h"
sync_file "$GEN/tmp/utopia_ffiFFI.modulemap" "$GEN/include/module.modulemap"
rm -rf "$GEN/tmp"

[ "$PLATFORM" = host ] && exit 0

# 2. The library for the platform Xcode builds for. Apple silicon only: there is no x86_64
#    simulator slice.
case "$PLATFORM" in
    iphonesimulator) TARGET=aarch64-apple-ios-sim ;;
    iphoneos) TARGET=aarch64-apple-ios ;;
    *) echo "error: build-core.sh: unsupported platform $PLATFORM" >&2; exit 1 ;;
esac
if [ "$CONFIG" = Release ]; then
    run_cargo build -p utopia-ffi --target "$TARGET" --release
    BUILT="$ROOT/target/$TARGET/release/libutopia_ffi.a"
else
    # No Rust debug info for the iOS copies: it is gigabytes on a tight disk, and Xcode
    # debugs the Swift side. The Mac build above keeps it (and shares `make check`'s cache).
    CARGO_EXTRA_ENV="CARGO_PROFILE_DEV_DEBUG=0" run_cargo build -p utopia-ffi --target "$TARGET"
    BUILT="$ROOT/target/$TARGET/debug/libutopia_ffi.a"
fi
OUT="$ROOT/target/xcode/$PLATFORM-$CONFIG"
mkdir -p "$OUT"
sync_file "$BUILT" "$OUT/libutopia_ffi.a"
