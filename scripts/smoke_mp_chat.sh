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
#
# A third scenario proves the server push path on one fresh `--sessions 2`
# server. A passive listener (`--chat-listen --chat-expect push-me`) completes
# hello first and sends no chat commands. A sender then runs
# `say:push-me|poll`, so the listener must consume the pushed `chat_sent` while
# the sender still receives its own response exactly once (12 sent / 13
# received). The listener must print `MP_CHAT_PUSH` with the deterministic
# `seq|player|text` checksum, `MP_CHAT_UNREAD unread=1 total=1`, and
# `MP_CHAT_BADGE status=ok unread=1 panel=1 text=1 visible=true`; then poll
# (`chat_since` after the pushed seq), print `MP_CHAT_UNREAD unread=0 total=1`
# and `MP_CHAT_BADGE status=ok unread=0 panel=0 text=- visible=false`, and
# disconnect. The sender must never print an unread or badge marker: its own
# `chat_sent` response is not a push and its active flow stays at 0. Both
# sessions must end cleanly.
#
# A fourth scenario proves chat catch-up across a real resume. Two fresh
# `--sessions 2` servers each run `--chat --chat-reconnect`: the client sends
# one `before-resume` line, disconnects, resumes with the same token, drops its
# local transcript, and asks `chat_since {after: 0}`. The returned log must
# contain exactly that one pre-resume entry, merged once. Each run must print
# `MP_CHAT_RECONNECT status=ok entries=1 last_seq=1 token_reused=true
# checksum=<16 hex>` (no `MP_CHAT`, no unread/push markers) and end at 13 sent
# / 15 received. Both markers must be byte-identical, which proves the token
# was reused and the catch-up did not duplicate the line.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"
CHAT_SCRIPT="say:hello|poll|say:world"
# sha256("#1 mp-world-client: hello\n#2 mp-world-client: world")[:16]
CHAT_CHECKSUM="c8ea9cd6475d80e8"
PUSH_SCRIPT="say:push-me|poll"
PUSH_TEXT="push-me"
PUSH_SENDER_NAME="mp-world-client"
LISTENER_NAME="mp-chat-listener"
# sha256("1|mp-world-client|push-me")[:16]
PUSH_MARKER_CHECKSUM="af94bd1e7ca9c364"
# sha256("#1 mp-world-client: push-me")[:16]
PUSH_CHAT_CHECKSUM="a664b2fdb8f94b8c"
RECONNECT_TEXT="before-resume"
# sha256("#1 mp-world-client: before-resume")[:16]
RECONNECT_CHECKSUM="55e0d244cfb6ae55"

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
LISTENER_LOG="$(mktemp)"
SERVER_PID=""
LISTENER_PID=""

stop_server() {
	if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
		kill "$SERVER_PID" 2>/dev/null || true
	fi
	if [[ -n "$SERVER_PID" ]]; then
		wait "$SERVER_PID" 2>/dev/null || true
	fi
	SERVER_PID=""
}

stop_listener() {
	if [[ -n "$LISTENER_PID" ]] && kill -0 "$LISTENER_PID" 2>/dev/null; then
		kill "$LISTENER_PID" 2>/dev/null || true
	fi
	if [[ -n "$LISTENER_PID" ]]; then
		wait "$LISTENER_PID" 2>/dev/null || true
	fi
	LISTENER_PID=""
}

cleanup() {
	stop_listener
	stop_server
	rm -f "$SERVER_LOG" "$LISTENER_LOG"
	rm -rf "$SMOKE_DIR"
}
trap cleanup EXIT

