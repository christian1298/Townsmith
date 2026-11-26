#!/usr/bin/env bash
set -euo pipefail

# Placement-UI smoke for the 3D multiplayer client.
#
# Seeds one content-addressed fixture library, starts a fresh mayor server per
# run, and drives the opt-in `--place-ui` path with a deterministic script:
#   select:0;move:8,0;rotate:90;place;move:1,0;place;cancel
# The first place succeeds; the second deliberately overlaps and must surface
# `PLACEMENT_OVERLAP`; cancel disconnects. Both fresh runs must print identical
# `MP_PLACE_UI*`, placement, and done markers, and no local path may leak into
# client output.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"
FIXTURE_X="32"
FIXTURE_Z="32"
SCRIPT="select:0;locks;lock;unlock;move:8,0;rotate:90;place;move:1,0;place;cancel"

if ! command -v "$GODOT_BIN" >/dev/null 2>&1; then
	echo "Godot binary not found: $GODOT_BIN (set GODOT=/path/to/godot)" >&2
	exit 2
fi

cargo build --quiet --manifest-path "$ROOT/Cargo.toml" -p townsmith-server
"$ROOT/scripts/build_godot_ext.sh" debug >/dev/null

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
	echo "placement-UI smoke failed: fixture marker mismatch: $FIXTURE_LINE" >&2
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

