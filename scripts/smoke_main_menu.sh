#!/usr/bin/env bash
# Headless smoke for the front-end shell (docs/86).
#
# Builds the debug GDExtension, imports once (with the usual single retry for a
# clean worktree), then runs the project's default scene with `--menu-smoke`
# twice. Both runs must print the same deterministic marker:
#
#   MAIN_MENU status=ok buttons=6 focus=new_city checks=12 checksum=<8 hex>
#
# It then starts the project with the existing `--smoke` user arg and requires
# the terrain marker. That proves `main_menu.gd` still forwards the old scene
# smokes to `main.tscn` now that the menu is the `run/main_scene`. The full
# `scripts/smoke_godot.sh` remains the terrain contract gate; this script only
# proves the hand-over, so it can run on every UI change without duplicating
# that contract. No display or human input is required.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

if ! command -v "$GODOT_BIN" >/dev/null 2>&1; then
	echo "Godot binary not found: $GODOT_BIN (set GODOT=/path/to/godot)" >&2
	exit 2
fi

echo "Building debug GDExtension..." >&2
"$ROOT/scripts/build_godot_ext.sh" debug >/dev/null

# Godot 4.7 can abort the first headless import while creating the project
# cache in a clean worktree. Retry once; a real failure still fails.
if ! "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1; then
	"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null
fi

MENU_CONTRACT='^MAIN_MENU status=ok buttons=6 focus=new_city checks=[0-9]+ checksum=[0-9a-f]{8}$'
SETTINGS_CONTRACT='^MAIN_MENU_SETTINGS status=ok volume=[0-9]+ muted=(true|false) checks=[0-9]+$'

# Run the menu smoke once and echo its marker on stdout. Diagnostics go to
# stderr so the caller can compare the two markers as plain strings.
run_menu() {
	local label="$1"
	local output godot_status marker settings

	set +e
	output="$(timeout 60s "$GODOT_BIN" --headless --path "$ROOT/godot" -- --menu-smoke 2>&1)"
	godot_status=$?
	set -e
	printf '%s\n' "$output" >&2

	if [[ $godot_status -ne 0 ]]; then
		echo "Main menu smoke failed ($label): Godot exited with status $godot_status" >&2
		exit 1
	fi

	marker="$(grep '^MAIN_MENU status=' <<<"$output" | tail -1 || true)"
	if [[ -z "$marker" ]]; then
		echo "Main menu smoke failed ($label): no MAIN_MENU status marker" >&2
		exit 1
	fi
	if [[ ! "$marker" =~ $MENU_CONTRACT ]]; then
		echo "Main menu smoke contract mismatch ($label): $marker" >&2
		exit 1
	fi
	if grep -q '^MAIN_MENU status=failed' <<<"$output"; then
		echo "Main menu smoke failed ($label): a failed marker was printed" >&2
		exit 1
	fi
	settings="$(grep '^MAIN_MENU_SETTINGS status=' <<<"$output" | tail -1 || true)"
	if [[ -z "$settings" ]]; then
		echo "Main menu smoke failed ($label): no MAIN_MENU_SETTINGS marker" >&2
		exit 1
	fi
	if [[ ! "$settings" =~ $SETTINGS_CONTRACT ]]; then
		echo "Main menu settings contract mismatch ($label): $settings" >&2
		exit 1
	fi
	printf '%s\n%s\n' "$marker" "$settings"
}

FIRST_ALL="$(run_menu first)"
FIRST="$(sed -n '1p' <<<"$FIRST_ALL")"
FIRST_SETTINGS="$(sed -n '2p' <<<"$FIRST_ALL")"
SECOND_ALL="$(run_menu second)"
SECOND="$(sed -n '1p' <<<"$SECOND_ALL")"
SECOND_SETTINGS="$(sed -n '2p' <<<"$SECOND_ALL")"

if [[ "$FIRST" != "$SECOND" ]]; then
	echo "Main menu smoke is not deterministic:" >&2
	echo "  first:  $FIRST" >&2
	echo "  second: $SECOND" >&2
	exit 1
fi
if [[ "$FIRST_SETTINGS" != "$SECOND_SETTINGS" ]]; then
	echo "Main menu settings are not deterministic:" >&2
	echo "  first:  $FIRST_SETTINGS" >&2
	echo "  second: $SECOND_SETTINGS" >&2
	exit 1
fi
echo "MAIN_MENU_SETTINGS_SMOKE runs=2 ${FIRST_SETTINGS#MAIN_MENU_SETTINGS }"
echo "MAIN_MENU_SMOKE runs=2 ${FIRST#MAIN_MENU }"

# Forwarding: the old `-- --smoke` entry point must still reach main.tscn.
set +e
FORWARD_OUTPUT="$(timeout 90s "$GODOT_BIN" --headless --path "$ROOT/godot" -- --smoke 2>&1)"
FORWARD_STATUS=$?
set -e
printf '%s\n' "$FORWARD_OUTPUT" >&2

if [[ $FORWARD_STATUS -ne 0 ]]; then
	echo "Forward smoke failed: Godot exited with status $FORWARD_STATUS" >&2
	exit 1
fi
FORWARD_LINE="$(grep '^SMOKE chunks=' <<<"$FORWARD_OUTPUT" | tail -1 || true)"
if [[ -z "$FORWARD_LINE" ]]; then
	echo "Forward smoke failed: no SMOKE marker, --smoke did not reach main.tscn" >&2
	exit 1
fi
FORWARD_CHUNKS="$(sed -n 's/.*chunks=\([0-9][0-9]*\).*/\1/p' <<<"$FORWARD_LINE")"
FORWARD_STATE="$(sed -n 's/.*status=\([^ ]*\).*/\1/p' <<<"$FORWARD_LINE")"
if [[ "$FORWARD_STATE" != "ok" || -z "$FORWARD_CHUNKS" || "$FORWARD_CHUNKS" -lt 9 ]]; then
	echo "Forward smoke contract mismatch: $FORWARD_LINE" >&2
	exit 1
fi
echo "MAIN_MENU_FORWARD --smoke chunks=$FORWARD_CHUNKS status=ok"
