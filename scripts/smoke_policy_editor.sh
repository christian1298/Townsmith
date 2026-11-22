#!/usr/bin/env bash
# Headless smoke for the standalone district policy editor.
#
# Runs `godot/policy_editor.tscn --smoke` twice, requires the bridge CRUD
# walkthrough (create two, list, update one, reject an invalid radius, remove
# both), and compares every `POLICY_EDITOR*` line between runs so the state
# checksum is proven deterministic. No display or human input is required.
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

run_editor() {
	local log="$1"
	local marker_file="$2"
	local output
	local godot_status
	set +e
	output="$(timeout 120s "$GODOT_BIN" --headless --path "$ROOT/godot" res://policy_editor.tscn -- --smoke 2>&1)"
	godot_status=$?
	set -e
	# Keep the captured log visible without polluting the command substitution
	# that reads the marker file.
	printf '%s\n' "$output" >&2

	if [[ $godot_status -ne 0 ]]; then
		echo "Policy editor smoke failed: Godot exited with status $godot_status" >&2
		exit 1
	fi

	local marker
	marker="$(grep '^POLICY_EDITOR status=' <<<"$output" | tail -1 || true)"
	if [[ -z "$marker" ]]; then
		echo "Policy editor smoke failed: no POLICY_EDITOR status marker" >&2
		exit 1
	fi
	if [[ ! "$marker" =~ ^POLICY_EDITOR\ status=ok\ policies=0\ checksum=[0-9a-f]{16}$ ]]; then
		echo "Policy editor smoke contract mismatch: $marker" >&2
		exit 1
	fi

	if ! grep -Eq '^POLICY_EDITOR_REJECT code=invalid_radius count=2$' <<<"$output"; then
		echo "Policy editor smoke failed: invalid-radius rejection marker missing" >&2
		exit 1
	fi
	if ! grep -Eq '^POLICY_EDITOR_OVERLAP touching=yes apart=no$' <<<"$output"; then
		echo "Policy editor smoke failed: overlap marker missing" >&2
		exit 1
	fi

	grep '^POLICY_EDITOR' <<<"$output" | grep -v '^POLICY_EDITOR status=' >"$log"
	printf '%s\n' "$marker" >"$marker_file"
}

run_editor "$SMOKE_DIR/first.txt" "$SMOKE_DIR/first.marker"
run_editor "$SMOKE_DIR/second.txt" "$SMOKE_DIR/second.marker"
FIRST_MARKER="$(cat "$SMOKE_DIR/first.marker")"
SECOND_MARKER="$(cat "$SMOKE_DIR/second.marker")"

if [[ "$FIRST_MARKER" != "$SECOND_MARKER" ]]; then
	echo "Policy editor smoke failed: state markers differ between runs" >&2
	echo "  first:  $FIRST_MARKER" >&2
	echo "  second: $SECOND_MARKER" >&2
	exit 1
fi

if ! diff -u "$SMOKE_DIR/first.txt" "$SMOKE_DIR/second.txt"; then
	echo "Policy editor smoke failed: deterministic marker lines differ" >&2
	exit 1
fi

echo "POLICY_EDITOR_SMOKE OK ${FIRST_MARKER#POLICY_EDITOR }"
