#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"
OUT="$(mktemp -d)"
trap 'rm -rf "$OUT"' EXIT

"$ROOT/scripts/build_godot_ext.sh" debug >/dev/null

# Godot 4.7 can abort the very first headless import while creating the
# project cache in a clean worktree. Retry once; a real failure still fails.
if ! "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1; then
	"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null
fi

for run in 1 2; do
	set +e
	OUTPUT="$(timeout 120s "$GODOT_BIN" --headless --path "$ROOT/godot" \
		res://stream_order_smoke.tscn -- --output "$OUT/run_$run.json" 2>&1)"
	GODOT_STATUS=$?
	set -e
	printf '%s\n' "$OUTPUT" >"$OUT/run_$run.log"
	if [[ $GODOT_STATUS -ne 0 ]]; then
		cat "$OUT/run_$run.log"
		echo "Stream order smoke failed: Godot run $run exited with status $GODOT_STATUS" >&2
		exit 1
	fi
done

cat "$OUT/run_1.log"

python3 - "$OUT" <<'PY'
import json
import pathlib
import sys

out = pathlib.Path(sys.argv[1])
runs = []
for run in (1, 2):
    path = out / f"run_{run}.json"
    if not path.exists():
        raise SystemExit(f"stream order smoke failed: missing {path}")
    runs.append(json.loads(path.read_text()))

first, second = runs
for index, run in enumerate(runs, 1):
    if run.get("status") != "ok":
        raise SystemExit(f"stream order smoke failed: run {index} status={run.get('status')!r} error={run.get('error')!r}")
    if not str(run.get("checksum", "")):
        raise SystemExit(f"stream order smoke failed: run {index} has no checksum")
    if int(run.get("commits", 0)) < 25:
        raise SystemExit(f"stream order smoke failed: run {index} committed only {run.get('commits')!r} chunks")
    if int(run.get("loaded", 0)) < 25:
        raise SystemExit(f"stream order smoke failed: run {index} loaded only {run.get('loaded')!r} chunks")
    if len(run.get("stages", [])) != 4:
        raise SystemExit(f"stream order smoke failed: run {index} has {len(run.get('stages', []))} stages, expected 4")

checks = (
    ("checksum", first.get("checksum"), second.get("checksum")),
    ("commits", first.get("commits"), second.get("commits")),
    ("loaded", first.get("loaded"), second.get("loaded")),
    ("sequence", first.get("sequence"), second.get("sequence")),
    ("stage_checksums",
     [stage.get("checksum") for stage in first.get("stages", [])],
     [stage.get("checksum") for stage in second.get("stages", [])]),
)
for name, a, b in checks:
    if a != b:
        raise SystemExit(f"stream order smoke failed: {name} differs across runs: {a!r} != {b!r}")

print(
    "STREAM_ORDER_SMOKE status=ok runs=2 stream_order=ok "
    f"checksum={first['checksum']} commits={first['commits']} "
    f"loaded={first['loaded']} stages={len(first.get('stages', []))}"
)
PY
