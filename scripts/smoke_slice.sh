#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

"$ROOT/scripts/build_godot_ext.sh" debug >/dev/null

# Godot 4.7 can abort the very first headless import while creating the
# project cache in a clean worktree. Retry once; a real failure still fails.
if ! "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1; then
	"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null
fi

# The slice benchmark scene owns its structural smoke path.
set +e
OUTPUT="$(timeout 120s "$GODOT_BIN" --headless --path "$ROOT/godot" res://slice_benchmark.tscn -- --smoke 2>&1)"
GODOT_STATUS=$?
set -e
printf '%s\n' "$OUTPUT"

if [[ $GODOT_STATUS -ne 0 ]]; then
	echo "Slice house smoke failed: Godot exited with status $GODOT_STATUS" >&2
	exit 1
fi

LINE="$(grep "SLICE_HOUSE_SMOKE " <<<"$OUTPUT" | tail -1 || true)"
if [[ -z "$LINE" ]]; then
	echo "Slice house smoke failed: no marker" >&2
	exit 1
fi
INSTANCES="$(sed -n 's/.*instances=\([0-9][0-9]*\).*/\1/p' <<<"$LINE")"
QUADS="$(sed -n 's/.*quads=\([0-9][0-9]*\).*/\1/p' <<<"$LINE")"
STATUS="$(sed -n 's/.*status=\([a-z_=0-9]*\).*/\1/p' <<<"$LINE")"
if [[ "$INSTANCES" != "21" || "$STATUS" != "ok" ]]; then
	echo "Slice house smoke contract mismatch: $LINE" >&2
	exit 1
fi
if [[ -z "$QUADS" || "$QUADS" -lt 1 ]]; then
	echo "Slice house smoke failed: no mesh quads in '$LINE'" >&2
	exit 1
fi

# A stale cached extension is a common failure; make the build marker visible.
grep -q "^SLICE_HOUSES " <<<"$OUTPUT" || {
	echo "Slice house smoke failed: no SLICE_HOUSES build marker" >&2
	exit 1
}
BUILD_LINE="$(grep "^SLICE_HOUSES " <<<"$OUTPUT" | tail -1)"
if [[ "$BUILD_LINE" != *"instances=21"* || "$BUILD_LINE" != *"surfaces=1"* || "$BUILD_LINE" != *"solid_voxels=266432"* || "$BUILD_LINE" != *"quads=293"* ]]; then
	echo "Slice house smoke contract mismatch: $BUILD_LINE" >&2
	exit 1
fi

echo "SLICE_HOUSE_SMOKE OK instances=${INSTANCES} quads=${QUADS}"