start_server() {
	local port="$1"
	local sessions="${2:-1}"
	: >"$SERVER_LOG"
	"$ROOT/target/debug/townsmith-server" --listen "$port" --sessions "$sessions" >"$SERVER_LOG" 2>&1 &
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
	if grep -q '^MP_CHAT_UNREAD ' <<<"$output"; then
		echo "chat smoke ($name) failed: scripted chat reported an unread line" >&2
		exit 1
	fi
	if grep -q '^MP_CHAT_BADGE ' <<<"$output"; then
		echo "chat smoke ($name) failed: scripted chat reported a badge line" >&2
		exit 1
	fi

	if [[ ! "$main_line" =~ ^MP_WORLD_CLIENT\ status=ok\ tick=6\ population=3277\ balance=100000\ roads=6\ buildings=4\ truncated=false\ buildings_truncated=false\ checksum=[0-9a-f]{16}\ road_checksum=48aff8a44b921666\ building_checksum=a98d781faa5c1d5b\ rendered_roads=6\ rendered_buildings=4\ center=0\.0,0\.0\ radius=96\.0\ session=session-[0-9a-f]{16}(-[0-9]+)?$ ]]; then
		echo "chat smoke ($name) main marker mismatch: $main_line" >&2
		exit 1
	fi
	if [[ ! "$chat_line" =~ ^MP_CHAT\ status=ok\ entries=2\ checksum=[0-9a-f]{16}$ ]]; then
		echo "chat smoke ($name) chat marker mismatch: $chat_line" >&2
		exit 1
	fi
	if [[ ! "$done_line" =~ ^MP_WORLD_CLIENT_DONE\ status=ok\ session=session-[0-9a-f]{16}(-[0-9]+)?\ tick=6\ frames_sent=13\ frames_received=14$ ]]; then
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

# run_push_block PORT
#
# One fresh `--sessions 2` server. The passive listener completes hello and
# waits; the sender chats once. The listener must see the pushed line and the
# sender must see only its own response.
run_push_block() {
	local port="$1"
	local listener_output
	local listener_status
	local sender_output
	local sender_status
	local push_line
	local push_count
	local unread_lines
	local unread_count
	local unread_first
	local unread_reset
	local badge_lines
	local badge_count
	local badge_first
	local badge_reset
	local main_line
	local chat_line
	local done_line

	start_server "$port" 2

	timeout 60s env -u DISPLAY -u WAYLAND_DISPLAY \
		"$GODOT_BIN" --headless --path "$ROOT/godot" res://mp_world_client.tscn -- \
		--smoke --chat-listen --chat-expect "$PUSH_TEXT" --port "$port" \
		--name "$LISTENER_NAME" --timeout-ms 30000 >"$LISTENER_LOG" 2>&1 &
	LISTENER_PID=$!
	wait_for_log "$LISTENER_LOG" "$LISTENER_PID" "MP_CHAT_LISTEN status=ready" 15

	set +e
	sender_output="$(timeout 60s env -u DISPLAY -u WAYLAND_DISPLAY \
		"$GODOT_BIN" --headless --path "$ROOT/godot" res://mp_world_client.tscn -- \
		--smoke --chat --port "$port" --chat-script "$PUSH_SCRIPT" 2>&1)"
	sender_status=$?
	set -e
	printf '%s\n' "$sender_output" >&2

	wait_for_exit "$LISTENER_PID" 15
	set +e
	wait "$LISTENER_PID"
	listener_status=$?
	set -e
	LISTENER_PID=""
	listener_output="$(cat "$LISTENER_LOG")"
	printf '%s\n' "$listener_output" >&2

	if [[ $listener_status -ne 0 ]]; then
		echo "chat push smoke failed: listener exited with status $listener_status" >&2
		exit 1
	fi
	if grep -q '^MP_WORLD_CLIENT status=fail ' <<<"$listener_output"; then
		echo "chat push smoke failed: listener reported a failure" >&2
		exit 1
	fi
	if [[ $sender_status -ne 0 ]]; then
		echo "chat push smoke failed: sender exited with status $sender_status" >&2
		exit 1
	fi
	if grep -q '^MP_WORLD_CLIENT status=fail ' <<<"$sender_output"; then
		echo "chat push smoke failed: sender reported a failure" >&2
		exit 1
	fi
	if grep -q '^MP_WORLD_CLIENT_ZERO ' <<<"$sender_output"; then
		echo "chat push smoke failed: sender ran the zero-view probe" >&2
		exit 1
	fi

	push_line="$(grep '^MP_CHAT_PUSH status=ok ' <<<"$listener_output" | tail -1 || true)"
	push_count="$(grep -c '^MP_CHAT_PUSH ' <<<"$listener_output" || true)"
	if [[ "$push_count" != "1" ]]; then
		echo "chat push smoke failed: listener saw $push_count push markers" >&2
		exit 1
	fi
	if [[ "$push_line" != "MP_CHAT_PUSH status=ok player=$PUSH_SENDER_NAME seq=1 text=$PUSH_TEXT checksum=$PUSH_MARKER_CHECKSUM" ]]; then
		echo "chat push smoke listener marker mismatch: $push_line" >&2
		exit 1
	fi

	unread_lines="$(grep '^MP_CHAT_UNREAD ' <<<"$listener_output" || true)"
	unread_count="$(grep -c '^MP_CHAT_UNREAD ' <<<"$listener_output" || true)"
	if [[ "$unread_count" != "2" ]]; then
		echo "chat push smoke failed: listener saw $unread_count unread markers (want 2)" >&2
		exit 1
	fi
	unread_first="$(sed -n '1p' <<<"$unread_lines")"
	unread_reset="$(sed -n '2p' <<<"$unread_lines")"
	if [[ "$unread_first" != "MP_CHAT_UNREAD unread=1 total=1" ]]; then
		echo "chat push smoke listener unread mismatch: $unread_first" >&2
		exit 1
	fi
	if [[ "$unread_reset" != "MP_CHAT_UNREAD unread=0 total=1" ]]; then
		echo "chat push smoke listener reset mismatch: $unread_reset" >&2
		exit 1
	fi
	if grep -q '^MP_CHAT_UNREAD ' <<<"$sender_output"; then
		echo "chat push smoke failed: sender counted its own chat_sent response" >&2
		exit 1
	fi

	badge_lines="$(grep '^MP_CHAT_BADGE ' <<<"$listener_output" || true)"
	badge_count="$(grep -c '^MP_CHAT_BADGE ' <<<"$listener_output" || true)"
	if [[ "$badge_count" != "2" ]]; then
		echo "chat push smoke failed: listener saw $badge_count badge markers (want 2)" >&2
		exit 1
	fi
	badge_first="$(sed -n '1p' <<<"$badge_lines")"
	badge_reset="$(sed -n '2p' <<<"$badge_lines")"
	if [[ "$badge_first" != "MP_CHAT_BADGE status=ok unread=1 panel=1 text=1 visible=true" ]]; then
		echo "chat push smoke listener badge mismatch: $badge_first" >&2
		exit 1
	fi
	if [[ "$badge_reset" != "MP_CHAT_BADGE status=ok unread=0 panel=0 text=- visible=false" ]]; then
		echo "chat push smoke listener badge reset mismatch: $badge_reset" >&2
		exit 1
	fi
	if grep -q '^MP_CHAT_BADGE ' <<<"$sender_output"; then
		echo "chat push smoke failed: sender reported a badge transition" >&2
		exit 1
	fi

	main_line="$(grep '^MP_WORLD_CLIENT status=ok ' <<<"$sender_output" | tail -1 || true)"
	chat_line="$(grep '^MP_CHAT status=ok ' <<<"$sender_output" | tail -1 || true)"
	done_line="$(grep '^MP_WORLD_CLIENT_DONE status=ok ' <<<"$sender_output" | tail -1 || true)"
	if [[ -z "$main_line" || -z "$chat_line" || -z "$done_line" ]]; then
		echo "chat push smoke failed: missing sender main, chat, or done marker" >&2
		exit 1
	fi
	if [[ ! "$main_line" =~ ^MP_WORLD_CLIENT\ status=ok\ tick=6\ population=3277\ balance=100000\ roads=6\ buildings=4\ truncated=false\ buildings_truncated=false\ checksum=[0-9a-f]{16}\ road_checksum=48aff8a44b921666\ building_checksum=a98d781faa5c1d5b\ rendered_roads=6\ rendered_buildings=4\ center=0\.0,0\.0\ radius=96\.0\ session=session-[0-9a-f]{16}(-[0-9]+)?$ ]]; then
		echo "chat push smoke sender main marker mismatch: $main_line" >&2
		exit 1
	fi
	if [[ "$chat_line" != "MP_CHAT status=ok entries=1 checksum=$PUSH_CHAT_CHECKSUM" ]]; then
		echo "chat push smoke sender chat marker mismatch: $chat_line" >&2
		exit 1
	fi
	if [[ ! "$done_line" =~ ^MP_WORLD_CLIENT_DONE\ status=ok\ session=session-[0-9a-f]{16}(-[0-9]+)?\ tick=6\ frames_sent=12\ frames_received=13$ ]]; then
		echo "chat push smoke sender done marker mismatch: $done_line" >&2
		exit 1
	fi

	# Listener command frames: hello, chat_since (catch-up poll), disconnect.
	# Sender: hello, six roads, set_interest, chat_since, chat, chat_since,
	# disconnect. The sender's 13 received frames include exactly one
	# chat_sent; a self-push would make 14. Server total: 15 commands.
	wait_for_log "$SERVER_LOG" "$SERVER_PID" "SERVER_LISTEN status=ok sessions=2 commands=15 " 10
	wait_for_exit "$SERVER_PID" 10
	stop_server
	if ! grep -Eq "SERVER_LISTEN status=ok sessions=2 commands=15 " "$SERVER_LOG"; then
		echo "chat push smoke failed: server did not report two sessions with 15 commands" >&2
		cat "$SERVER_LOG" >&2
		exit 1
	fi
}

# run_reconnect_block NAME PORT PREFIX
#
# One fresh `--sessions 2` server. The client sends one `before-resume` line,
# disconnects, resumes with the recorded token, drops its local transcript, and
# catches the line up with `chat_since {after: 0}`. Exactly one pre-resume
# entry may come back; a missing or repeated line fails the block.
run_reconnect_block() {
	local name="$1"
	local port="$2"
	local prefix="$3"
	local output
	local godot_status
	local main_line
	local reconnect_line
	local reconnect_count
	local reconnect_checksum
	local done_line

	start_server "$port" 2

	set +e
	output="$(timeout 60s env -u DISPLAY -u WAYLAND_DISPLAY \
		"$GODOT_BIN" --headless --path "$ROOT/godot" res://mp_world_client.tscn -- \
		--smoke --chat --chat-reconnect --chat-text "$RECONNECT_TEXT" \
		--port "$port" 2>&1)"
	godot_status=$?
	set -e
	printf '%s\n' "$output" >&2

	if [[ $godot_status -ne 0 ]]; then
		echo "chat reconnect smoke ($name) failed: Godot exited with status $godot_status" >&2
		exit 1
	fi
	if grep -q '^MP_WORLD_CLIENT status=fail ' <<<"$output"; then
		echo "chat reconnect smoke ($name) failed: client reported a failure" >&2
		exit 1
	fi
	if grep -q '^MP_WORLD_CLIENT_ZERO ' <<<"$output"; then
		echo "chat reconnect smoke ($name) failed: reconnect mode ran the zero-view probe" >&2
		exit 1
	fi
	# Reconnect prints MP_CHAT_RECONNECT only. A scripted MP_CHAT marker would
	# mean the normal chat path ran, and a push/unread marker would mean the
	# first line was echoed back instead of caught up.
	if grep -q '^MP_CHAT ' <<<"$output"; then
		echo "chat reconnect smoke ($name) failed: printed a scripted MP_CHAT marker" >&2
		exit 1
	fi
	if grep -q '^MP_CHAT_UNREAD \|^MP_CHAT_BADGE \|^MP_CHAT_PUSH ' <<<"$output"; then
		echo "chat reconnect smoke ($name) failed: printed a push, unread, or badge marker" >&2
		exit 1
	fi

	main_line="$(grep '^MP_WORLD_CLIENT status=ok ' <<<"$output" | tail -1 || true)"
	reconnect_line="$(grep '^MP_CHAT_RECONNECT status=ok ' <<<"$output" | tail -1 || true)"
	reconnect_count="$(grep -c '^MP_CHAT_RECONNECT ' <<<"$output" || true)"
	done_line="$(grep '^MP_WORLD_CLIENT_DONE status=ok ' <<<"$output" | tail -1 || true)"
	if [[ -z "$main_line" || -z "$reconnect_line" || -z "$done_line" ]]; then
		echo "chat reconnect smoke ($name) failed: missing main, reconnect, or done marker" >&2
		exit 1
	fi
	if [[ "$reconnect_count" != "1" ]]; then
		echo "chat reconnect smoke ($name) failed: saw $reconnect_count reconnect markers" >&2
		exit 1
	fi

	if [[ ! "$main_line" =~ ^MP_WORLD_CLIENT\ status=ok\ tick=6\ population=3277\ balance=100000\ roads=6\ buildings=4\ truncated=false\ buildings_truncated=false\ checksum=[0-9a-f]{16}\ road_checksum=48aff8a44b921666\ building_checksum=a98d781faa5c1d5b\ rendered_roads=6\ rendered_buildings=4\ center=0\.0,0\.0\ radius=96\.0\ session=session-[0-9a-f]{16}(-[0-9]+)?$ ]]; then
		echo "chat reconnect smoke ($name) main marker mismatch: $main_line" >&2
		exit 1
	fi
	# entries=1 and last_seq=1: the resumed session recovered exactly the one
	# line sent before the disconnect and did not replay it as a second entry.
	if [[ ! "$reconnect_line" =~ ^MP_CHAT_RECONNECT\ status=ok\ entries=1\ last_seq=1\ token_reused=true\ checksum=[0-9a-f]{16}$ ]]; then
		echo "chat reconnect smoke ($name) reconnect marker mismatch: $reconnect_line" >&2
		exit 1
	fi
	reconnect_checksum="$(sed -n 's/.* checksum=\([0-9a-f]\{16\}\)$/\1/p' <<<"$reconnect_line")"
	if [[ "$reconnect_checksum" != "$RECONNECT_CHECKSUM" ]]; then
		echo "chat reconnect smoke ($name) failed: checksum '$reconnect_checksum' != '$RECONNECT_CHECKSUM'" >&2
		exit 1
	fi
	# Ten commands/frames on the first session (hello, six roads, set_interest,
	# chat, disconnect) and three on the resumed one (resume, chat_since,
	# disconnect). Fifteen responses include the resumed welcome and snapshot;
	# a re-sent line or an extra chat_sent would push this count up.
	if [[ ! "$done_line" =~ ^MP_WORLD_CLIENT_DONE\ status=ok\ session=session-[0-9a-f]{16}(-[0-9]+)?\ tick=6\ frames_sent=13\ frames_received=15$ ]]; then
		echo "chat reconnect smoke ($name) done marker mismatch: $done_line" >&2
		exit 1
	fi

	printf '%s\n' "$main_line" >"$prefix.main"
	printf '%s\n' "$reconnect_line" >"$prefix.reconnect"
	printf '%s\n' "$done_line" >"$prefix.done"
	printf '%s\n' "$reconnect_checksum" >"$prefix.checksum"

	wait_for_log "$SERVER_LOG" "$SERVER_PID" "SERVER_LISTEN status=ok sessions=2 commands=13 " 10
	wait_for_exit "$SERVER_PID" 10
	stop_server
	if ! grep -Eq "SERVER_LISTEN status=ok sessions=2 commands=13 " "$SERVER_LOG"; then
		echo "chat reconnect smoke ($name) failed: server did not report two sessions with 13 commands" >&2
		cat "$SERVER_LOG" >&2
		exit 1
	fi
}


# claim_port [EXPLICIT]
#
# Honors an explicit port (environment override) or picks a free one, and
# guarantees it is not already claimed by another scenario in this run.
claim_port() {
	local port="${1:-}"
	local used
	local clash
	if [[ -n "$port" ]]; then
		for used in "${PORT_POOL[@]}"; do
			if [[ "$port" == "$used" ]]; then
				echo "chat smoke failed: duplicate explicit port $port" >&2
				exit 1
			fi
		done
	else
		clash=0
		for _ in 1 2 3 4 5 6 7 8; do
			port="$(pick_port "")"
			clash=0
			for used in "${PORT_POOL[@]}"; do
				if [[ "$port" == "$used" ]]; then
					clash=1
				fi
			done
			if [[ $clash -eq 0 ]]; then
				break
			fi
		done
		if [[ $clash -ne 0 ]]; then
			echo "chat smoke failed: could not allocate a unique port" >&2
			exit 1
		fi
	fi
	PORT_POOL+=("$port")
	printf '%s\n' "$port"
}

PORT_POOL=()
FIRST_PORT="$(claim_port "${MP_CHAT_PORT:-}")"
SECOND_PORT="$(claim_port "${MP_CHAT_SECOND_PORT:-}")"
THIRD_PORT="$(claim_port "${MP_CHAT_PUSH_PORT:-}")"
RECONNECT_FIRST_PORT="$(claim_port "${MP_CHAT_RECONNECT_PORT:-}")"
RECONNECT_SECOND_PORT="$(claim_port "${MP_CHAT_RECONNECT_SECOND_PORT:-}")"

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

run_push_block "$THIRD_PORT"

run_reconnect_block reconnect_a "$RECONNECT_FIRST_PORT" "$SMOKE_DIR/reconnect_a"
run_reconnect_block reconnect_b "$RECONNECT_SECOND_PORT" "$SMOKE_DIR/reconnect_b"

for marker in main reconnect done; do
	if ! diff -u "$SMOKE_DIR/reconnect_a.$marker" "$SMOKE_DIR/reconnect_b.$marker"; then
		echo "chat reconnect smoke failed: $marker markers differ between runs" >&2
		exit 1
	fi
done
if [[ "$(cat "$SMOKE_DIR/reconnect_a.checksum")" != "$(cat "$SMOKE_DIR/reconnect_b.checksum")" ]]; then
	echo "chat reconnect smoke failed: checksums differ between runs" >&2
	exit 1
fi

echo "MP_CHAT_SMOKE status=ok runs=2 entries=2 checksum=$(cat "$SMOKE_DIR/first.checksum") world_checksum=$(sed -n 's/.* checksum=\([0-9a-f]\{16\}\).*/\1/p' "$SMOKE_DIR/first.main") frames_sent=13 frames_received=14"
echo "MP_CHAT_PUSH_SMOKE status=ok sessions=2 listener=$LISTENER_NAME sender=$PUSH_SENDER_NAME seq=1 text=$PUSH_TEXT checksum=$PUSH_MARKER_CHECKSUM sender_chat_checksum=$PUSH_CHAT_CHECKSUM sender_frames_sent=12 sender_frames_received=13"
echo "MP_CHAT_UNREAD_SMOKE status=ok listener_unread=1 listener_total=1 listener_reset=0 sender_unread=0"
echo "MP_CHAT_BADGE status=ok transitions=2 unread=1->0 panel=1->0 text=1->- visible=true->false sender_badges=0"
echo "MP_CHAT_RECONNECT_SMOKE status=ok runs=2 sessions=2 entries=1 last_seq=1 token_reused=true checksum=$(cat "$SMOKE_DIR/reconnect_a.checksum") frames_sent=13 frames_received=15"
