#!/usr/bin/env bash
# Headless smoke for the optional Q-101 session telemetry probe.
#
# Runs the real world scene with `--playtest-smoke` for a few seconds without
# input, then requires the marker and validates a fresh JSON session. This does
# not replace `smoke_world_scene.sh`; both paths stay independent.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

"$ROOT/scripts/build_godot_ext.sh" debug >/dev/null

# Godot 4.7 can abort the first headless import in a clean worktree.
if ! "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1; then
	"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null
fi

SESSION_JSON="$TMP/playtest_session.json"
set +e
OUTPUT="$(timeout 60s "$GODOT_BIN" --headless --path "$ROOT/godot" res://world_placement.tscn -- --playtest-smoke --playtest-out "$SESSION_JSON" 2>&1)"
GODOT_STATUS=$?
set -e
printf '%s\n' "$OUTPUT"

if [[ $GODOT_STATUS -ne 0 ]]; then
	echo "Playtest probe smoke failed: Godot exited with status $GODOT_STATUS" >&2
	exit 1
fi

MARKER="$(grep -E '^PLAYTEST_PROBE ' <<<"$OUTPUT" | tail -1 || true)"
if [[ -z "$MARKER" ]]; then
	echo "Playtest probe smoke failed: no PLAYTEST_PROBE marker" >&2
	exit 1
fi
if [[ "$MARKER" != *"status=ok"* ]]; then
	echo "Playtest probe smoke failed: $MARKER" >&2
	exit 1
fi
if [[ ! -f "$SESSION_JSON" ]]; then
	echo "Playtest probe smoke failed: missing $SESSION_JSON" >&2
	exit 1
fi

python3 - "$SESSION_JSON" "$MARKER" <<'PY'
import json
import pathlib
import re
import sys

path = pathlib.Path(sys.argv[1])
marker = sys.argv[2]
data = json.loads(path.read_text())
required = ["status", "started_at", "duration_s", "godot_version", "seed", "window_size", "events", "snapshots"]
missing = [key for key in required if key not in data]
if missing:
    raise SystemExit(f"playtest probe smoke failed: missing JSON fields {missing}")
if data["status"] != "ok":
    raise SystemExit(f"playtest probe smoke failed: status={data['status']!r}")
if not isinstance(data["events"], list):
    raise SystemExit("playtest probe smoke failed: events is not a list")
snapshots = data["snapshots"]
if not isinstance(snapshots, list) or not snapshots:
    raise SystemExit("playtest probe smoke failed: snapshots is empty")
snapshot_fields = [
    "mode", "selected_id", "placements", "construction_sites",
    "construction_completed", "population", "dashboard_visible",
    "materialization_visible",
]
for field in snapshot_fields:
    if field not in snapshots[-1]:
        raise SystemExit(f"playtest probe smoke failed: snapshot missing {field}")
if not isinstance(data["window_size"], dict):
    raise SystemExit("playtest probe smoke failed: window_size is not an object")
if not str(data["started_at"]).endswith("Z"):
    raise SystemExit(f"playtest probe smoke failed: started_at={data['started_at']!r}")
if float(data["duration_s"]) <= 0.0:
    raise SystemExit(f"playtest probe smoke failed: duration_s={data['duration_s']!r}")

match = re.search(r"events=(\d+) .*snapshots=(\d+)", marker)
if not match:
    raise SystemExit(f"playtest probe smoke failed: marker fields missing: {marker!r}")
if int(match.group(1)) != len(data["events"]):
    raise SystemExit("playtest probe smoke failed: marker event count does not match JSON")
if int(match.group(2)) != len(snapshots):
    raise SystemExit("playtest probe smoke failed: marker snapshot count does not match JSON")
print(
    "PLAYTEST_PROBE_SMOKE status=ok events={} snapshots={} duration_s={:.2f}".format(
        len(data["events"]), len(snapshots), float(data["duration_s"])
    )
)
PY
