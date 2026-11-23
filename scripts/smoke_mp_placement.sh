#!/usr/bin/env bash
set -euo pipefail

# Server-authoritative placement smoke for the 3D multiplayer client (D-032).
#
# Writes a deterministic client fixture package into a temporary content-
# addressed library, starts a fresh `townsmith-server --listen PORT
# --sessions 1 --role mayor --blueprint-library LIB` per run, and runs
# `godot/mp_world_client.tscn --smoke` with the fixture hash. The client sends
# hello, six roads, two set_interest viewports, place_blueprint, and disconnect;
# the server answers welcome, six road snapshots, two snapshots, two views, the
# placement result, and goodbye. Each run must print:
#   MP_PLACEMENT status=ok id=1 hash=blake3:<64 hex> \
#     placement_checksum=<16 hex> world_checksum=<16 hex> \
#     center=32.0,32.0 footprint=3.2,3.2 tick=7
#   MP_WORLD_CLIENT_DONE status=ok ... tick=7 frames_sent=11 frames_received=13
#
# The second run must produce identical placement/done markers and checksums.
# Readiness is a bounded poll of the server log; there are no fixed sleeps. This
# is a debug client integration, not a placement UI, prediction, upload/fetch,
# benchmark gate, or the 1M render gate.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"
FIXTURE_X="32"
FIXTURE_Z="32"

if ! command -v "$GODOT_BIN" >/dev/null 2>&1; then
	echo "Godot binary not found: $GODOT_BIN (set GODOT=/path/to/godot)" >&2
	exit 2
fi

cargo build --quiet --manifest-path "$ROOT/Cargo.toml" -p townsmith-server

# Warm the Godot import cache once so both measured runs have identical output.
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
FIXTURE_DIR="$SMOKE_DIR/library"

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

FIXTURE_LINE="$("$ROOT/target/debug/townsmith-server" --placement-fixture "$FIXTURE_DIR")"
if [[ ! "$FIXTURE_LINE" =~ ^SERVER_PLACEMENT_FIXTURE\ hash=blake3:[0-9a-f]{64}\ status=ok$ ]]; then
	echo "3D placement smoke failed: fixture marker mismatch: $FIXTURE_LINE" >&2
	exit 1
fi
PLACE_HASH="${FIXTURE_LINE#SERVER_PLACEMENT_FIXTURE hash=}"
PLACE_HASH="${PLACE_HASH% status=ok}"
if [[ ! "$PLACE_HASH" =~ ^blake3:[0-9a-f]{64}$ ]]; then
	echo "3D placement smoke failed: parsed fixture hash is invalid: $PLACE_HASH" >&2
	exit 1
fi

