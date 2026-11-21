#!/usr/bin/env bash
# Request accounting for one Utopia client against the Cloudflare Worker relay (PRD §12.3,
# §12.8): how many Worker requests a device makes while idle with its realtime socket open,
# and while it is editing, broken down by route. Those are the numbers the cost section of
# docs/self-host-relay-cloudflare.md is based on.
#
# One scratch graph, one `utopia sync --continuous` daemon (`--local` wrangler, no account),
# the realtime socket on, E2E on as recommended. The counts come from wrangler's own request
# log (`[wrangler:info] GET /v1/graphs/…/manifest 200 OK`), sliced by line offset, not by an
# instrumented relay.
#
#   IDLE_SECONDS=180 ACTIVE_SECONDS=180 EDIT_SECONDS=10 ./relay-cloudflare/scripts/measure-requests.sh
set -uo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
repo=$(cd "$here/.." && pwd)
source "$HOME/.cargo/env" 2>/dev/null || true
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

BIN="${UTOPIA_BIN:-$CARGO_TARGET_DIR/debug/utopia}"
WORK="${MEASURE_DIR:-$repo/target/relay-request-counts}"
CONFIG="$WORK/config"
GRAPH="$WORK/Count.utopia"
STATE="$WORK/relay-state"
IDLE_SECONDS="${IDLE_SECONDS:-180}"
ACTIVE_SECONDS="${ACTIVE_SECONDS:-180}"
EDIT_SECONDS="${EDIT_SECONDS:-10}"
TOKEN="$(openssl rand -hex 32)"
PASS="$(openssl rand -hex 16)"
GRAPH_ID="count-$(openssl rand -hex 6)"
WRANGLER_LOG="$WORK/wrangler.log"

mkdir -p "$WORK"
rm -rf "$STATE" "$GRAPH" "$GRAPH.owner" "$CONFIG"
mkdir -p "$CONFIG"

avail_kb=$(df -k "$repo" | awk 'NR==2 {print $4}')
if [ -z "$avail_kb" ] || [ "$avail_kb" -lt $((3 * 1024 * 1024)) ]; then
  echo "refusing to run: less than 3 GB free" >&2
  exit 1
fi
if [ ! -x "$BIN" ]; then
  echo "no utopia binary at $BIN (cargo build -p utopia-app, or set UTOPIA_BIN)" >&2
  exit 1
fi

PORT=$(node -e 'const s=require("net").createServer(); s.listen(0,"127.0.0.1",()=>{console.log(s.address().port);s.close()})')
BASE="http://127.0.0.1:$PORT"
WRANGLER_PID=
DAEMON=

now_ms() { perl -MTime::HiRes=time -e 'printf "%.0f", time * 1000'; }

stop_relay() {
  [ -n "${WRANGLER_PID:-}" ] || return 0
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
}
stop_daemon() {
  [ -n "${DAEMON:-}" ] || return 0
  kill "$DAEMON" 2>/dev/null
  for _ in $(seq 1 60); do
    kill -0 "$DAEMON" 2>/dev/null || break
    sleep 0.25
  done
  kill -9 "$DAEMON" 2>/dev/null
  wait "$DAEMON" 2>/dev/null
  DAEMON=
}
trap 'stop_daemon; stop_relay' EXIT INT TERM

echo "== local relay on $BASE (graph id $GRAPH_ID) =="
( cd "$here" && exec npx --no-install wrangler dev --local --ip 127.0.0.1 --port "$PORT" \
    --persist-to "$STATE" \
    --var RELAY_TOKENS:"$TOKEN" --var QUOTA_BYTES:1073741824 ) >"$WRANGLER_LOG" 2>&1 &
WRANGLER_PID=$!
for _ in $(seq 1 120); do
  curl -sf "$BASE/healthz" >/dev/null 2>&1 && break
  sleep 0.5
done
curl -sf "$BASE/healthz" >/dev/null || {
  echo "relay did not start:" >&2
  tail -30 "$WRANGLER_LOG" >&2
  exit 1
}
# The health probes above are not part of any measurement window.

env UTOPIA_CONFIG_DIR="$CONFIG" UTOPIA_SYNC_TOKEN="$TOKEN" UTOPIA_SYNC_PASSPHRASE="$PASS" \
  "$BIN" sync setup --graph "$GRAPH" --mode relay --url "$BASE" --graph-id "$GRAPH_ID" \
  >"$WORK/setup.log" 2>&1 || {
  echo "sync setup failed:" >&2
  cat "$WORK/setup.log" >&2
  exit 1
}
KEY=$(printf '%s' "$(realpath "$GRAPH")" | shasum -a 256 | cut -d' ' -f1)
mkdir -p "$CONFIG/mcp"
printf '{"version":1,"access":"write","when_closed":false}\n' >"$CONFIG/mcp/$KEY.json"

