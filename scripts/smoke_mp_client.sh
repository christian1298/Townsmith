#!/usr/bin/env bash
set -euo pipefail

# Protocol v0 end-to-end smoke: Rust server `--listen` <-> Godot client.
#
# Block 1 starts the server on an ephemeral loopback port with `--once`, runs
# the headless Godot protocol probe, and asserts the MP_CLIENT marker plus the
# exchanged frame counts. Block 2 starts a fresh server with `--sessions 2`,
# runs the probe again with `--reconnect`, and asserts that the first session's
# token resumes on a second connection (`MP_CLIENT_RECONNECT ... resumed=true`).
# Readiness and completion are bounded polls of the server log; there are no
# fixed sleeps. The server is killed on exit.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

if ! command -v "$GODOT_BIN" >/dev/null 2>&1; then
	echo "Godot binary not found: $GODOT_BIN (set GODOT=/path/to/godot)" >&2
	exit 2
fi

# Pure-helper assertions (projection, bounded road parsing, checksum) run
# without a display or a server, so a broken preview fails before the socket
# exchange and is reported separately from the protocol contract.
set +e
HELPER_OUTPUT="$(timeout 30s "$GODOT_BIN" --headless --path "$ROOT/godot" res://mp_client.tscn -- \
	--self-test 2>&1)"
HELPER_STATUS=$?
set -e
printf '%s\n' "$HELPER_OUTPUT"
if [[ $HELPER_STATUS -ne 0 ]] || ! grep -q '^MP_INTEREST_HELPER status=ok ' <<<"$HELPER_OUTPUT"; then
	echo "Multiplayer interest helper self-test failed (status $HELPER_STATUS)" >&2
	exit 1
fi

cargo build --manifest-path "$ROOT/Cargo.toml" -p townsmith-server >/dev/null

# The bind-then-close probe can race with the server bind, but the readiness
# loop tolerates it; the port is not held open.
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

# start_server PORT [SERVER ARGS...]
#
# Readiness is the log line, not a TCP probe: a probe would consume one of the
# capped sessions.
start_server() {
	local port="$1"
	shift
	: >"$SERVER_LOG"
	"$ROOT/target/debug/townsmith-server" --listen "$port" "$@" >"$SERVER_LOG" 2>&1 &
	SERVER_PID=$!
	wait_for_log "$SERVER_LOG" "$SERVER_PID" "SERVER_LISTEN status=ok address=" 10
}

PORT="$(pick_port "${MP_CLIENT_PORT:-}")"

start_server "$PORT" --once

set +e
OUTPUT="$(timeout 60s "$GODOT_BIN" --headless --path "$ROOT/godot" res://mp_client.tscn -- \
	--smoke --port "$PORT" 2>&1)"
GODOT_STATUS=$?
set -e
printf '%s\n' "$OUTPUT"

if [[ $GODOT_STATUS -ne 0 ]]; then
	echo "Multiplayer client smoke failed: Godot exited with status $GODOT_STATUS" >&2
	exit 1
fi

LINE="$(grep "^MP_CLIENT " <<<"$OUTPUT" | tail -1 || true)"
if [[ -z "$LINE" ]]; then
	echo "Multiplayer client smoke failed: no MP_CLIENT marker" >&2
	exit 1
fi
if [[ "$LINE" != MP_CLIENT\ status=ok\ welcome=ok\ ping=ok\ snapshot=ok\ * ]]; then
	echo "Multiplayer client smoke contract mismatch: $LINE" >&2
	exit 1
fi

TICK="$(sed -n 's/.* tick=\([0-9][0-9]*\).*/\1/p' <<<"$LINE")"
POPULATION="$(sed -n 's/.* population=\([0-9][0-9]*\).*/\1/p' <<<"$LINE")"
CHECKSUM="$(sed -n 's/.* snapshot_checksum=\([0-9a-f]\{16\}\).*/\1/p' <<<"$LINE")"
INTEREST_CHECKSUM="$(sed -n 's/.* interest_render=ok roads=[0-9][0-9]* checksum=\([0-9a-f]\{16\}\).*/\1/p' <<<"$LINE")"
FRAMES_SENT="$(sed -n 's/.* frames_sent=\([0-9][0-9]*\).*/\1/p' <<<"$LINE")"
FRAMES_RECEIVED="$(sed -n 's/.* frames_received=\([0-9][0-9]*\).*/\1/p' <<<"$LINE")"

for FIELD in "reject=ok" "road=ok" "interest=ok" "roads=1" "truncated=false" \
	"interest_render=ok" "cleared=true"; do
	if [[ "$LINE" != *"$FIELD"* ]]; then
		echo "Multiplayer client smoke failed: missing $FIELD ($LINE)" >&2
		exit 1
	fi
done
if [[ -z "$TICK" ]] || (( TICK < 2 )); then
	echo "Multiplayer client smoke failed: bad tick '${TICK:-none}'" >&2
	exit 1
fi
if [[ -z "$POPULATION" ]] || (( POPULATION < 1 )); then
	echo "Multiplayer client smoke failed: bad population '${POPULATION:-none}'" >&2
	exit 1
fi
if [[ ! "$CHECKSUM" =~ ^[0-9a-f]{16}$ ]]; then
	echo "Multiplayer client smoke failed: bad snapshot checksum '${CHECKSUM:-none}'" >&2
	exit 1
fi
if [[ ! "$INTEREST_CHECKSUM" =~ ^[0-9a-f]{16}$ ]]; then
	echo "Multiplayer client smoke failed: bad interest render checksum '${INTEREST_CHECKSUM:-none}'" >&2
	exit 1