start_server() {
	local port="$1"
	: >"$SERVER_LOG"
	"$ROOT/target/debug/townsmith-server" --listen "$port" --sessions 1 --role mayor \
		--blueprint-library "$FIXTURE_DIR" >"$SERVER_LOG" 2>&1 &
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
	local placement_line
	local done_line
	local placed_hash
	local placement_checksum
	local world_checksum

	start_server "$port"

	set +e
	output="$(timeout 60s env -u DISPLAY -u WAYLAND_DISPLAY \
		"$GODOT_BIN" --headless --path "$ROOT/godot" res://mp_world_client.tscn -- \
		--smoke --port "$port" --place-hash "$PLACE_HASH" \
		--place-x "$FIXTURE_X" --place-z "$FIXTURE_Z" 2>&1)"
	godot_status=$?
	set -e
	printf '%s\n' "$output" >&2

	if [[ $godot_status -ne 0 ]]; then
		echo "3D placement smoke ($name) failed: Godot exited with status $godot_status" >&2
		exit 1
	fi
	if grep -q '^MP_WORLD_CLIENT status=fail ' <<<"$output"; then
		echo "3D placement smoke ($name) failed: client reported a failure" >&2
		exit 1
	fi
	if grep -Eq '/(tmp|media)/' <<<"$output"; then
		echo "3D placement smoke ($name) failed: client output leaked a filesystem path" >&2
		exit 1
	fi

	placement_line="$(grep '^MP_PLACEMENT status=ok ' <<<"$output" | tail -1 || true)"
	done_line="$(grep '^MP_WORLD_CLIENT_DONE status=ok ' <<<"$output" | tail -1 || true)"
	if [[ -z "$placement_line" || -z "$done_line" ]]; then
		echo "3D placement smoke ($name) failed: missing placement or done marker" >&2
		exit 1
	fi

	if [[ ! "$placement_line" =~ ^MP_PLACEMENT\ status=ok\ id=1\ hash=blake3:[0-9a-f]{64}\ placement_checksum=[0-9a-f]{16}\ world_checksum=[0-9a-f]{16}\ center=32\.0,32\.0\ footprint=3\.2,3\.2\ tick=7$ ]]; then
		echo "3D placement smoke ($name) placement marker mismatch: $placement_line" >&2
		exit 1
	fi
	if [[ ! "$done_line" =~ ^MP_WORLD_CLIENT_DONE\ status=ok\ session=session-[0-9a-f]{16}\ tick=7\ frames_sent=11\ frames_received=13$ ]]; then
		echo "3D placement smoke ($name) done marker mismatch: $done_line" >&2
		exit 1
	fi

	placed_hash="$(sed -n 's/.* hash=\(blake3:[0-9a-f]\{64\}\).*/\1/p' <<<"$placement_line")"
	placement_checksum="$(sed -n 's/.* placement_checksum=\([0-9a-f]\{16\}\).*/\1/p' <<<"$placement_line")"
	world_checksum="$(sed -n 's/.* world_checksum=\([0-9a-f]\{16\}\).*/\1/p' <<<"$placement_line")"

	if [[ "$placed_hash" != "$PLACE_HASH" ]]; then
		echo "3D placement smoke ($name) failed: echoed hash '$placed_hash' does not match '$PLACE_HASH'" >&2
		exit 1
	fi
	if [[ ! "$placement_checksum" =~ ^[0-9a-f]{16}$ || ! "$world_checksum" =~ ^[0-9a-f]{16}$ ]]; then
		echo "3D placement smoke ($name) failed: missing 16-hex checksums" >&2
		exit 1
	fi

	printf '%s\n' "$placement_line" >"$prefix.placement"
	printf '%s\n' "$done_line" >"$prefix.done"
	printf '%s\n' "$placement_checksum" >"$prefix.placement_checksum"
	printf '%s\n' "$world_checksum" >"$prefix.world_checksum"

	# Eleven commands: hello, six roads, two set_interest, place_blueprint, and
	# disconnect. Thirteen responses: welcome, six road snapshots, two
	# snapshots, two interest views, the placement result, and goodbye.
	wait_for_log "$SERVER_LOG" "$SERVER_PID" "SERVER_LISTEN status=ok sessions=1 commands=11 " 10
	wait_for_exit "$SERVER_PID" 10
	stop_server
	if ! grep -Eq "SERVER_LISTEN status=ok sessions=1 commands=11 " "$SERVER_LOG"; then
		echo "3D placement smoke ($name) failed: server did not report one 11-command session" >&2
		cat "$SERVER_LOG" >&2
		exit 1
	fi
}

FIRST_PORT="$(pick_port "${MP_PLACEMENT_PORT:-}")"
SECOND_PORT="$(pick_port "${MP_PLACEMENT_SECOND_PORT:-}")"
for _ in 1 2 3 4 5; do
	[[ "$SECOND_PORT" != "$FIRST_PORT" ]] && break
	SECOND_PORT="$(pick_port "")"
done
if [[ "$SECOND_PORT" == "$FIRST_PORT" ]]; then
	echo "3D placement smoke failed: both runs selected port $FIRST_PORT" >&2
	exit 1
fi

run_block first "$FIRST_PORT" "$SMOKE_DIR/first"
run_block second "$SECOND_PORT" "$SMOKE_DIR/second"

for marker in placement done placement_checksum world_checksum; do
	if ! diff -u "$SMOKE_DIR/first.$marker" "$SMOKE_DIR/second.$marker"; then
		echo "3D placement smoke failed: $marker markers differ between runs" >&2
		exit 1
	fi
done

echo "MP_PLACEMENT_SMOKE status=ok runs=2 id=1 placement_checksum=$(cat "$SMOKE_DIR/first.placement_checksum") world_checksum=$(cat "$SMOKE_DIR/first.world_checksum") commands=11"
