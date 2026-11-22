# Shared by the scripts in this directory: sourced (after `here` and `repo` are set), not run.

source "$HOME/.cargo/env" 2>/dev/null || true
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"
if [ -z "${DEVELOPER_DIR:-}" ] && command -v xcode-select >/dev/null 2>&1; then
  DEVELOPER_DIR=$(xcode-select -p 2>/dev/null) && export DEVELOPER_DIR
fi
# Wrangler never asks a question or sends telemetry from these scripts.
export WRANGLER_SEND_METRICS=false

# The directory cargo builds into: $CARGO_TARGET_DIR when it is set (the way to share one
# target directory between worktrees), otherwise whatever cargo itself resolves (a
# `build.target-dir` in a .cargo/config.toml, or <repo>/target).
cargo_target_dir() {
  if [ -n "${CARGO_TARGET_DIR:-}" ]; then
    printf '%s\n' "$CARGO_TARGET_DIR"
    return 0
  fi
  (cd "$repo" && cargo metadata --format-version 1 --no-deps) |
    node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>console.log(JSON.parse(s).target_directory))'
}

# Refuse to start when the disk cannot hold a build: $1 = GB, $2 = a path on that disk.
require_free_gb() {
  local avail_kb
  avail_kb=$(df -k "$2" | awk 'NR==2 {print $4}')
  if [ -z "$avail_kb" ] || [ "$avail_kb" -lt $(($1 * 1024 * 1024)) ]; then
    echo "refusing to run: less than $1 GB free on $2 ($((${avail_kb:-0} / 1024 / 1024)) GB)" >&2
    return 1
  fi
}

require_wrangler() {
  if [ ! -d "$here/node_modules/wrangler" ]; then
    echo "missing relay-cloudflare/node_modules: run 'npm ci' in $here first" >&2
    return 1
  fi
}
