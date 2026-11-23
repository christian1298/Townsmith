#!/usr/bin/env bash
set -euo pipefail

# 3D multiplayer world client smoke.
#
# Starts a fresh `townsmith-server --listen PORT --sessions 1` per run and runs
# `godot/mp_world_client.tscn --smoke`. The client sends hello, six
# deterministic place_road commands, a 96 m interest view, renders the bounded
# interest view in 3D, then asks for a zero/narrow 16 m viewport (zero roads and
# buildings) and disconnects. Each run must print:
#   MP_WORLD_CLIENT status=ok tick=6 population=3277 balance=100000 \
#     roads=6 buildings=4 truncated=false buildings_truncated=false \
#     checksum=<16 hex> road_checksum=<16 hex> building_checksum=<16 hex> ...
#   MP_WORLD_CLIENT_ZERO status=ok ... roads=0 buildings=0 ...
#   MP_WORLD_CLIENT_DONE status=ok ... frames_sent=10 frames_received=12
#
# The second run must produce identical marker lines and checksums. Readiness
# is a bounded poll of the server log; there are no fixed sleeps. This is a
# debug render integration, not gameplay networking, a prediction/streaming
# implementation, a benchmark gate, or the 1M render gate.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

if ! command -v "$GODOT_BIN" >/dev/null 2>&1; then
	echo "Godot binary not found: $GODOT_BIN (set GODOT=/path/to/godot)" >&2
	exit 2
fi

cargo build --quiet --manifest-path "$ROOT/Cargo.toml" -p townsmith-server

# Godot may import the project cache on the first headless run. Warm it once on
# a fresh worktree so the two measured runs have identical output. The scene
# does not use the GDExtension, so unrelated editor-scene parse errors while the
# extension is absent must not fail this render smoke.
if [[ ! -d "$ROOT/godot/.godot" ]]; then
	"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1 || true
fi

pick_port() {
	if [[ -n "${1:-}" ]]; then
		printf '%s\n' "$1"
		return
	fi
	python3 - <<'PY'
import socket

with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
    probe.bind(("127.0.0.1", 0))
    print(probe.getsockname()[1])
PY
}

# wait_for_log LOG PID NEEDLE SECONDS
#
# Polls the log until NEEDLE appears. Fails as soon as the server process is
# gone without writing it, so a bind failure or early exit is reported instead
# of waiting out the deadline.
wait_for_log() {
	python3 - "$1" "$2" "$3" "$4" <<'PY'
import os
import sys
import time

log_path, pid_text, needle, seconds = sys.argv[1:5]
pid = int(pid_text)
deadline = time.monotonic() + float(seconds)
while time.monotonic() < deadline:
    try:
        with open(log_path, "r", encoding="utf-8", errors="replace") as handle:
            if needle in handle.read():
                sys.exit(0)
    except OSError:
        pass
    try:
        os.kill(pid, 0)
    except OSError:
        print(f"server exited before reporting `{needle}`", file=sys.stderr)
        sys.exit(1)
    time.sleep(0.02)
print(f"timed out waiting for `{needle}`", file=sys.stderr)
sys.exit(1)
PY
}

# wait_for_exit PID SECONDS
wait_for_exit() {
	python3 - "$1" "$2" <<'PY'
import os
import sys
import time

pid = int(sys.argv[1])
deadline = time.monotonic() + float(sys.argv[2])
while time.monotonic() < deadline:
    try:
        os.kill(pid, 0)
    except OSError:
        sys.exit(0)
    time.sleep(0.02)
print("timed out waiting for the server to exit", file=sys.stderr)
sys.exit(1)
PY
}

SMOKE_DIR="$(mktemp -d)"
SERVER_LOG="$(mktemp)"
SERVER_PID=""

stop_server() {
	if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
		kill "$SERVER_PID" 2>/dev/null || true
	fi
	if [[ -n "$SERVER_PID" ]]; then
		wait "$SERVER_PID" 2>/dev/null || true
	fi
	SERVER_PID=""
}

