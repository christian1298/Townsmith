#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="${1:-debug}"

if [[ "$MODE" == "release" ]]; then
	cargo build --manifest-path "$ROOT/Cargo.toml" --release -p godot_ext
	SOURCE="$ROOT/target/release/libtownsmith_core.so"
else
	cargo build --manifest-path "$ROOT/Cargo.toml" -p godot_ext
	SOURCE="$ROOT/target/debug/libtownsmith_core.so"
fi

mkdir -p "$ROOT/godot/bin"
cp "$SOURCE" "$ROOT/godot/bin/libtownsmith_core.so"
echo "Copied $SOURCE -> $ROOT/godot/bin/libtownsmith_core.so"
