#!/usr/bin/env bash
# Two real Utopia instances against the Cloudflare Worker relay (PRD §12, §12.8).
#
# Two scratch graphs (A and B) share one graph id through one `wrangler dev --local`
# relay, with E2E encryption on. Two `utopia sync --continuous` daemons own them; every
# edit goes through `utopia mcp` (JSON-RPC on stdin, the Remote path into the daemon), so
# what this proves is exactly the path an assistant or the user drives.
#
#   (a) an edit on A appears on B and an edit on B appears on A;
#   (b) concurrent `update_block` on the same block from both sides converge;
#   (c) a WebSocket notice brings a change to B well before its 60 s idle poll;
#   (d) the relay's local state (DO SQLite, R2, trace store) holds only ciphertext.
#       Attachments have no CLI/MCP upload path: upload/download is covered by the Rust
#       client test `attachments_sync_encrypted_and_verified`
#       (scripts/test-against-rust-client.sh), which uploads an encrypted file through the
#       same Worker.
#
# Nothing leaves this machine: `--local` runs workerd, no account is touched, both graphs
# and the config dir live under `target/`. Set UTOPIA_SYNC_TOKEN / UTOPIA_SYNC_PASSPHRASE to
# reuse credentials; otherwise scratch ones are generated for this run.
set -uo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
repo=$(cd "$here/.." && pwd)
# shellcheck source=lib.sh
source "$here/scripts/lib.sh"