cleanup() {
	stop_server
	rm -f "$SERVER_LOG"
	rm -rf "$SMOKE_DIR"
}
trap cleanup EXIT

start_server() {
	local port="$1"
	: >"$SERVER_LOG"
	"$ROOT/target/debug/townsmith-server" --listen "$port" --sessions 1 >"$SERVER_LOG" 2>&1 &
	SERVER_PID=$!
	wait_for_log "$SERVER_LOG" "$SERVER_PID" "SERVER_LISTEN status=ok address=" 10
}

# run_block NAME PORT PREFIX
run_block() {
	local name="$1"
	local port="$2"
	local prefix="$3"
	local output
	local godot_status
	local main_line
	local zero_line
	local done_line
	local main_checksum
	local zero_checksum
	local road_checksum
	local building_checksum

	start_server "$port"

	set +e
	output="$(timeout 60s env -u DISPLAY -u WAYLAND_DISPLAY \
		"$GODOT_BIN" --headless --path "$ROOT/godot" res://mp_world_client.tscn -- \
		--smoke --port "$port" 2>&1)"
	godot_status=$?
	set -e
	printf '%s\n' "$output" >&2

	if [[ $godot_status -ne 0 ]]; then
		echo "3D world client smoke ($name) failed: Godot exited with status $godot_status" >&2
		exit 1
	fi

	main_line="$(grep '^MP_WORLD_CLIENT status=ok ' <<<"$output" | tail -1 || true)"
	zero_line="$(grep '^MP_WORLD_CLIENT_ZERO status=ok ' <<<"$output" | tail -1 || true)"
	done_line="$(grep '^MP_WORLD_CLIENT_DONE status=ok ' <<<"$output" | tail -1 || true)"

	if grep -q '^MP_WORLD_CLIENT status=fail ' <<<"$output"; then
		echo "3D world client smoke ($name) failed: client reported a failure" >&2
		exit 1
	fi
	if [[ -z "$main_line" || -z "$zero_line" || -z "$done_line" ]]; then
		echo "3D world client smoke ($name) failed: missing main, zero, or done marker" >&2
		exit 1
	fi

	if [[ ! "$main_line" =~ ^MP_WORLD_CLIENT\ status=ok\ tick=6\ population=3277\ balance=100000\ roads=6\ buildings=4\ truncated=false\ buildings_truncated=false\ checksum=[0-9a-f]{16}\ road_checksum=48aff8a44b921666\ building_checksum=a98d781faa5c1d5b\ rendered_roads=6\ rendered_buildings=4\ center=0\.0,0\.0\ radius=96\.0\ session=session-[0-9a-f]{16}$ ]]; then
		echo "3D world client smoke ($name) main marker mismatch: $main_line" >&2
		exit 1
	fi
	if [[ ! "$zero_line" =~ ^MP_WORLD_CLIENT_ZERO\ status=ok\ tick=6\ population=3277\ balance=100000\ roads=0\ buildings=0\ truncated=false\ buildings_truncated=false\ checksum=[0-9a-f]{16}\ rendered_roads=0\ rendered_buildings=0\ center=32\.0,32\.0\ radius=16\.0$ ]]; then
		echo "3D world client smoke ($name) zero marker mismatch: $zero_line" >&2
		exit 1
	fi
	if [[ ! "$done_line" =~ ^MP_WORLD_CLIENT_DONE\ status=ok\ session=session-[0-9a-f]{16}\ tick=6\ frames_sent=10\ frames_received=12$ ]]; then
		echo "3D world client smoke ($name) done marker mismatch: $done_line" >&2
		exit 1
	fi

	main_checksum="$(sed -n 's/.* checksum=\([0-9a-f]\{16\}\).*/\1/p' <<<"$main_line")"
	zero_checksum="$(sed -n 's/.* checksum=\([0-9a-f]\{16\}\).*/\1/p' <<<"$zero_line")"
	road_checksum="$(sed -n 's/.* road_checksum=\([0-9a-f]\{16\}\).*/\1/p' <<<"$main_line")"
	building_checksum="$(sed -n 's/.* building_checksum=\([0-9a-f]\{16\}\).*/\1/p' <<<"$main_line")"

	if [[ ! "$main_checksum" =~ ^[0-9a-f]{16}$ || "$main_checksum" != "$zero_checksum" ]]; then
		echo "3D world client smoke ($name) failed: set_interest moved the server checksum ('${main_checksum:-none}' vs '${zero_checksum:-none}')" >&2
		exit 1
	fi
	if [[ ! "$road_checksum" =~ ^[0-9a-f]{16}$ || ! "$building_checksum" =~ ^[0-9a-f]{16}$ ]]; then
		echo "3D world client smoke ($name) failed: render checksum missing" >&2
		exit 1
	fi

	grep '^MP_WORLD_CLIENT' <<<"$output" >"$prefix.txt"
	printf '%s\n' "$main_line" >"$prefix.main"
	printf '%s\n' "$zero_line" >"$prefix.zero"
	printf '%s\n' "$main_checksum" >"$prefix.checksum"

	# The server reports the accepted session and command count after the
	# client disconnects. Ten commands: hello, six roads, two set_interest, and
	# disconnect. Twelve responses: welcome, six road snapshots, two snapshots,
	# two interest views, and goodbye.
	wait_for_log "$SERVER_LOG" "$SERVER_PID" "SERVER_LISTEN status=ok sessions=1 commands=10 " 10
	wait_for_exit "$SERVER_PID" 10
	stop_server
	if ! grep -Eq "SERVER_LISTEN status=ok sessions=1 commands=10 " "$SERVER_LOG"; then
		echo "3D world client smoke ($name) failed: server did not report one 10-command session" >&2
		cat "$SERVER_LOG" >&2
		exit 1
	fi
}

