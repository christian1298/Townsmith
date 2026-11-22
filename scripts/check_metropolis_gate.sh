#!/usr/bin/env bash
# Evaluate the newest recorded Metropolis gate (G5) JSON against docs/68.
#
# With no arguments, finds the newest late_metropolis_v1.json and
# cohort_1m_v1.json under benchmarks/results/gate/ and evaluates whichever
# exist. Arguments are forwarded to scripts/check_metropolis_gate.py, so an
# explicit run is:
#
#   ./scripts/check_metropolis_gate.sh \
#     --result benchmarks/results/gate/late_metropolis_v1.json \
#     --scene late_metropolis_v1
#
# Env:
#   GATE_DIR          result directory (default benchmarks/results/gate)
#   REQUIRE_HARDWARE  1 adds --require-hardware (non-reference rig fails)
#   GATE_REPORT       write the evaluator report to this path
#
# The evaluator is measurement-free. It does not substitute for a recorded
# reference-hardware C1/C2 run and is intentionally not wired into check.sh.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PY="$ROOT/scripts/check_metropolis_gate.py"
GATE_DIR="${GATE_DIR:-$ROOT/benchmarks/results/gate}"

if [[ ${#} -gt 0 ]]; then
	exec python3 "$PY" "$@"
fi

shopt -s nullglob
ARGS=()
FOUND=0
for SCENE in late_metropolis_v1 cohort_1m_v1; do
	LATEST=""
	for CANDIDATE in "$GATE_DIR/$SCENE.json" "$GATE_DIR/${SCENE}_"*.json; do
		[[ -f "$CANDIDATE" ]] || continue
		if [[ -z "$LATEST" || "$CANDIDATE" -nt "$LATEST" ]]; then
			LATEST="$CANDIDATE"
		fi
	done
	if [[ -n "$LATEST" ]]; then
		ARGS+=(--result "$LATEST")
		FOUND=$((FOUND + 1))
	fi
done

if [[ "$FOUND" -eq 0 ]]; then
	cat >&2 <<EOF
check_metropolis_gate: no gate result found in $GATE_DIR
Expected:
  $GATE_DIR/late_metropolis_v1.json
  $GATE_DIR/cohort_1m_v1.json
Run the reference-hardware gate first (see docs/68 §6 and §10), or pass
--result PATH explicitly. No reference-hardware result exists in the repository.
EOF
	exit 2
fi

if [[ "${REQUIRE_HARDWARE:-0}" == "1" ]]; then
	ARGS+=(--require-hardware)
fi
if [[ -n "${GATE_REPORT:-}" ]]; then
	ARGS+=(--report "$GATE_REPORT")
fi

exec python3 "$PY" "${ARGS[@]}"
