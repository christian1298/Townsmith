#!/usr/bin/env bash
# Headless smoke for the standalone settlement ownership editor.
#
# Runs `godot/settlement_editor.tscn --smoke` twice and requires the local
# bridge walkthrough (found two, claim, invite manager/builder, rename, set tax,
# transfer, change role, remove member, two-press release, and seven rejections)
# plus the local
# player registry path (seed three names, register a fourth, cycle the invite
# target, invite by the selected name's id, cap/invalid-name rejection) and the
# client-side paging path (23 records over 8-row pages: empty/one-page hidden,
# PageDown/PageUp, off-page selection fallback, number-key page indexing,
# selection survives refresh, every id reachable once). Every
# `SETTLEMENT_EDITOR*` line is diffed between runs so the state checksum and
# the action sequence are proven deterministic. No display or server is needed.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

if [[ "${1:-}" != "--smoke" ]]; then
	echo "Usage: $0 --smoke" >&2
	exit 2
fi

if ! command -v "$GODOT_BIN" >/dev/null 2>&1; then
	echo "Godot binary not found: $GODOT_BIN (set GODOT=/path/to/godot)" >&2
	exit 2
fi

# The scene is standalone but the project loads the GDExtension. Build it only
# when missing so a clean worktree can run the smoke.
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

run_editor() {
	local log="$1"
	local marker_file="$2"
	local extra="${3:-}"
	local output
	local godot_status
	set +e
	output="$(timeout 120s "$GODOT_BIN" --headless --path "$ROOT/godot" res://settlement_editor.tscn -- --smoke $extra 2>&1)"
	godot_status=$?
	set -e
	printf '%s\n' "$output" >&2

	if [[ $godot_status -ne 0 ]]; then
		echo "Settlement editor smoke failed: Godot exited with status $godot_status" >&2
		exit 1
	fi

	local marker
	marker="$(grep '^SETTLEMENT_EDITOR status=' <<<"$output" | tail -1 || true)"
	if [[ -z "$marker" ]]; then
		echo "Settlement editor smoke failed: no SETTLEMENT_EDITOR status marker" >&2
		exit 1
	fi
	if [[ ! "$marker" =~ ^SETTLEMENT_EDITOR\ status=ok\ actions=[0-9]+\ checksum=[0-9a-f]{16}$ ]]; then
		echo "Settlement editor smoke contract mismatch: $marker" >&2
		exit 1
	fi

	local actions
	actions="$(sed -E 's/^SETTLEMENT_EDITOR status=ok actions=([0-9]+) checksum=[0-9a-f]+$/\1/' <<<"$marker")"
	if [[ "$actions" != "56" ]]; then
		echo "Settlement editor smoke failed: expected 56 actions, got $actions" >&2
		exit 1
	fi

	for expected in \
		"claim_already_owned code=settlement.already_owned expected=settlement.already_owned match=1" \
		"rename_unowned code=settlement.unowned expected=settlement.unowned match=1" \
		"claim_player_zero code=invalid_player_id expected=invalid_player_id match=1" \
		"stale_revision code=settlement.revision_conflict expected=settlement.revision_conflict match=1" \
		"owner_role_rejected code=settlement.role_invalid expected=settlement.role_invalid match=1" \
		"remove_owner_rejected code=settlement.owner_required expected=settlement.owner_required match=1" \
		"release_repeat code=settlement.unowned expected=settlement.unowned match=1" \
		"player_limit code=player.limit expected=player.limit match=1" \
		"player_invalid_name code=player.invalid_name expected=player.invalid_name match=1"
	do
		if ! grep -Fq "name=${expected%% *} ${expected#* }" <<<"$output"; then
			echo "Settlement editor smoke failed: rejection marker missing: $expected" >&2
			exit 1
		fi
	done

	if ! grep -Fxq 'SETTLEMENT_EDITOR_PLAYERS phase=seed count=3 roster=Players: #1 Mayor, #2 Builder, #3 Auditor' <<<"$output"; then
		echo "Settlement editor smoke failed: seeded player roster missing" >&2
		exit 1
	fi
	if ! grep -Fxq 'SETTLEMENT_EDITOR_PLAYERS phase=register count=4 roster=Players: #1 Mayor, #2 Builder, #3 Auditor, #4 Player 4' <<<"$output"; then
		echo "Settlement editor smoke failed: registered player roster missing" >&2
		exit 1
	fi
	if ! grep -Fxq 'SETTLEMENT_EDITOR_TARGET id=2 name=Builder' <<<"$output"; then
		echo "Settlement editor smoke failed: first target cycle missing" >&2
		exit 1
	fi
	if ! grep -Fxq 'SETTLEMENT_EDITOR_TARGET id=3 name=Auditor' <<<"$output"; then
		echo "Settlement editor smoke failed: second target cycle missing" >&2
		exit 1
	fi
	if ! grep -Fxq 'SETTLEMENT_EDITOR_TARGET_EMPTY kept=3 message=1' <<<"$output"; then
		echo "Settlement editor smoke failed: empty-registry target guard missing" >&2
		exit 1
	fi
	if ! grep -Fxq 'SETTLEMENT_EDITOR_INVITE settlement=4 target=2 name=Builder members=1:owner, 2:manager' <<<"$output"; then
		echo "Settlement editor smoke failed: name-based invite marker missing" >&2
		exit 1
	fi
	if ! grep -Eq '^SETTLEMENT_EDITOR_RELEASE phase=arm code=confirm_required revision=2 members=1:owner, 2:manager$' <<<"$output"; then
		echo "Settlement editor smoke failed: release arm marker missing" >&2
		exit 1
	fi
	if ! grep -Eq '^SETTLEMENT_EDITOR_RELEASE phase=confirm owned=0 owner=unowned members=(none|keine) revision=3 checksum=[0-9a-f]{16}$' <<<"$output"; then
		echo "Settlement editor smoke failed: release confirm marker missing" >&2
		exit 1
	fi
	if ! grep -Fxq 'SETTLEMENT_EDITOR_RELEASE phase=repeat code=settlement.unowned revision=3' <<<"$output"; then
		echo "Settlement editor smoke failed: release repeat marker missing" >&2
		exit 1
	fi

	if ! grep -Eq '^SETTLEMENT_EDITOR_STATE count=2 id=2 owned=1 owner=3 revision=8 tax=12 members=1:manager, 2:manager, 3:owner$' <<<"$output"; then
		echo "Settlement editor smoke failed: final state marker mismatch" >&2
		exit 1
	fi
	for expected in \
		'SETTLEMENT_EDITOR_PAGING phase=empty page=1/1 hidden=1 rows=0' \
		'SETTLEMENT_EDITOR_PAGING phase=single page=1/1 hidden=1 rows=1' \
		'SETTLEMENT_EDITOR_PAGING phase=seed count=23 page=1/3 hidden=0 rows=8' \
		'SETTLEMENT_EDITOR_PAGING phase=down page=2/3 selected=9 rows=8 first=9 last=16' \
		'SETTLEMENT_EDITOR_PAGING phase=down_last page=3/3 selected=17 rows=7 first=17 last=23' \
		'SETTLEMENT_EDITOR_PAGING phase=clamp page=3/3 selected=17 clamped=1' \
		'SETTLEMENT_EDITOR_PAGING phase=up page=2/3 selected=9 rows=8 first=9 last=16' \
		'SETTLEMENT_EDITOR_PAGING phase=number page=2/3 index=3 selected=11' \
		'SETTLEMENT_EDITOR_PAGING phase=jump page=3/3 selected=23 rows=7' \
		'SETTLEMENT_EDITOR_PAGING phase=refresh page=3/3 selected=23 checked=1' \
		'SETTLEMENT_EDITOR_PAGING phase=walk pages=3 ids=23 unique=23 rows=8/8/7'
	do
		if ! grep -Fxq "$expected" <<<"$output"; then
			echo "Settlement editor smoke failed: paging marker missing: $expected" >&2
			exit 1
		fi
	done
	if ! grep -Eq '^SETTLEMENT_EDITOR_UNOWNED id=1 owned=0 owner=unowned revision=0$' <<<"$output"; then
		echo "Settlement editor smoke failed: unowned marker mismatch" >&2
		exit 1
	fi
	if grep -q '^SETTLEMENT_EDITOR_FAIL' <<<"$output"; then
		echo "Settlement editor smoke failed: internal check failure marker present" >&2
		exit 1
	fi

	grep '^SETTLEMENT_EDITOR' <<<"$output" | grep -v '^SETTLEMENT_EDITOR status=' >"$log"
	printf '%s\n' "$marker" >"$marker_file"
}

