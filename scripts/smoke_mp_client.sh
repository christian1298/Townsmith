#!/usr/bin/env bash
set -euo pipefail

# Protocol v0 end-to-end smoke: Rust server `--listen` <-> Godot client.
#
# Starts the server on an ephemeral loopback port, retries the port connect
# with a deadline (no fixed startup sleep), runs the headless Godot client, and
# asserts the MP_CLIENT marker plus the exchanged frame counts. The server is
# killed on exit.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

if ! command -v "$GODOT_BIN" >/dev/null 2>&1; then
	echo "Godot binary not found: $GODOT_BIN (set GODOT=/path/to/godot)" >&2
	exit 2
fi

cargo build --manifest-path "$ROOT/Cargo.toml" -p townsmith-server >/dev/null

if [[ -n "${MP_CLIENT_PORT:-}" ]]; then
	PORT="$MP_CLIENT_PORT"
else
	PORT="$(python3 - <<'PY'
import socket

with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
    probe.bind(("127.0.0.1", 0))
    print(probe.getsockname()[1])
PY
)"
fi

SERVER_LOG="$(mktemp)"
SERVER_PID=""

cleanup() {
	if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
		kill "$SERVER_PID" 2>/dev/null || true
		wait "$SERVER_PID" 2>/dev/null || true
	fi
	rm -f "$SERVER_LOG"
}
trap cleanup EXIT

"$ROOT/target/debug/townsmith-server" --listen "$PORT" >"$SERVER_LOG" 2>&1 &
SERVER_PID=$!

# Bounded retry loop. The connect attempt is the readiness probe; the process
# check fails fast if the server exited instead of waiting for the deadline.
python3 - "$PORT" "$SERVER_PID" <<'PY'
import os
import socket
import sys
import time

port = int(sys.argv[1])
pid = int(sys.argv[2])
deadline = time.monotonic() + 10.0
while time.monotonic() < deadline:
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=0.25):
            sys.exit(0)
    except OSError:
        pass
    try:
        os.kill(pid, 0)
    except OSError:
        print("server exited before binding the port", file=sys.stderr)
        sys.exit(1)
    time.sleep(0.02)
print("timed out waiting for the server port", file=sys.stderr)
sys.exit(1)
PY

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
CHECKSUM="$(sed -n 's/.* checksum=\([0-9a-f]*\).*/\1/p' <<<"$LINE")"
FRAMES_SENT="$(sed -n 's/.* frames_sent=\([0-9][0-9]*\).*/\1/p' <<<"$LINE")"
FRAMES_RECEIVED="$(sed -n 's/.* frames_received=\([0-9][0-9]*\).*/\1/p' <<<"$LINE")"

for FIELD in "reject=ok" "road=ok"; do
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
	echo "Multiplayer client smoke failed: bad checksum '${CHECKSUM:-none}'" >&2
	exit 1
fi
if [[ "$FRAMES_SENT" != "6" || "$FRAMES_RECEIVED" != "6" ]]; then
	echo "Multiplayer client smoke failed: frames sent=${FRAMES_SENT:-none} received=${FRAMES_RECEIVED:-none}" >&2
	exit 1
fi
if ! grep -q "status=session frames=12" "$SERVER_LOG"; then
	echo "Multiplayer client smoke failed: server did not report a 12-frame session" >&2
	cat "$SERVER_LOG" >&2
	exit 1
fi

echo "MP_CLIENT_SMOKE status=ok port=$PORT tick=$TICK population=$POPULATION checksum=$CHECKSUM frames=12"