run_block() {
	local name="$1"
	local port="$2"
	local prefix="$3"
	local output
	local godot_status

	start_server "$port"

	set +e
	output="$(timeout 60s env -u DISPLAY -u WAYLAND_DISPLAY \
		"$GODOT_BIN" --headless --path "$ROOT/godot" res://mp_world_client.tscn -- \
		--smoke --port "$port" --place-ui --library "$FIXTURE_DIR" \
		--place-ui-script "$SCRIPT" \
		--place-x "$FIXTURE_X" --place-z "$FIXTURE_Z" 2>&1)"
	godot_status=$?
	set -e
	printf '%s\n' "$output" >&2

	if [[ $godot_status -ne 0 ]]; then
		echo "placement-UI smoke ($name) failed: Godot exited with status $godot_status" >&2
		exit 1
	fi
	if grep -q '^MP_WORLD_CLIENT status=fail ' <<<"$output"; then
		echo "placement-UI smoke ($name) failed: client reported a failure" >&2
		exit 1
	fi
	if grep -Eq '/(tmp|media)/' <<<"$output"; then
		echo "placement-UI smoke ($name) failed: client output leaked a filesystem path" >&2
		exit 1
	fi

	local ui_line
	local locks_line
	local lock_line
	local unlock_line
	local place_line
	local reject_line
	local done_line
	local client_done
	ui_line="$(grep '^MP_PLACE_UI status=ok ' <<<"$output" | tail -1 || true)"
	mapfile -t LOCK_LINES < <(grep '^MP_PLACE_UI_LOCKS status=ok ' <<<"$output")
	locks_line="${LOCK_LINES[0]:-}"
	locks_after_lock="${LOCK_LINES[2]:-}"
	locks_after_unlock="${LOCK_LINES[3]:-}"
	lock_line="$(grep '^MP_PLACE_UI_LOCK status=ok ' <<<"$output" | tail -1 || true)"
	unlock_line="$(grep '^MP_PLACE_UI_UNLOCK status=ok ' <<<"$output" | tail -1 || true)"
	place_line="$(grep '^MP_PLACE_UI_PLACE status=ok ' <<<"$output" | tail -1 || true)"
	reject_line="$(grep '^MP_PLACE_UI_REJECT status=ok ' <<<"$output" | tail -1 || true)"
	done_line="$(grep '^MP_PLACE_UI_DONE status=ok ' <<<"$output" | tail -1 || true)"
	client_done="$(grep '^MP_WORLD_CLIENT_DONE status=ok ' <<<"$output" | tail -1 || true)"
	if [[ ${#LOCK_LINES[@]} -ne 4 || -z "$ui_line" || -z "$locks_line" || -z "$locks_after_lock" || -z "$locks_after_unlock" || -z "$lock_line" || -z "$unlock_line" || -z "$place_line" || -z "$reject_line" || -z "$done_line" || -z "$client_done" ]]; then
		echo "placement-UI smoke ($name) failed: missing UI markers" >&2
		exit 1
	fi

	if [[ ! "$ui_line" =~ ^MP_PLACE_UI\ status=ok\ mode=ui\ entries=1\ selected=0\ id=local:client_smoke_house\ hash=blake3:[0-9a-f]{64}\ bounds=3\.2,3\.2,3\.2\ footprint=3\.2,3\.2\ library_checksum=[0-9a-f]{16}$ ]]; then
		echo "placement-UI smoke ($name) catalog marker mismatch: $ui_line" >&2
		exit 1
	fi
	if [[ "$locks_line" != *"count=0"* || "$locks_line" != *"tick=0"* || "$locks_line" != *"visible=false"* || "$locks_line" != *"text=Locks: 0"* ]]; then
		echo "placement-UI smoke ($name) initial locks marker mismatch: $locks_line" >&2
		exit 1
	fi
	if [[ "$locks_after_lock" != *"count=1"* || "$locks_after_lock" != *"tick=7"* || "$locks_after_lock" != *"visible=true"* || "$locks_after_lock" != *"text=Locks: 1;"* || "$locks_after_lock" != *"#1 299t"* ]]; then
		echo "placement-UI smoke ($name) held-lock marker mismatch: $locks_after_lock" >&2
		exit 1
	fi
	if [[ "$locks_after_unlock" != *"count=0"* || "$locks_after_unlock" != *"tick=8"* || "$locks_after_unlock" != *"visible=false"* || "$locks_after_unlock" != *"text=Locks: 0"* ]]; then
		echo "placement-UI smoke ($name) unlocked marker mismatch: $locks_after_unlock" >&2
		exit 1
	fi
	if [[ ! "$lock_line" =~ ^MP_PLACE_UI_LOCK\ status=ok\ selected=0\ hash=blake3:[0-9a-f]{64}\ player_id=1\ count=1\ checksum=[0-9a-f]{16}\ tick=7\ lock=LOCKED_you\ lease=299t$ ]]; then
		echo "placement-UI smoke ($name) lock marker mismatch: $lock_line" >&2
		exit 1
	fi
	if [[ ! "$unlock_line" =~ ^MP_PLACE_UI_UNLOCK\ status=ok\ selected=0\ hash=blake3:[0-9a-f]{64}\ count=0\ checksum=[0-9a-f]{16}\ tick=8\ lock=unlocked$ ]]; then
		echo "placement-UI smoke ($name) unlock marker mismatch: $unlock_line" >&2
		exit 1
	fi
	if [[ ! "$place_line" =~ ^MP_PLACE_UI_PLACE\ status=ok\ index=0\ id=1\ hash=blake3:[0-9a-f]{64}\ center=40\.0,32\.0\ rotation=90\ footprint=3\.2,3\.2\ placement_checksum=[0-9a-f]{16}\ world_checksum=[0-9a-f]{16}\ tick=9$ ]]; then
		echo "placement-UI smoke ($name) place marker mismatch: $place_line" >&2
		exit 1
	fi
	if [[ "$reject_line" != "MP_PLACE_UI_REJECT status=ok index=0 code=PLACEMENT_OVERLAP" ]]; then
		echo "placement-UI smoke ($name) reject marker mismatch: $reject_line" >&2
		exit 1
	fi
	if [[ ! "$done_line" =~ ^MP_PLACE_UI_DONE\ status=ok\ mode=ui\ actions=10\ accepted=1\ rejected=1\ placements=1\ selected=0\ error=PLACEMENT_OVERLAP\ locks=1\ unlocks=1\ lock_checksum=[0-9a-f]{16}$ ]]; then
		echo "placement-UI smoke ($name) done marker mismatch: $done_line" >&2
		exit 1
	fi
	if [[ ! "$client_done" =~ ^MP_WORLD_CLIENT_DONE\ status=ok\ session=session-[0-9a-f]{16}\ tick=9\ frames_sent=18\ frames_received=20$ ]]; then
		echo "placement-UI smoke ($name) client done mismatch: $client_done" >&2
		exit 1
	fi

	printf '%s\n' "$locks_line" >"$prefix.locks"
	printf '%s\n' "$lock_line" >"$prefix.lock"
	printf '%s\n' "$unlock_line" >"$prefix.unlock"
	printf '%s\n' "$ui_line" >"$prefix.ui"
	printf '%s\n' "$place_line" >"$prefix.place"
	printf '%s\n' "$reject_line" >"$prefix.reject"
	printf '%s\n' "$done_line" >"$prefix.done"
	printf '%s\n' "$client_done" >"$prefix.client_done"

	wait_for_log "$SERVER_LOG" "$SERVER_PID" "SERVER_LISTEN status=ok sessions=1 commands=18 " 10
	wait_for_exit "$SERVER_PID" 10
	stop_server
	if ! grep -Eq "SERVER_LISTEN status=ok sessions=1 commands=18 " "$SERVER_LOG"; then
		echo "placement-UI smoke ($name) failed: server did not report one 18-command session" >&2
		cat "$SERVER_LOG" >&2
		exit 1
	fi
}

FIRST_PORT="$(pick_port)"
SECOND_PORT="$(pick_port)"
for _ in 1 2 3 4 5; do
	[[ "$SECOND_PORT" != "$FIRST_PORT" ]] && break
	SECOND_PORT="$(pick_port)"
done
if [[ "$SECOND_PORT" == "$FIRST_PORT" ]]; then
	echo "placement-UI smoke failed: both runs selected port $FIRST_PORT" >&2
	exit 1
fi

run_block first "$FIRST_PORT" "$SMOKE_DIR/first"
run_block second "$SECOND_PORT" "$SMOKE_DIR/second"

for marker in locks lock unlock ui place reject done client_done; do
	if ! diff -u "$SMOKE_DIR/first.$marker" "$SMOKE_DIR/second.$marker"; then
		echo "placement-UI smoke failed: $marker markers differ between runs" >&2
		exit 1
	fi
done

echo "MP_PLACE_UI_SMOKE status=ok runs=2 entries=1 accepted=1 rejected=1 locks=1 unlocks=1 commands=18"
