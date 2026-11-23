#!/usr/bin/env bash
# Assemble recorded 1M harness output into the docs/68 late gate result.
#
# The Python assembler is measurement-free: it copies cohort, save, streaming,
# and optional render numbers into the late_metropolis_v1 schema. It never
# renders or invents FPS/VRAM/draw-call values. Missing render evidence stays
# null and the result is `status=insufficient`.
#
# `status=complete` also requires the late_metropolis_v1 scene on the
# documented late_flyover_v1 route and the §5.5 run shape on that route. A
# missing shape field, dev late_* route, or wrapper with no gate route is
# `insufficient`, with render.scene/render.route in `missing`.
#
# Examples:
#   ./scripts/assemble_gate_result.sh \
#     --cohort /tmp/cohort_1m_v1.json \
#     --save /tmp/save_1m.json \
#     --streaming /tmp/streaming_scale.json \
#     --out benchmarks/results/gate/late_metropolis_v1.json
#
#   ./scripts/assemble_gate_result.sh --render /tmp/late_render.json \
#     --cohort /tmp/cohort_1m_v1.json --out /tmp/late.json
#
#   ./scripts/assemble_gate_result.sh --self-test
#
# Arguments are forwarded unchanged. With no --out (or --out -) the JSON goes
# to stdout and the GATE_RESULT_ASSEMBLE marker goes to stderr.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PY="$ROOT/scripts/assemble_gate_result.py"

if [[ ! -f "$PY" ]]; then
	echo "assemble_gate_result: missing $PY" >&2
	exit 2
fi

exec python3 "$PY" "$@"
