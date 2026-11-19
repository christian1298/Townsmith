#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

"$ROOT/scripts/build_godot_ext.sh" debug >/dev/null
"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null
exec "$GODOT_BIN" --headless --path "$ROOT/godot" --quit-after 3
