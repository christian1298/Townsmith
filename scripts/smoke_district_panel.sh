#!/usr/bin/env bash
# Headless smoke for the standalone read-only district overview.
#
# Builds the debug GDExtension, imports once (with the usual single retry for a
# clean worktree), runs `godot/district_panel.tscn --smoke` twice headless, and
# requires the two-settlement fixture contract: the 256 m district summary, the
# smallest-id containing policy, and identical panel checksums/markers. It then
# runs `--empty` and requires the empty-session marker. No display or human
# input is required.
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

PANEL_CONTRACT='^DISTRICT_PANEL\ status=ok\ settlements=2\ selected=1\ population=0\ policy=1\ checksum=[0-9a-f]{8}$'
EMPTY_CONTRACT='^DISTRICT_PANEL\ status=ok\ settlements=0\ selected=none\ population=0\ policy=none\ checksum=[0-9a-f]{8}$'
TAX_CONTRACT='^DISTRICT_TAX\ alpha=0%\ beta=0%\ selected=0%$'

run_panel() {
	local mode="$1"
	local log="$2"
	local marker_file="$3"
	local contract="$4"
	local output
	local godot_status
	set +e
	output="$(timeout 120s "$GODOT_BIN" --headless --path "$ROOT/godot" res://district_panel.tscn -- "$mode" 2>&1)"
	godot_status=$?
	set -e
	printf '%s\n' "$output" >&2

	if [[ $godot_status -ne 0 ]]; then
		echo "District panel smoke failed ($mode): Godot exited with status $godot_status" >&2
		exit 1
	fi

	local marker
	marker="$(grep '^DISTRICT_PANEL status=' <<<"$output" | tail -1 || true)"
	if [[ -z "$marker" ]]; then
		echo "District panel smoke failed ($mode): no DISTRICT_PANEL status marker" >&2
		exit 1
	fi
	if [[ ! "$marker" =~ $contract ]]; then
		echo "District panel smoke contract mismatch ($mode): $marker" >&2
		exit 1
	fi

	if grep -Eq '^DISTRICT_PANEL status=failed' <<<"$output"; then
		echo "District panel smoke failed ($mode): a failed marker was printed" >&2
		exit 1
	fi

	if [[ "$mode" == "--smoke" ]]; then
		local tax_marker
		tax_marker="$(grep '^DISTRICT_TAX ' <<<"$output" | tail -1 || true)"
		if [[ -z "$tax_marker" ]]; then
			echo "District panel smoke failed ($mode): no DISTRICT_TAX marker" >&2
			exit 1
		fi
		if [[ ! "$tax_marker" =~ $TAX_CONTRACT ]]; then
			echo "District panel smoke tax contract mismatch ($mode): $tax_marker" >&2
			exit 1
		fi
	fi

	grep -E '^DISTRICT_PANEL|^DISTRICT_TAX' <<<"$output" | grep -v '^DISTRICT_PANEL status=' >"$log" || true
	printf '%s\n' "$marker" >"$marker_file"
}

run_panel --smoke "$SMOKE_DIR/first.txt" "$SMOKE_DIR/first.marker" "$PANEL_CONTRACT"
run_panel --smoke "$SMOKE_DIR/second.txt" "$SMOKE_DIR/second.marker" "$PANEL_CONTRACT"

FIRST_MARKER="$(cat "$SMOKE_DIR/first.marker")"
SECOND_MARKER="$(cat "$SMOKE_DIR/second.marker")"

if [[ "$FIRST_MARKER" != "$SECOND_MARKER" ]]; then
	echo "District panel smoke failed: panel markers differ between runs" >&2
	echo "  first:  $FIRST_MARKER" >&2
	echo "  second: $SECOND_MARKER" >&2
	exit 1
fi

if ! diff -u "$SMOKE_DIR/first.txt" "$SMOKE_DIR/second.txt"; then
	echo "District panel smoke failed: deterministic panel lines differ" >&2
	exit 1
fi

run_panel --empty "$SMOKE_DIR/empty.txt" "$SMOKE_DIR/empty.marker" "$EMPTY_CONTRACT"

echo "DISTRICT_PANEL_SMOKE OK ${FIRST_MARKER#DISTRICT_PANEL }"
