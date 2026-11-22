#!/usr/bin/env bash
# Launch the world scene with the Q-101 telemetry probe and collect the session.
#
# Usage:
#   GODOT=/path/to/godot ./scripts/playtest_session.sh [output-directory]
#
# The probe is inert unless `--playtest` is passed. This wrapper also writes a
# human-readable skeleton next to the raw JSON. Recordings are local only.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"
OUT_DIR="${1:-$ROOT/playtest_sessions/$(date -u +%Y%m%dT%H%M%SZ)}"
mkdir -p "$OUT_DIR"
OUT_DIR="$(cd "$OUT_DIR" && pwd)"
SESSION_JSON="$OUT_DIR/playtest_session.json"
SESSION_MD="$OUT_DIR/playtest_session.md"
LOG="$OUT_DIR/godot.log"
BUILD_COMMIT="$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"

if [[ ! -f "$ROOT/godot/bin/libtownsmith_core.so" ]]; then
	echo "GDExtension library missing; building debug..."
	"$ROOT/scripts/build_godot_ext.sh" debug
fi

# A clean worktree has no .godot cache yet. Import before the windowed run so
# the first session does not fail before the probe can start.
if [[ ! -d "$ROOT/godot/.godot" ]]; then
	if ! "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1; then
		"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null
	fi
fi

set +e
"$GODOT_BIN" --path "$ROOT/godot" res://world_placement.tscn -- \
	--playtest --playtest-out "$SESSION_JSON" 2>&1 | tee "$LOG"
GODOT_STATUS=${PIPESTATUS[0]}
set -e

MARKER="$(grep -E '^PLAYTEST_PROBE ' "$LOG" | tail -1 || true)"
if [[ -z "$MARKER" ]]; then
	echo "Playtest session failed: no PLAYTEST_PROBE marker (Godot exit $GODOT_STATUS)." >&2
	exit 1
fi
if [[ "$MARKER" != *"status=ok"* ]]; then
	echo "Playtest session failed: $MARKER" >&2
	exit 1
fi
if [[ ! -f "$SESSION_JSON" ]]; then
	echo "Playtest session failed: missing $SESSION_JSON" >&2
	exit 1
fi

python3 - "$SESSION_JSON" "$SESSION_MD" "$BUILD_COMMIT" <<'PY'
import json
import pathlib
import re
import sys
from collections import Counter

session_path = pathlib.Path(sys.argv[1])
out_path = pathlib.Path(sys.argv[2])
build_commit = sys.argv[3]
data = json.loads(session_path.read_text())

snapshots = data.get("snapshots") or []
last = snapshots[-1] if snapshots else {}
window = data.get("window_size") or {}
events = data.get("events") or []
presses = Counter(
    str(event.get("action") or event.get("key") or "?")
    for event in events
    if event.get("pressed")
)

def cell(value):
    return str(value).replace("|", "\\|")

def action_rows():
    if not presses:
        return "| _No mapped presses recorded._ | 0 |"
    rows = []
    for action, count in sorted(presses.items(), key=lambda item: (-item[1], item[0])):
        rows.append(f"| `{cell(action)}` | {count} |")
    return "\n".join(rows)

def issue_block(number):
    return f"""### Issue {number}

- **Title:**
- **Route step and timestamp (`t_s`):**
- **What the tester did:**
- **What they expected:**
- **What happened:**
- **Repro (exact keys/clicks):**
- **Severity:** blocker / major / minor
- **Evidence:**
- **Suspected cause:**
- **Suggested fix (one line):**
- **Owner / follow-up:**
"""

markdown = f"""# Townsmith playtest session

> Generated from `{session_path.name}`. Keep the JSON, this file, and `godot.log` local; do not commit recordings.
> Fill in the ratings and issues while the session is fresh. Use `t_s` from the JSON to correlate a finding with the route.

## Session metadata

| Field | Value |
|---|---|
| Started (UTC) | {cell(data.get("started_at", ""))} |
| Duration | {float(data.get("duration_s", 0.0)):.1f} s |
| Godot version | {cell(data.get("godot_version", ""))} |
| Seed | {cell(data.get("seed", ""))} |
| Window | {cell(window.get("width", "?"))}x{cell(window.get("height", "?"))} |
| Build commit | `{cell(build_commit)}` |
| Events recorded | {len(events)} |
| Snapshots | {len(snapshots)} |
| Tester (name or ID) | |
| Prior Townsmith experience | none / some / team member |
| Observer | |
| Hardware | |
| Locale (EN / DE) | |

## End state

| Field | Value |
|---|---|
| Mode | {cell(last.get("mode", ""))} |
| Selected id | {cell(last.get("selected_id", ""))} |
| Placements | {cell(last.get("placements", ""))} |
| Construction sites (active / completed / stalled) | {cell(last.get("construction_active", ""))} / {cell(last.get("construction_completed", ""))} / {cell(last.get("construction_stalled", ""))} |
| Population | {cell(last.get("population", ""))} |
| Completed buildings | {cell(last.get("completed_buildings", ""))} |
| Dashboard visible | {cell(last.get("dashboard_visible", ""))} |
| Materialization visible | {cell(last.get("materialization_visible", ""))} |

## Control usage (mapped presses)

| Action | Presses |
|---|---:|
{action_rows()}

Unmapped keys remain in `events[]` with an empty `action`. This table counts presses only, not releases.

## 5. Five rating questions

Ask after the save/load step, before discussing anything. Use a 1-5 scale: **1 = strongly disagree, 5 = strongly agree**. Add one line of "why" for any score of 1-2 or 5.

| # | Question | Score (1-5) | Why / quote |
|---|---|---:|---|
| R1 | I always knew what to do next. | | |
| R2 | Placing a building and watching it get constructed felt clear and responsive. | | |
| R3 | I understood how industry, utilities, and services changed the city. | | |
| R4 | Saving and loading gave me confidence that nothing was lost. | | |
| R5 | I would play another 30 minutes without a guide. | | |

Optional single question at the end: **What is the one thing you would fix first?**

## 6. Top-three usability issues

Rank by severity and frequency, not by how easy the fix is. Use one block per issue.

| Rank | Route step | Issue in one line | Severity | Evidence |
|---:|---|---|---|---|
| 1 | | | blocker / major / minor | |
| 2 | | | blocker / major / minor | |
| 3 | | | blocker / major / minor | |

{issue_block(1)}

{issue_block(2)}

{issue_block(3)}

Severity guide: **blocker** = cannot continue without help; **major** = continues but misunderstands a system or loses work; **minor** = friction, wording, or polish.

## Correlating with the raw session

`playtest_session.json` contains every non-echo key/mouse press with monotonic `t_s`, a control `action`, and a snapshot every 10 s. Snapshots record `mode`, `selected_id`, `placements`, construction counts, `population`, and dashboard/materialization visibility. Use the JSON when the observer notes and the route timing disagree.
"""

out_path.write_text(markdown)
print(f"Wrote {out_path}")
PY

echo "Playtest session written to $OUT_DIR"
if [[ $GODOT_STATUS -ne 0 ]]; then
	echo "Warning: Godot exited with status $GODOT_STATUS after writing the session." >&2
fi
