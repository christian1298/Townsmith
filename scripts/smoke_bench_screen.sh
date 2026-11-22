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
GATE_MISSING="$SMOKE_DIR/gate-missing"
GATE_PRESENT="$SMOKE_DIR/gate-present"
GATE_FAIL="$SMOKE_DIR/gate-fail"
mkdir -p "$GATE_MISSING" "$GATE_PRESENT" "$GATE_FAIL"

# Deterministic missing state: an empty gate directory, whatever the checkout
# happens to hold locally under benchmarks/results/gate/.
set +e
OUTPUT="$(timeout 120s "$GODOT_BIN" --headless --path "$ROOT/godot" res://bench_screen.tscn -- \
	--smoke --gate-dir "$GATE_MISSING" --output "$REPORT" 2>&1)"
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
if ! grep -q '^BENCH_SCREEN_GATE render=missing cohort=missing$' <<<"$OUTPUT"; then
	echo "Benchmark screen smoke failed: reference-gate missing marker mismatch" >&2
	exit 1
fi
if ! grep -q '^BENCH_SCREEN_GATE_EVAL render=insufficient cohort=insufficient$' <<<"$OUTPUT"; then
	echo "Benchmark screen smoke failed: reference-gate missing evaluation mismatch" >&2
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

gate = report.get("reference_gate")
assert isinstance(gate, dict) and set(gate) == {"render", "cohort"}, gate
expected = {
    "render": "benchmarks/results/gate/late_metropolis_v1.json",
    "cohort": "benchmarks/results/gate/cohort_1m_v1.json",
}
for gate_id, file_rel in expected.items():
    entry = gate[gate_id]
    assert entry["id"] == gate_id, entry
    assert entry["file"] == file_rel, entry
    assert entry["state"] == "missing", entry
    assert entry["recorded"] is False, entry
    assert entry["note"], entry
    assert entry["evaluation"]["status"] == "insufficient", entry
    assert entry["evaluation"]["reason"] == "no_recorded_file", entry
print("BENCH_SCREEN_EXPORT OK scenes=3 graphs=3 rating=%s" % report["overall"]["rating"])
PY

# Optional present-state fixture: the evaluator path is exercised against a
# copied synthetic result in a temp directory, never against a committed file.
cat >"$GATE_PRESENT/late_metropolis_v1.json" <<'JSON'
{
  "format_version": 1,
  "gate": "G5",
  "config": "C1_local_server",
  "scene": "late_metropolis_v1",
  "route": "late_flyover_v1",
  "settings": "standard_1080p_high_v1",
  "duration_s": 210,
  "warmup_s": 15,
  "runs": 3,
  "headless": false,
  "hardware": {
    "cpu": "AMD Ryzen 7 5800X 8-Core Processor",
    "cpu_cores": 8,
    "cpu_threads": 16,
    "gpu": "NVIDIA GeForce RTX 3060 12 GB",
    "driver": "580.00",
    "ram_gb": 32,
    "display": "1920x1080",
    "os": "Windows 11"
  },
  "results": {
    "fps_avg": 31.4,
    "fps_low_1pct": 24.8,
    "fps_low_0_1pct": 18.2,
    "frametime_ms_p95": 38.0,
    "frametime_ms_p99": 44.0,
    "frametime_ms_p99_9": 55.0,
    "frametime_ms_max": 120.0,
    "hitches_50ms": 1,
    "stalls_250ms": 0,
    "streaming_stalls_100ms": 0,
    "dropped_chunks": 0,
    "draw_calls_p99": 1840,
    "triangles_avg": 7600000,
    "vram_mb_peak": 9800,
    "ram_mb_peak": 12000,
    "save_ms": 24000,
    "load_ms": 21000,
    "save_bytes": 182808978,
    "overhead_pct": 1.2,
    "fps_avg_spread_pct": 2.4,
    "valid": true
  },
  "valid": true
}
JSON
cat >"$GATE_PRESENT/cohort_1m_v1.json" <<'JSON'
{
  "format_version": 1,
  "gate": "G5",
  "config": "C2_dedicated_server",
  "scene": "cohort_1m_v1",
  "route": "cohort_3600_ticks_v1",
  "settings": "headless_1hz_v1",
  "duration_s": 3600,
  "warmup_s": 0,
  "runs": 3,
  "headless": true,
  "hardware": {
    "cpu": "AMD Ryzen 7 5800X 8-Core Processor",
    "cpu_cores": 8,
    "cpu_threads": 16,
    "ram_gb": 32,
    "os": "Linux"
  },
  "results": {
    "ticks": 3600,
    "cohort_tick_ms_p99": 8.1,
    "full_tick_ms_p99": 40.0,
    "active_agents_ms_p99": 8.5,
    "pathfinding_ms_p99": 2.0,
    "sim_cpu_ms_per_s": 180.0,
    "server_rss_mb_peak": 3500,
    "server_network_stall_ms_max": 4.0,
    "save_ms": 24000,
    "load_ms": 25000,
    "save_bytes": 182808978,
    "traffic_build_ms": 200,
    "traffic_incremental_ms_p99": 10,
    "traffic_flow_ms_p99": 3,
    "traffic_queries_5000_ms": 8,
    "valid": true
  },
  "valid": true
}
JSON