# The binary of `cargo build -p utopia-app`, wherever cargo puts it (CARGO_TARGET_DIR is
# respected), unless UTOPIA_BIN names another. E2E_DIR moves this run's state elsewhere.
BIN="${UTOPIA_BIN:-$(cargo_target_dir)/debug/utopia}"
WORK="${E2E_DIR:-$repo/target/e2e-two-instances}"
CONFIG="$WORK/config"
A="$WORK/A.utopia"
B="$WORK/B.utopia"
STATE="$WORK/relay-state"
TOKEN="${UTOPIA_SYNC_TOKEN:-$(openssl rand -hex 32)}"
PASS="${UTOPIA_SYNC_PASSPHRASE:-$(openssl rand -hex 16)}"
GRAPH_ID="e2e-$(openssl rand -hex 6)"
mkdir -p "$WORK" "$CONFIG"
rm -rf "$STATE" "$A" "$B" "$A.owner" "$B.owner" "$CONFIG"/*

avail_kb=$(df -k "$repo" | awk 'NR==2 {print $4}')
if [ -z "$avail_kb" ] || [ "$avail_kb" -lt $((3 * 1024 * 1024)) ]; then
  echo "refusing to run: less than 3 GB free" >&2
  exit 1
fi
if [ ! -d "$here/node_modules/wrangler" ]; then
  echo "missing relay-cloudflare/node_modules: run 'npm install' in $here first" >&2
  exit 1
fi
if [ ! -x "$BIN" ]; then
  echo "no utopia binary at $BIN (cargo build -p utopia-app, or set UTOPIA_BIN)" >&2
  exit 1
fi

PORT=$(node -e 'const s=require("net").createServer(); s.listen(0,"127.0.0.1",()=>{console.log(s.address().port);s.close()})')
BASE="http://127.0.0.1:$PORT"
WRANGLER_PID=
DAEMON_A=
DAEMON_B=

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

stop_daemons() {
  local pid
  for pid in ${DAEMON_A:-} ${DAEMON_B:-}; do
    kill "$pid" 2>/dev/null
  done
  for _ in $(seq 1 60); do
    local alive=0
    for pid in ${DAEMON_A:-} ${DAEMON_B:-}; do
      kill -0 "$pid" 2>/dev/null && alive=1
    done
    [ "$alive" -eq 0 ] && break
    sleep 0.25
  done
  for pid in ${DAEMON_A:-} ${DAEMON_B:-}; do
    kill -9 "$pid" 2>/dev/null
    wait "$pid" 2>/dev/null
  done
  DAEMON_A= DAEMON_B=
}
trap 'stop_daemons; stop_relay' EXIT INT TERM

fail() {
  echo "FAIL: $*" >&2
  echo "--- A log ---" >&2; tail -25 "$WORK/a.log" 2>/dev/null >&2
  echo "--- B log ---" >&2; tail -25 "$WORK/b.log" 2>/dev/null >&2
  exit 1
}

start_relay() {
  ( cd "$here" && exec npx --no-install wrangler dev --local --ip 127.0.0.1 --port "$PORT" \
      --persist-to "$STATE" \
      --var RELAY_TOKENS:"$TOKEN" --var QUOTA_BYTES:1073741824 ) >"$WORK/wrangler.log" 2>&1 &
  WRANGLER_PID=$!
  for _ in $(seq 1 120); do
    curl -sf "$BASE/healthz" >/dev/null 2>&1 && return 0
    if ! kill -0 "$WRANGLER_PID" 2>/dev/null; then
      echo "wrangler exited during start-up:" >&2
      tail -30 "$WORK/wrangler.log" >&2
      return 1
    fi
    sleep 0.5
  done
  echo "wrangler did not answer /healthz" >&2
  return 1
}

# One `utopia mcp` process per call: legacy era (initialize, then tools/call), which is the
# simplest handshake for a script and the path Codex uses (docs/mcp-design.md §6).
INIT='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"e2e-two-instances","version":"1"}}}'

call_tool() { # graph, tool, args-json -> the tool's text, or a nonzero exit with the error
  local graph=$1 tool=$2 args=$3 answer
  answer=$(
    {
      printf '%s\n' "$INIT"
      jq -cn --arg t "$tool" --argjson a "$args" \
        '{jsonrpc:"2.0",id:2,method:"tools/call",params:{name:$t,arguments:$a}}'
    } | env UTOPIA_CONFIG_DIR="$CONFIG" "$BIN" mcp --graph "$graph" 2>>"$WORK/mcp.stderr" |
      jq -c 'select(.id == 2)' | tail -1
  )
  if [ -z "$answer" ]; then
    echo "no answer from utopia mcp ($tool); see $WORK/mcp.stderr" >&2
    return 1
  fi
  if [ "$(jq -r 'has("error")' <<<"$answer")" = "true" ]; then
    echo "mcp $tool failed: $(jq -r '.error.message' <<<"$answer")" >&2
    return 1
  fi
  jq -r '.result.content[0].text // empty' <<<"$answer"
}

read_page() { call_tool "$1" read_page "$(jq -cn --arg p "$2" '{page:$p}')"; }

append_blocks() { # graph, page, text
  call_tool "$1" append_blocks "$(jq -cn --arg p "$2" --arg m "- $3" '{page:$p,markdown:$m,create:true}')"
}

update_block() { # graph, block id, text
  call_tool "$1" update_block "$(jq -cn --arg i "$2" --arg t "$3" '{id:$i,text:$t}')"
}

block_id_of() { # outline on stdin, marker text -> the first block id whose line holds it
  local marker=$1
  grep -F "$marker" | grep -o '{id:[A-Za-z0-9]*}' | head -1 | sed 's/{id:\(.*\)}/\1/'
}

block_line() { # outline on stdin, block id -> that block's text
  local id=$1
  grep -F "{id:$id}" | head -1 | sed -e 's/^ *- //' -e "s/ {id:$id}\$//"
}

# The assistant access setting (PRD §13.8 MC-1..MC-4): write, so the MCP calls above reach
# the daemon's /v1/op instead of being refused. File name: sha256 of the canonical path.
allow_writes() { # graph path
  local key
  key=$(printf '%s' "$(realpath "$1")" | shasum -a 256 | cut -d' ' -f1)
  mkdir -p "$CONFIG/mcp"
  printf '{"version":1,"access":"write","when_closed":false}\n' >"$CONFIG/mcp/$key.json"
}

run_daemon() { # graph, log
  env UTOPIA_CONFIG_DIR="$CONFIG" \
      UTOPIA_SYNC_TOKEN="$TOKEN" UTOPIA_SYNC_PASSPHRASE="$PASS" \
      "$BIN" sync --continuous --graph "$1" >"$2" 2>&1 &
  echo $!
}

echo "== starting the local relay on $BASE (graph id $GRAPH_ID) =="
start_relay || exit 1

echo "== setting up two scratch graphs (E2E on) =="
for graph in "$A" "$B"; do
  env UTOPIA_CONFIG_DIR="$CONFIG" UTOPIA_SYNC_TOKEN="$TOKEN" UTOPIA_SYNC_PASSPHRASE="$PASS" \
    "$BIN" sync setup --graph "$graph" --mode relay --url "$BASE" --graph-id "$GRAPH_ID" \
    >"$graph.setup.log" 2>&1 || {
    echo "sync setup failed for $graph:" >&2
    cat "$graph.setup.log" >&2
    exit 1
  }
  allow_writes "$graph"
done
echo "   both graphs use the relay graph $GRAPH_ID"

echo "== starting the two daemons =="
DAEMON_A=$(run_daemon "$A" "$WORK/a.log")
DAEMON_B=$(run_daemon "$B" "$WORK/b.log")
for log in "$WORK/a.log" "$WORK/b.log"; do
  for _ in $(seq 1 120); do
    grep -q "realtime listener started" "$log" && break
    sleep 0.5
  done
  grep -q "realtime listener started" "$log" || fail "a daemon never started its listener"
done

# Both need to own their graph and serve the loopback API before the first MCP call.
for graph in "$A" "$B"; do
  for _ in $(seq 1 120); do
    read_page "$graph" "E2E" >/dev/null 2>&1 && break
    sleep 0.5
  done
done

CANARY_A="canary-a-$(openssl rand -hex 4)"
CANARY_B="canary-b-$(openssl rand -hex 4)"
CANARY_SEED="seed-$(openssl rand -hex 4)"
CANARY_CA="concurrent-a-$(openssl rand -hex 4)"
CANARY_CB="concurrent-b-$(openssl rand -hex 4)"
CANARY_LIVE="notice-$(openssl rand -hex 4)"

echo "== (a) an edit on A appears on B =="
append_blocks "$A" "E2E" "hello from A $CANARY_A" >/dev/null || fail "append on A"
for _ in $(seq 1 60); do
  read_page "$B" "E2E" 2>/dev/null | grep -q "$CANARY_A" && break
  sleep 0.5
done
read_page "$B" "E2E" | grep -q "$CANARY_A" || fail "A's edit never reached B"
echo "   B reads: $(read_page "$B" "E2E" | grep "$CANARY_A")"

echo "== (a) an edit on B appears on A =="
append_blocks "$B" "E2E" "hello from B $CANARY_B" >/dev/null || fail "append on B"
for _ in $(seq 1 60); do
  read_page "$A" "E2E" 2>/dev/null | grep -q "$CANARY_B" && break
  sleep 0.5
done
read_page "$A" "E2E" | grep -q "$CANARY_B" || fail "B's edit never reached A"
echo "   A reads: $(read_page "$A" "E2E" | grep "$CANARY_B")"

echo "== (b) concurrent update_block on one block from both sides =="
append_blocks "$A" "E2E" "concurrent seed $CANARY_SEED" >/dev/null || fail "seed block"
BLOCK=
for _ in $(seq 1 60); do
  BLOCK=$(read_page "$B" "E2E" 2>/dev/null | block_id_of "$CANARY_SEED")
  [ -n "$BLOCK" ] && break
  sleep 0.5
done
[ -n "$BLOCK" ] || fail "B never saw the seed block"
echo "   editing block $BLOCK on both sides at the same time"
# No rev check on either side, and both writes happen before either side's next pull can
# import the other's update: a genuine concurrent CRDT update, not a last-writer overwrite.
update_block "$A" "$BLOCK" "concurrent from A $CANARY_CA" >"$WORK/concurrent-a.json" &
PA=$!
update_block "$B" "$BLOCK" "concurrent from B $CANARY_CB" >"$WORK/concurrent-b.json" &
PB=$!
wait $PA || fail "the concurrent update on A failed"
wait $PB || fail "the concurrent update on B failed"
MERGED=
TA=
TB=
for _ in $(seq 1 120); do
  TA=$(read_page "$A" "E2E" 2>/dev/null | block_line "$BLOCK")
  TB=$(read_page "$B" "E2E" 2>/dev/null | block_line "$BLOCK")
  if [ -n "$TA" ] && [ "$TA" = "$TB" ]; then
    MERGED=$TA
    break
  fi
  sleep 0.5
done
[ -n "$MERGED" ] || fail "the concurrent edit did not converge: A=$(printf %q "$TA") B=$(printf %q "$TB")"
echo "   merged text on both: $MERGED"
case "$MERGED" in
  *"$CANARY_CA"*"$CANARY_CB"* | *"$CANARY_CB"*"$CANARY_CA"*)
    echo "   both concurrent edits survived: a true merge, not a last-writer overwrite" ;;
  *)
    echo "   NOTE: only one side's text survived ($MERGED); the stores converged but the"
    echo "         merge did not keep both inserts. Reported as an observation, not a failure." ;;
esac

echo "== (c) a realtime notice brings a change before the 60 s idle poll =="
# Wait for B to back off to the slow cadence: then a pull cannot be its own poll.
SLOW_WAIT=0
while [ "$SLOW_WAIT" -lt 240 ]; do
  grep -q "next in 60s" "$WORK/b.log" && break
  sleep 1
  SLOW_WAIT=$((SLOW_WAIT + 1))
done
grep -q "next in 60s" "$WORK/b.log" || fail "B never reached the slow poll (waited ${SLOW_WAIT}s)"
LINES_BEFORE=$(wc -l <"$WORK/b.log" | tr -d ' ')
echo "   B is idle at a 60 s poll; A appends one block now"
T0=$(now_ms)
append_blocks "$A" "E2E" "live notice $CANARY_LIVE" >/dev/null || fail "the realtime append failed"
LATENCY=
for _ in $(seq 1 150); do
  if tail -n +"$((LINES_BEFORE + 1))" "$WORK/b.log" | grep -q "pulled"; then
    LATENCY=$(( $(now_ms) - T0 ))
    break
  fi
  sleep 0.2
done
[ -n "$LATENCY" ] || fail "B never pulled A's change while idle (no notice arrived)"
[ "$LATENCY" -lt 25000 ] || fail "B took ${LATENCY} ms: not a notice, a poll (60 s)"
echo "   B pulled ${LATENCY} ms after the edit, far inside its 60 s poll:"
tail -n +"$((LINES_BEFORE + 1))" "$WORK/b.log" | grep "pulled" | head -3 | sed 's/^/     /'

echo "== (d) the relay stored only ciphertext =="
PLAINTEXT_HITS=0
for marker in "$CANARY_A" "$CANARY_B" "$CANARY_SEED" "$CANARY_CA" "$CANARY_CB" "$CANARY_LIVE"; do
  if grep -rqF "$marker" "$STATE" 2>/dev/null; then
    echo "   PLAINTEXT in relay state: $marker" >&2
    PLAINTEXT_HITS=$((PLAINTEXT_HITS + 1))
  fi
done
FILES=$(find "$STATE" -type f | wc -l | tr -d ' ')
echo "   scanned $FILES relay-state files (DO SQLite + WAL, R2 objects, trace store)"
[ "$PLAINTEXT_HITS" -eq 0 ] || fail "the relay stored plaintext"
# Sanity: the plaintext really is on both devices (else the scan proves nothing).
read_page "$A" "E2E" | grep -q "$CANARY_LIVE" || fail "A does not hold its own plaintext"
read_page "$B" "E2E" | grep -q "$CANARY_LIVE" || fail "B does not hold the synced plaintext"
echo "   all six markers are on both devices and in none of the relay's $FILES files"

echo "== done: stopping the daemons and the relay =="
stop_daemons
stop_relay
echo "== two-instance end-to-end run passed =="
