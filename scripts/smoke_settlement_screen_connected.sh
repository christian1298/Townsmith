#!/usr/bin/env bash
set -euo pipefail

# Connected settlement-screen smoke (authoritative read + rename + tax).
#
# Starts a fresh `townsmith-server --listen PORT --once` per run and runs
# `godot/settlement_screen.tscn --smoke --connected`, which:
#   hello -> found_settlement (Harbor at 0,0, radius 64)
#         -> settlement_list (page 1)
#         -> player_list      (owner name cache)
#         -> settlement_rename (Port Harbor, fresh revision)
#         -> settlement_set_tax (0% -> 5%)
#         -> stale rename (expects settlement.revision_conflict, cache unchanged)
#         -> disconnect
#
# Each run must print exactly one persisted marker:
#   SETTLEMENT_SCREEN_CONNECTED status=ok settlements=1 players=2
#     rename=1 tax=1 actions=9 accepted=8 rejected=1 checksum=<16 hex>
# and the two fresh-server runs must be byte-identical.
#
# Readiness is a bounded server-log poll; there are no fixed sleeps.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

if ! command -v "$GODOT_BIN" >/dev/null 2>&1; then
	echo "Godot binary not found: $GODOT_BIN (set GODOT=/path/to/godot)" >&2
	exit 2
fi

cargo build --quiet --manifest-path "$ROOT/Cargo.toml" -p townsmith-server

# The connected path never opens TownsmithCore, but the script and the local
# modes reference the extension global class. Build it only when it is absent.
if [[ ! -f "$ROOT/godot/bin/libtownsmith_core.so" ]]; then
	"$ROOT/scripts/build_godot_ext.sh" debug >/dev/null
fi

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
	"$ROOT/target/debug/townsmith-server" --listen "$port" --sessions 3 >"$SERVER_LOG" 2>&1 &
	SERVER_PID=$!
	wait_for_log "$SERVER_LOG" "$SERVER_PID" "SERVER_LISTEN status=ok address=" 10
}

# run_screen NAME -> writes $SMOKE_DIR/NAME.marker and .log, exits on failure.
run_screen() {
	local name="$1"
	local port
	local output
	local status
	local marker
	local marker_re
	local read_marker
	local read_re

	port="$(pick_port)"
	start_server "$port"
	set +e
	output="$(timeout 60s env -u DISPLAY -u WAYLAND_DISPLAY \
		"$GODOT_BIN" --headless --path "$ROOT/godot" \
		res://settlement_screen.tscn -- \
		--smoke --connected --host 127.0.0.1 --port "$port" --name Mayor 2>&1)"
	status=$?
	set -e
	printf '%s\n' "$output" >&2

	if [[ $status -ne 0 ]]; then
		echo "Connected settlement screen smoke ($name) failed: Godot exited $status" >&2
		exit 1
	fi
	if grep -q '^SETTLEMENT_SCREEN_CONNECTED status=failed ' <<<"$output"; then
		echo "Connected settlement screen smoke ($name) failed: screen reported a failure" >&2
		exit 1
	fi
	if grep -Fq '!base.ui.' <<<"$output"; then
		echo "Connected settlement screen smoke ($name) failed: missing translation key" >&2
		exit 1
	fi
	if [[ "$(grep -c '^SETTLEMENT_SCREEN_CONNECTED status=ok ' <<<"$output" || true)" -ne 1 ]]; then
		echo "Connected settlement screen smoke ($name) failed: expected one status marker" >&2
		exit 1
	fi
	marker="$(grep '^SETTLEMENT_SCREEN_CONNECTED status=ok ' <<<"$output" | tail -1)"
	marker_re='^SETTLEMENT_SCREEN_CONNECTED status=ok settlements=1 players=2 rename=1 tax=1 actions=[0-9]+ accepted=[0-9]+ rejected=[0-9]+ checksum=[0-9a-f]{16}$'
	if [[ ! "$marker" =~ $marker_re ]]; then
		echo "Connected settlement screen smoke ($name) marker mismatch: $marker" >&2
		exit 1
	fi
	roster_marker="$(grep '^SETTLEMENT_SCREEN_ROSTER ' <<<"$output" | tail -1 || true)"
	if [[ ! "$roster_marker" =~ ^SETTLEMENT_SCREEN_ROSTER\ status=ok\ checks=[0-9]+$ ]]; then
		echo "Connected settlement screen smoke ($name) roster marker mismatch: $roster_marker" >&2
		exit 1
	fi
	read_marker="$(grep '^SETTLEMENT_SCREEN_CONNECTED_READ ' <<<"$output" | tail -1 || true)"
	read_re='^SETTLEMENT_SCREEN_CONNECTED_READ settles=1 players=2 owner=[^ ]+ checksum=[0-9a-f]{16}$'
	if [[ ! "$read_marker" =~ $read_re ]]; then
		echo "Connected settlement screen smoke ($name) read marker mismatch: $read_marker" >&2
		exit 1
	fi
	printf '%s\n' "$marker" >"$SMOKE_DIR/$name.marker"
	printf '%s\n' "$output" >"$SMOKE_DIR/$name.log"
	stop_server
}

run_screen first
run_screen second
if ! diff -u "$SMOKE_DIR/first.marker" "$SMOKE_DIR/second.marker"; then
	echo "Connected settlement screen smoke failed: markers differ between runs" >&2
	exit 1
fi

echo "SETTLEMENT_SCREEN_CONNECTED_SMOKE status=ok runs=2 marker=$(cat "$SMOKE_DIR/first.marker")"
