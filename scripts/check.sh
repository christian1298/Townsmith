#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

"$ROOT/scripts/check_locale.sh"
"$ROOT/scripts/check_content_schema.py"

cargo fmt --manifest-path "$ROOT/Cargo.toml" --all --check
cargo clippy --manifest-path "$ROOT/Cargo.toml" --workspace --all-targets -- -D warnings
cargo test --manifest-path "$ROOT/Cargo.toml" --workspace
