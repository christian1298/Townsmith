#!/usr/bin/env bash
# Summarize Q-101 playtest capture reports (docs/66 - Playtest Kit.md §9).
#
# With no arguments, reads every *.json in docs/playtest/ and prints the
# aggregate plus the machine-readable marker:
#
#   PLAYTEST_SUMMARY reports=N completed=N issues=N status=ok|partial
#
# Arguments are forwarded to scripts/summarize_playtest.py, so an explicit run
# is:
#
#   ./scripts/check_playtest_report.sh path/to/q101_capture.json
#   ./scripts/check_playtest_report.sh docs/playtest --markdown /tmp/q101.md
#   ./scripts/check_playtest_report.sh --self-test
#
# Captures exported by godot/playtest_capture.tscn default to user:// (Linux:
# ~/.local/share/godot/app_userdata/"Townsmith Phase 0"/q101_playtest_*.json),
# or wherever --output pointed. Copy the files to keep into docs/playtest/:
#
#   mkdir -p docs/playtest
#   cp ~/.local/share/godot/app_userdata/"Townsmith Phase 0"/q101_playtest_*.json \
#       docs/playtest/
#   ./scripts/check_playtest_report.sh
#
# Env:
#   PLAYTEST_DIR    report directory (default docs/playtest)
#   PLAYTEST_MD     write the Markdown aggregate to this path
#   PLAYTEST_JSON   write the deterministic JSON aggregate to this path
#
# Exit: 0 = reports read (marker status is ok or partial); 2 = no reports,
# malformed/unsupported input, or an unwritable output path. A failure prints
# PLAYTEST_SUMMARY_ERROR and no marker.
#
# The summarizer is measurement-free and observation-only. It counts what an
# observer already recorded; it does not run the route or answer Q-101.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PY="$ROOT/scripts/summarize_playtest.py"
REPORT_DIR="${PLAYTEST_DIR:-$ROOT/docs/playtest}"

if [[ ! -f "$PY" ]]; then
	echo "check_playtest_report: missing $PY" >&2
	exit 2
fi

# Forward explicit arguments (report paths, --markdown/--json, --self-test).
if [[ ${#} -gt 0 ]]; then
	exec python3 "$PY" "$@"
fi

shopt -s nullglob
REPORTS=("$REPORT_DIR"/*.json)
if [[ ${#REPORTS[@]} -eq 0 ]]; then
	cat >&2 <<EOF
check_playtest_report: no capture reports in $REPORT_DIR

Usage:
  ./scripts/check_playtest_report.sh [REPORT...] [--markdown PATH] [--json PATH]

With no REPORT, every *.json in the report directory is summarized (default:
docs/playtest). Export a capture from godot/playtest_capture.tscn (F7) and copy
it into that directory, or pass the exported file or directory explicitly.

  user:// path (Linux):
    ~/.local/share/godot/app_userdata/Townsmith Phase 0/q101_playtest_*.json

  ./scripts/check_playtest_report.sh \\
    ~/.local/share/godot/app_userdata/"Townsmith Phase 0"/q101_playtest_*.json

  # Fixtures and tool checks:
  ./scripts/check_playtest_report.sh --self-test

Expected marker:
  PLAYTEST_SUMMARY reports=N completed=N issues=N status=ok|partial

A summary is not a playtest: a human must run the Q-101 session and record it
in docs/66 §8 before this wrapper has anything to aggregate.
EOF
	exit 2
fi

ARGS=("$REPORT_DIR")
if [[ -n "${PLAYTEST_MD:-}" ]]; then
	ARGS+=(--markdown "$PLAYTEST_MD")
fi
if [[ -n "${PLAYTEST_JSON:-}" ]]; then
	ARGS+=(--json "$PLAYTEST_JSON")
fi

exec python3 "$PY" "${ARGS[@]}"
