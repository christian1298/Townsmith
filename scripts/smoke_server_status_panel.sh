#!/usr/bin/env bash
set -euo pipefail

# Read-only server-status panel smoke: Rust server `--listen` <-> Godot panel.
#
# Valid path: starts a fresh `townsmith-server --once` on an ephemeral loopback
# port, waits for `SERVER_LISTEN`, runs the headless status panel, and asserts
# the `SERVER_STATUS_PANEL status=ok ...` marker. The same command runs against
# a second fresh server; both markers must match byte for byte.
#
# Watch success path: a fresh server serves one connection for three
# `server_status` requests spaced by a short interval. The aggregate
# `SERVER_STATUS_WATCH status=ok iterations=3 ...` marker must be identical on a
# second fresh server, and the server reports five commands (hello, three
# status requests, disconnect). One iteration is checked separately so the
# marker count cannot drift. A stub with advancing tick/checksum values proves
# the aggregate accepts forward movement and records tick_first/tick_last.
#
# Watch failure matrix (each case runs twice; the whole line must match):
#   - EOF after one answer           -> status.closed iteration=2
#   - EOF before the first answer    -> status.closed iteration=1
#   - invalid JSON frame             -> status.frame  iteration=1
#   - length prefix + partial body   -> status.frame  iteration=1
#   - status without `tick`          -> status.shape  iteration=1
#   - status with a string `tick`    -> status.shape  iteration=1
#   - welcome then silence           -> status.watch_timeout iteration=1
#   - hello then silence             -> status.connect iteration=1
#   - server process exits after one -> status.closed iteration=2
#   - tick moves backwards           -> status.watch_order iteration=2
# Watch never reconnects: every stub asserts exactly one accept and one socket.
#
# Real failure variants:
#   - closed loopback port           -> status.connect (one-shot + watch)
#   - server pinned to another hash  -> content_hash.mismatch (one-shot + watch)
#   - real server SIGKILLed after one status answer -> status.closed iteration=2,
#     with exit status 137 observed for the server process
#   - malformed fixture              -> status.frame
#   - bad --iterations/--interval-ms -> status.watch_args iteration=0, exit 1
#
# Fault paths (no fixed sleeps, all bounded):
#   - fixtures with a missing/wrong-typed field -> status.shape
#   - a fixture with an unknown field -> tolerated (status=ok)
#   - a fixture with valid zero counters -> status=ok tick=0 population=0 ...
#
# Readiness and completion are bounded polls of the server/stub log. The
# server, stub, and kill proxy are all killed on exit.

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
# Polls the log until NEEDLE appears. Fails as soon as the process is gone
# without writing it, so a bind failure or early exit is reported instead of
# waiting out the deadline.
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
        print(f"process exited before reporting `{needle}`", file=sys.stderr)
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
print("timed out waiting for the process to exit", file=sys.stderr)
sys.exit(1)
PY
}

fail() {
	echo "Server status panel smoke failed ($1): $2" >&2
	exit 1
}

SERVER_LOG="$(mktemp)"
STUB_LOG="$(mktemp)"
PROXY_LOG="$(mktemp)"
FIXTURE_DIR="$(mktemp -d)"
SERVER_PID=""
STUB_PID=""
PROXY_PID=""
OUTPUT=""
GODOT_STATUS=0
LAST_WATCH_LINE=""

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

stop_proxy() {
	if [[ -n "$PROXY_PID" ]] && kill -0 "$PROXY_PID" 2>/dev/null; then
		kill "$PROXY_PID" 2>/dev/null || true
	fi
	if [[ -n "$PROXY_PID" ]]; then
		wait "$PROXY_PID" 2>/dev/null || true
	fi
	PROXY_PID=""
}

