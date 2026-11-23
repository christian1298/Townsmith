#!/usr/bin/env bash
# Headless smoke for the standalone 3D interest-view road + building preview.
#
# Runs `godot/interest_preview.tscn --smoke` twice. Each run writes and reloads
# a deterministic fixture (60 in-view + 4 out-of-view roads at the 64-road wire
# cap, plus malformed entries and no building list), builds the scene, checks
# counts/centreline bounds/camera/checksum, tears down, rebuilds, and prints the
# same `INTEREST_PREVIEW status=ok …` and `INTEREST_PREVIEW_BUILDINGS …` markers
# twice. It also probes missing/empty/wrong-typed and malformed building
# payloads, a 5-building synthetic view checked against CPU-side transform
# mirrors, and an 80-record over-cap payload that must clamp to 64 boxes and set
# `truncated=true`. This script compares those markers and every
# `INTEREST_PREVIEW*` line between the two processes.
#
# This is a visualization harness, not client render integration and not a
# benchmark gate. No display is required.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

if ! command -v "$GODOT_BIN" >/dev/null 2>&1; then
	echo "Godot binary not found: $GODOT_BIN (set GODOT=/path/to/godot)" >&2
	exit 2
fi

# The scene is standalone but the project loads the GDExtension. Build it only
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

run_preview() {
	local log="$1"
	local marker_file="$2"
	local output
	local godot_status
	set +e
	output="$(timeout 120s env -u DISPLAY -u WAYLAND_DISPLAY \
		"$GODOT_BIN" --headless --path "$ROOT/godot" res://interest_preview.tscn -- --smoke 2>&1)"
	godot_status=$?
	set -e
	# Keep the captured log visible without polluting the command substitution
	# that reads the marker file.
	printf '%s\n' "$output" >&2

	if [[ $godot_status -ne 0 ]]; then
		echo "Interest preview smoke failed: Godot exited with status $godot_status" >&2
		exit 1
	fi

	local -a markers=()
	while IFS= read -r line; do
		markers+=("$line")
	done < <(grep '^INTEREST_PREVIEW status=' <<<"$output" || true)
	if [[ ${#markers[@]} -ne 2 ]]; then
		echo "Interest preview smoke failed: expected two INTEREST_PREVIEW markers, got ${#markers[@]}" >&2
		exit 1
	fi
	if [[ "${markers[0]}" != "${markers[1]}" ]]; then
		echo "Interest preview smoke failed: markers differ within one run" >&2
		echo "  first:  ${markers[0]}" >&2
		echo "  second: ${markers[1]}" >&2
		exit 1
	fi
	if [[ ! "${markers[0]}" =~ ^INTEREST_PREVIEW\ status=ok\ roads=64\ instances=64\ truncated=false\ checksum=[0-9a-f]{16}$ ]]; then
		echo "Interest preview smoke contract mismatch: ${markers[0]}" >&2
		exit 1
	fi

	if ! grep -Eq '^INTEREST_PREVIEW_CAP raw=80 parsed=64$' <<<"$output"; then
		echo "Interest preview smoke failed: over-cap probe marker missing" >&2
		exit 1
	fi
	if ! grep -Eq '^INTEREST_PREVIEW_BUILDINGS status=ok buildings=0 boxes=0 truncated=false checksum=[0-9a-f]{16}$' <<<"$output"; then
		echo "Interest preview smoke failed: zero-building marker missing or mismatched" >&2
		exit 1
	fi
	if ! grep -Eq '^INTEREST_PREVIEW_BUILDINGS_TOLERANT missing=0 empty=0 wrong_type=0 malformed=4 parsed=1$' <<<"$output"; then
		echo "Interest preview smoke failed: building tolerance marker missing or mismatched" >&2
		exit 1
	fi
	if ! grep -Eq '^INTEREST_PREVIEW_BUILDINGS_VIEW status=ok buildings=5 boxes=5 truncated=false checksum=[0-9a-f]{16}$' <<<"$output"; then
		echo "Interest preview smoke failed: synthetic building view marker missing or mismatched" >&2
		exit 1
	fi
	if ! grep -Eq '^INTEREST_PREVIEW_BUILDINGS_CAP status=ok raw=80 buildings=64 boxes=64 truncated=true checksum=[0-9a-f]{16}$' <<<"$output"; then
		echo "Interest preview smoke failed: over-cap building marker missing or mismatched" >&2
		exit 1
	fi
	if ! grep -Eq '^INTEREST_PREVIEW_BUILDINGS_FLAG status=ok buildings=1 boxes=1 truncated=true$' <<<"$output"; then
		echo "Interest preview smoke failed: server truncation flag marker missing or mismatched" >&2
		exit 1
	fi
	if ! grep -Eq '^INTEREST_PREVIEW_TEARDOWN instances=0 nodes=0$' <<<"$output"; then
		echo "Interest preview smoke failed: teardown marker missing" >&2
		exit 1
	fi
	if ! grep -Eq '^INTEREST_PREVIEW_VIEW tick=42 center=120.0,-80.0 radius=96.0 raw=67 roads=64 inside=60 outside=4 truncated=false$' <<<"$output"; then
		echo "Interest preview smoke failed: view marker missing or mismatched" >&2
		exit 1
	fi
	if ! grep -Eq '^INTEREST_PREVIEW_GEOMETRY instances=64 multimesh=1 mesh=3 bounds=-?[0-9]+\.[0-9],-?[0-9]+\.[0-9],-?[0-9]+\.[0-9],-?[0-9]+\.[0-9] camera=ok$' <<<"$output"; then
		echo "Interest preview smoke failed: geometry marker missing or mismatched" >&2
		exit 1
	fi

	grep '^INTEREST_PREVIEW' <<<"$output" >"$log"
	printf '%s\n' "${markers[0]}" >"$marker_file"
}

run_preview "$SMOKE_DIR/first.txt" "$SMOKE_DIR/first.marker"
run_preview "$SMOKE_DIR/second.txt" "$SMOKE_DIR/second.marker"
FIRST_MARKER="$(cat "$SMOKE_DIR/first.marker")"
SECOND_MARKER="$(cat "$SMOKE_DIR/second.marker")"

if [[ "$FIRST_MARKER" != "$SECOND_MARKER" ]]; then
	echo "Interest preview smoke failed: markers differ between runs" >&2
	echo "  first:  $FIRST_MARKER" >&2
	echo "  second: $SECOND_MARKER" >&2
	exit 1
fi

if ! diff -u "$SMOKE_DIR/first.txt" "$SMOKE_DIR/second.txt"; then
	echo "Interest preview smoke failed: deterministic marker lines differ" >&2
	exit 1
fi

echo "INTEREST_PREVIEW_SMOKE OK ${FIRST_MARKER#INTEREST_PREVIEW }"
