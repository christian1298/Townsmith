#!/usr/bin/env bash
# Headless smoke for the standalone read-only settlement screen.
#
# Builds the debug GDExtension, imports once (with the usual single retry for a
# clean worktree), then runs `godot/settlement_screen.tscn --smoke` twice
# headless. Each run seeds 23 local settlements through the bridge, exercises
# 8/8/7 cursor paging, page-local selection, player-name/#id fallbacks, the
# D-039 projection at 0/5/100/invalid, and EN/DE key coverage, then prints one
# deterministic marker. The two runs must match byte-for-byte.
#
# Also runs `--empty` and `--unavailable` and requires the localized state
# markers. No display or human input is required.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

if ! command -v "$GODOT_BIN" >/dev/null 2>&1; then
	echo "Godot binary not found: $GODOT_BIN (set GODOT=/path/to/godot)" >&2
	exit 2
fi

echo "Building debug GDExtension..."
"$ROOT/scripts/build_godot_ext.sh" debug >/dev/null

# Godot 4.7 can abort the first headless import while creating the project
# cache in a clean worktree. Retry once; a real failure still fails.
if ! "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1; then
	"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null
fi

SMOKE_DIR="$(mktemp -d)"
trap 'rm -rf "$SMOKE_DIR"' EXIT

CONTRACT='^SETTLEMENT_SCREEN status=ok pages=3 settlements=23 selected=2 actions=11 accepted=8 rejected=3 checksum=[0-9a-f]{16}$'
PAGES_CONTRACT='^SETTLEMENT_SCREEN_PAGES sizes=8/8/7 selected_last=23$'
FOCUS_CONTRACT='^SETTLEMENT_SCREEN_FOCUS group=rows tab=paging shift=ok arrows=ok modal=trap esc=detail$'
NAMES_CONTRACT='^SETTLEMENT_SCREEN_NAMES owner=Mayor#7 manager=Manager#11 builder=Builder#13 member=Member#12 fallback=#9001 owner_fallback=#9002$'
PROJECTION_CONTRACT='^SETTLEMENT_SCREEN_PROJECTION tax0=x1\.00 tax5=x1\.05 tax100=x2\.00 invalid=tax\.projection\.rate_out_of_range$'
LOCALE_CONTRACT='^SETTLEMENT_SCREEN_LOCALE en=ok de=ok$'
GAMEPAD_CONTRACT='^SETTLEMENT_SCREEN_GAMEPAD status=ok checks=[0-9]+$'
A11Y_CONTRACT='^SETTLEMENT_SCREEN_A11Y status=ok checks=[0-9]+$'
EMPTY_CONTRACT='^SETTLEMENT_SCREEN status=empty settlements=0$'
UNAVAILABLE_CONTRACT='^SETTLEMENT_SCREEN status=unavailable code=no_core$'

run_screen() {
	local mode="$1"
	local log="$2"
	local marker_file="$3"
	local contract="$4"
	local output
	local godot_status
	set +e
	output="$(timeout 120s "$GODOT_BIN" --headless --path "$ROOT/godot" res://settlement_screen.tscn -- "$mode" 2>&1)"
	godot_status=$?
	set -e
	printf '%s\n' "$output" >&2

	if [[ $godot_status -ne 0 ]]; then
		echo "Settlement screen smoke failed ($mode): Godot exited with status $godot_status" >&2
		exit 1
	fi

	local marker
	marker="$(grep '^SETTLEMENT_SCREEN status=' <<<"$output" | tail -1 || true)"
	if [[ -z "$marker" ]]; then
		echo "Settlement screen smoke failed ($mode): no SETTLEMENT_SCREEN status marker" >&2
		exit 1
	fi
	if [[ ! "$marker" =~ $contract ]]; then
		echo "Settlement screen smoke contract mismatch ($mode): $marker" >&2
		exit 1
	fi

	if grep -Eq '^SETTLEMENT_SCREEN status=failed|^SETTLEMENT_SCREEN_FAIL ' <<<"$output"; then
		echo "Settlement screen smoke failed ($mode): a failure marker was printed" >&2
		exit 1
	fi

	if grep -Fq '!base.ui.' <<<"$output"; then
		echo "Settlement screen smoke failed ($mode): missing translation key" >&2
		exit 1
	fi

	if [[ "$mode" == "--smoke" ]]; then
		local name contract_name
		for pair in \
			"SETTLEMENT_SCREEN_PAGES|$PAGES_CONTRACT" \
			"SETTLEMENT_SCREEN_FOCUS|$FOCUS_CONTRACT" \
			"SETTLEMENT_SCREEN_NAMES|$NAMES_CONTRACT" \
			"SETTLEMENT_SCREEN_PROJECTION|$PROJECTION_CONTRACT" \
			"SETTLEMENT_SCREEN_LOCALE|$LOCALE_CONTRACT" \
			"SETTLEMENT_SCREEN_GAMEPAD|$GAMEPAD_CONTRACT" \
			"SETTLEMENT_SCREEN_A11Y|$A11Y_CONTRACT"; do
			name="${pair%%|*}"
			contract_name="${pair#*|}"
			local line
			line="$(grep "^${name} " <<<"$output" | tail -1 || true)"
			if [[ -z "$line" ]]; then
				echo "Settlement screen smoke failed ($mode): no ${name} marker" >&2
				exit 1
			fi
			if [[ ! "$line" =~ $contract_name ]]; then
				echo "Settlement screen smoke contract mismatch ($mode): $line" >&2
				exit 1
			fi
		done
	fi

	grep -E '^SETTLEMENT_SCREEN(_PAGES|_FOCUS|_NAMES|_PROJECTION|_LOCALE|_GAMEPAD)? ' <<<"$output" | grep -v '^SETTLEMENT_SCREEN status=' >"$log" || true
	printf '%s\n' "$marker" >"$marker_file"
}

run_screen --smoke "$SMOKE_DIR/first.txt" "$SMOKE_DIR/first.marker" "$CONTRACT"
run_screen --smoke "$SMOKE_DIR/second.txt" "$SMOKE_DIR/second.marker" "$CONTRACT"

FIRST_MARKER="$(cat "$SMOKE_DIR/first.marker")"
SECOND_MARKER="$(cat "$SMOKE_DIR/second.marker")"

if [[ "$FIRST_MARKER" != "$SECOND_MARKER" ]]; then
	echo "Settlement screen smoke failed: markers differ between runs" >&2
	echo "  first:  $FIRST_MARKER" >&2
	echo "  second: $SECOND_MARKER" >&2
	exit 1
fi

if ! diff -u "$SMOKE_DIR/first.txt" "$SMOKE_DIR/second.txt"; then
	echo "Settlement screen smoke failed: deterministic screen lines differ" >&2
	exit 1
fi

run_screen --empty "$SMOKE_DIR/empty.txt" "$SMOKE_DIR/empty.marker" "$EMPTY_CONTRACT"
run_screen --unavailable "$SMOKE_DIR/unavailable.txt" "$SMOKE_DIR/unavailable.marker" "$UNAVAILABLE_CONTRACT"

echo "SETTLEMENT_SCREEN_SMOKE OK ${FIRST_MARKER#SETTLEMENT_SCREEN }"
