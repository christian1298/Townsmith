#!/usr/bin/env bash
# Headless smoke for the standalone Q-101 playtest capture form.
#
# Runs `godot/playtest_capture.tscn` twice with `--smoke --output <tmp>`, checks
# the marker and the exported JSON (route stages, timer transitions, issue
# validation, schema), then compares the two payloads with `captured_at`
# removed. No display and no human input are required.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

if ! command -v "$GODOT_BIN" >/dev/null 2>&1; then
	echo "Godot binary not found: $GODOT_BIN (set GODOT=/path/to/godot)" >&2
	exit 2
fi

# The capture scene is standalone, but the project still points at the
# GDExtension. Build it only when it is missing so a clean worktree can run.
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
REPORT_ONE="$SMOKE_DIR/q101_capture_1.json"
REPORT_TWO="$SMOKE_DIR/q101_capture_2.json"

run_capture() {
	local report="$1"
	local output
	local godot_status
	set +e
	output="$(timeout 120s "$GODOT_BIN" --headless --path "$ROOT/godot" res://playtest_capture.tscn -- \
		--smoke --output "$report" 2>&1)"
	godot_status=$?
	set -e
	printf '%s\n' "$output"

	if [[ $godot_status -ne 0 ]]; then
		echo "Playtest capture smoke failed: Godot exited with status $godot_status" >&2
		exit 1
	fi

	local marker
	marker="$(grep '^PLAYTEST_CAPTURE ' <<<"$output" | tail -1 || true)"
	if [[ -z "$marker" ]]; then
		echo "Playtest capture smoke failed: no PLAYTEST_CAPTURE marker" >&2
		exit 1
	fi
	if [[ "$marker" != PLAYTEST_CAPTURE\ status=ok\ stages=8* ]]; then
		echo "Playtest capture smoke contract mismatch: $marker" >&2
		exit 1
	fi
	if [[ "$marker" != *"completed=8"* || "$marker" != *"issues=2"* || "$marker" != *"report=ok"* ]]; then
		echo "Playtest capture smoke failed: unexpected counts in $marker" >&2
		exit 1
	fi

	if ! grep -q '^PLAYTEST_CAPTURE_STAGES orient,starter_build,house,industry,utility,service,save_load,ratings$' <<<"$output"; then
		echo "Playtest capture smoke failed: route stage list mismatch" >&2
		exit 1
	fi
	if ! grep -q '^PLAYTEST_CAPTURE_TIMER start=ok pause=ok resume=ok reset=ok elapsed_s=120.000$' <<<"$output"; then
		echo "Playtest capture smoke failed: timer transition marker missing" >&2
		exit 1
	fi
	if ! grep -q '^PLAYTEST_CAPTURE_ISSUES empty=ignored high=accepted count=2$' <<<"$output"; then
		echo "Playtest capture smoke failed: issue validation marker missing" >&2
		exit 1
	fi
	if [[ ! -s "$report" ]]; then
		echo "Playtest capture smoke failed: report was not written ($report)" >&2
		exit 1
	fi
}

run_capture "$REPORT_ONE"
run_capture "$REPORT_TWO"

python3 - "$REPORT_ONE" <<'PY'
import json
import re
import sys

path = sys.argv[1]
with open(path, encoding="utf-8") as handle:
    report = json.load(handle)

required = {
    "schema",
    "suite",
    "status",
    "captured_at",
    "route",
    "tester",
    "timer",
    "stages_completed",
    "stages_total",
    "stages",
    "issues",
    "notes",
    "validation",
}
missing = sorted(required - report.keys())
assert not missing, f"missing fields: {missing}"

assert report["schema"] == 1, report["schema"]
assert report["suite"] == "q101_playtest_capture", report["suite"]
assert report["status"] == "ok", report["status"]
assert report["route"]["id"] == "q101", report["route"]
assert report["route"]["total_minutes"] == 30, report["route"]
assert report["stages_total"] == 8, report["stages_total"]
assert report["stages_completed"] == 8, report["stages_completed"]

stages = report["stages"]
assert len(stages) == 8, len(stages)
assert [stage["id"] for stage in stages] == [
    "orient",
    "starter_build",
    "house",
    "industry",
    "utility",
    "service",
    "save_load",
    "ratings",
], stages
assert all(stage["complete"] for stage in stages), stages
assert [stage["target_start_min"] for stage in stages] == [0, 3, 8, 13, 19, 22, 25, 29], stages
assert all(stage["completed_at_s"] is not None for stage in stages), stages

timer = report["timer"]
assert timer["started"] is True, timer
assert timer["running"] is False, timer
assert abs(timer["elapsed_s"] - 120.0) < 0.001, timer
assert timer["route_minutes"] == 30, timer

tester = report["tester"]
assert tester["name"] == "Smoke Tester", tester
assert tester["experience"] == "some", tester
assert tester["build"] == "smoke-build-822c509", tester
assert tester["seed"] == "2026", tester

issues = report["issues"]
assert len(issues) == 2, issues
assert issues[0]["row"] == 2, issues
assert issues[0]["area"] == "starter_build", issues
assert issues[0]["severity"] == "high", issues
assert issues[0]["reproducible"] is True, issues
assert issues[1]["row"] == 3, issues
assert issues[1]["area"] == "save_load", issues
assert issues[1]["severity"] == "medium", issues
assert issues[1]["reproducible"] is False, issues
assert all(issue["description"] for issue in issues), issues

validation = report["validation"]
assert validation["complete"] is True, validation
assert validation["warnings"] == [], validation

assert re.match(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$", report["captured_at"]), report["captured_at"]
print("PLAYTEST_CAPTURE_EXPORT OK stages=8 issues=2")
PY

python3 - "$REPORT_ONE" "$REPORT_TWO" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    first = json.load(handle)
with open(sys.argv[2], encoding="utf-8") as handle:
    second = json.load(handle)

first.pop("captured_at", None)
second.pop("captured_at", None)

if first != second:
    differing = sorted(key for key in set(first) | set(second) if first.get(key) != second.get(key))
    raise SystemExit(f"playtest capture payload is not deterministic; differing keys: {differing}")

print("PLAYTEST_CAPTURE_DETERMINISTIC OK (captured_at ignored)")
PY

echo "PLAYTEST_CAPTURE_SMOKE OK stages=8"
