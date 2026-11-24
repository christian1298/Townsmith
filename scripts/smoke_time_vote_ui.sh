#!/usr/bin/env bash
# Headless synthetic smoke for the developer time-vote panel.
#
# Runs `godot/time_vote_panel_smoke.tscn` twice, requires the synthetic contract
# marker, and diffs the stable `TIME_VOTE_UI` lines. No server, display, or
# human input is required; the panel is client-only and this script does not
# touch the Rust crates.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

if ! command -v "$GODOT_BIN" >/dev/null 2>&1; then
	echo "Godot binary not found: $GODOT_BIN (set GODOT=/path/to/godot)" >&2
	exit 2
fi

# Godot 4.7 can abort the first headless import while creating the project
# cache in a clean worktree. Retry once; a real failure still fails.
if ! "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1; then
	"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null
fi

SMOKE_DIR="$(mktemp -d)"
trap 'rm -rf "$SMOKE_DIR"' EXIT

run_once() {
	local log="$1"
	local status
	set +e
	timeout 120s "$GODOT_BIN" --headless --path "$ROOT/godot" res://time_vote_panel_smoke.tscn >"$log" 2>&1
	status=$?
	set -e

	if [[ $status -ne 0 ]]; then
		cat "$log" >&2
		echo "Time-vote panel smoke failed: Godot exited with status $status" >&2
		exit 1
	fi
	if grep -q '^TIME_VOTE_UI_FAIL ' "$log"; then
		cat "$log" >&2
		echo "Time-vote panel smoke reported failures" >&2
		exit 1
	fi

	grep '^TIME_VOTE_UI' "$log" || true
}

FIRST="$(run_once "$SMOKE_DIR/run1.log")"
SECOND="$(run_once "$SMOKE_DIR/run2.log")"

if [[ -z "$FIRST" ]]; then
	echo "Time-vote panel smoke failed: no TIME_VOTE_UI marker" >&2
	exit 1
fi

if ! diff -u <(printf '%s\n' "$FIRST") <(printf '%s\n' "$SECOND"); then
	echo "Time-vote panel smoke failed: the two synthetic runs differ" >&2
	exit 1
fi

LINE="$(grep '^TIME_VOTE_UI mode=synthetic ' <<<"$FIRST" | tail -1 || true)"
CONTRACT='^TIME_VOTE_UI mode=synthetic status=ok cases=[0-9]+ checks=[0-9]+ commands=5 errors=0 checksum=[0-9a-f]{8}$'
if [[ -z "$LINE" ]]; then
	echo "Time-vote panel smoke failed: no final synthetic marker" >&2
	exit 1
fi
if [[ ! "$LINE" =~ $CONTRACT ]]; then
	echo "Time-vote panel smoke contract mismatch: $LINE" >&2
	exit 1
fi

printf '%s\n' "$FIRST"
echo "Time-vote panel smoke passed: two identical synthetic runs"