REPORT_PRESENT="$SMOKE_DIR/benchmark_report_present.json"
set +e
OUTPUT_PRESENT="$(timeout 120s "$GODOT_BIN" --headless --path "$ROOT/godot" res://bench_screen.tscn -- \
	--smoke --gate-dir "$GATE_PRESENT" --output "$REPORT_PRESENT" 2>&1)"
GODOT_STATUS=$?
set -e
printf '%s\n' "$OUTPUT_PRESENT"

if [[ $GODOT_STATUS -ne 0 ]]; then
	echo "Benchmark screen fixture smoke failed: Godot exited with status $GODOT_STATUS" >&2
	exit 1
fi
if ! grep -q '^BENCH_SCREEN_GATE render=present cohort=present$' <<<"$OUTPUT_PRESENT"; then
	echo "Benchmark screen fixture smoke failed: reference-gate present marker mismatch" >&2
	exit 1
fi
if ! grep -q '^BENCH_SCREEN_GATE_EVAL render=pass cohort=pass$' <<<"$OUTPUT_PRESENT"; then
	echo "Benchmark screen fixture smoke failed: reference-gate pass evaluation mismatch" >&2
	exit 1
fi
if ! grep -q '^BENCH_SCREEN status=ok scenes=3 rating=' <<<"$OUTPUT_PRESENT"; then
	echo "Benchmark screen fixture smoke failed: BENCH_SCREEN marker mismatch" >&2
	exit 1
fi

python3 - "$REPORT_PRESENT" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    report = json.load(handle)

gate = report.get("reference_gate")
assert isinstance(gate, dict), gate
expected_scenes = {"render": "late_metropolis_v1", "cohort": "cohort_1m_v1"}
for gate_id, scene in expected_scenes.items():
    entry = gate[gate_id]
    assert entry["state"] == "present", entry
    assert entry["recorded"] is True, entry
    assert entry["scene"] == scene, entry
    assert entry["hardware"], entry
    assert entry["runs"] == 3, entry
    assert entry["duration_s"] > 0, entry
    metric = entry["key_metric"]
    assert metric["text"] and metric["value"] > 0, entry
    evaluation = entry["evaluation"]
    assert evaluation["status"] == "pass", evaluation
    assert evaluation["line"].startswith("METROPOLIS_GATE status=pass"), evaluation
    assert evaluation["exit_code"] == 0, evaluation
print("BENCH_SCREEN_GATE_EXPORT OK render=pass cohort=pass")
PY

# Fail fixture: identical provenance, two metrics outside docs/68 §8.2
# (render average 20 FPS, cohort tick p99 20 ms). Derived from the present
# fixture so the only difference is the intended failure.
python3 - "$GATE_PRESENT" "$GATE_FAIL" <<'PY'
import json
import pathlib
import sys

