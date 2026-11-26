#!/usr/bin/env bash
# Headless streamed-world placement scene smoke. Exercises the ghost preview,
# water rejection, placement, save/load, and node rebuild without a display.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

"$ROOT/scripts/build_godot_ext.sh" debug >/dev/null

# Godot 4.7 can abort the first headless import in a clean worktree.
if ! "$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1; then
	"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null
fi

set +e
OUTPUT="$(timeout 120s "$GODOT_BIN" --headless --path "$ROOT/godot" res://world_placement.tscn -- --scene-smoke 2>&1)"
GODOT_STATUS=$?
set -e
printf '%s\n' "$OUTPUT"

if [[ $GODOT_STATUS -ne 0 ]]; then
	echo "World scene smoke failed: Godot exited with status $GODOT_STATUS" >&2
	exit 1
fi

LINE="$(grep "WORLD_SCENE " <<<"$OUTPUT" | tail -1 || true)"
if [[ -z "$LINE" ]]; then
	echo "World scene smoke failed: no marker" >&2
	exit 1
fi
if [[ "$LINE" != *"rooms=4"* || "$LINE" != *"preview=ok"* || "$LINE" != *"water=ok"* || "$LINE" != *"rotate=ok"* || "$LINE" != *"edits=ok"* || "$LINE" != *"chunk_cells=ok"* || "$LINE" != *"place=ok"* || "$LINE" != *"roads=ok"* || "$LINE" != *"lots=ok"* || "$LINE" != *"settlements=ok"* || "$LINE" != *"build=ok"* || "$LINE" != *"done=ok"* || "$LINE" != *"trades=ok"* || "$LINE" != *"render=ok"* || "$LINE" != *"structure=ok"* || "$LINE" != *"age=ok"* || "$LINE" != *"research=ok"* || "$LINE" != *"tutorial=ok"* || "$LINE" != *"starter=ok"* || "$LINE" != *"named=ok"* || "$LINE" != *"citizens=ok"*
		|| "$LINE" != *"citizen_rows=ok"*
		|| "$LINE" != *"content_i18n=ok"* || "$LINE" != *"content_ui=ok"* || "$LINE" != *"follow=ok"* || "$LINE" != *"citizen_i18n=ok"* || "$LINE" != *"materialization=ok"* || "$LINE" != *"materialization_step=ok"* || "$LINE" != *"materialization_fade=ok"* || "$LINE" != *"movement_scene=ok"* || "$LINE" != *"movement_pool=ok"*
		|| "$LINE" != *"movement_routes=ok"* || "$LINE" != *"movement_lod=ok"* || "$LINE" != *"occupancy=ok"* || "$LINE" != *"industry=ok"* || "$LINE" != *"utilities=ok"* || "$LINE" != *"services=ok"* || "$LINE" != *"economy=ok"* || "$LINE" != *"dashboard=ok"* || "$LINE" != *"district_ui=ok"* || "$LINE" != *"policy_ui=ok"* || "$LINE" != *"policy_edit=ok"* || "$LINE" != *"district_select=ok"* || "$LINE" != *"settlement_select=ok"* || "$LINE" != *"settlement_detail=ok"* || "$LINE" != *"dashboard_tax=ok"* || "$LINE" != *"dashboard_tax_projection=ok"* || "$LINE" != *"dashboard_claim=ok"* || "$LINE" != *"dashboard_transfer=ok"* || "$LINE" != *"dashboard_release=ok"* || "$LINE" != *"dashboard_rename=ok"* || "$LINE" != *"dashboard_members=ok"* || "$LINE" != *"dashboard_member_cap=ok"* || "$LINE" != *"heatmap_ui=ok"* || "$LINE" != *"heatmap_modes=ok"*
		|| "$LINE" != *"hud_i18n=ok"* || "$LINE" != *"hud_body_i18n=ok"* || "$LINE" != *"hud_status_i18n=ok"* || "$LINE" != *"dashboard_body_i18n=ok"*
		|| "$LINE" != *"advisor_i18n=ok"*
		|| "$LINE" != *"advisor_detail_i18n=ok"* || "$LINE" != *"modal_i18n=ok"*
		|| "$LINE" != *"number_i18n=ok"* || "$LINE" != *"traffic=ok"* || "$LINE" != *"select=ok"* || "$LINE" != *"enter=ok"* || "$LINE" != *"return=ok"* || "$LINE" != *"save=ok"* || "$LINE" != *"load=ok"* || "$LINE" != *"guard=ok"*
		|| "$LINE" != *"campaign_hud=ok"*
	|| "$LINE" != *"lifecycle_scene=ok"* || "$LINE" != *"status=ok"* ]]; then
	echo "World scene smoke contract mismatch: $LINE" >&2
	exit 1
fi

TAX_LINE="$(grep "DASHBOARD_TAX " <<<"$OUTPUT" | tail -1 || true)"
if [[ -z "$TAX_LINE" ]]; then
	echo "World scene smoke failed: no DASHBOARD_TAX marker" >&2
	exit 1
fi
if [[ "$TAX_LINE" != *"DASHBOARD_TAX cycle=ok 0->5 95->100 100->0 unowned=ok"* ]]; then
	echo "World scene smoke tax contract mismatch: $TAX_LINE" >&2
	exit 1
fi

CLAIM_LINE="$(grep "DASHBOARD_CLAIM " <<<"$OUTPUT" | tail -1 || true)"
if [[ -z "$CLAIM_LINE" ]]; then
	echo "World scene smoke failed: no DASHBOARD_CLAIM marker" >&2
	exit 1
