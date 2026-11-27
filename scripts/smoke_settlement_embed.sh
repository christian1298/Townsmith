#!/usr/bin/env bash
set -euo pipefail

# Shared-core embedded settlement screen smoke. Runs the same scene twice and
# requires identical markers; the host core must survive the screen leaving
# the tree and keep its settlement checksum.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

if ! command -v "$GODOT_BIN" >/dev/null 2>&1; then
	echo "Godot binary not found: $GODOT_BIN (set GODOT=/path/to/godot)" >&2
	exit 2
fi

run_once() {
	timeout 60s "$GODOT_BIN" --headless --path "$ROOT/godot" res://settlement_embed_smoke.tscn 2>&1
}

FIRST="$(run_once)"
SECOND="$(run_once)"
FIRST_MARKER="$(grep '^SETTLEMENT_EMBED_SMOKE ' <<<"$FIRST" | tail -1 || true)"
SECOND_MARKER="$(grep '^SETTLEMENT_EMBED_SMOKE ' <<<"$SECOND" | tail -1 || true)"

if [[ -z "$FIRST_MARKER" || "$FIRST_MARKER" != "$SECOND_MARKER" ]]; then
	echo "Settlement embed smoke failed: markers differ or missing" >&2
	echo " first:  $FIRST_MARKER"
	echo " second: $SECOND_MARKER"
	exit 1
fi
if [[ "$FIRST_MARKER" != SETTLEMENT_EMBED_SMOKE\ status=ok\ settlements=1\ * ]]; then
	echo "Settlement embed smoke contract mismatch: $FIRST_MARKER" >&2
	exit 1
fi

echo "SETTLEMENT_EMBED_SMOKE status=ok runs=2 ${FIRST_MARKER#SETTLEMENT_EMBED_SMOKE status=ok }"
