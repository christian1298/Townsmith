#!/usr/bin/env python3
"""Assemble a docs/68 late_metropolis_v1 gate result from recorded evidence.

The assembler copies real numbers out of existing harness reports into the
schema in ``benchmarks/results/gate/README.md``. It never renders, measures, or
synthesises a number:

* ``--cohort``    -> ``cohort_1m_v1`` report (cohort, agent, sim, server RSS)
* ``--save``      -> ``save_1m`` report (save/load time and save size)
* ``--streaming`` -> ``streaming_scale`` report (stall counters when recorded)
* ``--render``    -> one late render run/summary (FPS, frame times, VRAM, ...)

A field with no recorded evidence is ``null`` and, when the gate needs it, is
listed in ``missing``. ``status=complete`` means the required late fields are
present, the render record is the evaluator scene (``late_metropolis_v1``) on
the documented gate route (``late_flyover_v1``), and the run has the §5.5 shape
(90 s+, 15 s warm-up, 3 runs, not headless). A missing shape field or any other
route is recorded in ``missing``/warnings and keeps the status ``insufficient``.
``status=complete`` does **not** mean the gate passed: the evaluator still
decides pass/fail, and the signed report is still required.

A multi-route summary contributes only the selected route's metrics; wrapper
level metric/result/route/shape keys are discarded so they cannot stand in for
route evidence. Writing a non-complete artifact under
``benchmarks/results/gate/`` prints a warning because the shell evaluator picks
the newest file.

The flat fields are the assembled evidence. ``results`` mirrors them under the
canonical docs/68 §6.3 evaluator names so ``check_metropolis_gate.py`` can read
the same file. ``provenance.fields`` records the exact input path for every
value, and ``thresholds`` copies the docs/68 late-scene budgets unchanged.
``checksum`` is SHA-256 over the canonical JSON with the checksum field
removed; the same inputs produce the same bytes and checksum.

Usage:
    ./scripts/assemble_gate_result.py \
        --cohort /tmp/cohort_1m_v1.json \
        --save /tmp/save_1m.json \
        --streaming /tmp/streaming_scale.json \
        --out benchmarks/results/gate/late_metropolis_v1.json

    # A render capture adds the render half; missing fields stay null.
    ./scripts/assemble_gate_result.py --cohort ... --save ... \
        --render /tmp/late_render.json --hardware-note "8c/16t / 3060 12 GB" \
        --out benchmarks/results/gate/late_metropolis_v1.json

    ./scripts/assemble_gate_result.py --self-test

With no ``--out`` (or ``--out -``) the JSON goes to stdout and the marker goes
to stderr. ``--out PATH`` writes the file and prints the marker on stdout.

Exit status:
    0  assembled (``status=complete`` or ``status=insufficient``)
    2  bad input: unreadable/invalid JSON, wrong scene/schema, or unwritable out
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import math
import os
import subprocess
import sys
import tempfile
from contextlib import redirect_stderr, redirect_stdout
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Sequence

SCENE = "late_metropolis_v1"
GATE_RENDER_ROUTE = "late_flyover_v1"
GATE = "G5"
FORMAT_VERSION = 1
MARKER = "GATE_RESULT_ASSEMBLE"
CHECKSUM_PREFIX = "sha256:"
THRESHOLD_SOURCE = "docs/68 §5, §8.2"
EVIDENCE_SCOPE = (
    "Copied from recorded harness output only. No renderer, GPU, FPS, frame "
    "time, VRAM, draw-call, or triangle value is synthesised. status=complete "
    "means the required late fields are present; it is not a gate pass."
)


class AssemblyError(Exception):
    """Bad input or an assembly that cannot be represented honestly."""


@dataclass(frozen=True)
class Metric:
    """One flat result field and the report keys that can supply it."""

    field: str
    aliases: tuple[str, ...]
    integer: bool = False


# docs/68 §8.2 #10-#12 and §5.3. These come from the headless cohort report.
COHORT_METRICS = (
    Metric(
        "cohort_tick_ms_p99",
        ("cohort_tick_ms_p99", "cohort_step_ms_p99", "cohort_ms_p99"),
    ),
    Metric(
        "cohort_aggregate_ms_p99",
        (
            "full_tick_ms_p99",
            "cohort_aggregate_ms_p99",
            "aggregate_ms_p99",
            "sim_tick_ms_p99",
        ),
    ),
    Metric(
        "agent_update_ms_p99",
        (
            "active_agents_ms_p99",
            "agent_update_ms_p99",
            "agents_ms_p99",
            "agent_tick_ms_p99",
        ),
    ),
    Metric("pathfinding_ms_p99", ("pathfinding_ms_p99", "path_ms_p99")),
    Metric("sim_cpu_ms_per_s", ("sim_cpu_ms_per_s", "total_sim_cpu_ms_per_s")),
    Metric(
        "server_rss_mb_peak",
        ("server_rss_mb_peak", "server_rss_mb", "rss_mb_peak"),
    ),
)

# docs/68 §8.2 #15. save_1m has no results object; every field is top level.
SAVE_METRICS = (
    Metric("save_ms", ("total_save_ms", "save_ms", "save_time_ms")),
    Metric("load_ms", ("total_load_ms", "load_ms", "load_time_ms")),
)

# The current streaming_scale harness records no frame-stall counters. If a
# future report adds them, copy them; otherwise they stay null.
STREAMING_METRICS = (
    Metric(
        "streaming_stalls_100ms",
        (
            "streaming_stalls_100ms",
            "streaming_stalls",
            "stalls_streaming_100ms",
        ),
        integer=True,
    ),
    Metric(
        "dropped_chunks",
        ("dropped_chunks", "dropped_chunks_total"),
        integer=True,
    ),
)

# docs/68 §5.1, §5.2, §5.4, §8.2. Only a render capture can supply these.
RENDER_METRICS = (
    Metric("fps_avg", ("fps_avg", "fps_median", "median_fps")),
    Metric("fps_1pct_low", ("fps_low_1pct", "low_1pct", "fps_1pct_low")),
    Metric(
        "fps_low_0_1pct",
        ("fps_low_0_1pct", "low_0_1pct", "fps_0_1pct_low"),
    ),
    Metric("frame_ms_p95", ("frametime_ms_p95", "p95_ms", "frame_ms_p95")),
    Metric("frame_ms_p99", ("frametime_ms_p99", "p99_ms", "frame_ms_p99")),
    Metric(
        "frame_ms_p99_9",
        (
            "frametime_ms_p99_9",
            "p99_9_ms",
            "frametime_ms_p999",
            "frame_ms_p99_9",
        ),
    ),
    Metric(
        "frame_ms_max",
        (
            "frametime_ms_max",
            "max_frame_ms",
            "frametime_ms_peak",
            "frame_ms_max",
        ),
    ),
    Metric(
        "hitches_50ms",
        ("hitches_50ms", "hitches_gt_50ms", "hitches_over_50ms"),
        integer=True,
    ),
    Metric(
        "hitches_250ms",
        ("stalls_250ms", "stalls_gt_250ms", "stalls", "hitches_250ms"),
        integer=True,
    ),
    Metric(
        "draw_calls",
        ("draw_calls_p99", "draw_calls"),
        integer=True,
    ),
    Metric(
        "triangles",
        ("triangles_avg", "triangles_median", "triangles"),
        integer=True,
    ),
    Metric(
        "vram_mb_peak",
        ("vram_mb_peak", "vram_peak_mb", "vram_mb"),
    ),
    Metric(
        "rss_mb_peak",
        ("ram_mb_peak", "rss_mb_peak", "rss_mb", "ram_peak_mb"),
    ),
    Metric("save_ms", ("save_ms", "save_time_ms")),
    Metric("load_ms", ("load_ms", "load_time_ms")),
    Metric(
        "streaming_stalls_100ms",
        (
            "streaming_stalls_100ms",
            "streaming_stalls",
            "stalls_streaming_100ms",
        ),
        integer=True,
    ),
    Metric(
        "dropped_chunks",
        ("dropped_chunks", "dropped_chunks_total"),
        integer=True,
    ),
    Metric(
        "overhead_pct",
        ("overhead_pct", "benchmark_overhead_pct"),
    ),
    Metric(
        "run_spread_pct",
        (
            "fps_avg_spread_pct",
            "run_spread_pct",
            "spread_pct",
            "variance_pct",
        ),
    ),
    Metric(
        "vbus_mb_peak",
        ("vbus_mb_peak", "vbus_peak_mb", "vbus_mb", "gpu_buffer_mb_peak"),
    ),
)

# The flat public fields, in stable schema order. frame_ms_max is included
# because the evaluator treats it as a required late field even though the
# tooling brief's short list usually omits it.
CORE_FIELDS = (
    "scene",
    "format_version",
    "seed",
    "hardware",
    "settings",
    "route",
    "duration_s",
    "warmup_s",
    "runs",
    "headless",
    "fps_avg",
    "fps_1pct_low",
    "frame_ms_p95",
    "frame_ms_max",
    "hitches_50ms",
    "hitches_250ms",
    "draw_calls",
    "triangles",
    "vram_mb_peak",
    "rss_mb_peak",
    "save_ms",
    "save_size_mb",
    "streaming_stalls_100ms",
    "overhead_pct",
    "run_spread_pct",
    "cohort_tick_ms_p99",
    "cohort_aggregate_ms_p99",
    "agent_update_ms_p99",
    "pathfinding_ms_p99",
    "sim_cpu_ms_per_s",
    "server_rss_mb_peak",
)

OPTIONAL_FIELDS = (
    "fps_low_0_1pct",
    "frame_ms_p99",
    "frame_ms_p99_9",
    "dropped_chunks",
    "vbus_mb_peak",
    "load_ms",
)

# The evaluator's canonical names. The flat block uses the short evidence
# names above; results makes the same file readable by
# check_metropolis_gate.py without a second conversion step.
RESULTS_KEYS = {
    "fps_avg": "fps_avg",
    "fps_1pct_low": "fps_low_1pct",
    "fps_low_0_1pct": "fps_low_0_1pct",
    "frame_ms_p95": "frametime_ms_p95",
    "frame_ms_p99": "frametime_ms_p99",
    "frame_ms_p99_9": "frametime_ms_p99_9",
    "frame_ms_max": "frametime_ms_max",
    "hitches_50ms": "hitches_50ms",
    "hitches_250ms": "stalls_250ms",
    "draw_calls": "draw_calls_p99",
    "triangles": "triangles_avg",
    "vram_mb_peak": "vram_mb_peak",
    "rss_mb_peak": "ram_mb_peak",
    "save_ms": "save_ms",
    "save_size_mb": "save_size_mb",
    "streaming_stalls_100ms": "streaming_stalls_100ms",
    "dropped_chunks": "dropped_chunks",
    "overhead_pct": "overhead_pct",
    "run_spread_pct": "fps_avg_spread_pct",
    "vbus_mb_peak": "vbus_mb_peak",
    "load_ms": "load_ms",
    "cohort_tick_ms_p99": "cohort_tick_ms_p99",
    "cohort_aggregate_ms_p99": "full_tick_ms_p99",
    "agent_update_ms_p99": "active_agents_ms_p99",
    "pathfinding_ms_p99": "pathfinding_ms_p99",
    "sim_cpu_ms_per_s": "sim_cpu_ms_per_s",
    "server_rss_mb_peak": "server_rss_mb_peak",
}

# Copied from docs/68 §5 and §8.2. The evaluator remains the source of truth;
# this block is a read-only snapshot for the assembled artifact.
THRESHOLDS = {
    "fps_avg": ">=30",
    "fps_1pct_low": ">=22",
    "fps_low_0_1pct": ">=15",
    "frame_ms_p95": "<=38",
    "frame_ms_p99": "<=45",
    "frame_ms_p99_9": "<=67",
    "frame_ms_max": "<=250",
    "hitches_50ms": "<=3",
    "hitches_250ms": "<=0",
    "draw_calls": "<2000",
    "triangles": "5000000..10000000",
    "vram_mb_peak": "<=10752 MiB (10.5 GiB)",
    "rss_mb_peak": "<=12288 MiB (12 GiB)",
    "save_ms": "<=30000",
    "load_ms": "<=30000",
    "save_size_mb": ">0",
    "streaming_stalls_100ms": "<=0",
    "dropped_chunks": "<=0",
    "overhead_pct": "<2",
    "run_spread_pct": "<5",
    "vbus_mb_peak": "<=8192 MiB (8 GiB, supplemental)",
    "cohort_tick_ms_p99": "<=10",
    "cohort_aggregate_ms_p99": "<=50",
    "agent_update_ms_p99": "<=10",
    "pathfinding_ms_p99": "<=4",
    "sim_cpu_ms_per_s": "<=250",
    "server_rss_mb_peak": "<=4096 MiB (4 GiB)",
    "runs": ">=3",
    "duration_s": ">=90 (target 90-120)",
    "warmup_s": ">=15",
    "headless": "false",
}

INPUT_SCENES = {
    "cohort": ("cohort_1m_v1",),
    "save": ("save_1m",),
    "streaming": ("streaming_scale",),
}
REQUIRED_SKIP = {"scene", "format_version", "seed"}
HARDWARE_FIELDS = ("cpu", "ram_gb", "gpu", "display")

# docs/68 §5.5 run shape. These must come from the selected render route; a
# wrapper-level or settings-level value is not route evidence.
SHAPE_FIELDS = ("duration_s", "warmup_s", "runs", "headless")
SAVE_SIZE_ALIASES = (
    "save_size_mb",
    "save_mb",
    "save_size_gb",
    "save_gb",
    "save_bytes",
    "world_save_bytes",
    "json_bytes",
)


def _render_metric_aliases() -> frozenset[str]:
    aliases = set(SAVE_SIZE_ALIASES)
    for metric in RENDER_METRICS:
        aliases.update(metric.aliases)
    return frozenset(aliases)


RENDER_METRIC_ALIASES = _render_metric_aliases()
WRAPPER_STRIP_FIELDS = RENDER_METRIC_ALIASES | frozenset(SHAPE_FIELDS) | {
    "results",
    "route",
    "route_id",
}

REPO_ROOT = Path(__file__).resolve().parent.parent
GATE_RESULTS_DIR = REPO_ROOT / "benchmarks" / "results" / "gate"


def _number(value: Any) -> float | None:
    if isinstance(value, bool) or value is None:
        return None
    if isinstance(value, (int, float)):
        number = float(value)
    elif isinstance(value, str):
        try:
            number = float(value.strip())
        except ValueError:
            return None
    else:
        return None
    return number if math.isfinite(number) else None


def _integer(value: Any) -> int | None:
    number = _number(value)
    return None if number is None else int(number)


def _text(value: Any) -> str | None:
    if isinstance(value, str) and value.strip():
        return value.strip()
    return None


def _present(value: Any) -> bool:
    if value is None:
        return False
    if isinstance(value, str):
        return bool(value.strip())
    if isinstance(value, (dict, list, tuple)):
        return bool(value)
    return True


def _flag(value: Any) -> bool | None:
    if isinstance(value, bool):
        return value
    if isinstance(value, str):
        lowered = value.strip().lower()
        if lowered == "true":
            return True
        if lowered == "false":
            return False
    return None


def _seed_value(value: Any) -> Any:
    if isinstance(value, bool) or value is None:
        return None
    if isinstance(value, int):
        return value
    if isinstance(value, str):
        text = value.strip()
        if not text:
            return None
        try:
            return int(text, 0)
        except ValueError:
            return text
    return None


def _read_json(path: Path) -> Any:
    try:
        text = path.read_text(encoding="utf-8")
    except FileNotFoundError as error:
        raise AssemblyError(f"file not found: {path}") from error
    except OSError as error:
        raise AssemblyError(f"cannot read {path}: {error}") from error
    try:
        return json.loads(text)
    except json.JSONDecodeError as error:
        raise AssemblyError(f"invalid JSON in {path}: {error}") from error


def _find(document: dict[str, Any], aliases: Sequence[str]) -> tuple[Any, str] | None:
    """Return the first recorded value and its dotted source path."""

    results = document.get("results")
    sources: list[tuple[str, dict[str, Any]]] = []
    if isinstance(results, dict):
        sources.append(("results.", results))
    sources.append(("", document))
    for prefix, source in sources:
        for alias in aliases:
            if alias not in source:
                continue
            value = source[alias]
            if not _present(value):
                continue
            return value, f"{prefix}{alias}"
    return None


def _find_number(
    document: dict[str, Any], aliases: Sequence[str]
) -> tuple[float, str] | None:
    found = _find(document, aliases)
    if found is None:
        return None
    raw, source = found
    number = _number(raw)
    if number is None:
        return None
    return number, source


def _find_save_size(document: dict[str, Any]) -> tuple[float, str] | None:
    direct = _find_number(document, ("save_size_mb", "save_mb"))
    if direct is not None:
        return direct
    gigabytes = _find_number(document, ("save_size_gb", "save_gb"))
    if gigabytes is not None:
        return gigabytes[0] * 1024.0, f"{gigabytes[1]}*1024"
    byte_count = _find_number(
        document, ("save_bytes", "world_save_bytes", "json_bytes")
    )
    if byte_count is not None:
        return (
            round(byte_count[0] / (1024.0 * 1024.0), 6),
            f"{byte_count[1]}/(1024*1024)",
        )
    return None


def _load_input(path: Path, role: str) -> dict[str, Any]:
    document = _read_json(path)
    if not isinstance(document, dict):
        raise AssemblyError(f"{role} input root must be a JSON object: {path}")
    if role in INPUT_SCENES:
        scene = _text(document.get("scene"))
        if scene is None:
            raise AssemblyError(f"{role} input has no scene id: {path}")
        if scene not in INPUT_SCENES[role]:
            expected = "|".join(INPUT_SCENES[role])
            raise AssemblyError(
                f"{role} input scene={scene!r} is not {expected}: {path}"
            )
    else:
        document = _select_render_route(document, path)
        _validate_render(document, path)

    results = document.get("results")
    if role == "cohort":
        if not isinstance(results, dict):
            raise AssemblyError(
                f"cohort input has no results object: {path}"
            )
    elif results is not None and not isinstance(results, dict):
        raise AssemblyError(f"{role} input results must be an object: {path}")
    return document


def _render_route_id(value: dict[str, Any]) -> str | None:
    """Return a route id from a route summary entry."""

    route = value.get("route")
    if route is None:
        route = value.get("route_id")
    if isinstance(route, dict):
        route = route.get("id") or route.get("name") or route.get("route")
    return _text(route)


def _strip_wrapper_evidence(document: dict[str, Any]) -> dict[str, Any]:
    """Drop wrapper-level render evidence that cannot be attributed to a route.

    A multi-route summary's top-level metric, results, route, and shape keys
    may belong to another route. Only the selected route may supply them.
    """

    merged: dict[str, Any] = {}
    for field, value in document.items():
        if field in WRAPPER_STRIP_FIELDS:
            continue
        if field == "settings" and isinstance(value, dict):
            value = {
                key: item
                for key, item in value.items()
                if key not in SHAPE_FIELDS
            }
        merged[field] = value
    return merged


def _select_render_route(
    document: dict[str, Any], path: Path
) -> dict[str, Any]:
    """Pick the documented late gate route from a multi-route summary.

    A route is acceptable only when its id is the docs/68 gate route
    (``late_flyover_v1``). A non-gate ``late_*`` route is selected for honest
    provenance but never reaches ``status=complete``; a summary with no gate
    route keeps no route evidence and fails closed.
    """

    routes = document.get("routes")
    if routes is None:
        return document
    if not isinstance(routes, dict):
        raise AssemblyError(f"render input routes must be an object: {path}")

    gate_candidates: list[tuple[str, dict[str, Any]]] = []
    late_candidates: list[tuple[str, dict[str, Any]]] = []
    for key, value in routes.items():
        if not isinstance(value, dict):
            continue
        route_id = _render_route_id(value)
        if route_id == GATE_RENDER_ROUTE:
            gate_candidates.append((key, value))
        elif "late" in key.lower() or "late" in (route_id or "").lower():
            late_candidates.append((key, value))

    if len(gate_candidates) > 1:
        names = ",".join(key for key, _ in gate_candidates)
        raise AssemblyError(
            f"render input has multiple {GATE_RENDER_ROUTE} routes ({names}); "
            f"pass one single-run summary: {path}"
        )
    if gate_candidates:
        key, route = gate_candidates[0]
    elif len(late_candidates) == 1:
        # Keep the recorded evidence and selected-route provenance, but the
        # route gate below keeps status=insufficient.
        key, route = late_candidates[0]
    else:
        if late_candidates:
            names = ",".join(key for key, _ in late_candidates)
            raise AssemblyError(
                f"render input has multiple late routes ({names}); pass one "
                f"single-run summary: {path}"
            )
        merged = _strip_wrapper_evidence(document)
        merged["_assembly_render_route_error"] = (
            f"render input has no {GATE_RENDER_ROUTE} route; pass one gate "
            f"route summary: {path}"
        )
        return merged

    merged = _strip_wrapper_evidence(document)
    merged.update(route)
    merged["_assembly_render_route"] = key
    return merged


def _validate_render(document: dict[str, Any], path: Path) -> None:
    results = document.get("results")
    if results is not None and not isinstance(results, dict):
        raise AssemblyError(f"render input results must be an object: {path}")
    routes = document.get("routes")
    if routes is not None and not isinstance(routes, dict):
        raise AssemblyError(f"render input routes must be an object: {path}")


def _render_problems(render: dict[str, Any] | None) -> dict[str, str]:
    """Return missing gate evidence, keyed by a ``missing`` entry name."""

    problems: dict[str, str] = {}
    if render is None:
        return problems

    scene = _text(render.get("scene"))
    if scene != SCENE:
        problems["render.scene"] = (
            f"render scene is not {SCENE}: {scene or 'absent'}"
        )

    route = _render_route_id(render)
    error = render.get("_assembly_render_route_error")
    if isinstance(error, str) and error:
        problems["render.route"] = error
    elif route != GATE_RENDER_ROUTE:
        problems["render.route"] = (
            f"render route is not the gate route {GATE_RENDER_ROUTE}: "
            f"{route or 'absent'}"
        )
    return problems


def _apply_metric(
    document: dict[str, Any],
    sources: dict[str, str | None],
    metric: Metric,
    value: float | None,
    source: str,
    *,
    overwrite: bool = False,
) -> None:
    if value is None:
        return
    if not overwrite and document.get(metric.field) is not None:
        return
    document[metric.field] = int(value) if metric.integer else value
    sources[metric.field] = source


def _apply_metrics(
    document: dict[str, Any],
    sources: dict[str, str | None],
    role: str,
    report: dict[str, Any],
    metrics: Sequence[Metric],
    *,
    overwrite: bool = False,
) -> None:
    for metric in metrics:
        found = _find_number(report, metric.aliases)
        if found is None:
            continue
        value, source = found
        _apply_metric(
            document,
            sources,
            metric,
            value,
            f"{role}.{source}",
            overwrite=overwrite,
        )


def _apply_save_size(
    document: dict[str, Any],
    sources: dict[str, str | None],
    role: str,
    report: dict[str, Any],
) -> None:
    if document.get("save_size_mb") is not None:
        return
    found = _find_save_size(report)
    if found is None:
        return
    value, source = found
    document["save_size_mb"] = value
    sources["save_size_mb"] = f"{role}.{source}"


def _copy_render_metadata(
    document: dict[str, Any],
    sources: dict[str, str | None],
    render: dict[str, Any] | None,
) -> None:
    if render is None:
        return

    route = render.get("route")
    if route is None:
        route = render.get("route_id")
    if _present(route):
        document["route"] = route
        sources["route"] = "render.route" if "route" in render else "render.route_id"

    settings = render.get("settings")
    if _present(settings):
        document["settings"] = settings
        sources["settings"] = "render.settings"

    settings_object = settings if isinstance(settings, dict) else {}
    for field in ("duration_s", "warmup_s"):
        value = _number(render.get(field))
        source = f"render.{field}"
        if value is None:
            value = _number(settings_object.get(field))
            source = f"render.settings.{field}"
        if value is not None:
            document[field] = value
            sources[field] = source

    runs = _integer(render.get("runs"))
    source = "render.runs"
    if runs is None:
        runs = _integer(settings_object.get("runs"))
        source = "render.settings.runs"
    if runs is not None:
        document["runs"] = runs
        sources["runs"] = source

    flag = _flag(render.get("headless"))
    if flag is not None:
        document["headless"] = flag
        sources["headless"] = "render.headless"

    hardware = render.get("hardware")
    if isinstance(hardware, dict) and hardware:
        document["hardware"] = hardware
        sources["hardware"] = "render.hardware"


def _hardware_present(
    document: dict[str, Any], hardware: dict[str, Any], field: str
) -> bool:
    if field == "cpu":
        for key in ("cpu", "processor", "cpu_model"):
            if _text(hardware.get(key)) is not None:
                return True
        for key in ("cpu_cores", "cores", "physical_cores"):
            if _integer(hardware.get(key)) is not None:
                return True
        return False
    if field == "ram_gb":
        for key in ("ram_gb", "memory_gb"):
            if _number(hardware.get(key)) is not None:
                return True
        for key in ("ram_mb", "memory_mb"):
            if _number(hardware.get(key)) is not None:
                return True
        for key in ("ram_bytes", "memory_bytes"):
            if _number(hardware.get(key)) is not None:
                return True
        return False
    if field == "gpu":
        return any(
            _text(hardware.get(key)) is not None
            for key in ("gpu", "video_adapter", "gpu_model")
        )
    if field == "display":
        for source in (hardware, document):
            for key in (
                "display",
                "display_resolution",
                "resolution",
                "window_size",
            ):
                if _present(source.get(key)):
                    return True
        settings = document.get("settings")
        if isinstance(settings, dict):
            for key in ("display", "resolution", "window_size"):
                if _present(settings.get(key)):
                    return True
        return False
    return False


def _missing_required(document: dict[str, Any]) -> list[str]:
    missing: list[str] = []
    for field in CORE_FIELDS:
        if field in REQUIRED_SKIP:
            continue
        if field == "hardware":
            hardware = document.get("hardware")
            if not isinstance(hardware, dict) or not hardware:
                missing.append("hardware")
                continue
            for hardware_field in HARDWARE_FIELDS:
                if not _hardware_present(document, hardware, hardware_field):
                    missing.append(f"hardware.{hardware_field}")
            continue
        if not _present(document.get(field)):
            missing.append(field)
    return missing


def _shape_problems(document: dict[str, Any]) -> list[str]:
    problems: list[str] = []
    duration = _number(document.get("duration_s"))
    if duration is None:
        problems.append("duration_s is absent (>=90 s required)")
    elif duration < 90.0:
        problems.append(f"duration_s={duration:g} is below 90 s")
    warmup = _number(document.get("warmup_s"))
    if warmup is None:
        problems.append("warmup_s is absent (>=15 s required)")
    elif warmup < 15.0:
        problems.append(f"warmup_s={warmup:g} is below 15 s")
    runs = _integer(document.get("runs"))
    if runs is None:
        problems.append("runs is absent (>=3 required)")
    elif runs < 3:
        problems.append(f"runs={runs} is below 3")
    headless = _flag(document.get("headless"))
    if headless is None:
        problems.append("headless is absent (false required)")
    elif headless is True:
        problems.append("headless=true cannot produce FPS evidence")
    return problems


def _input_summary(path: Path, document: dict[str, Any]) -> dict[str, Any]:
    summary: dict[str, Any] = {"path": str(path)}
    for key in (
        "format_version",
        "scene",
        "suite",
        "seed",
        "route",
        "headless",
        "status",
    ):
        if key in document:
            summary[key] = document[key]
    checksum = _find(document, ("checksum", "combined_checksum"))
    if checksum is not None:
        summary["checksum"] = str(checksum[0])
    selected_route = document.get("_assembly_render_route")
    if isinstance(selected_route, str):
        summary["selected_route"] = selected_route
    return summary


def _warnings(
    document: dict[str, Any],
    cohort: dict[str, Any] | None,
    save: dict[str, Any] | None,
    streaming: dict[str, Any] | None,
    render: dict[str, Any] | None,
) -> list[str]:
    messages: list[str] = []
    if render is None:
        messages.append(
            "no render result supplied: FPS, frame-time, VRAM, draw-call, and "
            "triangle fields are null"
        )
    if cohort is not None and document.get("pathfinding_ms_p99") is None:
        messages.append("cohort input records no pathfinding_ms_p99")
    if streaming is not None and document.get("streaming_stalls_100ms") is None:
        messages.append(
            "streaming input records no >100 ms frame-stall counter; "
            "streaming_stalls_100ms is null"
        )
    if save is None and document.get("save_ms") is None:
        messages.append("no save result supplied: save_ms/save_size_mb are null")
    for problem in _shape_problems(document):
        messages.append(f"run shape: {problem}")
    for problem in _render_problems(render).values():
        messages.append(f"render evidence: {problem}")
    if render is not None:
        selected = render.get("_assembly_render_route")
        if isinstance(selected, str):
            messages.append(
                f"multi-route render summary: selected route={selected}"
            )
    hardware = document.get("hardware")
    if isinstance(hardware, dict):
        gpu = None
        for key in ("gpu", "video_adapter", "gpu_model"):
            gpu = _text(hardware.get(key))
            if gpu is not None:
                break
        if gpu is not None and "3060" not in gpu.lower():
            messages.append(
                f"recorded GPU is not the docs/68 §3.1 RTX 3060 reference: {gpu}"
            )
    return messages


def assemble_document(
    *,
    cohort: dict[str, Any] | None = None,
    save: dict[str, Any] | None = None,
    streaming: dict[str, Any] | None = None,
    render: dict[str, Any] | None = None,
    scene: str = SCENE,
    seed: Any = None,
    hardware_note: str | None = None,
) -> tuple[dict[str, Any], list[str], int]:
    """Build the result document, its missing-required list, and field count."""

    if scene != SCENE:
        raise AssemblyError(
            f"unsupported assembly scene={scene!r}; only {SCENE} is assembled"
        )

    document: dict[str, Any] = {
        "format_version": FORMAT_VERSION,
        "gate": GATE,
        "scene": SCENE,
        "seed": None,
        "hardware": None,
        "settings": None,
        "route": None,
        "duration_s": None,
        "warmup_s": None,
        "runs": None,
        "headless": None,
    }
    for field in CORE_FIELDS + OPTIONAL_FIELDS:
        document.setdefault(field, None)

    sources: dict[str, str | None] = {field: None for field in document}
    sources["format_version"] = "assembler"
    sources["gate"] = "assembler"
    sources["scene"] = "assembler"

    resolved_seed = _seed_value(seed)
    seed_source = "cli" if resolved_seed is not None else None
    if resolved_seed is None and render is not None:
        resolved_seed = _seed_value(render.get("seed"))
        seed_source = "render.seed" if resolved_seed is not None else None
    document["seed"] = resolved_seed
    sources["seed"] = seed_source

    # Dedicated harness values win over a render capture for the fields they
    # own; the render fallback fills them only when the dedicated input is
    # absent. Render-only fields are always filled from the render report.
    if cohort is not None:
        _apply_metrics(document, sources, "cohort", cohort, COHORT_METRICS)
    if save is not None:
        _apply_metrics(document, sources, "save", save, SAVE_METRICS)
        _apply_save_size(document, sources, "save", save)
    if streaming is not None:
        _apply_metrics(
            document, sources, "streaming", streaming, STREAMING_METRICS
        )
    if render is not None:
        _copy_render_metadata(document, sources, render)
        _apply_metrics(document, sources, "render", render, RENDER_METRICS)
        _apply_save_size(document, sources, "render", render)

    results = {
        canonical: document.get(flat)
        for flat, canonical in RESULTS_KEYS.items()
    }
    missing = _missing_required(document)
    for field in _render_problems(render):
        if field not in missing:
            missing.append(field)
    shape_problems = _shape_problems(document)
    status = "complete" if not missing and not shape_problems else "insufficient"

    # Stable role order, with the path each document came from. The caller
    # supplies paths through assemble_from_paths; direct callers get a summary
    # without a path so the document stays deterministic.
    inputs = _provenance_inputs(
        cohort=cohort,
        save=save,
        streaming=streaming,
        render=render,
    )

    provenance: dict[str, Any] = {
        "assembler": "scripts/assemble_gate_result.py",
        "format_version": FORMAT_VERSION,
        "gate": GATE,
        "scene": SCENE,
        "thresholds_source": THRESHOLD_SOURCE,
        "inputs": inputs,
        "fields": {
            field: {
                "source": sources.get(field),
            }
            for field in document
            if field not in ("provenance", "status", "checksum", "results")
        },
        "warnings": _warnings(document, cohort, save, streaming, render),
        "evidence_scope": EVIDENCE_SCOPE,
    }
    if hardware_note:
        provenance["hardware_note"] = hardware_note
    if document.get("hardware") is not None:
        provenance["hardware_source"] = "render.hardware"

    document["results"] = results
    document["thresholds"] = {
        "source": THRESHOLD_SOURCE,
        "values": dict(THRESHOLDS),
    }
    document["missing"] = missing
    document["provenance"] = provenance
    document["status"] = status
    document["checksum"] = _checksum(document)
    field_count = sum(1 for value in results.values() if value is not None)
    return document, missing, field_count


def _provenance_inputs(
    *,
    cohort: dict[str, Any] | None,
    save: dict[str, Any] | None,
    streaming: dict[str, Any] | None,
    render: dict[str, Any] | None,
) -> dict[str, Any]:
    """Input summaries without inventing paths for direct in-memory callers."""

    inputs: dict[str, Any] = {}
    for role, value in (
        ("cohort", cohort),
        ("save", save),
        ("streaming", streaming),
        ("render", render),
    ):
        if value is None:
            continue
        labels = value.get("_assembly_path")
        if isinstance(labels, str):
            inputs[role] = _input_summary(Path(labels), value)
        else:
            inputs[role] = {
                key: value[key]
                for key in (
                    "format_version",
                    "scene",
                    "suite",
                    "seed",
                    "route",
                    "headless",
                    "status",
                )
                if key in value
            }
    return inputs


def _checksum(document: dict[str, Any]) -> str:
    payload = {
        key: value for key, value in document.items() if key != "checksum"
    }
    encoded = json.dumps(
        payload, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    ).encode("utf-8")
    return CHECKSUM_PREFIX + hashlib.sha256(encoded).hexdigest()


def _write_document(path: Path, document: dict[str, Any]) -> None:
    text = json.dumps(
        document, indent=2, sort_keys=True, ensure_ascii=False
    )
    text += "\n"
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        temporary = path.with_name(path.name + ".tmp")
        temporary.write_text(text, encoding="utf-8")
        os.replace(temporary, path)
    except OSError as error:
        raise AssemblyError(f"cannot write {path}: {error}") from error


def _in_gate_results_dir(path: Path) -> bool:
    try:
        path.resolve().relative_to(GATE_RESULTS_DIR)
    except (OSError, ValueError):
        return False
    return True


def assemble_from_paths(
    *,
    cohort_path: str | None,
    save_path: str | None,
    streaming_path: str | None,
    render_path: str | None,
    scene: str,
    seed: Any,
    hardware_note: str | None,
) -> tuple[dict[str, Any], list[str], int]:
    loaded: dict[str, dict[str, Any] | None] = {}
    for role, raw_path in (
        ("cohort", cohort_path),
        ("save", save_path),
        ("streaming", streaming_path),
        ("render", render_path),
    ):
        if not raw_path:
            loaded[role] = None
            continue
        path = Path(raw_path)
        document = _load_input(path, role)
        document["_assembly_path"] = str(path)
        loaded[role] = document
    return assemble_document(
        cohort=loaded["cohort"],
        save=loaded["save"],
        streaming=loaded["streaming"],
        render=loaded["render"],
        scene=scene,
        seed=seed,
        hardware_note=hardware_note,
    )


def _marker(
    document: dict[str, Any],
    missing: list[str],
    field_count: int,
    out: str,
) -> str:
    return (
        f"{MARKER} scene={document['scene']} fields={field_count} "
        f"missing={len(missing)} out={out} status={document['status']} "
        f"checksum={document['checksum']}"
    )


# --- self-test -------------------------------------------------------------


def _cohort_fixture() -> dict[str, Any]:
    return {
        "format_version": 1,
        "gate": "G5",
        "scene": "cohort_1m_v1",
        "route": "cohort_3600_ticks_v1",
        "settings": "headless_1hz_v1",
        "seed": 2026,
        "results": {
            "ticks": 3600,
            "cohort_tick_ms_p99": 0.2839,
            "full_tick_ms_p99": 3.5638,
            "active_agents_ms_p99": 3.3255,
            "pathfinding_ms_p99": 2.0,
            "sim_cpu_ms_per_s": 3.5638,
            "server_rss_mb_peak": 4.42,
            "checksum": "bb42539220e4c6a6",
        },
    }


def _save_fixture() -> dict[str, Any]:
    return {
        "scene": "save_1m",
        "seed": 2026,
        "placements": 250_000,
        "population": 1_000_000,
        "total_save_ms": 24_000.0,
        "total_load_ms": 21_000.0,
        "json_bytes": 182_808_978,
        "combined_checksum": "0123456789abcdef",
        "status": "ok",
    }


def _streaming_fixture() -> dict[str, Any]:
    return {
        "scene": "streaming_scale",
        "format_version": 1,
        "render": False,
        "seed": 2026,
        "streaming_stalls_100ms": 0,
        "dropped_chunks": 0,
        "checksum": "4dfb48204244e377",
        "status": "ok",
    }


def _render_fixture() -> dict[str, Any]:
    return {
        "format_version": 1,
        "scene": "late_metropolis_v1",
        "seed": 2026,
        "route": "late_flyover_v1",
        "settings": "standard_1080p_high_v1",
        "duration_s": 120,
        "warmup_s": 15,
        "runs": 3,
        "headless": False,
        "hardware": {
            "cpu": "AMD Ryzen 7 5800X 8-Core Processor",
            "cpu_cores": 8,
            "cpu_threads": 16,
            "gpu": "NVIDIA GeForce RTX 3060 12 GB",
            "ram_gb": 32,
            "display": "1920x1080",
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
            "draw_calls_p99": 1840,
            "triangles_avg": 7_600_000,
            "vram_mb_peak": 9800,
            "ram_mb_peak": 12000,
            "overhead_pct": 1.2,
            "fps_avg_spread_pct": 2.4,
            "valid": True,
        },
    }


def _file_fixture(directory: Path, name: str, payload: Any) -> Path:
    path = directory / name
    path.write_text(json.dumps(payload), encoding="utf-8")
    return path


def _run_cli(argv: list[str]) -> tuple[int, str, str]:
    stdout = io.StringIO()
    stderr = io.StringIO()
    with redirect_stdout(stdout), redirect_stderr(stderr):
        code = main(argv)
    return code, stdout.getvalue(), stderr.getvalue()


def run_self_test() -> int:
    failures: list[str] = []
    cases = 0

    def case(name: str, body: Any) -> None:
        nonlocal cases
        cases += 1
        try:
            body()
        except AssertionError as error:
            failures.append(f"{name}: {error}")
        except Exception as error:  # noqa: BLE001 - report the fixture failure
            failures.append(f"{name}: {type(error).__name__}: {error}")

    with tempfile.TemporaryDirectory(prefix="townsmith-assemble-") as raw:
        directory = Path(raw)
        cohort_path = _file_fixture(directory, "cohort.json", _cohort_fixture())
        save_path = _file_fixture(directory, "save.json", _save_fixture())
        streaming_path = _file_fixture(
            directory, "streaming.json", _streaming_fixture()
        )
        render_path = _file_fixture(directory, "render.json", _render_fixture())

        def all_present() -> None:
            document, missing, fields = assemble_from_paths(
                cohort_path=str(cohort_path),
                save_path=str(save_path),
                streaming_path=str(streaming_path),
                render_path=str(render_path),
                scene=SCENE,
                seed=None,
                hardware_note=None,
            )
            assert document["status"] == "complete", document["status"]
            assert missing == [], missing
            assert document["fps_avg"] == 31.4, document["fps_avg"]
            assert document["fps_1pct_low"] == 24.8
            assert document["cohort_tick_ms_p99"] == 0.2839
            assert document["cohort_aggregate_ms_p99"] == 3.5638
            assert document["agent_update_ms_p99"] == 3.3255
            assert document["save_ms"] == 24_000.0
            assert document["streaming_stalls_100ms"] == 0
            assert fields >= 25, fields
            assert document["checksum"].startswith(CHECKSUM_PREFIX)
            assert document["results"]["fps_low_1pct"] == 24.8
            assert document["results"]["full_tick_ms_p99"] == 3.5638
            assert document["results"]["stalls_250ms"] == 0
            assert document["results"]["frametime_ms_p95"] == 38.0
            assert document["thresholds"]["values"]["fps_avg"] == ">=30"
            assert document["provenance"]["fields"]["fps_avg"]["source"] == (
                "render.results.fps_avg"
            )
            assert document["provenance"]["hardware_source"] == "render.hardware"
            assert "cohort" in document["provenance"]["inputs"]
            assert "render" in document["provenance"]["inputs"]

        case("all_present", all_present)

        def missing_render() -> None:
            document, missing, fields = assemble_from_paths(
                cohort_path=str(cohort_path),
                save_path=str(save_path),
                streaming_path=str(streaming_path),
                render_path=None,
                scene=SCENE,
                seed=None,
                hardware_note=None,
            )
            assert document["status"] == "insufficient", document["status"]
            assert document["fps_avg"] is None
            assert document["vram_mb_peak"] is None
            assert document["draw_calls"] is None
            assert document["route"] is None
            assert document["results"]["fps_avg"] is None
            assert document["cohort_tick_ms_p99"] == 0.2839
            assert document["save_ms"] == 24_000.0
            assert document["streaming_stalls_100ms"] == 0
            assert "fps_avg" in missing
            assert "route" in missing
            assert "hardware" in missing
            assert fields >= 10, fields

        case("missing_render", missing_render)

        def partial_render() -> None:
            partial = json.loads(json.dumps(_render_fixture()))
            del partial["results"]["vram_mb_peak"]
            path = _file_fixture(directory, "partial_render.json", partial)
            document, missing, _ = assemble_from_paths(
                cohort_path=str(cohort_path),
                save_path=str(save_path),
                streaming_path=str(streaming_path),
                render_path=str(path),
                scene=SCENE,
                seed=None,
                hardware_note=None,
            )
            assert document["status"] == "insufficient"
            assert document["fps_avg"] == 31.4
            assert document["vram_mb_peak"] is None
            assert "vram_mb_peak" in missing, missing

        case("partial_render", partial_render)

        def bad_json() -> None:
            path = directory / "bad.json"
            path.write_text("{", encoding="utf-8")
            try:
                _load_input(path, "cohort")
            except AssemblyError as error:
                assert "invalid JSON" in str(error), error
            else:
                raise AssertionError("bad JSON was accepted")

        case("bad_json", bad_json)

        def bad_schema() -> None:
            payload = _cohort_fixture()
            payload["results"] = []
            path = _file_fixture(directory, "bad_schema.json", payload)
            try:
                _load_input(path, "cohort")
            except AssemblyError as error:
                assert "results" in str(error), error
            else:
                raise AssertionError("bad schema was accepted")

        case("bad_schema", bad_schema)

        def unexpected_scene() -> None:
            payload = _cohort_fixture()
            payload["scene"] = "streaming_scale"
            path = _file_fixture(directory, "wrong_scene.json", payload)
            try:
                _load_input(path, "cohort")
            except AssemblyError as error:
                assert "scene" in str(error), error
            else:
                raise AssertionError("unexpected scene was accepted")

        case("unexpected_scene", unexpected_scene)

        def multi_route_render() -> None:
            payload = {
                "format_version": 1,
                "suite": "phase0_godot_route",
                "hardware": {
                    "processor": "AMD Ryzen 9 7950X 16-Core Processor",
                    "logical_processor_count": 32,
                    "video_adapter": "NVIDIA GeForce RTX 4080",
                    "window_size": [1920, 1080],
                    "os": "Linux",
                },
                "settings": {
                    "runs": 3,
                    "duration_s": 5,
                    "warmup_s": 3,
                    "renderer": "gl_compatibility",
                },
                "routes": {
                    "city_mid": {
                        "route": "phase0_city_mid_v1",
                        "duration_s": 5.0,
                        "fps_avg": 1200.0,
                        "fps_low_1pct": 800.0,
                    },
                    "city_late": {
                        "route": "phase0_city_late_v1",
                        "duration_s": 5.0,
                        "warmup_s": 3.0,
                        "fps_avg": 766.2,
                        "fps_low_1pct": 434.6,
                        "frametime_ms_p99": 2.301,
                    },
                },
            }
            path = _file_fixture(directory, "multi_render.json", payload)
            document, missing, _ = assemble_from_paths(
                cohort_path=str(cohort_path),
                save_path=str(save_path),
                streaming_path=str(streaming_path),
                render_path=str(path),
                scene=SCENE,
                seed=None,
                hardware_note=None,
            )
            assert document["fps_avg"] == 766.2
            assert document["route"] == "phase0_city_late_v1"
            assert document["hardware"]["video_adapter"].endswith("RTX 4080")
            assert document["provenance"]["inputs"]["render"][
                "selected_route"
            ] == "city_late"
            assert document["status"] == "insufficient"
            assert "vram_mb_peak" in missing
            assert any(
                "RTX 3060" in warning
                for warning in document["provenance"]["warnings"]
            )

        case("multi_route_render", multi_route_render)

        def multiple_late_routes() -> None:
            payload = {
                "routes": {
                    "city_late": {"route": "late_a", "fps_avg": 30.0},
                    "late_alt": {"route": "late_b", "fps_avg": 31.0},
                }
            }
            path = _file_fixture(directory, "two_late.json", payload)
            try:
                _load_input(path, "render")
            except AssemblyError as error:
                assert "multiple late routes" in str(error), error
            else:
                raise AssertionError("ambiguous multi-route render was accepted")

        case("multiple_late_routes", multiple_late_routes)

        def no_inputs() -> None:
            document, missing, fields = assemble_from_paths(
                cohort_path=None,
                save_path=None,
                streaming_path=None,
                render_path=None,
                scene=SCENE,
                seed=None,
                hardware_note=None,
            )
            assert document["status"] == "insufficient"
            assert fields == 0, fields
            for field in (
                "fps_avg",
                "fps_1pct_low",
                "frame_ms_p95",
                "draw_calls",
                "triangles",
                "vram_mb_peak",
                "save_ms",
                "cohort_tick_ms_p99",
            ):
                assert document[field] is None, field
            assert "fps_avg" in missing
            assert "cohort_tick_ms_p99" in missing

        case("no_inputs", no_inputs)

        def round_trip() -> None:
            first, missing, fields = assemble_from_paths(
                cohort_path=str(cohort_path),
                save_path=str(save_path),
                streaming_path=str(streaming_path),
                render_path=str(render_path),
                scene=SCENE,
                seed=None,
                hardware_note="fixture rig",
            )
            second, _, _ = assemble_from_paths(
                cohort_path=str(cohort_path),
                save_path=str(save_path),
                streaming_path=str(streaming_path),
                render_path=str(render_path),
                scene=SCENE,
                seed=None,
                hardware_note="fixture rig",
            )
            assert first["checksum"] == second["checksum"]
            out = directory / "out" / "late.json"
            _write_document(out, first)
            loaded = json.loads(out.read_text(encoding="utf-8"))
            assert loaded == first
            assert loaded["checksum"] == _checksum(loaded)
            assert loaded["provenance"]["hardware_note"] == "fixture rig"
            before = out.read_bytes()
            _write_document(out, first)
            assert out.read_bytes() == before
            assert len(missing) == 0 and fields >= 25

        case("round_trip", round_trip)

        def provenance() -> None:
            document, missing, _ = assemble_from_paths(
                cohort_path=str(cohort_path),
                save_path=str(save_path),
                streaming_path=str(streaming_path),
                render_path=str(render_path),
                scene=SCENE,
                seed=None,
                hardware_note=None,
            )
            fields = document["provenance"]["fields"]
            assert fields["cohort_tick_ms_p99"]["source"] == (
                "cohort.results.cohort_tick_ms_p99"
            )
            assert fields["save_ms"]["source"] == "save.total_save_ms"
            assert fields["save_size_mb"]["source"] == (
                "save.json_bytes/(1024*1024)"
            )
            assert fields["streaming_stalls_100ms"]["source"] == (
                "streaming.streaming_stalls_100ms"
            )
            assert fields["hitches_250ms"]["source"] == (
                "render.results.stalls_250ms"
            )
            assert fields["fps_avg"]["source"] is not None
            assert missing == []
            for field, entry in fields.items():
                if field in ("format_version", "gate", "scene"):
                    continue
                if document.get(field) is None:
                    assert entry["source"] is None, field
                else:
                    assert entry["source"] is not None, field

        case("provenance", provenance)

        def cli_marker() -> None:
            out = directory / "cli" / "late.json"
            code, stdout, stderr = _run_cli(
                [
                    "--cohort",
                    str(cohort_path),
                    "--save",
                    str(save_path),
                    "--streaming",
                    str(streaming_path),
                    "--out",
                    str(out),
                ]
            )
            assert code == 0, (code, stderr)
            assert not stderr.strip(), stderr
            assert stdout.startswith(MARKER + " "), stdout
            assert "status=insufficient" in stdout, stdout
            assert out.exists()
            loaded = json.loads(out.read_text(encoding="utf-8"))
            assert loaded["status"] == "insufficient"
            assert loaded["fps_avg"] is None

        case("cli_marker", cli_marker)

        def cli_bad_json() -> None:
            bad = directory / "cli_bad.json"
            bad.write_text("{", encoding="utf-8")
            out = directory / "cli" / "never.json"
            code, stdout, stderr = _run_cli(
                ["--cohort", str(bad), "--out", str(out)]
            )
            assert code == 2, (code, stdout, stderr)
            assert "invalid JSON" in stderr, stderr
            assert not out.exists()

        case("cli_bad_json", cli_bad_json)

        def missing_shape_fields() -> None:
            partial = json.loads(json.dumps(_render_fixture()))
            for field in ("duration_s", "warmup_s", "runs"):
                del partial[field]
            path = _file_fixture(directory, "render_no_shape.json", partial)
            document, missing, _ = assemble_from_paths(
                cohort_path=str(cohort_path),
                save_path=str(save_path),
                streaming_path=str(streaming_path),
                render_path=str(path),
                scene=SCENE,
                seed=None,
                hardware_note=None,
            )
            assert document["status"] == "insufficient", document["status"]
            for field in ("duration_s", "warmup_s", "runs"):
                assert field in missing, missing
                assert document[field] is None, field
            assert document["headless"] is False
            warnings = " ".join(document["provenance"]["warnings"])
            assert "duration_s is absent" in warnings, warnings
            assert "warmup_s is absent" in warnings, warnings
            assert "runs is absent" in warnings, warnings

        case("missing_shape_fields", missing_shape_fields)

        def no_gate_route() -> None:
            payload = {
                "format_version": 1,
                "scene": "late_metropolis_v1",
                "hardware": _render_fixture()["hardware"],
                "settings": {
                    "runs": 3,
                    "duration_s": 120,
                    "warmup_s": 15,
                },
                "routes": {
                    "city_mid": {
                        "route": "phase0_city_mid_v1",
                        "duration_s": 5.0,
                        "warmup_s": 3.0,
                        "fps_avg": 1200.0,
                    },
                },
            }
            path = _file_fixture(directory, "no_gate_route.json", payload)
            document, missing, _ = assemble_from_paths(
                cohort_path=str(cohort_path),
                save_path=str(save_path),
                streaming_path=str(streaming_path),
                render_path=str(path),
                scene=SCENE,
                seed=None,
                hardware_note=None,
            )
            assert document["status"] == "insufficient", document["status"]
            assert document["route"] is None, document["route"]
            assert document["fps_avg"] is None, document["fps_avg"]
            assert "render.route" in missing, missing
            assert "duration_s" in missing, missing
            warnings = " ".join(document["provenance"]["warnings"])
            assert "no late_flyover_v1 route" in warnings, warnings

        case("no_gate_route", no_gate_route)

        def malformed_routes() -> None:
            payload = {
                "format_version": 1,
                "scene": "late_metropolis_v1",
                "routes": [],
            }
            path = _file_fixture(directory, "malformed_routes.json", payload)
            out = directory / "malformed" / "never.json"
            code, stdout, stderr = _run_cli(
                ["--render", str(path), "--out", str(out)]
            )
            assert code == 2, (code, stdout, stderr)
            assert "routes must be an object" in stderr, stderr
            assert not out.exists()

        case("malformed_routes", malformed_routes)

        def non_gate_late_route() -> None:
            route = {
                "route": "late_phase0_flyover",
                "duration_s": 120,
                "warmup_s": 15,
                "runs": 3,
                "headless": False,
            }
            route.update(_render_fixture()["results"])
            payload = {
                "format_version": 1,
                "scene": "late_metropolis_v1",
                "seed": 2026,
                # Wrapper-level values must not win over the selected route.
                "fps_avg": 999.0,
                "results": {"fps_avg": 999.0, "vram_mb_peak": 1.0},
                "hardware": _render_fixture()["hardware"],
                "settings": {"renderer": "forward_plus"},
                "routes": {"late_phase0": route},
            }
            path = _file_fixture(
                directory, "non_gate_late_route.json", payload
            )
            document, missing, _ = assemble_from_paths(
                cohort_path=str(cohort_path),
                save_path=str(save_path),
                streaming_path=str(streaming_path),
                render_path=str(path),
                scene=SCENE,
                seed=None,
                hardware_note=None,
            )
            assert document["status"] == "insufficient", document["status"]
            assert document["route"] == "late_phase0_flyover"
            assert document["fps_avg"] == 31.4, document["fps_avg"]
            assert document["results"]["fps_avg"] == 31.4
            assert document["vram_mb_peak"] == 9800, document["vram_mb_peak"]
            assert missing == ["render.route"], missing
            warnings = " ".join(document["provenance"]["warnings"])
            assert "late_flyover_v1" in warnings, warnings

        case("non_gate_late_route", non_gate_late_route)

        def evaluator_passes_complete_fixture() -> None:
            document, missing, _ = assemble_from_paths(
                cohort_path=str(cohort_path),
                save_path=str(save_path),
                streaming_path=str(streaming_path),
                render_path=str(render_path),
                scene=SCENE,
                seed=None,
                hardware_note=None,
            )
            assert document["status"] == "complete", document["status"]
            assert missing == [], missing
            out = directory / "evaluator" / "late.json"
            _write_document(out, document)
            evaluator = Path(__file__).resolve().with_name(
                "check_metropolis_gate.py"
            )
            completed = subprocess.run(
                [sys.executable, str(evaluator), "--result", str(out)],
                capture_output=True,
                text=True,
                check=False,
            )
            assert completed.returncode == 0, (
                completed.returncode,
                completed.stdout[-400:],
                completed.stderr,
            )
            assert "METROPOLIS_GATE status=pass" in completed.stdout

        case("evaluator_passes_complete_fixture", evaluator_passes_complete_fixture)

    if failures:
        for failure in failures:
            print(f"{MARKER}_SELFTEST_FAIL {failure}")
        print(
            f"{MARKER}_SELFTEST cases={cases} failures={len(failures)} "
            "status=failed"
        )
        return 1

    print(f"{MARKER}_SELFTEST cases={cases} status=ok")
    return 0


# --- CLI -------------------------------------------------------------------


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Assemble recorded 1M harness output into the docs/68 "
            "late_metropolis_v1 result schema. Measurement-free; never "
            "synthesises FPS/VRAM/draw-call values."
        ),
        epilog=(
            "Exit 0 = assembled (status may be insufficient); 2 = bad input "
            "or unwritable output. Example: assemble_gate_result.py --cohort "
            "/tmp/cohort_1m_v1.json --save /tmp/save_1m.json --out "
            "benchmarks/results/gate/late_metropolis_v1.json"
        ),
    )
    parser.add_argument(
        "--cohort",
        metavar="PATH",
        help="cohort_1m_v1 gate report (cohort/agent/sim/server RSS)",
    )
    parser.add_argument(
        "--save",
        metavar="PATH",
        help="save_1m report (save/load time and save size)",
    )
    parser.add_argument(
        "--streaming",
        metavar="PATH",
        help="streaming_scale report (stall counters when recorded)",
    )
    parser.add_argument(
        "--render",
        metavar="PATH",
        help=(
            "one late render capture/summary; absent fields stay null and "
            "status is insufficient"
        ),
    )
    parser.add_argument(
        "--out",
        metavar="PATH",
        help="output JSON path; omit or '-' to write JSON to stdout",
    )
    parser.add_argument(
        "--scene",
        default=SCENE,
        metavar="SCENE",
        help=f"assembly scene (default and only supported value: {SCENE})",
    )
    parser.add_argument(
        "--seed",
        metavar="SEED",
        help="top-level seed; default is the render result's seed when present",
    )
    parser.add_argument(
        "--hardware-note",
        metavar="TEXT",
        help="free-form hardware note recorded in provenance (not a metric)",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="run temp-fixture tests for present/missing/bad/provenance cases",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)

    if args.self_test:
        return run_self_test()

    try:
        document, missing, field_count = assemble_from_paths(
            cohort_path=args.cohort,
            save_path=args.save,
            streaming_path=args.streaming,
            render_path=args.render,
            scene=args.scene,
            seed=args.seed,
            hardware_note=args.hardware_note,
        )
    except AssemblyError as error:
        print(f"assemble_gate_result: {error}", file=sys.stderr)
        return 2

    out = args.out
    if out is None or out == "-":
        sys.stdout.write(
            json.dumps(
                document, indent=2, sort_keys=True, ensure_ascii=False
            )
        )
        sys.stdout.write("\n")
        print(
            _marker(document, missing, field_count, "-"),
            file=sys.stderr,
        )
        return 0

    try:
        _write_document(Path(out), document)
    except AssemblyError as error:
        print(f"assemble_gate_result: {error}", file=sys.stderr)
        return 2
    if document["status"] != "complete" and _in_gate_results_dir(Path(out)):
        print(
            "assemble_gate_result: warning: wrote "
            f"status={document['status']} under benchmarks/results/gate/; "
            "check_metropolis_gate.sh selects the newest file, so this "
            "artifact can shadow a complete result",
            file=sys.stderr,
        )
    print(_marker(document, missing, field_count, out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