FIRST_PORT="$(pick_port "${MP_WORLD_CLIENT_PORT:-}")"
SECOND_PORT="$(pick_port "${MP_WORLD_CLIENT_SECOND_PORT:-}")"
for _ in 1 2 3 4 5; do
	[[ "$SECOND_PORT" != "$FIRST_PORT" ]] && break
	SECOND_PORT="$(pick_port "")"
done
if [[ "$SECOND_PORT" == "$FIRST_PORT" ]]; then
	echo "3D world client smoke failed: both runs selected port $FIRST_PORT" >&2
	exit 1
fi

run_block first "$FIRST_PORT" "$SMOKE_DIR/first"
run_block second "$SECOND_PORT" "$SMOKE_DIR/second"

if ! diff -u "$SMOKE_DIR/first.main" "$SMOKE_DIR/second.main"; then
	echo "3D world client smoke failed: main markers differ between runs" >&2
	exit 1
fi
if ! diff -u "$SMOKE_DIR/first.zero" "$SMOKE_DIR/second.zero"; then
	echo "3D world client smoke failed: zero-view markers differ between runs" >&2
	exit 1
fi
if ! diff -u "$SMOKE_DIR/first.txt" "$SMOKE_DIR/second.txt"; then
	echo "3D world client smoke failed: deterministic MP_WORLD_CLIENT lines differ between runs" >&2
	exit 1
fi
if [[ "$(cat "$SMOKE_DIR/first.checksum")" != "$(cat "$SMOKE_DIR/second.checksum")" ]]; then
	echo "3D world client smoke failed: server checksums differ between runs" >&2
	exit 1
fi

echo "MP_WORLD_CLIENT_SMOKE status=ok runs=2 tick=6 population=3277 balance=100000 roads=6 buildings=4 checksum=$(cat "$SMOKE_DIR/first.checksum") road_checksum=48aff8a44b921666 building_checksum=a98d781faa5c1d5b zero_roads=0 zero_buildings=0"