cleanup() {
	stop_server
	stop_stub
	stop_proxy
	rm -f "$SERVER_LOG"
	rm -f "$STUB_LOG"
	rm -f "$PROXY_LOG"
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

# start_stub MODE PORT
#
# Minimal protocol-v0 peer. MODE chooses the fault:
#   close-after-one, close-before-answer, advance, malformed-json,
#   partial-frame, missing-field, wrong-type, silent-status, silent-hello,
#   exit-after-one, regress.
# Every mode accepts exactly one connection and closes the listener, so a panel
# that reconnected would fail anyway. Completion is logged as
# `STUB_DONE mode=<mode> requests=<n> accepts=1`.
start_stub() {
	local mode="$1"
	local port="$2"
	: >"$STUB_LOG"
	python3 - "$port" "$mode" >"$STUB_LOG" 2>&1 <<'PY' &
import json
import os
import socket
import struct
import sys

port = int(sys.argv[1])
mode = sys.argv[2]
requests = 0

BASE_STATUS = {
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
}


def send(conn, obj):
    payload = json.dumps(obj).encode("utf-8")
    conn.sendall(struct.pack(">I", len(payload)) + payload)


def send_raw(conn, payload):
    conn.sendall(struct.pack(">I", len(payload)) + payload)


def recv_exact(conn, size):
    data = b""
    while len(data) < size:
        chunk = conn.recv(size - len(data))
        if not chunk:
            return None
        data += chunk
    return data


def read_frame(conn):
    header = recv_exact(conn, 4)
    if header is None:
        return None
    (length,) = struct.unpack(">I", header)
    if length == 0 or length > 1024 * 1024:
        return None
    body = recv_exact(conn, length)
    if body is None:
        return None
    try:
        return json.loads(body)
    except json.JSONDecodeError:
        return None


def make_status(tick=7, checksum="0123456789abcdef", raw_tick=None):
    status = dict(BASE_STATUS)
    status["tick"] = tick
    status["checksum"] = checksum
    if raw_tick is not None:
        status["tick"] = raw_tick
    return status


def wait_for_close(conn):
    conn.settimeout(10.0)
    try:
        while True:
            chunk = conn.recv(4096)
            if not chunk:
                return
    except OSError:
        return


def done():
    print(f"STUB_DONE mode={mode} requests={requests} accepts=1", flush=True)


def error(message):
    print(f"STUB_ERROR mode={mode} error={message}", flush=True)
    sys.exit(2)


listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
listener.bind(("127.0.0.1", port))
listener.listen(1)
print(f"STUB_LISTEN status=ok mode={mode}", flush=True)

conn, _ = listener.accept()
listener.close()
print("STUB_ACCEPT accepts=1", flush=True)
conn.settimeout(10.0)

hello = read_frame(conn)
if hello is None or hello.get("type") != "hello":
    error("expected=hello")

if mode == "silent-hello":
    wait_for_close(conn)
    conn.close()
    done()
    sys.exit(0)

send(conn, {
    "type": "welcome",
    "protocol_version": 1,
    "session_id": "stub-session",
    "resumed": False,
})

if mode == "advance":
    for tick in (7, 8, 9):
        request = read_frame(conn)
        if request is None or request.get("type") != "server_status":
            error("expected=server_status")
        requests += 1
        send(conn, make_status(tick=tick, checksum=f"{tick:016x}"))
    request = read_frame(conn)
    if request is None or request.get("type") != "disconnect":
        error("expected=disconnect")
    send(conn, {"type": "goodbye", "reason": "client_disconnect"})
    conn.close()
    done()
    sys.exit(0)

request = read_frame(conn)
if request is None or request.get("type") != "server_status":
    error("expected=server_status")
requests = 1

if mode == "close-before-answer":
    conn.close()
    done()
    sys.exit(0)

if mode == "malformed-json":
    send_raw(conn, b'{"type":')
    wait_for_close(conn)
    conn.close()
    done()
    sys.exit(0)

if mode == "partial-frame":
    # A complete length prefix followed by only part of the declared body.
    conn.sendall(struct.pack(">I", 64) + b'{"type":')
    conn.close()
    done()
    sys.exit(0)

if mode == "missing-field":
    status = make_status()
    status.pop("tick")
    send(conn, status)
    wait_for_close(conn)
    conn.close()
    done()
    sys.exit(0)

if mode == "wrong-type":
    send(conn, make_status(raw_tick="7"))
    wait_for_close(conn)
    conn.close()
    done()
    sys.exit(0)

if mode == "silent-status":
    wait_for_close(conn)
    conn.close()
    done()
    sys.exit(0)

if mode == "close-after-one":
    send(conn, make_status())
    conn.close()
    done()
    sys.exit(0)

if mode == "exit-after-one":
    send(conn, make_status())
    done()
    os._exit(0)

if mode == "regress":
    send(conn, make_status(tick=7, checksum="0000000000000007"))
    request = read_frame(conn)
    if request is None or request.get("type") != "server_status":
        error("expected=server_status")
    requests += 1
    send(conn, make_status(tick=6, checksum="0000000000000006"))
    wait_for_close(conn)
    conn.close()
    done()
    sys.exit(0)

error(f"unknown mode {mode}")
PY
	STUB_PID=$!
	wait_for_log "$STUB_LOG" "$STUB_PID" "STUB_LISTEN status=ok" 10
}

# start_kill_proxy PORT UPSTREAM_PORT SERVER_PID
#
# Relays one panel connection to the real server and SIGKILLs that server
# immediately after the first `server_status` answer is relayed. The panel then
# sees the server disappear between iterations.
start_kill_proxy() {
	local port="$1"
	local upstream_port="$2"
	local server_pid="$3"
	: >"$PROXY_LOG"
	python3 - "$port" "$upstream_port" "$server_pid" >"$PROXY_LOG" 2>&1 <<'PY' &
import json
import os
import select
import signal
import socket
import struct
import sys

listen_port = int(sys.argv[1])
upstream_port = int(sys.argv[2])
server_pid = int(sys.argv[3])
MAX_FRAME = 1024 * 1024


def parse_frames(buffer):
    frames = []
    while len(buffer) >= 4:
        (length,) = struct.unpack(">I", buffer[:4])
        if length == 0 or length > MAX_FRAME:
            frames.append(None)
            buffer = buffer[4:]
            continue
        if len(buffer) < 4 + length:
            break
        body = buffer[4:4 + length]
        buffer = buffer[4 + length:]
        try:
            frames.append(json.loads(body))
        except json.JSONDecodeError:
            frames.append(None)
    return frames, buffer


listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
listener.bind(("127.0.0.1", listen_port))
listener.listen(1)
print("KILL_PROXY_LISTEN status=ok", flush=True)

client, _ = listener.accept()
listener.close()
upstream = socket.create_connection(("127.0.0.1", upstream_port), timeout=10)
client.settimeout(10)
upstream.settimeout(10)
print("KILL_PROXY_ACCEPT accepts=1", flush=True)

to_server = b""
to_client = b""
requests = 0

while True:
    readable, _, _ = select.select([client, upstream], [], [], 10)
    if not readable:
        print("KILL_PROXY error=timeout", flush=True)
        sys.exit(2)
    for source in readable:
        try:
            data = source.recv(65536)
        except OSError:
            print("KILL_PROXY status=error code=recv", flush=True)
            sys.exit(2)
        if not data:
            print("KILL_PROXY status=error code=closed", flush=True)
            sys.exit(2)
        if source is client:
            upstream.sendall(data)
            to_server += data
            frames, to_server = parse_frames(to_server)
            for frame in frames:
                if isinstance(frame, dict) and frame.get("type") == "server_status":
                    requests += 1
        else:
            client.sendall(data)
            to_client += data
            frames, to_client = parse_frames(to_client)
            for frame in frames:
                if isinstance(frame, dict) and frame.get("type") == "server_status":
                    os.kill(server_pid, signal.SIGKILL)
                    print(
                        "KILL_PROXY_TRIGGER status=ok"
                        f" status_frames=1 requests={requests}",
                        flush=True,
                    )
                    client.close()
                    upstream.close()
                    sys.exit(0)
PY
	PROXY_PID=$!
	wait_for_log "$PROXY_LOG" "$PROXY_PID" "KILL_PROXY_LISTEN status=ok" 10
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

# expect_watch_fail LABEL EXPECTED [panel args...]
#
# Runs a smoke watch without adding --watch. Asserts exit 1 and an exact fail
# marker, then stores the line in LAST_WATCH_LINE.
expect_watch_fail() {
	local label="$1"
	local expected="$2"
	shift 2
	run_godot "$label" --smoke --watch "$@"
	if [[ $GODOT_STATUS -ne 1 ]]; then
		fail "$label" "Godot exited with status $GODOT_STATUS, expected 1"
	fi
	local line
	line="$(watch_marker_line)"
	if [[ "$line" != "$expected" ]]; then
		fail "$label" "expected '$expected', got: ${line:-<no marker>}"
	fi
	LAST_WATCH_LINE="$line"
	printf '%s\n' "$line"
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
# One fresh server per run. Asserts the aggregate marker, that the tick never
# moves backwards, and hello + N status requests + disconnect on one session.
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
	if [[ -z "$first" || -z "$last" || "$last" -lt "$first" ]]; then
		fail "$label" "tick moved backwards during watch: first=$first last=$last"
	fi

	wait_for_log "$SERVER_LOG" "$SERVER_PID" "SERVER_LISTEN status=ok sessions=1 " 10
	wait_for_exit "$SERVER_PID" 10
	stop_server

	local expected_commands=$((iterations + 2))
	if grep -q "SERVER_LISTEN status=error" "$SERVER_LOG"; then
		fail "$label" "server error"
	fi
	if ! grep -q "sessions=1 commands=$expected_commands " "$SERVER_LOG"; then
		fail "$label" "expected hello + $iterations status + disconnect commands"
	fi

	printf '%s\n' "$line"
}

# run_watch_stub_case MODE EXPECTED_MARKER
#
# Starts one fault stub, asserts the exact watch failure, then asserts the stub
# saw exactly one accept and finished on its own. The marker is stored in
# LAST_WATCH_LINE; no command substitution is used so cleanup PIDs stay valid.
run_watch_stub_case() {
	local mode="$1"
	local expected="$2"
	local port
	port="$(pick_port)"

	start_stub "$mode" "$port"
	expect_watch_fail "watch-$mode" "$expected" \
		--iterations 3 --interval-ms 50 --host 127.0.0.1 --port "$port" \
		--name status-panel --timeout-ms 500

	wait_for_log "$STUB_LOG" "$STUB_PID" "STUB_DONE mode=$mode " 10
	wait_for_exit "$STUB_PID" 10
	stop_stub
	if ! grep -q "STUB_DONE mode=$mode requests=[0-9]* accepts=1" "$STUB_LOG"; then
		fail "watch-$mode" "stub did not finish with one accept"
	fi
	if grep -q "STUB_ERROR" "$STUB_LOG"; then
		fail "watch-$mode" "stub protocol error"
	fi
	printf '%s\n' "$LAST_WATCH_LINE"
}

# run_watch_advance LABEL
#
# The stub advances tick 7 -> 9 over three polls and closes with a goodbye.
# Proves the aggregate accepts forward movement and records both ends.
run_watch_advance() {
	local label="$1"
	local port
	port="$(pick_port)"

	start_stub advance "$port"
	run_godot "$label" --smoke --watch --iterations 3 --interval-ms 50 \
		--host 127.0.0.1 --port "$port" --name status-panel --timeout-ms 500
	if [[ $GODOT_STATUS -ne 0 ]]; then
		fail "$label" "Godot exited with status $GODOT_STATUS"
	fi
	local line
	line="$(watch_marker_line)"
	if [[ ! "$line" =~ ^SERVER_STATUS_WATCH\ status=ok\ iterations=3\ checksum=[0-9a-f]{16}\ tick_first=7\ tick_last=9$ ]]; then
		fail "$label" "bad advancing marker: ${line:-<no marker>}"
	fi
	wait_for_log "$STUB_LOG" "$STUB_PID" "STUB_DONE mode=advance requests=3 accepts=1" 10
	wait_for_exit "$STUB_PID" 10
	stop_stub
	LAST_WATCH_LINE="$line"
	printf '%s\n' "$line"
}

# run_watch_pin LABEL
#
# A real server pins a different content hash; watch mode must report the
# server's reject at iteration 1 and exit 1 without reconnecting.
run_watch_pin() {
	local label="$1"
	local port
	local pin_hash
	pin_hash="$(python3 -c 'print("f" * 64)')"
	port="$(pick_port)"

	start_server "$port" --content-hash "$pin_hash"
	expect_watch_fail "$label" \
		"SERVER_STATUS_WATCH status=fail iteration=1 code=content_hash.mismatch" \
		--iterations 3 --interval-ms 50 --host 127.0.0.1 --port "$port" \
		--name status-panel --timeout-ms 5000
	stop_server
	printf '%s\n' "$LAST_WATCH_LINE"
}

# run_watch_kill LABEL
#
# The real server is SIGKILLed through a relay immediately after the first
# status answer. The panel must fail at iteration 2 without reconnecting, and
# the server's exit status must prove it was killed rather than shut down.
run_watch_kill() {
	local label="$1"
	local upstream_port
	local client_port
	upstream_port="$(pick_port)"
	client_port="$(pick_port)"

	start_server "$upstream_port"
	local server_pid="$SERVER_PID"
	start_kill_proxy "$client_port" "$upstream_port" "$server_pid"

	expect_watch_fail "$label" \
		"SERVER_STATUS_WATCH status=fail iteration=2 code=status.closed" \
		--iterations 5 --interval-ms 500 --host 127.0.0.1 --port "$client_port" \
		--name status-panel --timeout-ms 2000

	wait_for_log "$PROXY_LOG" "$PROXY_PID" "KILL_PROXY_TRIGGER status=ok" 10
	wait_for_exit "$PROXY_PID" 10
	stop_proxy
	if grep -q "KILL_PROXY status=error" "$PROXY_LOG"; then
		fail "$label" "kill proxy error"
	fi
	if kill -0 "$server_pid" 2>/dev/null; then
		fail "$label" "server survived the SIGKILL"
	fi
	set +e
	wait "$server_pid"
	local server_status=$?
	set -e
	SERVER_PID=""
	if [[ $server_status -ne 137 ]]; then
		fail "$label" "expected server exit 137 from SIGKILL, got $server_status"
	fi
	printf '%s\n' "$LAST_WATCH_LINE"
}

# run_watch_args LABEL EXPECTED [panel args...]
run_watch_args() {
	local label="$1"
	local expected="$2"
	shift 2
	expect_watch_fail "$label" "$expected" "$@"
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

(root / "malformed.json").write_text('{"type": "server_status",\n', encoding="utf-8")
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

# Exactly one iteration must report iterations=1, not 2 or 3.
WATCH_SINGLE_LINE="$(run_watch watch-single 1 50)"

# A server that advances the tick is accepted and both ends are reported.
WATCH_ADVANCE_FIRST="$(run_watch_advance watch-advance-first)"
WATCH_ADVANCE_SECOND="$(run_watch_advance watch-advance-second)"

if [[ "$WATCH_ADVANCE_FIRST" != "$WATCH_ADVANCE_SECOND" ]]; then
	echo "Server status panel smoke failed: watch advance runs differ" >&2
	echo "  first:  $WATCH_ADVANCE_FIRST" >&2
	echo "  second: $WATCH_ADVANCE_SECOND" >&2
	exit 1
fi

# Watch failure matrix: each fake mode runs twice and must print the same line.
# The loop keeps PIDs in the parent shell so a failing case is still cleaned up.
for case in \
	"close-after-one|SERVER_STATUS_WATCH status=fail iteration=2 code=status.closed" \
	"close-before-answer|SERVER_STATUS_WATCH status=fail iteration=1 code=status.closed" \
	"malformed-json|SERVER_STATUS_WATCH status=fail iteration=1 code=status.frame" \
	"partial-frame|SERVER_STATUS_WATCH status=fail iteration=1 code=status.frame" \
	"missing-field|SERVER_STATUS_WATCH status=fail iteration=1 code=status.shape" \
	"wrong-type|SERVER_STATUS_WATCH status=fail iteration=1 code=status.shape" \
	"silent-status|SERVER_STATUS_WATCH status=fail iteration=1 code=status.watch_timeout" \
	"silent-hello|SERVER_STATUS_WATCH status=fail iteration=1 code=status.connect" \
	"exit-after-one|SERVER_STATUS_WATCH status=fail iteration=2 code=status.closed" \
	"regress|SERVER_STATUS_WATCH status=fail iteration=2 code=status.watch_order"; do
	mode="${case%%|*}"
	expected="${case#*|}"
	run_watch_stub_case "$mode" "$expected" >/dev/null
	first="$LAST_WATCH_LINE"
	run_watch_stub_case "$mode" "$expected" >/dev/null
	second="$LAST_WATCH_LINE"
	if [[ "$first" != "$second" ]]; then
		echo "Server status panel smoke failed: watch-$mode runs differ" >&2
		echo "  first:  $first" >&2
		echo "  second: $second" >&2
		exit 1
	fi
done
WATCH_FAULT_FIRST_LINE="SERVER_STATUS_WATCH status=fail iteration=2 code=status.closed"

# The real server is killed after one answer; run it twice for a stable line.
WATCH_KILL_FIRST="$(run_watch_kill watch-kill-first)"
WATCH_KILL_SECOND="$(run_watch_kill watch-kill-second)"

if [[ "$WATCH_KILL_FIRST" != "$WATCH_KILL_SECOND" ]]; then
	echo "Server status panel smoke failed: watch kill runs differ" >&2
	echo "  first:  $WATCH_KILL_FIRST" >&2
	echo "  second: $WATCH_KILL_SECOND" >&2
	exit 1
fi

# Content-hash reject in watch mode: the server's stable code, iteration 1.
WATCH_PIN_FIRST="$(run_watch_pin watch-pin-first)"
WATCH_PIN_SECOND="$(run_watch_pin watch-pin-second)"

if [[ "$WATCH_PIN_FIRST" != "$WATCH_PIN_SECOND" ]]; then
	echo "Server status panel smoke failed: watch pin runs differ" >&2
	echo "  first:  $WATCH_PIN_FIRST" >&2
	echo "  second: $WATCH_PIN_SECOND" >&2
	exit 1
fi

# Connection refused (or a connect timeout): the port was just released and
# nothing is listening on it. One-shot and watch share the stable code.
CLOSED_PORT="$(pick_port)"
expect_fail "closed-port" "status.connect" \
	--smoke --host 127.0.0.1 --port "$CLOSED_PORT" --timeout-ms 2000
expect_watch_fail "watch-closed-port" \
	"SERVER_STATUS_WATCH status=fail iteration=1 code=status.connect" \
	--iterations 3 --interval-ms 50 --host 127.0.0.1 --port "$CLOSED_PORT" \
	--timeout-ms 1000

# The server pins a different content hash, so the hello is rejected with the
# server's own stable code.
PIN_HASH="$(python3 -c 'print("f" * 64)')"
PIN_PORT="$(pick_port)"
start_server "$PIN_PORT" --content-hash "$PIN_HASH"
expect_fail "content-hash-pin" "content_hash.mismatch" \
	--smoke --host 127.0.0.1 --port "$PIN_PORT" --name status-panel --timeout-ms 5000
stop_server

# Bad watch arguments fail before a socket exists and report iteration 0.
ARGS_LINE="SERVER_STATUS_WATCH status=fail iteration=0 code=status.watch_args"
run_watch_args "args-iterations-zero" "$ARGS_LINE arg=--iterations" \
	--iterations 0 --host 127.0.0.1 --port 1
run_watch_args "args-iterations-negative" "$ARGS_LINE arg=--iterations" \
	--iterations -1 --host 127.0.0.1 --port 1
run_watch_args "args-iterations-text" "$ARGS_LINE arg=--iterations" \
	--iterations abc --host 127.0.0.1 --port 1
run_watch_args "args-iterations-high" "$ARGS_LINE arg=--iterations" \
	--iterations 101 --host 127.0.0.1 --port 1
run_watch_args "args-interval-zero" "$ARGS_LINE arg=--interval-ms" \
	--interval-ms 0 --host 127.0.0.1 --port 1
run_watch_args "args-interval-low" "$ARGS_LINE arg=--interval-ms" \
	--interval-ms 49 --host 127.0.0.1 --port 1
run_watch_args "args-interval-high" "$ARGS_LINE arg=--interval-ms" \
	--interval-ms 5001 --host 127.0.0.1 --port 1
run_watch_args "args-interval-text" "$ARGS_LINE arg=--interval-ms" \
	--interval-ms later --host 127.0.0.1 --port 1

# Shape faults and forward-compatible extras are checked without a socket.
expect_fail "fixture-missing-tick" "status.shape" \
	--smoke --fixture-json "$FIXTURE_DIR/missing_tick.json"
expect_fail "fixture-wrong-type" "status.shape" \
	--smoke --fixture-json "$FIXTURE_DIR/wrong_type.json"
expect_fail "fixture-malformed" "status.frame" \
	--smoke --fixture-json "$FIXTURE_DIR/malformed.json"

# Zero values are valid and extras are ignored by the protocol; both must
# render the same authoritative marker.
ZERO_MARKER="SERVER_STATUS_PANEL status=ok tick=0 population=0 settlements=0 checksum=0000000000000000"
expect_fixture_ok "fixture-zero" "$FIXTURE_DIR/zero.json" "$ZERO_MARKER"
expect_fixture_ok "fixture-extra-field" "$FIXTURE_DIR/extra_field.json" "$ZERO_MARKER"

echo "SERVER_STATUS_PANEL_FAULTS status=ok connect=1 reject=1 shape=2 frame=1 fixture_ok=2"
echo "SERVER_STATUS_WATCH_SMOKE status=ok runs=2 $WATCH_FIRST_LINE"
echo "SERVER_STATUS_WATCH_SINGLE status=ok $WATCH_SINGLE_LINE"
echo "SERVER_STATUS_WATCH_ADVANCE status=ok runs=2 $WATCH_ADVANCE_FIRST"
echo "SERVER_STATUS_WATCH_FAULT status=ok runs=2 $WATCH_FAULT_FIRST_LINE"
echo "SERVER_STATUS_WATCH_FAULTS status=ok cases=10 runs=2"
echo "SERVER_STATUS_WATCH_KILL status=ok runs=2 $WATCH_KILL_FIRST"
echo "SERVER_STATUS_WATCH_PIN status=ok runs=2 $WATCH_PIN_FIRST"
echo "SERVER_STATUS_WATCH_ARGS status=ok cases=8 runs=1"
echo "SERVER_STATUS_PANEL_SMOKE status=ok runs=2 $FIRST_LINE"
