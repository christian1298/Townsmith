#!/usr/bin/env bash
# Headless synthetic smoke for the fail-closed content-mismatch screen.
#
# No server and no socket: this only exercises `content_mismatch_screen.gd`
# through a fresh scene instance twice. Each run builds two panel instances,
# checks the hidden initial state, `set_failure` values, the rendered labels,
# and the Retry/Quit signals. Both Godot runs must print the same marker:
#
#   CONTENT_MISMATCH_UI status=ok runs=2 code=content_hash.mismatch checksum=<8 hex>
#
# Set GODOT=/path/to/godot when Godot is not on PATH.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

if ! command -v "$GODOT_BIN" >/dev/null 2>&1; then
	echo "Godot binary not found: $GODOT_BIN (set GODOT=/path/to/godot)" >&2
	exit 2
fi

# The screen itself is pure GDScript, but the project imports its GDExtension.
# Build only when the library is not already present.
if [[ ! -f "$ROOT/godot/bin/libtownsmith_core.so" ]]; then
	echo "Building debug GDExtension..."
	"$ROOT/scripts/build_godot_ext.sh" debug >/dev/null
fi

# Godot 4.7 can abort the first headless import while creating the project
# cache in a clean worktree. Retry once; a real failure still fails.
if ! "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1; then
	"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null
fi

SMOKE_DIR="$(mktemp -d)"
trap 'rm -rf "$SMOKE_DIR"' EXIT

CONTRACT='^CONTENT_MISMATCH_UI status=ok runs=2 code=content_hash[.]mismatch checksum=[0-9a-f]{8}$'

run_screen() {
	local log="$1"
	local marker_file="$2"
	local output
	local godot_status
	set +e
	output="$(timeout 120s "$GODOT_BIN" --headless --path "$ROOT/godot" res://content_mismatch_screen.tscn -- --smoke 2>&1)"
	godot_status=$?
	set -e
	printf '%s\n' "$output" >&2

	if [[ $godot_status -ne 0 ]]; then
		echo "Content mismatch screen failed: Godot exited with status $godot_status" >&2
		exit 1
	fi

	if grep -Eq 'SCRIPT ERROR|Parse Error|Failed to load script' <<<"$output"; then
		echo "Content mismatch screen failed: Godot reported a script error" >&2
		exit 1
	fi

	local marker
	marker="$(grep '^CONTENT_MISMATCH_UI status=' <<<"$output" | tail -1 || true)"
	if [[ -z "$marker" ]]; then
		echo "Content mismatch screen failed: no CONTENT_MISMATCH_UI status marker" >&2
		exit 1
	fi
	if [[ ! "$marker" =~ $CONTRACT ]]; then
		echo "Content mismatch screen contract mismatch: $marker" >&2
		exit 1
	fi

	if grep -Eq '^CONTENT_MISMATCH_UI status=fail|^CONTENT_MISMATCH_UI_FAIL' <<<"$output"; then
		echo "Content mismatch screen failed: a failure marker or check was printed" >&2
		exit 1
	fi

	grep '^CONTENT_MISMATCH_UI' <<<"$output" >"$log" || true
	printf '%s\n' "$marker" >"$marker_file"
}

run_screen "$SMOKE_DIR/first.txt" "$SMOKE_DIR/first.marker"
run_screen "$SMOKE_DIR/second.txt" "$SMOKE_DIR/second.marker"

FIRST_MARKER="$(cat "$SMOKE_DIR/first.marker")"
SECOND_MARKER="$(cat "$SMOKE_DIR/second.marker")"

if [[ "$FIRST_MARKER" != "$SECOND_MARKER" ]]; then
	echo "Content mismatch screen failed: markers differ between runs" >&2
	echo "  first:  $FIRST_MARKER" >&2
	echo "  second: $SECOND_MARKER" >&2
	exit 1
fi

if ! diff -u "$SMOKE_DIR/first.txt" "$SMOKE_DIR/second.txt"; then
	echo "Content mismatch screen failed: deterministic stage lines differ" >&2
	exit 1
fi

echo "$SECOND_MARKER"