fi
if [[ "$LINE" != *"interest_render=ok roads=1 checksum=$INTEREST_CHECKSUM rendered=1 cleared=true"* ]]; then
	echo "Multiplayer client smoke failed: interest render contract ($LINE)" >&2
	exit 1
fi
if [[ "$FRAMES_SENT" != "7" || "$FRAMES_RECEIVED" != "8" ]]; then
	echo "Multiplayer client smoke failed: frames sent=${FRAMES_SENT:-none} received=${FRAMES_RECEIVED:-none}" >&2
	exit 1
fi
# The report is printed as the process exits, so wait for it before reaping.
wait_for_log "$SERVER_LOG" "$SERVER_PID" "SERVER_LISTEN status=ok sessions=1 commands=7 " 10
stop_server
if ! grep -Eq "SERVER_LISTEN status=ok sessions=1 commands=7 " "$SERVER_LOG"; then
	echo "Multiplayer client smoke failed: server did not report a 7-command, 15-frame session" >&2
	cat "$SERVER_LOG" >&2
	exit 1
fi

RECONNECT_PORT="$(pick_port "${MP_CLIENT_RECONNECT_PORT:-}")"
for _ in 1 2 3 4 5; do
	[[ "$RECONNECT_PORT" != "$PORT" ]] && break
	RECONNECT_PORT="$(pick_port "")"
done
if [[ "$RECONNECT_PORT" == "$PORT" ]]; then
	echo "Multiplayer client smoke failed: reconnect port matches the first port" >&2
	exit 1
fi

# Two sessions share one in-memory world: the first disconnects, the second
# resumes it with the token from the first `welcome`.
start_server "$RECONNECT_PORT" --sessions 2

set +e
RECONNECT_OUTPUT="$(timeout 60s "$GODOT_BIN" --headless --path "$ROOT/godot" res://mp_client.tscn -- \
	--smoke --reconnect --port "$RECONNECT_PORT" 2>&1)"
RECONNECT_STATUS=$?
set -e
printf '%s\n' "$RECONNECT_OUTPUT"

if [[ $RECONNECT_STATUS -ne 0 ]]; then
	echo "Multiplayer reconnect smoke failed: Godot exited with status $RECONNECT_STATUS" >&2
	exit 1
fi

RECONNECT_LINE="$(grep "^MP_CLIENT_RECONNECT " <<<"$RECONNECT_OUTPUT" | tail -1 || true)"
if [[ -z "$RECONNECT_LINE" ]]; then
	echo "Multiplayer reconnect smoke failed: no MP_CLIENT_RECONNECT marker" >&2
	exit 1
fi
if [[ "$RECONNECT_LINE" != MP_CLIENT_RECONNECT\ status=ok\ resumed=true\ * ]]; then
	echo "Multiplayer reconnect smoke contract mismatch: $RECONNECT_LINE" >&2
	exit 1
fi

RESUME_TOKEN="$(sed -n 's/.* token=\([^ ]*\).*/\1/p' <<<"$RECONNECT_LINE")"
RESUME_TICK="$(sed -n 's/.* tick=\([0-9][0-9]*\).*/\1/p' <<<"$RECONNECT_LINE")"
RESUME_CHECKSUM="$(sed -n 's/.* snapshot_checksum=\([0-9a-f]\{16\}\).*/\1/p' <<<"$RECONNECT_LINE")"
RESUME_FRAMES_SENT="$(sed -n 's/.* frames_sent=\([0-9][0-9]*\).*/\1/p' <<<"$RECONNECT_LINE")"
RESUME_FRAMES_RECEIVED="$(sed -n 's/.* frames_received=\([0-9][0-9]*\).*/\1/p' <<<"$RECONNECT_LINE")"

if [[ ! "$RESUME_TOKEN" =~ ^session-[0-9a-f]{16}$ ]]; then
	echo "Multiplayer reconnect smoke failed: bad resume token '${RESUME_TOKEN:-none}'" >&2
	exit 1
fi
if [[ -z "$RESUME_TICK" ]] || (( RESUME_TICK < 1 )); then
	echo "Multiplayer reconnect smoke failed: bad resumed tick '${RESUME_TICK:-none}'" >&2
	exit 1
fi
if [[ ! "$RESUME_CHECKSUM" =~ ^[0-9a-f]{16}$ ]]; then
	echo "Multiplayer reconnect smoke failed: bad resumed checksum '${RESUME_CHECKSUM:-none}'" >&2
	exit 1
fi
if [[ "$RESUME_FRAMES_SENT" != "7" || "$RESUME_FRAMES_RECEIVED" != "8" ]]; then
	echo "Multiplayer reconnect smoke failed: frames sent=${RESUME_FRAMES_SENT:-none} received=${RESUME_FRAMES_RECEIVED:-none}" >&2
	exit 1
fi
wait_for_log "$SERVER_LOG" "$SERVER_PID" "SERVER_LISTEN status=ok sessions=2 commands=7 " 10
wait_for_exit "$SERVER_PID" 10
stop_server
if ! grep -Eq "SERVER_LISTEN status=ok sessions=2 commands=7 " "$SERVER_LOG"; then
	echo "Multiplayer reconnect smoke failed: server did not serve two sessions with 7 commands" >&2
	cat "$SERVER_LOG" >&2
	exit 1
fi

echo "MP_CLIENT_SMOKE status=ok port=$PORT tick=$TICK population=$POPULATION checksum=$CHECKSUM interest_checksum=$INTEREST_CHECKSUM frames=15"
echo "MP_CLIENT_RECONNECT_SMOKE status=ok port=$RECONNECT_PORT token=$RESUME_TOKEN tick=$RESUME_TICK checksum=$RESUME_CHECKSUM frames=15"