run_editor "$SMOKE_DIR/first.txt" "$SMOKE_DIR/first.marker"
run_editor "$SMOKE_DIR/second.txt" "$SMOKE_DIR/second.marker"
run_editor "$SMOKE_DIR/de.txt" "$SMOKE_DIR/de.marker" "--smoke-de"
FIRST_MARKER="$(cat "$SMOKE_DIR/first.marker")"
SECOND_MARKER="$(cat "$SMOKE_DIR/second.marker")"

if [[ "$FIRST_MARKER" != "$SECOND_MARKER" ]]; then
	echo "Settlement editor smoke failed: state markers differ between runs" >&2
	echo "  first:  $FIRST_MARKER" >&2
	echo "  second: $SECOND_MARKER" >&2
	exit 1
fi

if ! diff -u "$SMOKE_DIR/first.txt" "$SMOKE_DIR/second.txt"; then
	echo "Settlement editor smoke failed: deterministic marker lines differ" >&2
	exit 1
fi

if ! grep -Fq 'SETTLEMENT_EDITOR_UI locale=de title=Editor für Siedlungsbesitz state=Seed 2.026 · 0 Siedlungen · Prüfsumme 0000000000000000' "$SMOKE_DIR/de.txt"; then
	echo "Settlement editor smoke failed: German UI marker missing" >&2
	cat "$SMOKE_DIR/de.txt" >&2
	exit 1
fi
if grep -q '!base.ui.' "$SMOKE_DIR/de.txt"; then
	echo "Settlement editor smoke failed: untranslated key leak" >&2
	exit 1
fi

echo "SETTLEMENT_EDITOR_SMOKE OK ${FIRST_MARKER#SETTLEMENT_EDITOR }"
