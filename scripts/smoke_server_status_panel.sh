#!/usr/bin/env bash
set -euo pipefail

# Read-only server-status panel smoke: Rust server `--listen` <-> Godot panel.
#
# Valid path: starts a fresh `townsmith-server --once` on an ephemeral loopback
# port, waits for `SERVER_LISTEN`, runs the headless status panel, and asserts
# the `SERVER_STATUS_PANEL status=ok ...` marker. The same command runs against
# a second fresh server; both markers must match byte for byte.
#
# Watch path: a fresh server serves one connection for three `server_status`
# requests spaced by a short interval. The aggregate
# `SERVER_STATUS_WATCH status=ok iterations=3 ...` marker must be identical on a
# second fresh server, and the server reports five commands (hello, three
# status requests, disconnect). A stub loopback server answers exactly one
# status request and then closes; the panel must print
# `SERVER_STATUS_WATCH status=fail iteration=2 code=status.closed` and exit 1,
# identically on two runs. Watch never reconnects.
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
STUB_LOG="$(mktemp)"
FIXTURE_DIR="$(mktemp -d)"
SERVER_PID=""
STUB_PID=""
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

stop_stub() {
	if [[ -n "$STUB_PID" ]] && kill -0 "$STUB_PID" 2>/dev/null; then
		kill "$STUB_PID" 2>/dev/null || true
	fi
	if [[ -n "$STUB_PID" ]]; then
		wait "$STUB_PID" 2>/dev/null || true
	fi
	STUB_PID=""
}