present, failing = (pathlib.Path(value) for value in sys.argv[1:3])
render = json.loads((present / "late_metropolis_v1.json").read_text(encoding="utf-8"))
render["results"]["fps_avg"] = 20.0
(failing / "late_metropolis_v1.json").write_text(
    json.dumps(render, indent=2) + "\n", encoding="utf-8")
cohort = json.loads((present / "cohort_1m_v1.json").read_text(encoding="utf-8"))
cohort["results"]["cohort_tick_ms_p99"] = 20.0
(failing / "cohort_1m_v1.json").write_text(
    json.dumps(cohort, indent=2) + "\n", encoding="utf-8")
PY

REPORT_FAIL="$SMOKE_DIR/benchmark_report_fail.json"
set +e
OUTPUT_FAIL="$(timeout 120s "$GODOT_BIN" --headless --path "$ROOT/godot" res://bench_screen.tscn -- \
	--smoke --gate-dir "$GATE_FAIL" --output "$REPORT_FAIL" 2>&1)"
GODOT_STATUS=$?
set -e
printf '%s\n' "$OUTPUT_FAIL"

if [[ $GODOT_STATUS -ne 0 ]]; then
	echo "Benchmark screen fail-fixture smoke failed: Godot exited with status $GODOT_STATUS" >&2
	exit 1
fi
if ! grep -q '^BENCH_SCREEN_GATE render=present cohort=present$' <<<"$OUTPUT_FAIL"; then
	echo "Benchmark screen fail-fixture smoke failed: reference-gate present marker mismatch" >&2
	exit 1
fi
if ! grep -q '^BENCH_SCREEN_GATE_EVAL render=fail cohort=fail$' <<<"$OUTPUT_FAIL"; then
	echo "Benchmark screen fail-fixture smoke failed: reference-gate fail evaluation mismatch" >&2
	exit 1
fi
if ! grep -q '^BENCH_SCREEN status=ok scenes=3 rating=' <<<"$OUTPUT_FAIL"; then
	echo "Benchmark screen fail-fixture smoke failed: BENCH_SCREEN marker mismatch" >&2
	exit 1
fi

python3 - "$REPORT_FAIL" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    report = json.load(handle)

gate = report.get("reference_gate")
assert isinstance(gate, dict), gate
expected = {
    "render": ("late_metropolis_v1", "fps_avg", 20.0),
    "cohort": ("cohort_1m_v1", "cohort_tick_ms_p99", 20.0),
}
for gate_id, (scene, metric_name, metric_value) in expected.items():
    entry = gate[gate_id]
    assert entry["state"] == "present", entry
    assert entry["recorded"] is True, entry
    assert entry["scene"] == scene, entry
    assert entry["runs"] == 3, entry
    metric = entry["key_metric"]
    assert metric["name"] == metric_name, metric
    assert metric["value"] == metric_value, metric
    evaluation = entry["evaluation"]
    assert evaluation["status"] == "fail", evaluation
    assert evaluation["line"].startswith("METROPOLIS_GATE status=fail"), evaluation
    assert evaluation["exit_code"] == 1, evaluation
print("BENCH_SCREEN_GATE_EXPORT OK render=fail cohort=fail")
PY

echo "BENCH_SCREEN_GATE_CASES OK missing=insufficient present=pass fail=fail"

# Interpreter resolution: OS.execute reports a missing program as 127, not only
# -1, so the python3 -> python fallback must continue after a 127. Both cases
# run the present fixture under a controlled PATH: one exposes only `python`,
# one exposes no interpreter at all. The host Python is resolved once up front.
PYTHON3_BIN="$(command -v python3 || true)"
if [[ -z "$PYTHON3_BIN" ]]; then
	echo "Benchmark screen smoke failed: python3 is required for the gate checks" >&2
	exit 2
fi
GODOT_ABS="$(command -v "$GODOT_BIN")"
GATE_PYTHON_ONLY="$SMOKE_DIR/gate-python-only-path"
GATE_NO_PYTHON="$SMOKE_DIR/gate-no-python-path"
mkdir -p "$GATE_PYTHON_ONLY" "$GATE_NO_PYTHON"
ln -s "$PYTHON3_BIN" "$GATE_PYTHON_ONLY/python"

