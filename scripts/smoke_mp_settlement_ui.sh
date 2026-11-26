#!/usr/bin/env bash
set -euo pipefail

# Connected settlement readout smoke.
#
# Starts a fresh `townsmith-server --listen PORT --sessions 1` per run and runs
# `godot/mp_world_client.tscn --smoke --settlement-ui` with the deterministic
# script `found:Harbor|list|at:0,0`. The client sends the normal six-road setup,
# reads the 96 m interest view, then founds one settlement, lists it, and asks
# which settlement contains (0,0). It prints:
#   MP_SETTLEMENT_UI status=ok settlements=1 members=1 at=1 checksum=<16 hex>
# before the unchanged MP_WORLD_CLIENT_DONE marker.
#
# The second run uses a fresh server and must produce the same settlement
# marker/checksum. Readiness is a bounded server-log poll; there are no fixed
# sleeps.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

if ! command -v "$GODOT_BIN" >/dev/null 2>&1; then
	echo "Godot binary not found: $GODOT_BIN (set GODOT=/path/to/godot)" >&2
	exit 2
fi

cargo build --quiet --manifest-path "$ROOT/Cargo.toml" -p townsmith-server

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
	shift
	: >"$SERVER_LOG"
	"$ROOT/target/debug/townsmith-server" --listen "$port" --sessions 1 "$@" >"$SERVER_LOG" 2>&1 &
	SERVER_PID=$!
	wait_for_log "$SERVER_LOG" "$SERVER_PID" "SERVER_LISTEN status=ok address=" 10
}

# run_script NAME PORT ROLE SCRIPT MARKER_REGEX
run_script() {
	local name="$1"
	local port="$2"
	local role="$3"
	local script="$4"
	local expected="$5"
	local output
	local status
	local settlement_line
	local done_line

	start_server "$port" --role "$role"
	set +e
	output="$(timeout 90s env -u DISPLAY -u WAYLAND_DISPLAY \
		"$GODOT_BIN" --headless --path "$ROOT/godot" res://mp_world_client.tscn -- \
		--smoke --port "$port" --settlement-ui \
		--settlement-script "$script" 2>&1)"
	status=$?
	set -e
	printf '%s\n' "$output" >&2

	if [[ $status -ne 0 ]]; then
		echo "Settlement smoke ($name) failed: Godot exited $status" >&2
		exit 1
	fi
	if grep -q '^MP_WORLD_CLIENT status=fail ' <<<"$output"; then
		echo "Settlement smoke ($name) failed: client reported a failure" >&2
		exit 1
	fi

	settlement_line="$(grep '^MP_SETTLEMENT_UI status=ok ' <<<"$output" | tail -1 || true)"
	if [[ ! "$settlement_line" =~ $expected ]]; then
		echo "Settlement smoke ($name) marker mismatch: ${settlement_line:-missing}" >&2
		exit 1
	fi
	done_line="$(grep '^MP_WORLD_CLIENT_DONE status=ok ' <<<"$output" | tail -1 || true)"
	if [[ -z "$done_line" ]]; then
		echo "Settlement smoke ($name) missing MP_WORLD_CLIENT_DONE" >&2
		exit 1
	fi
	printf '%s\n' "$settlement_line" >"$SMOKE_DIR/$name.marker"
	printf '%s\n' "$output" >"$SMOKE_DIR/$name.log"
	stop_server
}

READ_ONLY_RE='^MP_SETTLEMENT_UI status=ok settlements=1 members=1 at=1 actions=3 accepted=3 rejected=0 checksum=[0-9a-f]{16}$'
MUTATION_RE='^MP_SETTLEMENT_UI status=ok settlements=1 members=0 at=none actions=4 accepted=4 rejected=0 checksum=[0-9a-f]{16}$'
REJECT_RE='^MP_SETTLEMENT_UI status=ok settlements=1 members=2 at=none actions=5 accepted=2 rejected=3 checksum=[0-9a-f]{16}$'

run_script read_first "$(pick_port)" mayor "found:Harbor|list|at:0,0" "$READ_ONLY_RE"
run_script read_second "$(pick_port)" mayor "found:Harbor|list|at:0,0" "$READ_ONLY_RE"
if ! diff -u "$SMOKE_DIR/read_first.marker" "$SMOKE_DIR/read_second.marker"; then
	echo "Settlement readout smoke failed: markers differ between runs" >&2
	exit 1
fi

run_script mutate_first "$(pick_port)" admin "found:Harbor|rename:Riverside|set_tax:5|release" "$MUTATION_RE"
run_script mutate_second "$(pick_port)" admin "found:Harbor|rename:Riverside|set_tax:5|release" "$MUTATION_RE"
if ! diff -u "$SMOKE_DIR/mutate_first.marker" "$SMOKE_DIR/mutate_second.marker"; then
	echo "Settlement mutation smoke failed: markers differ between runs" >&2
	exit 1
fi
for needle in \
	'MP_SETTLEMENT_ACTION action=rename ok=1 code=none revision=2' \
	'MP_SETTLEMENT_ACTION action=set_tax ok=1 code=none revision=2' \
	'MP_SETTLEMENT_ACTION action=release ok=1 code=none revision=3'; do
	if ! grep -Fq "$needle" "$SMOKE_DIR/mutate_first.log"; then
		echo "Settlement mutation smoke missing: $needle" >&2
		exit 1
	fi
done

run_script reject_first "$(pick_port)" mayor "found:Harbor|claim|invite:9999:builder|set_role:9998:manager|remove:9998" "$REJECT_RE"
run_script reject_second "$(pick_port)" mayor "found:Harbor|claim|invite:9999:builder|set_role:9998:manager|remove:9998" "$REJECT_RE"
if ! diff -u "$SMOKE_DIR/reject_first.marker" "$SMOKE_DIR/reject_second.marker"; then
	echo "Settlement rejection smoke failed: markers differ between runs" >&2
	exit 1
fi
for needle in \
	'MP_SETTLEMENT_ACTION action=claim ok=0 code=settlement.already_owned' \
	'MP_SETTLEMENT_ACTION action=set_role ok=0 code=settlement.not_member' \
	'MP_SETTLEMENT_ACTION action=remove ok=0 code=settlement.not_member'; do
	if ! grep -Fq "$needle" "$SMOKE_DIR/reject_first.log"; then
		echo "Settlement rejection smoke missing: $needle" >&2
		exit 1
	fi
done

MARKER="$(cat "$SMOKE_DIR/read_first.marker")"
echo "MP_SETTLEMENT_UI_SMOKE status=ok runs=6 ${MARKER#MP_SETTLEMENT_UI status=ok }"