fi
if [[ ! "$CLAIM_LINE" =~ DASHBOARD_CLAIM\ status=ok\ id=[0-9]+\ player=[0-9]+\ revision=0-\>1\ owned=[0-9]+\ second=rejected\ hidden=untouched\ checksum=[0-9a-f]{16} ]]; then
	echo "World scene smoke claim contract mismatch: $CLAIM_LINE" >&2
	exit 1
fi

TRANSFER_LINE="$(grep "DASHBOARD_TRANSFER " <<<"$OUTPUT" | tail -1 || true)"
if [[ -z "$TRANSFER_LINE" ]]; then
	echo "World scene smoke failed: no DASHBOARD_TRANSFER marker" >&2
	exit 1
fi
if [[ ! "$TRANSFER_LINE" =~ DASHBOARD_TRANSFER\ status=ok\ id=[0-9]+\ from=[0-9]+\ to=[0-9]+\ revision=[0-9]+-\>[0-9]+\ back=[0-9]+ ]]; then
	echo "World scene smoke transfer contract mismatch: $TRANSFER_LINE" >&2
	exit 1
fi

RELEASE_LINE="$(grep "DASHBOARD_RELEASE " <<<"$OUTPUT" | tail -1 || true)"
if [[ -z "$RELEASE_LINE" ]]; then
	echo "World scene smoke failed: no DASHBOARD_RELEASE marker" >&2
	exit 1
fi
if [[ ! "$RELEASE_LINE" =~ DASHBOARD_RELEASE\ status=ok\ id=[0-9]+\ owner_before=[0-9]+\ revision=[0-9]+-\>[0-9]+\ second=rejected\ unowned=rejected\ checksum=[0-9a-f]{16} ]]; then
	echo "World scene smoke release contract mismatch: $RELEASE_LINE" >&2
	exit 1
fi

RENAME_LINE="$(grep "DASHBOARD_RENAME " <<<"$OUTPUT" | grep "status=ok" | tail -1 || true)"
if [[ -z "$RENAME_LINE" ]]; then
	echo "World scene smoke failed: no DASHBOARD_RENAME success marker" >&2
	exit 1
fi
if [[ ! "$RENAME_LINE" =~ DASHBOARD_RENAME\ status=ok\ id=[0-9]+\ revision=[0-9]+-\>[0-9]+\ checksum=[0-9a-f]{16} ]]; then
	echo "World scene smoke rename contract mismatch: $RENAME_LINE" >&2
	exit 1
fi

RENAME_SMOKE_LINE="$(grep "DASHBOARD_RENAME_SMOKE " <<<"$OUTPUT" | tail -1 || true)"
if [[ -z "$RENAME_SMOKE_LINE" ]]; then
	echo "World scene smoke failed: no DASHBOARD_RENAME_SMOKE marker" >&2
	exit 1
fi
if [[ "$RENAME_SMOKE_LINE" != *"status=ok"* || "$RENAME_SMOKE_LINE" != *"empty=rejected"* || "$RENAME_SMOKE_LINE" != *"stale=rejected"* || "$RENAME_SMOKE_LINE" != *"esc=ok"* ]]; then
	echo "World scene smoke rename reject contract mismatch: $RENAME_SMOKE_LINE" >&2
	exit 1
fi

MEMBERS_LINE="$(grep "DASHBOARD_MEMBERS " <<<"$OUTPUT" | tail -1 || true)"
if [[ -z "$MEMBERS_LINE" ]]; then
	echo "World scene smoke failed: no DASHBOARD_MEMBERS marker" >&2
	exit 1
fi
if [[ ! "$MEMBERS_LINE" =~ DASHBOARD_MEMBERS\ status=ok\ id=[0-9]+\ player=[0-9]+\ role=manager\ remove=[0-9]+\ revision=[0-9]+-\>[0-9]+\ checksum=[0-9a-f]{16} ]]; then
	echo "World scene smoke members contract mismatch: $MEMBERS_LINE" >&2
	exit 1
fi
NAMES_LINE="$(grep "DASHBOARD_MEMBER_NAMES " <<<"$OUTPUT" | tail -1 || true)"
if [[ -z "$NAMES_LINE" ]]; then
	echo "World scene smoke failed: no DASHBOARD_MEMBER_NAMES marker" >&2
	exit 1
fi
if [[ "$NAMES_LINE" != *"status=ok"* || "$NAMES_LINE" != *"named=Builder"* || "$NAMES_LINE" != *"fallback=#9001"* ]]; then
	echo "World scene smoke member names contract mismatch: $NAMES_LINE" >&2
	exit 1
fi
CAP_LINE="$(grep "DASHBOARD_MEMBER_CAP " <<<"$OUTPUT" | tail -1 || true)"
if [[ -z "$CAP_LINE" ]]; then
	echo "World scene smoke failed: no DASHBOARD_MEMBER_CAP marker" >&2
	exit 1
fi
if [[ "$CAP_LINE" != *"status=ok"* || "$CAP_LINE" != *"rows=8"* || "$CAP_LINE" != *"overflow=+2"* || "$CAP_LINE" != *"short=ok"* || "$CAP_LINE" != *"unowned=ok"* ]]; then
	echo "World scene smoke member cap contract mismatch: $CAP_LINE" >&2
	exit 1
fi