REPORT_PYTHON_ONLY="$SMOKE_DIR/benchmark_report_python_only.json"
set +e
OUTPUT_PYTHON_ONLY="$(timeout 120s env PATH="$GATE_PYTHON_ONLY" "$GODOT_ABS" --headless --path "$ROOT/godot" res://bench_screen.tscn -- \
	--smoke --gate-dir "$GATE_PRESENT" --output "$REPORT_PYTHON_ONLY" 2>&1)"
GODOT_STATUS=$?
set -e
printf '%s\n' "$OUTPUT_PYTHON_ONLY"

if [[ $GODOT_STATUS -ne 0 ]]; then
	echo "Benchmark screen python-only smoke failed: Godot exited with status $GODOT_STATUS" >&2
	exit 1
fi
if ! grep -q '^BENCH_SCREEN_GATE_EVAL render=pass cohort=pass$' <<<"$OUTPUT_PYTHON_ONLY"; then
	echo "Benchmark screen python-only smoke failed: python fallback did not evaluate" >&2
	exit 1
fi
if ! grep -q '^BENCH_SCREEN status=ok scenes=3 rating=' <<<"$OUTPUT_PYTHON_ONLY"; then
	echo "Benchmark screen python-only smoke failed: BENCH_SCREEN marker mismatch" >&2
	exit 1
fi
python3 - "$REPORT_PYTHON_ONLY" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    report = json.load(handle)

for gate_id in ("render", "cohort"):
    evaluation = report["reference_gate"][gate_id]["evaluation"]
    assert evaluation["status"] == "pass", evaluation
    assert evaluation["exit_code"] == 0, evaluation
print("BENCH_SCREEN_GATE_FALLBACK OK python_only=pass")
PY

REPORT_NO_PYTHON="$SMOKE_DIR/benchmark_report_no_python.json"
set +e
OUTPUT_NO_PYTHON="$(timeout 120s env PATH="$GATE_NO_PYTHON" "$GODOT_ABS" --headless --path "$ROOT/godot" res://bench_screen.tscn -- \
	--smoke --gate-dir "$GATE_PRESENT" --output "$REPORT_NO_PYTHON" 2>&1)"
GODOT_STATUS=$?
set -e
printf '%s\n' "$OUTPUT_NO_PYTHON"

if [[ $GODOT_STATUS -ne 0 ]]; then
	echo "Benchmark screen no-python smoke failed: Godot exited with status $GODOT_STATUS" >&2
	exit 1
fi
if ! grep -q '^BENCH_SCREEN_GATE render=present cohort=present$' <<<"$OUTPUT_NO_PYTHON"; then
	echo "Benchmark screen no-python smoke failed: present marker mismatch" >&2
	exit 1
fi
if ! grep -q '^BENCH_SCREEN_GATE_EVAL render=insufficient cohort=insufficient$' <<<"$OUTPUT_NO_PYTHON"; then
	echo "Benchmark screen no-python smoke failed: evaluation was not insufficient" >&2
	exit 1
fi
if ! grep -q '^BENCH_SCREEN status=ok scenes=3 rating=' <<<"$OUTPUT_NO_PYTHON"; then
	echo "Benchmark screen no-python smoke failed: BENCH_SCREEN marker mismatch" >&2
	exit 1
fi
python3 - "$REPORT_NO_PYTHON" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    report = json.load(handle)

for gate_id in ("render", "cohort"):
    entry = report["reference_gate"][gate_id]
    assert entry["state"] == "present", entry
    evaluation = entry["evaluation"]
    assert evaluation["status"] == "insufficient", evaluation
    assert evaluation["reason"] == "python_missing", evaluation
    assert evaluation["exit_code"] == -1, evaluation
print("BENCH_SCREEN_GATE_NO_PYTHON OK render=insufficient cohort=insufficient reason=python_missing")
PY

echo "BENCH_SCREEN_SMOKE OK rating=${RATING}"
