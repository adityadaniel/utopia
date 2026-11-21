#!/usr/bin/env bash
# The real Utopia sync client against the Cloudflare Worker relay (PRD §12.8).
#
# Starts `wrangler dev --local` (a local workerd: Durable Objects on SQLite, a simulated R2
# bucket, WebSockets — no Cloudflare account, no network), waits for /healthz, runs
# `crates/utopia-syncd/tests/cloudflare.rs` twice, and always stops wrangler:
#
#   1. the default run, workers with a 1 GiB quota, skipping the quota tests;
#   2. a second run with QUOTA_BYTES=4096 for the two tests that need a graph near its
#      quota (`--skip quota` in the first run, the `quota` filter in the second).
#
# Nothing here deploys, logs in or touches a Cloudflare account. The relay's state lives in
# `relay-cloudflare/.wrangler/state` (pass `RM_STATE=1` to start from an empty one).
set -uo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
repo=$(cd "$here/.." && pwd)
source "$HOME/.cargo/env" 2>/dev/null || true
# One shared target directory: never a second one (a worktree's own target/ is 2+ GB).
if [ -z "${CARGO_TARGET_DIR:-}" ]; then
  if [ -d "$HOME/Developer/utopia/target" ]; then
    export CARGO_TARGET_DIR="$HOME/Developer/utopia/target"
  else
    export CARGO_TARGET_DIR="$repo/target"
  fi
fi
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"
export DEVELOPER_DIR="${DEVELOPER_DIR:-$(xcode-select -p 2>/dev/null)}"
export WRANGLER_SEND_METRICS=false

avail_kb=$(df -k "$repo" | awk 'NR==2 {print $4}')
if [ -z "$avail_kb" ] || [ "$avail_kb" -lt $((3 * 1024 * 1024)) ]; then
  echo "refusing to build: less than 3 GB free ($((avail_kb / 1024 / 1024)) GB)" >&2
  exit 1
fi
if [ ! -d "$here/node_modules/wrangler" ]; then
  echo "missing relay-cloudflare/node_modules: run 'npm install' in $here first" >&2
  exit 1
fi
if [ "${RM_STATE:-0}" = "1" ]; then
  rm -rf "$here/.wrangler/state"
fi

TOKEN="${UTOPIA_RELAY_TOKEN:-test-token}"
PORT=$(node -e 'const s=require("net").createServer(); s.listen(0,"127.0.0.1",()=>{console.log(s.address().port);s.close()})')
BASE="http://127.0.0.1:$PORT"
STATE="$here/.wrangler/state"
LOG_DIR="$repo/target/relay-cloudflare-tests"
mkdir -p "$LOG_DIR"
WRANGLER_PID=

stop_relay() {
  [ -n "${WRANGLER_PID:-}" ] || return 0
  # workerd is a child of wrangler; killing only the parent would leave it holding the
  # port and the state files.
  local children
  children=$(pgrep -P "$WRANGLER_PID" 2>/dev/null || true)
  kill "$WRANGLER_PID" $children 2>/dev/null
  for _ in $(seq 1 40); do
    kill -0 "$WRANGLER_PID" 2>/dev/null || break
    sleep 0.25
  done
  kill -9 "$WRANGLER_PID" $children 2>/dev/null
  wait "$WRANGLER_PID" 2>/dev/null
  WRANGLER_PID=
  sleep 0.3
}
trap 'stop_relay' EXIT INT TERM

start_relay() { # $1 = QUOTA_BYTES, $2 = log file
  ( cd "$here" && exec npx --no-install wrangler dev --local --ip 127.0.0.1 --port "$PORT" \
      --persist-to "$STATE" \
      --var RELAY_TOKENS:"$TOKEN" --var QUOTA_BYTES:"$1" ) >"$2" 2>&1 &
  WRANGLER_PID=$!
  for _ in $(seq 1 120); do
    if curl -sf "$BASE/healthz" >/dev/null 2>&1; then
      echo "relay up on $BASE (quota $1 bytes, log $2)"
      return 0
    fi
    if ! kill -0 "$WRANGLER_PID" 2>/dev/null; then
      echo "wrangler exited during start-up:" >&2
      tail -40 "$2" >&2
      return 1
    fi
    sleep 0.5
  done
  echo "wrangler did not answer /healthz within 60 s; last log lines:" >&2
  tail -40 "$2" >&2
  return 1
}

run_tests() { # $1 = log file; remaining args go to the test binary
  local log=$1
  shift
  ( cd "$repo" &&
    env UTOPIA_RELAY_URL="$BASE" \
        UTOPIA_RELAY_TOKEN="$TOKEN" \
        UTOPIA_RELAY_STATE_DIR="$STATE" \
        UTOPIA_RELAY_QUOTA="${UTOPIA_RELAY_QUOTA:-}" \
        cargo test -p utopia-syncd --test cloudflare -- "$@" ) 2>&1 | tee "$log"
  return "${PIPESTATUS[0]}"
}

status=0
echo "== phase 1: client scenarios (1 GiB quota) =="
start_relay 1073741824 "$LOG_DIR/wrangler-phase1.log" || exit 1
UTOPIA_RELAY_QUOTA= run_tests "$LOG_DIR/cargo-phase1.log" --skip quota || status=1
stop_relay

if [ "$status" -ne 0 ]; then
  echo "== phase 1 failed; see $LOG_DIR ==" >&2
  exit "$status"
fi

echo "== phase 2: quota scenarios (QUOTA_BYTES=4096) =="
start_relay 4096 "$LOG_DIR/wrangler-phase2.log" || exit 1
UTOPIA_RELAY_QUOTA=4096 run_tests "$LOG_DIR/cargo-phase2.log" quota || status=1
stop_relay

if [ "$status" -ne 0 ]; then
  echo "== phase 2 failed; see $LOG_DIR ==" >&2
  exit "$status"
fi
echo "== all Rust-client scenarios passed against the Worker =="
