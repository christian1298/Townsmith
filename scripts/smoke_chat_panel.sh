#!/usr/bin/env bash
# Headless deterministic smoke for the standalone developer chat panel.
#
# Runs `godot/chat_panel.tscn -- --smoke` twice, requires the synthetic contract
# marker `CHAT_PANEL status=ok cases=N checks=N checksum=<8 hex>` plus the
# reconnect merge marker `MP_CHAT_CATCHUP status=ok added=N skipped=N
# last_seq=N unread=N`, and diffs every `CHAT_PANEL*` / `MP_CHAT_CATCHUP` line
# between the two processes. It also probes the panel headlessly *without*
# `--smoke` and requires the usage text plus exit status 2.
#
# No socket, server, display, or human input is required; the panel is a
# client-only debug scene and this script does not touch the Rust crates.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

if ! command -v "$GODOT_BIN" >/dev/null 2>&1; then
	echo "Godot binary not found: $GODOT_BIN (set GODOT=/path/to/godot)" >&2
	exit 2
fi

# The panel is standalone but the project loads the GDExtension. Build it only
# when missing so a clean worktree can run the smoke.
if [[ ! -f "$ROOT/godot/bin/libtownsmith_core.so" ]]; then
	echo "GDExtension library missing; building debug..."
	"$ROOT/scripts/build_godot_ext.sh" debug >/dev/null
fi

# Godot 4.7 can abort the first headless import while creating the project
# cache in a clean worktree. Retry once; a real failure still fails.
if ! "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1; then
	"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null
fi

SMOKE_DIR="$(mktemp -d)"
trap 'rm -rf "$SMOKE_DIR"' EXIT

CONTRACT='^CHAT_PANEL status=ok cases=[0-9]+ checks=[0-9]+ checksum=[0-9a-f]{8}$'
CATCHUP_CONTRACT='^MP_CHAT_CATCHUP status=ok added=[1-9][0-9]* skipped=[1-9][0-9]* last_seq=[0-9]+ unread=[0-9]+$'

run_usage_probe() {
	local log="$1"
	local status
	set +e
	timeout 60s env -u DISPLAY -u WAYLAND_DISPLAY \
		"$GODOT_BIN" --headless --path "$ROOT/godot" res://chat_panel.tscn >"$log" 2>&1
	status=$?
	set -e

	if [[ $status -ne 2 ]]; then
		cat "$log" >&2
		echo "Chat panel usage probe failed: expected exit 2 without --smoke, got $status" >&2
		exit 1
	fi
	if ! grep -q '^Usage: godot --headless --path godot res://chat_panel.tscn -- --smoke$' "$log"; then
		cat "$log" >&2
		echo "Chat panel usage probe failed: missing usage line" >&2
		exit 1
	fi
}

run_smoke() {
	local log="$1"
	local status
	set +e
	timeout 120s env -u DISPLAY -u WAYLAND_DISPLAY \
		"$GODOT_BIN" --headless --path "$ROOT/godot" res://chat_panel.tscn -- --smoke >"$log" 2>&1
	status=$?
	set -e

	if [[ $status -ne 0 ]]; then
		cat "$log" >&2
		echo "Chat panel smoke failed: Godot exited with status $status" >&2
		exit 1
	fi
	if grep -q '^CHAT_PANEL_FAIL ' "$log"; then
		cat "$log" >&2
		echo "Chat panel smoke reported failures" >&2
		exit 1
	fi

	grep -E '^(CHAT_PANEL|MP_CHAT_CATCHUP)' "$log" || true
}

run_usage_probe "$SMOKE_DIR/usage.log"

FIRST="$(run_smoke "$SMOKE_DIR/run1.log")"
SECOND="$(run_smoke "$SMOKE_DIR/run2.log")"

if [[ -z "$FIRST" ]]; then
	echo "Chat panel smoke failed: no CHAT_PANEL marker" >&2
	exit 1
fi

if ! diff -u <(printf '%s\n' "$FIRST") <(printf '%s\n' "$SECOND"); then
	echo "Chat panel smoke failed: the two synthetic runs differ" >&2
	exit 1
fi

LINE="$(grep '^CHAT_PANEL status=' <<<"$FIRST" | tail -1 || true)"
if [[ -z "$LINE" ]]; then
	echo "Chat panel smoke failed: no final status marker" >&2
	exit 1
fi
if [[ ! "$LINE" =~ $CONTRACT ]]; then
	echo "Chat panel smoke contract mismatch: $LINE" >&2
	exit 1
fi
if ! grep -q '^CHAT_PANEL_STAGE name=locale ' <<<"$FIRST"; then
	echo "Chat panel smoke failed: missing locale stage" >&2
	exit 1
fi
if ! grep -q '^CHAT_PANEL_LOCALE de=ok fr=en en=ok ' <<<"$FIRST"; then
	echo "Chat panel smoke failed: missing EN/DE locale evidence" >&2
	exit 1
fi

CATCHUP_LINE="$(grep '^MP_CHAT_CATCHUP status=' <<<"$FIRST" | tail -1 || true)"
if [[ -z "$CATCHUP_LINE" ]]; then
	echo "Chat panel smoke failed: no reconnect catch-up marker" >&2
	exit 1
fi
if [[ ! "$CATCHUP_LINE" =~ $CATCHUP_CONTRACT ]]; then
	echo "Chat panel smoke catch-up contract mismatch: $CATCHUP_LINE" >&2
	exit 1
fi

printf '%s\n' "$FIRST"
echo "Chat panel smoke passed: usage probe + two identical synthetic runs"
