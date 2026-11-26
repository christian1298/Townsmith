#!/usr/bin/env bash
set -euo pipefail

# Connected settlement readout smoke.
#
# Starts a fresh `townsmith-server --listen PORT --sessions 1` per run and runs
# `godot/mp_world_client.tscn --smoke --settlement-ui` with the deterministic
# script `found:Harbor|list|at:0,0`. The client sends the normal six-road setup,
# reads the 96 m interest view, then founds one settlement, lists it, and asks
# which settlement contains (0,0). It prints:
#   MP_SETTLEMENT_UI status=ok settlements=1 members=1 at=1 checksum=<16 hex>
# before the unchanged MP_WORLD_CLIENT_DONE marker.
#
# The second run uses a fresh server and must produce the same settlement
# marker/checksum. Readiness is a bounded server-log poll; there are no fixed
# sleeps.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

if ! command -v "$GODOT_BIN" >/dev/null 2>&1; then
	echo "Godot binary not found: $GODOT_BIN (set GODOT=/path/to/godot)" >&2
	exit 2
fi

cargo build --quiet --manifest-path "$ROOT/Cargo.toml" -p townsmith-server

if [[ ! -d "$ROOT/godot/.godot" ]]; then
	"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1 || true
fi

pick_port() {
	python3 - <<'PY'
import socket

with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
    probe.bind(("127.0.0.1", 0))
    print(probe.getsockname()[1])
PY
}

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

run_once() {
	local name="$1"
	local port="$2"
	local output
	local status
	local settlement_line
	local done_line

	start_server "$port"
	set +e
	output="$(timeout 90s env -u DISPLAY -u WAYLAND_DISPLAY \
		"$GODOT_BIN" --headless --path "$ROOT/godot" res://mp_world_client.tscn -- \
		--smoke --port "$port" --settlement-ui \
		--settlement-script "found:Harbor|list|at:0,0" 2>&1)"
	status=$?
	set -e
	printf '%s\n' "$output" >&2

	if [[ $status -ne 0 ]]; then
		echo "Settlement readout smoke ($name) failed: Godot exited $status" >&2
		exit 1
	fi
	if grep -q '^MP_WORLD_CLIENT status=fail ' <<<"$output"; then
		echo "Settlement readout smoke ($name) failed: client reported a failure" >&2
		exit 1
	fi

	settlement_line="$(grep '^MP_SETTLEMENT_UI status=ok ' <<<"$output" | tail -1 || true)"
	if [[ ! "$settlement_line" =~ ^MP_SETTLEMENT_UI\ status=ok\ settlements=1\ members=1\ at=1\ checksum=[0-9a-f]{16}$ ]]; then
		echo "Settlement readout smoke ($name) marker mismatch: ${settlement_line:-missing}" >&2
		exit 1
	fi
	done_line="$(grep '^MP_WORLD_CLIENT_DONE status=ok ' <<<"$output" | tail -1 || true)"
	if [[ ! "$done_line" =~ frames_sent=12\ frames_received=13$ ]]; then
		echo "Settlement readout smoke ($name) done marker mismatch: ${done_line:-missing}" >&2
		exit 1
	fi
	printf '%s\n' "$settlement_line" >"$SMOKE_DIR/$name.marker"
	stop_server
}

run_once first "$(pick_port)"
run_once second "$(pick_port)"

if ! diff -u "$SMOKE_DIR/first.marker" "$SMOKE_DIR/second.marker"; then
	echo "Settlement readout smoke failed: markers differ between runs" >&2
	exit 1
fi

MARKER="$(cat "$SMOKE_DIR/first.marker")"
echo "MP_SETTLEMENT_UI_SMOKE status=ok runs=2 ${MARKER#MP_SETTLEMENT_UI status=ok }"