cleanup() {
	stop_server
	stop_stub
	rm -f "$SERVER_LOG"
	rm -f "$STUB_LOG"
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

# start_stub PORT
#
# Minimal protocol-v0 peer: welcome, one valid server_status answer, then close
# without a goodbye. It exists only to prove the panel fails watch mode instead
# of reconnecting.
start_stub() {
	local port="$1"
	: >"$STUB_LOG"
	python3 - "$port" >"$STUB_LOG" 2>&1 <<'PY' &
import json
import socket
import struct
import sys

port = int(sys.argv[1])


def send(conn, obj):
    payload = json.dumps(obj).encode("utf-8")
    conn.sendall(struct.pack(">I", len(payload)) + payload)


def read_frame(conn):
    header = b""
    while len(header) < 4:
        chunk = conn.recv(4 - len(header))
        if not chunk:
            return None
        header += chunk
    (length,) = struct.unpack(">I", header)
    body = b""
    while len(body) < length:
        chunk = conn.recv(length - len(body))
        if not chunk:
            return None
        body += chunk
    return json.loads(body)


server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
server.bind(("127.0.0.1", port))
server.listen(1)
print("STUB_LISTEN status=ok", flush=True)

conn, _ = server.accept()
hello = read_frame(conn)
if hello is None or hello.get("type") != "hello":
    print("STUB_ERROR expected=hello", flush=True)
    sys.exit(2)
send(conn, {
    "type": "welcome",
    "protocol_version": 1,
    "session_id": "stub-session",
    "resumed": False,
})
status = read_frame(conn)
if status is None or status.get("type") != "server_status":
    print("STUB_ERROR expected=server_status", flush=True)
    sys.exit(2)
send(conn, {
    "type": "server_status",
    "tick": 7,
    "population": 11,
    "balance": 0,
    "roads": 2,
    "buildings": 3,
    "settlements": 1,
    "placements": 4,
    "online": 1,
    "chat_seq": 0,
    "locks": 0,
    "checksum": "0123456789abcdef",
})
conn.close()
server.close()
print("STUB_DONE requests=1", flush=True)
PY
	STUB_PID=$!
	wait_for_log "$STUB_LOG" "$STUB_PID" "STUB_LISTEN status=ok" 10
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

watch_marker_line() {
	grep "^SERVER_STATUS_WATCH " <<<"$OUTPUT" | tail -1 || true
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

# run_watch LABEL ITERATIONS INTERVAL_MS
#
# One fresh server per run. Asserts the aggregate marker, an unchanged tick
# across the requested polls, and the server-side command count (hello + three
# status requests + disconnect).
run_watch() {
	local label="$1"
	local iterations="$2"
	local interval_ms="$3"
	local port
	port="$(pick_port)"

	start_server "$port"
	run_godot "$label" --smoke --watch --iterations "$iterations" \
		--interval-ms "$interval_ms" --host 127.0.0.1 --port "$port" \
		--name status-panel --timeout-ms 5000
	if [[ $GODOT_STATUS -ne 0 ]]; then
		fail "$label" "Godot exited with status $GODOT_STATUS"
	fi

	local line
	line="$(watch_marker_line)"
	if [[ ! "$line" =~ ^SERVER_STATUS_WATCH\ status=ok\ iterations=$iterations\ checksum=[0-9a-f]{16}\ tick_first=[0-9]+\ tick_last=[0-9]+$ ]]; then
		fail "$label" "bad watch marker: ${line:-<no marker>}"
	fi
	local first=""
	local last=""
	if [[ "$line" =~ tick_first=([0-9]+)\ tick_last=([0-9]+) ]]; then
		first="${BASH_REMATCH[1]}"
		last="${BASH_REMATCH[2]}"
	fi
	if [[ -z "$first" || "$first" != "$last" ]]; then
		fail "$label" "tick moved during watch: first=$first last=$last"
	fi

	wait_for_log "$SERVER_LOG" "$SERVER_PID" "SERVER_LISTEN status=ok sessions=1 " 10
	wait_for_exit "$SERVER_PID" 10
	stop_server

	if grep -q "SERVER_LISTEN status=error" "$SERVER_LOG"; then
		fail "$label" "server error"
	fi
	if ! grep -q "sessions=1 commands=5 " "$SERVER_LOG"; then
		fail "$label" "expected hello + 3 status + disconnect commands"
	fi

	printf '%s\n' "$line"
}

# run_watch_fail LABEL
#
# The stub answers one status request and closes. The panel must report the
# failed iteration and exit 1; a reconnect would hit a closed listener and is
# not part of the contract.
run_watch_fail() {
	local label="$1"
	local port
	port="$(pick_port)"

	start_stub "$port"
	# A 500 ms interval guarantees the panel observes the peer's FIN before its
	# next request, so the stable code is status.closed, not a write race.
	run_godot "$label" --smoke --watch --iterations 3 --interval-ms 500 \
		--host 127.0.0.1 --port "$port" --name status-panel --timeout-ms 5000
	if [[ $GODOT_STATUS -ne 1 ]]; then
		fail "$label" "Godot exited with status $GODOT_STATUS, expected 1"
	fi

	local line
	line="$(watch_marker_line)"
	if [[ "$line" != "SERVER_STATUS_WATCH status=fail iteration=2 code=status.closed" ]]; then
		fail "$label" "expected stable watch failure, got: ${line:-<no marker>}"
	fi
	stop_stub
	if ! grep -q "STUB_DONE requests=1" "$STUB_LOG"; then
		fail "$label" "stub did not answer exactly one status request"
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

# Watch pass path: three polls over one connection, then a normal disconnect.
# Each run gets a fresh server, so an identical aggregate marker is a
# determinism check, not accumulated state.
WATCH_FIRST_LINE="$(run_watch watch-first 3 50)"
WATCH_SECOND_LINE="$(run_watch watch-second 3 50)"

if [[ "$WATCH_FIRST_LINE" != "$WATCH_SECOND_LINE" ]]; then
	echo "Server status panel smoke failed: watch runs differ" >&2
	echo "  first:  $WATCH_FIRST_LINE" >&2
	echo "  second: $WATCH_SECOND_LINE" >&2
	exit 1
fi

# Watch failure path: one good answer, then EOF. The panel reports iteration 2
# and exits 1 without reconnecting; the marker must be stable across runs.
WATCH_FAIL_FIRST_LINE="$(run_watch_fail watch-fail-first)"
WATCH_FAIL_SECOND_LINE="$(run_watch_fail watch-fail-second)"

if [[ "$WATCH_FAIL_FIRST_LINE" != "$WATCH_FAIL_SECOND_LINE" ]]; then
	echo "Server status panel smoke failed: watch failure runs differ" >&2
	echo "  first:  $WATCH_FAIL_FIRST_LINE" >&2
	echo "  second: $WATCH_FAIL_SECOND_LINE" >&2
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
echo "SERVER_STATUS_WATCH_SMOKE status=ok runs=2 $WATCH_FIRST_LINE"
echo "SERVER_STATUS_WATCH_FAULT status=ok runs=2 $WATCH_FAIL_FIRST_LINE"
echo "SERVER_STATUS_PANEL_SMOKE status=ok runs=2 $FIRST_LINE"
