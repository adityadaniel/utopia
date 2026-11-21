#!/usr/bin/env bash
# The real Utopia sync client against the Cloudflare Worker relay (PRD §12.8).
#
# Starts `wrangler dev --local` (a local workerd: Durable Objects on SQLite, a simulated R2
# bucket, WebSockets — no Cloudflare account, no network), waits for /healthz, and runs two
# test binaries of crates/utopia-syncd against it with UTOPIA_RELAY_REQUIRED=1, so a test
# that cannot reach the Worker fails instead of skipping:
#
#   tests/cloudflare.rs     the relay contract (tests/contract/mod.rs), the same scenarios
#                           tests/http.rs runs against utopia-syncd;
#   tests/differential.rs   one raw HTTP script sent to the Worker and to an in-process
#                           utopia-syncd, answers compared.
#
# in two runs, each on its own wrangler instance, and always stops wrangler:
#
#   1. the default 1 GiB quota, every test without "quota" in its name;
#   2. QUOTA_BYTES=4096, the tests with "quota" in their name.
#
# Nothing here deploys, logs in or touches a Cloudflare account.
#
# Environment (all optional):
#   CARGO_TARGET_DIR         kept when set (to share one target directory); otherwise cargo
#                            decides
#   RELAY_CHECK_DIR          logs and the default state directory
#                            (default: <repo>/target/relay-check)
#   UTOPIA_RELAY_STATE_DIR   wrangler's --persist-to directory; default: a fresh one under
#                            RELAY_CHECK_DIR for this run, deleted afterwards (KEEP_STATE=1
#                            keeps it), so two runs never share one
#   UTOPIA_RELAY_TOKEN       the token given to RELAY_TOKENS (default: test-token)
set -uo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
repo=$(cd "$here/.." && pwd)
# shellcheck source=lib.sh
source "$here/scripts/lib.sh"

require_free_gb 3 "$repo" || exit 1
require_wrangler || exit 1

WORK="${RELAY_CHECK_DIR:-$repo/target/relay-check}"
LOG_DIR="$WORK/logs"
mkdir -p "$LOG_DIR"
OWN_STATE=
if [ -n "${UTOPIA_RELAY_STATE_DIR:-}" ]; then
  STATE="$UTOPIA_RELAY_STATE_DIR"
  mkdir -p "$STATE"
else
  STATE=$(mktemp -d "$WORK/state.XXXXXX") || exit 1
  OWN_STATE=1
fi
SMALL_QUOTA=4096 # crates/utopia-syncd/tests/contract/mod.rs SMALL_QUOTA
TOKEN="${UTOPIA_RELAY_TOKEN:-test-token}"
PORT=$(node -e 'const s=require("net").createServer(); s.listen(0,"127.0.0.1",()=>{console.log(s.address().port);s.close()})')
BASE="http://127.0.0.1:$PORT"
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
cleanup() {
  stop_relay
  if [ -n "$OWN_STATE" ] && [ "${KEEP_STATE:-0}" != "1" ]; then
    rm -rf "$STATE"
  fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# An empty env file: given one, wrangler reads neither a developer's .dev.vars nor .env, so
# their RELAY_TOKENS or ALLOW_ANONYMOUS cannot leak into the run; only the --vars below apply.
NO_ENV="$WORK/empty.env"
: >"$NO_ENV"

start_relay() { # $1 = QUOTA_BYTES, $2 = log file
  ( cd "$here" && exec npx --no-install wrangler dev --local --ip 127.0.0.1 --port "$PORT" \
      --persist-to "$STATE" --env-file "$NO_ENV" \
      --var RELAY_TOKENS:"$TOKEN" --var QUOTA_BYTES:"$1" ) </dev/null >"$2" 2>&1 &
  WRANGLER_PID=$!
  for _ in $(seq 1 120); do
    if curl -sf "$BASE/healthz" >/dev/null 2>&1; then
      echo "relay up on $BASE (quota $1 bytes, state $STATE, log $2)"
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

run_tests() { # $1 = log file, $2 = UTOPIA_RELAY_QUOTA; remaining args go to the test binaries
  local log=$1 quota=$2
  shift 2
  ( cd "$repo" &&
    env UTOPIA_RELAY_URL="$BASE" \
        UTOPIA_RELAY_TOKEN="$TOKEN" \
        UTOPIA_RELAY_STATE_DIR="$STATE" \
        UTOPIA_RELAY_QUOTA="$quota" \
        UTOPIA_RELAY_REQUIRED=1 \
        cargo test -p utopia-syncd --test cloudflare --test differential -- "$@" ) 2>&1 |
    tee "$log"
  local status=${PIPESTATUS[0]}
  [ "$status" -eq 0 ] || return "$status"
  # A filter that matches nothing passes with 0 tests; that is a broken run, not a green one.
  local ran
  ran=$(sed -n 's/^test result: ok\. \([0-9]*\) passed.*/\1/p' "$log" | awk '{s += $1} END {print s + 0}')
  if [ "$ran" -eq 0 ]; then
    echo "no test ran (filter: $*); see $log" >&2
    return 1
  fi
  echo "($ran tests passed)"
}

echo "== run 1: contract and differential (1 GiB quota) =="
start_relay 1073741824 "$LOG_DIR/wrangler-default.log" || exit 1
run_tests "$LOG_DIR/cargo-default.log" "" --skip quota || {
  echo "== run 1 failed; logs in $LOG_DIR ==" >&2
  exit 1
}
stop_relay

echo "== run 2: quota scenarios (QUOTA_BYTES=$SMALL_QUOTA) =="
start_relay "$SMALL_QUOTA" "$LOG_DIR/wrangler-quota.log" || exit 1
run_tests "$LOG_DIR/cargo-quota.log" "$SMALL_QUOTA" quota || {
  echo "== run 2 failed; logs in $LOG_DIR ==" >&2
  exit 1
}
stop_relay

echo "== the contract and the differential script passed against the Worker =="
