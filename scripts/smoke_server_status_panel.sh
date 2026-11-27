#!/usr/bin/env bash
set -euo pipefail

# Read-only server-status panel smoke: Rust server `--listen` <-> Godot panel.
#
# Valid path: starts a fresh `townsmith-server --once` on an ephemeral loopback
# port, waits for `SERVER_LISTEN`, runs the headless status panel, and asserts
# the `SERVER_STATUS_PANEL status=ok ...` marker. The same command runs against
# a second fresh server; both markers must match byte for byte.
#
# Fault paths (no fixed sleeps, all bounded):
#   - a closed loopback port -> status.connect
#   - a server pinned to a different --content-hash -> content_hash.mismatch
#   - fixtures with a missing/wrong-typed field -> status.shape
#   - a fixture with an unknown field -> tolerated (status=ok)
#   - a fixture with valid zero counters -> status=ok tick=0 population=0 ...
#
# Readiness and completion are bounded polls of the server log. The server is
# killed on exit.

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

fail() {
	echo "Server status panel smoke failed ($1): $2" >&2
	exit 1
}

SERVER_LOG="$(mktemp)"
FIXTURE_DIR="$(mktemp -d)"
SERVER_PID=""
OUTPUT=""
GODOT_STATUS=0

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
	rm -rf "$FIXTURE_DIR"
}
trap cleanup EXIT

start_server() {
	local port="$1"
	shift
	: >"$SERVER_LOG"
	"$ROOT/target/debug/townsmith-server" --listen "$port" --once "$@" >"$SERVER_LOG" 2>&1 &
	SERVER_PID=$!
	wait_for_log "$SERVER_LOG" "$SERVER_PID" "SERVER_LISTEN status=ok address=" 10
}

# run_godot LABEL [panel args...]
#
# Captures stdout/stderr in OUTPUT and the exit status in GODOT_STATUS.
run_godot() {
	local label="$1"
	shift
	set +e
	OUTPUT="$(timeout 60s "$GODOT_BIN" --headless --path "$ROOT/godot" \
		res://server_status_panel.tscn -- "$@" 2>&1)"
	GODOT_STATUS=$?
	set -e
	printf '%s\n' "$OUTPUT" >&2
}

marker_line() {
	grep "^SERVER_STATUS_PANEL " <<<"$OUTPUT" | tail -1 || true
}

assert_ok_marker() {
	local label="$1"
	local line="$2"
	if [[ -z "$line" ]]; then
		fail "$label" "no SERVER_STATUS_PANEL marker"
	fi
	if [[ ! "$line" =~ ^SERVER_STATUS_PANEL\ status=ok\ tick=[0-9]+\ population=[0-9]+\ settlements=[0-9]+\ checksum=[0-9a-f]{16}$ ]]; then
		fail "$label" "bad marker: $line"
	fi
}

# expect_fail LABEL CODE [panel args...]
#
# A matching `--expect-fail` turns the panel failure into exit 0; the marker
# must still name the expected stable code.
expect_fail() {
	local label="$1"
	local expected="$2"
	shift 2
	run_godot "$label" --expect-fail "$expected" "$@"
	if [[ $GODOT_STATUS -ne 0 ]]; then
		fail "$label" "Godot exited with status $GODOT_STATUS, expected $expected"
	fi
	local line
	line="$(marker_line)"
	if [[ "$line" != "SERVER_STATUS_PANEL status=fail code=$expected" ]]; then
		fail "$label" "expected code=$expected, got: ${line:-<no marker>}"
	fi
}

# expect_fixture_ok LABEL PATH EXPECTED_MARKER
expect_fixture_ok() {
	local label="$1"
	local path="$2"
	local expected="$3"
	run_godot "$label" --smoke --fixture-json "$path"
	if [[ $GODOT_STATUS -ne 0 ]]; then
		fail "$label" "Godot exited with status $GODOT_STATUS"
	fi
	local line
	line="$(marker_line)"
	if [[ "$line" != "$expected" ]]; then
		fail "$label" "expected '$expected', got: ${line:-<no marker>}"
	fi
}

# Valid path: each run gets its own server process, port, and in-memory world,
# so a second run checks determinism rather than accumulating state.
run_panel() {
	local label="$1"
	local port
	port="$(pick_port)"

	start_server "$port"
	run_godot "$label" --smoke --host 127.0.0.1 --port "$port" \
		--name status-panel --timeout-ms 5000
	if [[ $GODOT_STATUS -ne 0 ]]; then
		fail "$label" "Godot exited with status $GODOT_STATUS"
	fi

	local line
	line="$(marker_line)"
	assert_ok_marker "$label" "$line"

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

python3 - "$FIXTURE_DIR" <<'PY'
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
base = {
    "type": "server_status",
    "tick": 0,
    "population": 0,
    "balance": 0,
    "roads": 0,
    "buildings": 0,
    "settlements": 0,
    "placements": 0,
    "online": 1,
    "chat_seq": 0,
    "locks": 0,
    "checksum": "0000000000000000",
}

def write(name, data):
    (root / name).write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")

write("zero.json", base)

missing = dict(base)
missing.pop("tick")
write("missing_tick.json", missing)

wrong_type = dict(base)
wrong_type["tick"] = "0"
write("wrong_type.json", wrong_type)

extra = dict(base)
extra["future_field"] = {"nested": True}
write("extra_field.json", extra)
PY

FIRST_LINE="$(run_panel first)"
SECOND_LINE="$(run_panel second)"

if [[ "$FIRST_LINE" != "$SECOND_LINE" ]]; then
	echo "Server status panel smoke failed: runs differ" >&2
	echo "  first:  $FIRST_LINE" >&2
	echo "  second: $SECOND_LINE" >&2
	exit 1
fi

# Connection refused (or a connect timeout): the port was just released and
# nothing is listening on it.
CLOSED_PORT="$(pick_port)"
expect_fail "closed-port" "status.connect" \
	--smoke --host 127.0.0.1 --port "$CLOSED_PORT" --timeout-ms 2000

# The server pins a different content hash, so the hello is rejected with the
# server's own stable code.
PIN_HASH="$(python3 -c 'print("f" * 64)')"
PIN_PORT="$(pick_port)"
start_server "$PIN_PORT" --content-hash "$PIN_HASH"
expect_fail "content-hash-pin" "content_hash.mismatch" \
	--smoke --host 127.0.0.1 --port "$PIN_PORT" --name status-panel --timeout-ms 5000
stop_server

# Shape faults and forward-compatible extras are checked without a socket.
expect_fail "fixture-missing-tick" "status.shape" \
	--smoke --fixture-json "$FIXTURE_DIR/missing_tick.json"
expect_fail "fixture-wrong-type" "status.shape" \
	--smoke --fixture-json "$FIXTURE_DIR/wrong_type.json"

# Zero values are valid and extras are ignored by the protocol; both must
# render the same authoritative marker.
ZERO_MARKER="SERVER_STATUS_PANEL status=ok tick=0 population=0 settlements=0 checksum=0000000000000000"
expect_fixture_ok "fixture-zero" "$FIXTURE_DIR/zero.json" "$ZERO_MARKER"
expect_fixture_ok "fixture-extra-field" "$FIXTURE_DIR/extra_field.json" "$ZERO_MARKER"

echo "SERVER_STATUS_PANEL_FAULTS status=ok connect=1 reject=1 shape=2 fixture_ok=2"
echo "SERVER_STATUS_PANEL_SMOKE status=ok runs=2 $FIRST_LINE"
