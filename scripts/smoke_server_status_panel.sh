#!/usr/bin/env bash
set -euo pipefail

# Read-only server-status panel smoke: Rust server `--listen` <-> Godot panel.
#
# Starts a fresh `townsmith-server --once` on an ephemeral loopback port, waits
# for `SERVER_LISTEN`, runs the headless status panel, and asserts the
# `SERVER_STATUS_PANEL status=ok ...` marker. The world is unmodified and the
# same commands run against a second fresh server; both markers must match
# byte for byte. Readiness and completion are bounded polls of the server log;
# there are no fixed sleeps. The server is killed on exit.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

if ! command -v "$GODOT_BIN" >/dev/null 2>&1; then
	echo "Godot binary not found: $GODOT_BIN (set GODOT=/path/to/godot)" >&2
	exit 2
fi

cargo build --manifest-path "$ROOT/Cargo.toml" -p townsmith-server >/dev/null

pick_port() {
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
}
trap cleanup EXIT

# run_panel LABEL -> prints the marker line on stdout.
#
# Each run gets its own server process, port, and in-memory world, so a second
# run checks determinism rather than accumulating state.
run_panel() {
	local label="$1"
	local port
	port="$(pick_port)"

	: >"$SERVER_LOG"
	"$ROOT/target/debug/townsmith-server" --listen "$port" --once >"$SERVER_LOG" 2>&1 &
	SERVER_PID=$!
	wait_for_log "$SERVER_LOG" "$SERVER_PID" "SERVER_LISTEN status=ok address=" 10

	set +e
	OUTPUT="$(timeout 60s "$GODOT_BIN" --headless --path "$ROOT/godot" \
		res://server_status_panel.tscn -- \
		--smoke --host 127.0.0.1 --port "$port" \
		--name status-panel --timeout-ms 5000 2>&1)"
	GODOT_STATUS=$?
	set -e
	printf '%s\n' "$OUTPUT" >&2

	if [[ $GODOT_STATUS -ne 0 ]]; then
		echo "Server status panel smoke failed ($label): Godot exited with status $GODOT_STATUS" >&2
		exit 1
	fi

	local line
	line="$(grep "^SERVER_STATUS_PANEL " <<<"$OUTPUT" | tail -1 || true)"
	if [[ -z "$line" ]]; then
		echo "Server status panel smoke failed ($label): no SERVER_STATUS_PANEL marker" >&2
		exit 1
	fi
	if [[ ! "$line" =~ ^SERVER_STATUS_PANEL\ status=ok\ tick=[0-9]+\ population=[0-9]+\ settlements=[0-9]+\ checksum=[0-9a-f]{16}$ ]]; then
		echo "Server status panel smoke failed ($label): bad marker: $line" >&2
		exit 1
	fi

	# A requested disconnect is visible in the server report; wait for it so
	# the process exits on its own rather than relying on cleanup.
	wait_for_log "$SERVER_LOG" "$SERVER_PID" "SERVER_LISTEN status=ok sessions=1 " 10
	wait_for_exit "$SERVER_PID" 10
	stop_server

	if grep -q "SERVER_LISTEN status=error" "$SERVER_LOG"; then
		echo "Server status panel smoke failed ($label): server error" >&2
		cat "$SERVER_LOG" >&2
		exit 1
	fi
	if ! grep -q "sessions=1 commands=3 " "$SERVER_LOG"; then
		echo "Server status panel smoke failed ($label): expected hello/status/disconnect commands" >&2
		cat "$SERVER_LOG" >&2
		exit 1
	fi

	printf '%s\n' "$line"
}

FIRST_LINE="$(run_panel first)"
SECOND_LINE="$(run_panel second)"

if [[ "$FIRST_LINE" != "$SECOND_LINE" ]]; then
	echo "Server status panel smoke failed: runs differ" >&2
	echo "  first:  $FIRST_LINE" >&2
	echo "  second: $SECOND_LINE" >&2
	exit 1
fi

echo "SERVER_STATUS_PANEL_SMOKE status=ok runs=2 $FIRST_LINE"
