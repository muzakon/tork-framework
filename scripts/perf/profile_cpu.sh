#!/bin/zsh
set -euo pipefail

if [[ $# -lt 2 ]]; then
  echo "usage: $0 <tork|axum> <scenario> [port]" >&2
  exit 1
fi

backend="$1"
scenario="$2"
port="${3:-3000}"
stamp="$(date +%Y%m%d-%H%M%S)"
out_dir="target/perf/$stamp/$backend/$scenario"
mkdir -p "$out_dir"

cargo run -p http-parity --bin parity_server --release -- "$backend" "$scenario" "127.0.0.1:$port" >"$out_dir/server.log" 2>&1 &
server_pid=$!
trap 'kill "$server_pid" 2>/dev/null || true' EXIT

sleep 2
xctrace record --template 'Time Profiler' --attach "$server_pid" --time-limit 15s --output "$out_dir/time-profile.trace" >/dev/null
cargo run -p http-parity --bin parity_load --release -- "$backend" "$scenario" --concurrency 64 --duration 10 --warmup 2 >"$out_dir/load.md"

echo "CPU profile: $out_dir/time-profile.trace"
