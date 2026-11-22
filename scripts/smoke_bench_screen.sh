#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

if ! command -v "$GODOT_BIN" >/dev/null 2>&1; then
	echo "Godot binary not found: $GODOT_BIN (set GODOT=/path/to/godot)" >&2
	exit 2
fi

"$ROOT/scripts/build_godot_ext.sh" debug >/dev/null

# Godot 4.7 can abort the very first headless import while creating the
# project cache in a clean worktree. Retry once; a real failure still fails.
if ! "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1; then
	"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null
fi

SMOKE_DIR="$(mktemp -d)"
trap 'rm -rf "$SMOKE_DIR"' EXIT
REPORT="$SMOKE_DIR/benchmark_report.json"

set +e
OUTPUT="$(timeout 120s "$GODOT_BIN" --headless --path "$ROOT/godot" res://bench_screen.tscn -- \
	--smoke --output "$REPORT" 2>&1)"
GODOT_STATUS=$?
set -e
printf '%s\n' "$OUTPUT"

if [[ $GODOT_STATUS -ne 0 ]]; then
	echo "Benchmark screen smoke failed: Godot exited with status $GODOT_STATUS" >&2
	exit 1
fi

LINE="$(grep "^BENCH_SCREEN " <<<"$OUTPUT" | tail -1 || true)"
if [[ -z "$LINE" ]]; then
	echo "Benchmark screen smoke failed: no BENCH_SCREEN marker" >&2
	exit 1
fi
if [[ "$LINE" != BENCH_SCREEN\ status=ok\ scenes=3\ rating=* ]]; then
	echo "Benchmark screen smoke contract mismatch: $LINE" >&2
	exit 1
fi
if ! grep -q '^BENCH_SCREEN_GRAPHS OK scenes=3$' <<<"$OUTPUT"; then
	echo "Benchmark screen smoke failed: graph marker missing" >&2
	exit 1
fi
RATING="${LINE##*rating=}"
RATING="${RATING%% *}"
case "$RATING" in
	excellent|good|playable|low) ;;
	*)
		echo "Benchmark screen smoke failed: unexpected rating '$RATING'" >&2
		exit 1
		;;
esac

if [[ ! -s "$REPORT" ]]; then
	echo "Benchmark screen smoke failed: report was not written ($REPORT)" >&2
	exit 1
fi

python3 - "$REPORT" <<'PY'
import json
import math
import sys

path = sys.argv[1]
with open(path, encoding="utf-8") as handle:
    report = json.load(handle)

ratings = {"excellent", "good", "playable", "low"}
graph_sources = {"fixture", "aggregated", "summary"}
assert report["suite"] == "bench_screen_report", report.get("suite")
assert report["overall"]["rating"] in ratings, report["overall"]
scenes = report["scenes"]
assert len(scenes) == 3, len(scenes)
assert [scene["id"] for scene in scenes] == ["early", "mid", "late"]
for scene in scenes:
    assert scene["source_file"], scene
    assert scene["rating"] in ratings, scene
    assert scene["median_fps"] > 0, scene
    assert scene["samples"] >= 1, scene
    assert scene["recommended_settings"]["resolution_scale_percent"] > 0, scene

    graph = scene.get("frame_graph")
    assert isinstance(graph, dict) and graph, scene.get("id")
    assert graph["source"] in graph_sources, graph.get("source")
    # --smoke always exercises the embedded deterministic fixtures.
    assert graph["source"] == "fixture", (scene["id"], graph["source"])
    samples = graph["samples"]
    assert isinstance(samples, list) and len(samples) >= 2, (scene["id"], len(samples))
    assert graph["sample_count"] == len(samples), scene["id"]
    for sample in samples:
        assert isinstance(sample, (int, float)) and math.isfinite(sample), sample
        assert sample > 0, sample
    p50 = graph["p50_ms"]
    p95 = graph["p95_ms"]
    p99 = graph["p99_ms"]
    peak = graph["max_ms"]
    for value in (p50, p95, p99, peak):
        assert isinstance(value, (int, float)) and math.isfinite(value), value
        assert value > 0, value
    assert p50 <= p95 <= p99 <= peak, (scene["id"], p50, p95, p99, peak)
    axis = graph["axis"]
    assert axis["x_min"] == 0, axis
    assert axis["x_max"] == len(samples) - 1, axis
    assert axis["y_min_ms"] == 0, axis
    assert axis["y_max_ms"] >= peak > axis["y_min_ms"], axis
    assert sorted(graph["reference_ms"]) == [16.7, 33.3], graph.get("reference_ms")
print("BENCH_SCREEN_EXPORT OK scenes=3 graphs=3 rating=%s" % report["overall"]["rating"])
PY

echo "BENCH_SCREEN_SMOKE OK rating=${RATING}"
