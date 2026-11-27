#!/usr/bin/env bash
set -euo pipefail

# Layered rejection smoke for the settlement screen.
#
# The screen exposes fixed local and connected smoke flows rather than an
# arbitrary action-script grammar. This script therefore pins the rejections
# those flows can observe, and separately runs the authoritative server
# settlement smoke for the codes the UI cannot script yet:
#   local screen:     claim owned, empty rename, owner remove
#   connected screen: stale rename, Mayor release
#   server:           role/tax/recipient/revision/owner matrix
#
# Every marker is deterministic and the two server runs must match exactly.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

if ! command -v "$GODOT_BIN" >/dev/null 2>&1; then
	echo "Godot binary not found: $GODOT_BIN (set GODOT=/path/to/godot)" >&2
	exit 2
fi

cargo build --quiet --manifest-path "$ROOT/Cargo.toml" -p townsmith-server

run_and_capture() {
	local label="$1"
	shift
	local output status
	set +e
	output="$("$@" 2>&1)"
	status=$?
	set -e
	printf '%s\n' "$output" >&2
	if [[ $status -ne 0 ]]; then
		echo "Settlement rejection smoke failed ($label): status $status" >&2
		exit 1
	fi
	printf '%s\n' "$output"
}

LOCAL_OUTPUT="$(run_and_capture local bash "$ROOT/scripts/smoke_settlement_screen.sh")"
CONNECTED_OUTPUT="$(run_and_capture connected bash "$ROOT/scripts/smoke_settlement_screen_connected.sh")"

LOCAL_MARKER="$(grep '^SETTLEMENT_SCREEN status=ok ' <<<"$LOCAL_OUTPUT" | tail -1)"
CONNECTED_MARKER="$(grep '^SETTLEMENT_SCREEN_CONNECTED status=ok ' <<<"$CONNECTED_OUTPUT" | tail -1)"

[[ -n "$LOCAL_MARKER" && -n "$CONNECTED_MARKER" ]] || {
	echo "Settlement rejection smoke failed: missing screen marker" >&2
	exit 1
}

for needle in \
	"action=claim ok=false code=settlement.already_owned" \
	"action=rename ok=false code=empty_name" \
	"action=remove_member ok=false code=settlement.owner_required" \
	"rejected=3"; do
	if ! grep -Fq "$needle" <<<"$LOCAL_OUTPUT"; then
		echo "Settlement rejection smoke failed: local missing '$needle'" >&2
		exit 1
	fi
done

for needle in \
	"action=release ok=false code=role.forbidden" \
	"rejected=1"; do
	if ! grep -Fq "$needle" <<<"$CONNECTED_OUTPUT"; then
		echo "Settlement rejection smoke failed: connected missing '$needle'" >&2
		exit 1
	fi
done

SERVER_FIRST="$("$ROOT/target/debug/townsmith-server" --settlement-smoke 2>&1)"
SERVER_SECOND="$("$ROOT/target/debug/townsmith-server" --settlement-smoke 2>&1)"
printf '%s\n' "$SERVER_FIRST" >&2

if [[ "$SERVER_FIRST" != "$SERVER_SECOND" ]]; then
	echo "Settlement rejection smoke failed: server smoke is not deterministic" >&2
	exit 1
fi

SERVER_MARKER="$(grep '^SERVER_SETTLEMENT status=ok ' <<<"$SERVER_FIRST" | tail -1)"
[[ -n "$SERVER_MARKER" ]] || {
	echo "Settlement rejection smoke failed: missing SERVER_SETTLEMENT marker" >&2
	exit 1
}

CHECKSUM="$(
	printf '%s\n%s\n%s\n' "$LOCAL_MARKER" "$CONNECTED_MARKER" "$SERVER_MARKER" |
		sha256sum | cut -c1-16
)"
echo "SETTLEMENT_SCREEN_REJECTIONS status=ok cases=5 rejected=4 checksum=$CHECKSUM"
