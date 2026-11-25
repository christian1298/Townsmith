#!/usr/bin/env bash
set -euo pipefail

# 3D multiplayer client chat smoke (`--chat`).
#
# Starts a fresh `townsmith-server --listen PORT --sessions 1` per run and runs
# `godot/mp_world_client.tscn --smoke --chat --chat-script ...`. The client
# sends hello, six deterministic place_road commands, a 96 m interest view,
# renders it, then skips the zero-view probe and runs the scripted chat
# sequence:
#   chat_since {after: 0} -> chat_log (empty on a fresh server)
#   chat {text: hello}    -> chat_sent #1
#   chat_since {after: 1} -> chat_log (the one own line, deduplicated)
#   chat {text: world}    -> chat_sent #2
#   disconnect            -> goodbye
#
# Each run must print:
#   MP_CHAT status=ok entries=2 checksum=<16 hex>
#   MP_WORLD_CLIENT status=ok ... (unchanged main marker)
#   MP_WORLD_CLIENT_DONE status=ok ... frames_sent=13 frames_received=14
# and must NOT print MP_WORLD_CLIENT_ZERO. Both runs must produce byte-identical
# markers. Because every run gets a fresh server, seq restarts at 1: entries=2
# proves the second run sees only its own chat, not an accumulated transcript.
#
# This is a debug render/networking integration, not a gameplay chat UI test,
# a benchmark gate, or the 1M render gate.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"
CHAT_SCRIPT="say:hello|poll|say:world"
# sha256("#1 mp-world-client: hello\n#2 mp-world-client: world")[:16]
CHAT_CHECKSUM="c8ea9cd6475d80e8"

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
	local chat_line
	local done_line
	local chat_checksum

	start_server "$port"

	set +e
	output="$(timeout 60s env -u DISPLAY -u WAYLAND_DISPLAY \
		"$GODOT_BIN" --headless --path "$ROOT/godot" res://mp_world_client.tscn -- \
		--smoke --chat --port "$port" --chat-script "$CHAT_SCRIPT" 2>&1)"
	godot_status=$?
	set -e
	printf '%s\n' "$output" >&2

	if [[ $godot_status -ne 0 ]]; then
		echo "chat smoke ($name) failed: Godot exited with status $godot_status" >&2
		exit 1
	fi

	main_line="$(grep '^MP_WORLD_CLIENT status=ok ' <<<"$output" | tail -1 || true)"
	chat_line="$(grep '^MP_CHAT status=ok ' <<<"$output" | tail -1 || true)"
	done_line="$(grep '^MP_WORLD_CLIENT_DONE status=ok ' <<<"$output" | tail -1 || true)"

	if grep -q '^MP_WORLD_CLIENT status=fail ' <<<"$output"; then
		echo "chat smoke ($name) failed: client reported a failure" >&2
		exit 1
	fi
	if grep -q '^MP_WORLD_CLIENT_ZERO ' <<<"$output"; then
		echo "chat smoke ($name) failed: chat mode ran the zero-view probe" >&2
		exit 1
	fi
	if [[ -z "$main_line" || -z "$chat_line" || -z "$done_line" ]]; then
		echo "chat smoke ($name) failed: missing main, chat, or done marker" >&2
		exit 1
	fi

	if [[ ! "$main_line" =~ ^MP_WORLD_CLIENT\ status=ok\ tick=6\ population=3277\ balance=100000\ roads=6\ buildings=4\ truncated=false\ buildings_truncated=false\ checksum=[0-9a-f]{16}\ road_checksum=48aff8a44b921666\ building_checksum=a98d781faa5c1d5b\ rendered_roads=6\ rendered_buildings=4\ center=0\.0,0\.0\ radius=96\.0\ session=session-[0-9a-f]{16}$ ]]; then
		echo "chat smoke ($name) main marker mismatch: $main_line" >&2
		exit 1
	fi
	if [[ ! "$chat_line" =~ ^MP_CHAT\ status=ok\ entries=2\ checksum=[0-9a-f]{16}$ ]]; then
		echo "chat smoke ($name) chat marker mismatch: $chat_line" >&2
		exit 1
	fi
	if [[ ! "$done_line" =~ ^MP_WORLD_CLIENT_DONE\ status=ok\ session=session-[0-9a-f]{16}\ tick=6\ frames_sent=13\ frames_received=14$ ]]; then
		echo "chat smoke ($name) done marker mismatch: $done_line" >&2
		exit 1
	fi

	chat_checksum="$(sed -n 's/.* checksum=\([0-9a-f]\{16\}\)$/\1/p' <<<"$chat_line")"
	if [[ "$chat_checksum" != "$CHAT_CHECKSUM" ]]; then
		echo "chat smoke ($name) failed: chat checksum '$chat_checksum' != '$CHAT_CHECKSUM'" >&2
		exit 1
	fi

	grep '^MP_WORLD_CLIENT ' <<<"$output" >"$prefix.txt"
	printf '%s\n' "$main_line" >"$prefix.main"
	printf '%s\n' "$chat_line" >"$prefix.chat"
	printf '%s\n' "$done_line" >"$prefix.done"
	printf '%s\n' "$chat_checksum" >"$prefix.checksum"

	# Thirteen accepted commands: hello, six roads, set_interest, chat_since,
	# chat, chat_since, chat, disconnect. Fourteen responses: welcome, six road
	# snapshots, snapshot, interest_view, two chat_logs, two chat_sents, goodbye.
	wait_for_log "$SERVER_LOG" "$SERVER_PID" "SERVER_LISTEN status=ok sessions=1 commands=13 " 10
	wait_for_exit "$SERVER_PID" 10
	stop_server
	if ! grep -Eq "SERVER_LISTEN status=ok sessions=1 commands=13 " "$SERVER_LOG"; then
		echo "chat smoke ($name) failed: server did not report one 13-command session" >&2
		cat "$SERVER_LOG" >&2
		exit 1
	fi
}

FIRST_PORT="$(pick_port "${MP_CHAT_PORT:-}")"
SECOND_PORT="$(pick_port "${MP_CHAT_SECOND_PORT:-}")"
for _ in 1 2 3 4 5; do
	[[ "$SECOND_PORT" != "$FIRST_PORT" ]] && break
	SECOND_PORT="$(pick_port "")"
done
if [[ "$SECOND_PORT" == "$FIRST_PORT" ]]; then
	echo "chat smoke failed: both runs selected port $FIRST_PORT" >&2
	exit 1
fi

run_block first "$FIRST_PORT" "$SMOKE_DIR/first"
run_block second "$SECOND_PORT" "$SMOKE_DIR/second"

for marker in main chat done; do
	if ! diff -u "$SMOKE_DIR/first.$marker" "$SMOKE_DIR/second.$marker"; then
		echo "chat smoke failed: $marker markers differ between runs" >&2
		exit 1
	fi
done
if ! diff -u "$SMOKE_DIR/first.txt" "$SMOKE_DIR/second.txt"; then
	echo "chat smoke failed: deterministic MP_WORLD_CLIENT lines differ between runs" >&2
	exit 1
fi
if [[ "$(cat "$SMOKE_DIR/first.checksum")" != "$(cat "$SMOKE_DIR/second.checksum")" ]]; then
	echo "chat smoke failed: chat checksums differ between runs" >&2
	exit 1
fi

echo "MP_CHAT_SMOKE status=ok runs=2 entries=2 checksum=$(cat "$SMOKE_DIR/first.checksum") world_checksum=$(sed -n 's/.* checksum=\([0-9a-f]\{16\}\).*/\1/p' "$SMOKE_DIR/first.main") frames_sent=13 frames_received=14"