env UTOPIA_CONFIG_DIR="$CONFIG" UTOPIA_SYNC_TOKEN="$TOKEN" UTOPIA_SYNC_PASSPHRASE="$PASS" \
  "$BIN" sync --continuous --graph "$GRAPH" >"$WORK/daemon.log" 2>&1 &
DAEMON=$!
for _ in $(seq 1 120); do
  grep -q "realtime listener started" "$WORK/daemon.log" && break
  sleep 0.5
done
grep -q "realtime listener started" "$WORK/daemon.log" || {
  echo "the daemon never started its listener:" >&2
  cat "$WORK/daemon.log" >&2
  exit 1
}
# One edit so the page exists and the first rounds are done; then wait for a quiet round so
# the idle window measures the cadence, not the start-up.
env UTOPIA_CONFIG_DIR="$CONFIG" UTOPIA_GRAPH="$GRAPH" "$BIN" capture "counting requests" >/dev/null 2>&1
for _ in $(seq 1 120); do
  grep -q "quiet, next in" "$WORK/daemon.log" && break
  sleep 0.5
done
sleep 2

log_lines() { wc -l <"$WRANGLER_LOG" | tr -d ' '; }

# Count the requests in [start+1, end], per route and per minute.
report() { # name, start line, end line, seconds
  local name=$1 start=$2 end=$3 seconds=$4
  local slice="$WORK/slice-$name.log"
  tail -n +"$((start + 1))" "$WRANGLER_LOG" | head -n "$((end - start))" >"$slice"
  awk -v name="$name" -v seconds="$seconds" '
    /^\[wrangler:info\] [A-Z]+ / {
      method = $2
      path = $3
      route = "other"
      if (path == "/healthz") route = "healthz"
      else if (path ~ /\/manifest$/) route = "manifest GET"
      else if (path ~ /\/pages\/.*\/updates$/) route = "updates " method
      else if (path ~ /\/pages\/.*\/snapshot$/) route = "snapshot POST"
      else if (path ~ /\/events$/) route = "events (websocket upgrade)"
      else if (path ~ /\/blobs\/.*/) route = "blobs " method
      count[route]++
      total++
    }
    END {
      printf "%s (%ds of wall clock):\n", name, seconds
      for (route in count) {
        printf "  %-32s %4d  %5.1f/min\n", route, count[route], count[route] * 60 / seconds
      }
      printf "  %-32s %4d  %5.1f/min\n", "total", total, total * 60 / seconds
      if (total == 0) print "  (no requests in this window)"
    }' "$slice"
}

echo "== idle: ${IDLE_SECONDS}s, no edits, realtime socket open =="
IDLE_START=$(log_lines)
IDLE_T0=$(now_ms)
sleep "$IDLE_SECONDS"
IDLE_END=$(log_lines)
IDLE_ELAPSED=$(( ($(now_ms) - IDLE_T0) / 1000 ))
report "idle" "$IDLE_START" "$IDLE_END" "$IDLE_ELAPSED"

echo "== active: one edit every ${EDIT_SECONDS}s for ${ACTIVE_SECONDS}s =="
ACTIVE_START=$(log_lines)
ACTIVE_T0=$(now_ms)
elapsed=0
n=0
while [ "$elapsed" -lt "$ACTIVE_SECONDS" ]; do
  n=$((n + 1))
  env UTOPIA_CONFIG_DIR="$CONFIG" UTOPIA_GRAPH="$GRAPH" "$BIN" capture "edit $n" \
    >/dev/null 2>>"$WORK/capture.log"
  sleep "$EDIT_SECONDS"
  elapsed=$(( ($(now_ms) - ACTIVE_T0) / 1000 ))
done
sleep 2 # let the last edit's round finish
ACTIVE_END=$(log_lines)
ACTIVE_ELAPSED=$(( ($(now_ms) - ACTIVE_T0) / 1000 ))
echo "   $n edits in ${ACTIVE_ELAPSED}s"
report "active" "$ACTIVE_START" "$ACTIVE_END" "$ACTIVE_ELAPSED"

echo "== the last daemon rounds =="
tail -n 5 "$WORK/daemon.log" 2>/dev/null | sed 's/^/  /'
echo "== raw slices: $WORK/slice-*.log; wrangler log: $WRANGLER_LOG =="
stop_daemon
stop_relay
echo "== request counting done =="
