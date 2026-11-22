#!/usr/bin/env python3
"""Evaluate a docs/68 Metropolis gate (G5) result JSON (stdlib only).

The tool answers one question: does this recorded JSON meet the late-scene
thresholds in ``docs/68 - Metropolis Gate``? It never renders, ticks, or
measures anything. A result without the required provenance is reported as
``insufficient`` (exit 2), not as a pass or a fail.

Thresholds are encoded constants copied from docs/68 sections 5 and 8. The
document is still the source of truth: if a threshold changes, change both.

Usage:
    ./scripts/check_metropolis_gate.py --result PATH [--result PATH ...]
    ./scripts/check_metropolis_gate.py --result late.json --scene late_metropolis_v1
    ./scripts/check_metropolis_gate.py --result late.json --require-hardware
    ./scripts/check_metropolis_gate.py --result late.json --report eval.json
    ./scripts/check_metropolis_gate.py --self-test

Exit status:
    0  every evaluated check passes
    1  at least one check fails (including a --require-hardware mismatch)
    2  insufficient data: missing required fields, unknown/wrong input,
       unreadable JSON, or no result files

Output: one ``METROPOLIS_GATE_CHECK ...`` line per check, then
``METROPOLIS_GATE status=... scenes=N checks=N failed=N hardware_match=...``.
"""

from __future__ import annotations

import argparse
import json
import math
import re
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Sequence

GIB = 1024.0
LATE_SCENE = "late_metropolis_v1"
COHORT_SCENE = "cohort_1m_v1"
SUPPORTED_SCENES = (LATE_SCENE, COHORT_SCENE)

# Late display scene. Source: docs/68 §5.1 (frame), §5.2 (render/memory),
# §5.4 (streaming/save/overhead/variance), §5.5 (run validity), §8.2 (checks).
LATE = {
    "fps_avg_min": 30.0,  # §8.2 #1
    "fps_low_1pct_min": 22.0,  # §8.2 #2
    "fps_low_0_1pct_min": 15.0,  # §8.2 #3
    "frametime_ms_p99_max": 45.0,  # §8.2 #4
    "frametime_ms_p99_9_max": 67.0,  # §8.2 #4
    "frametime_ms_max_max": 250.0,  # §8.2 #5 / §5.1 stall
    "hitches_50ms_max": 3,  # §8.2 #5
    "stalls_250ms_max": 0,  # §8.2 #5
    "streaming_stalls_100ms_max": 0,  # §8.2 #16
    "dropped_chunks_max": 0,  # §8.2 #16
    "draw_calls_p99_max": 2000.0,  # §8.2 #8, exclusive
    "triangles_min": 5_000_000.0,  # §8.2 #8
    "triangles_max": 10_000_000.0,  # §8.2 #8
    "vram_mb_peak_max": 10.5 * GIB,  # §8.2 #9
    "ram_mb_peak_max": 12.0 * GIB,  # §8.2 #9
    # Supplemental asset/GPU-buffer budget from the tooling brief. docs/68 has
    # no separate VBUS row; this tighter cap is optional and only enforced
    # when the field is recorded. It does not replace the 10.5 GB VRAM rule.
    "vbus_mb_peak_max": 8.0 * GIB,
    "save_ms_max": 30_000.0,  # §8.2 #15
    "load_ms_max": 30_000.0,  # §8.2 #15
    "overhead_pct_max": 2.0,  # §8.2 #17, exclusive
    "spread_pct_max": 5.0,  # §8.2 #17, exclusive
    "runs_min": 3,  # §5.5 / §8.1
    "duration_s_min": 90.0,  # §5.5 (90-120 s for Late)
    "warmup_s_min": 15.0,  # §5.5
}

# Headless 1M scene. Source: docs/68 §5.3, §5.4, §8.2 #10-#15.
COHORT = {
    "cohort_tick_ms_p99_max": 10.0,  # §8.2 #10
    "full_tick_ms_p99_max": 50.0,  # §8.2 #10
    "active_agents_ms_p99_max": 10.0,  # §8.2 #11
    "pathfinding_ms_p99_max": 4.0,  # §8.2 #11
    "sim_cpu_ms_per_s_max": 250.0,  # §8.2 #12
    "server_rss_mb_peak_max": 4096.0,  # §5.3
    "network_stall_ms_max": 16.0,  # §5.3
    "save_ms_max": 30_000.0,  # §8.2 #15
    "load_ms_max": 30_000.0,  # §8.2 #15
    "ticks_min": 3600,  # §4 cohort_1m_v1
    "traffic_build_ms_max": 250.0,  # §8.2 #14
    "traffic_incremental_ms_p99_max": 16.0,  # §8.2 #14
    "traffic_flow_ms_p99_max": 4.0,  # §8.2 #14
    "traffic_queries_5000_ms_max": 10.0,  # §8.2 #14
    "runs_min": 3,  # §6.2 step 7
}

LATE_PATHS = {
    "fps_avg": ("fps_avg", "fps_median", "median_fps"),
    "fps_low_1pct": ("fps_low_1pct", "low_1pct", "fps_1pct_low"),
    "fps_low_0_1pct": ("fps_low_0_1pct", "low_0_1pct", "fps_0_1pct_low"),
    "frametime_ms_p95": ("frametime_ms_p95", "p95_ms"),
    "frametime_ms_p99": ("frametime_ms_p99", "p99_ms"),
    "frametime_ms_p99_9": ("frametime_ms_p99_9", "p99_9_ms", "frametime_ms_p999"),
    "frametime_ms_max": ("frametime_ms_max", "max_frame_ms", "frametime_ms_peak"),
    "hitches_50ms": ("hitches_50ms", "hitches_gt_50ms", "hitches_over_50ms"),
    "stalls_250ms": ("stalls_250ms", "stalls_gt_250ms", "stalls"),
    "streaming_stalls_100ms": (
        "streaming_stalls_100ms",
        "streaming_stalls",
        "stalls_streaming_100ms",
    ),
    "dropped_chunks": ("dropped_chunks", "dropped_chunks_total"),
    "draw_calls_p99": ("draw_calls_p99", "draw_calls"),
    "triangles": ("triangles_avg", "triangles_median", "triangles"),
    "vram_mb_peak": ("vram_mb_peak", "vram_peak_mb", "vram_mb"),
    "ram_mb_peak": ("ram_mb_peak", "rss_mb_peak", "rss_mb", "ram_peak_mb"),
    "vbus_mb_peak": (
        "vbus_mb_peak",
        "vbus_peak_mb",
        "vbus_mb",
        "gpu_buffer_mb_peak",
    ),
    "save_ms": ("save_ms", "save_time_ms"),
    "load_ms": ("load_ms", "load_time_ms"),
    "overhead_pct": ("overhead_pct", "benchmark_overhead_pct"),
    "spread_pct": ("fps_avg_spread_pct", "spread_pct", "variance_pct"),
}

COHORT_PATHS = {
    "ticks": ("ticks", "tick_count", "simulated_ticks"),
    "cohort_tick_ms_p99": (
        "cohort_tick_ms_p99",
        "cohort_step_ms_p99",
        "cohort_ms_p99",
    ),
    "full_tick_ms_p99": (
        "full_tick_ms_p99",
        "sim_tick_ms_p99",
        "aggregate_ms_p99",
    ),
    "active_agents_ms_p99": (
        "active_agents_ms_p99",
        "agents_ms_p99",
        "agent_tick_ms_p99",
    ),
    "pathfinding_ms_p99": ("pathfinding_ms_p99", "path_ms_p99"),
    "sim_cpu_ms_per_s": ("sim_cpu_ms_per_s", "total_sim_cpu_ms_per_s"),
    "server_rss_mb_peak": ("server_rss_mb_peak", "server_rss_mb", "rss_mb_peak"),
    "network_stall_ms_max": (
        "server_network_stall_ms_max",
        "network_stall_ms_max",
    ),
    "save_ms": ("save_ms", "save_time_ms"),
    "load_ms": ("load_ms", "load_time_ms"),
    "traffic_build_ms": ("traffic_build_ms",),
    "traffic_incremental_ms_p99": ("traffic_incremental_ms_p99",),
    "traffic_flow_ms_p99": ("traffic_flow_ms_p99",),
    "traffic_queries_5000_ms": (
        "traffic_queries_5000_ms",
        "traffic_5k_queries_ms",
    ),
}

MISSING = object()

# Only used when a CPU model string has no explicit core/thread count. Counts
# are physical cores / logical threads; models not in this table need explicit
# cpu_cores/cpu_threads or an "8c/16t"-style string.
CPU_MODELS = {
    "5700x": (8, 16),
    "5800x": (8, 16),
    "5800x3d": (8, 16),
    "7700x": (8, 16),
    "7800x3d": (8, 16),
    "12400": (6, 12),
    "12600": (10, 16),
    "13400": (10, 16),
    "13600": (14, 20),
    "7950x": (16, 32),
    "7900x": (12, 24),
    "13900": (24, 32),
}


@dataclass
class Check:
    name: str
    status: str
    value: Any
    threshold: str = ""
    source: str = ""
    detail: str = ""


@dataclass
class ResultEval:
    path: Path
    scene: str | None = None
    checks: list[Check] = field(default_factory=list)
    insufficient: bool = False
    hardware_match: bool | None = None
    error: str | None = None

    @property
    def failed(self) -> int:
        return sum(1 for check in self.checks if check.status == "fail")

    @property
    def status(self) -> str:
        if self.failed:
            return "fail"
        if self.insufficient or self.error:
            return "insufficient"
        return "pass"


def _get(document: Any, paths: Sequence[str]) -> Any:
    for path in paths:
        current = document
        found = True
        for segment in path.split("."):
            if isinstance(current, dict) and segment in current:
                current = current[segment]
            else:
                found = False
                break
        if found and current is not None:
            return current
    return MISSING


def _number(value: Any) -> Any:
    if isinstance(value, bool) or value is MISSING:
        return MISSING
    if isinstance(value, (int, float)):
        number = float(value)
    elif isinstance(value, str):
        try:
            number = float(value.strip())
        except ValueError:
            return MISSING
    else:
        return MISSING
    return number if math.isfinite(number) else MISSING


def _integer(value: Any) -> Any:
    number = _number(value)
    if number is MISSING:
        return MISSING
    return int(number)


def _text(value: Any) -> Any:
    if isinstance(value, str) and value.strip():
        return value.strip()
    return MISSING


def _fmt(value: Any) -> str:
    if value is MISSING:
        return "-"
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, float):
        return f"{value:g}"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, str):
        return json.dumps(" ".join(value.split()), ensure_ascii=True)
    return json.dumps(value, ensure_ascii=True)


def _clean(text: str, limit: int = 220) -> str:
    text = " ".join(str(text).split()).replace('"', "'")
    return text[:limit]


class _Evaluator:
    def __init__(self, path: Path, scene: str, require_hardware: bool) -> None:
        self.result = ResultEval(path=path, scene=scene)
        self.require_hardware = require_hardware

    def add(
        self,
        name: str,
        status: str,
        value: Any,
        threshold: str = "",
        source: str = "",
        detail: str = "",
    ) -> None:
        self.result.checks.append(
            Check(
                name=name,
                status=status,
                value=value,
                threshold=threshold,
                source=source,
                detail=detail,
            )
        )

    def missing(self, name: str, detail: str) -> None:
        self.result.insufficient = True
        self.add(name, "missing", MISSING, "", "", detail)

    def required_text(self, name: str, value: Any, detail: str) -> None:
        if value is MISSING:
            self.missing(name, detail)
            return
        self.add(name, "pass", value, "", "", detail)

    def number(
        self,
        name: str,
        value: Any,
        *,
        minimum: float | None = None,
        maximum: float | None = None,
        strict_max: bool = False,
        required: bool = False,
        source: str = "",
        detail: str = "",
    ) -> None:
        if value is MISSING:
            if required:
                self.missing(name, detail or "required numeric field missing")
            return
        if minimum is not None and maximum is not None:
            ok = minimum <= value <= maximum
            threshold = f"{minimum:g}..{maximum:g}"
        elif minimum is not None:
            ok = value >= minimum
            threshold = f">={minimum:g}"
        elif maximum is not None:
            ok = value < maximum if strict_max else value <= maximum
            threshold = f"<{maximum:g}" if strict_max else f"<={maximum:g}"
        else:
            ok = True
            threshold = ""
        self.add(name, "pass" if ok else "fail", value, threshold, source, detail)

    def flag(
        self,
        name: str,
        value: Any,
        *,
        expected: bool,
        required: bool = False,
        source: str = "",
        detail: str = "",
    ) -> None:
        if value is MISSING:
            if required:
                self.missing(name, detail or "required flag missing")
            return
        if isinstance(value, str):
            lowered = value.strip().lower()
            actual = {"true": True, "false": False}.get(lowered)
        else:
            actual = bool(value)
        if actual is None:
            if required:
                self.missing(name, detail or "required flag is not boolean")
            return
        self.add(
            name,
            "pass" if actual is expected else "fail",
            actual,
            "true" if expected else "false",
            source,
            detail,
        )

    def hardware(
        self,
        name: str,
        ok: bool,
        value: Any,
        threshold: str,
        detail: str,
    ) -> None:
        if ok:
            status = "pass"
        else:
            status = "fail" if self.require_hardware else "warn"
        self.add(name, status, value, threshold, "docs/68:3.1", detail)


def _gpu_match(hardware: dict[str, Any]) -> tuple[bool, str, str]:
    raw = _get(hardware, ("gpu", "video_adapter", "gpu_model"))
    if raw is MISSING:
        return False, "", "gpu missing"
    text = str(raw)
    lowered = text.lower()
    vram = MISSING
    match = re.search(r"(\d+)\s*gb\b", lowered)
    if match:
        vram = float(match.group(1))
    else:
        vram = _number(_get(hardware, ("gpu_vram_gb", "vram_gb")))
    is_3060 = re.search(r"\brtx\s*3060\b", lowered) is not None
    is_ti = re.search(r"\b3060\s*ti\b", lowered) is not None
    if not is_3060:
        return False, text, "not an RTX 3060"
    if is_ti:
        return False, text, "RTX 3060 Ti is not the 12 GB reference"
    if vram is MISSING:
        return False, text, "VRAM not recorded (reference is the 12 GB card)"
    if vram < 12:
        return False, text, f"{vram:g} GB card is not the 12 GB reference"
    return True, text, "RTX 3060 12 GB"


def _cpu_match(hardware: dict[str, Any]) -> tuple[bool, str, str]:
    cores = _integer(_get(hardware, ("cpu_cores", "cores", "physical_cores")))
    threads = _integer(
        _get(
            hardware,
            (
                "cpu_threads",
                "threads",
                "logical_cores",
                "logical_processor_count",
            ),
        )
    )
    raw = _get(hardware, ("cpu", "processor", "cpu_model"))
    text = str(raw) if raw is not MISSING else ""

    if cores is MISSING or threads is MISSING:
        match = re.search(
            r"(\d+)\s*(?:c|cores?|core)\b\s*[/x\u00d7]\s*(\d+)\s*"
            r"(?:t|threads?|thread)\b",
            text,
            re.IGNORECASE,
        )
        if match:
            if cores is MISSING:
                cores = int(match.group(1))
            if threads is MISSING:
                threads = int(match.group(2))
    if cores is MISSING:
        match = re.search(r"(\d+)\s*[- ]?core", text, re.IGNORECASE)
        if match:
            cores = int(match.group(1))
    if threads is MISSING:
        match = re.search(r"(\d+)\s*[- ]?thread", text, re.IGNORECASE)
        if match:
            threads = int(match.group(1))
    if cores is MISSING or threads is MISSING:
        lowered = text.lower()
        for model, (model_cores, model_threads) in CPU_MODELS.items():
            if model in lowered:
                if cores is MISSING:
                    cores = model_cores
                if threads is MISSING:
                    threads = model_threads
                break
    if cores is MISSING or threads is MISSING:
        return False, text, "core/thread count not recorded"
    ok = cores >= 8 and threads >= 16
    return ok, text, f"{cores}c/{threads}t (reference 8c/16t)"


def _ram_match(hardware: dict[str, Any]) -> tuple[bool, Any, str]:
    raw = _get(hardware, ("ram_gb", "memory_gb"))
    if raw is MISSING:
        mb = _number(_get(hardware, ("ram_mb", "memory_mb")))
        if mb is not MISSING:
            raw = mb / GIB
        else:
            size = _number(_get(hardware, ("ram_bytes", "memory_bytes")))
            if size is not MISSING:
                raw = size / (GIB * GIB)
    value = _number(raw)
    if value is MISSING:
        return False, MISSING, "RAM not recorded"
    # A 32 GB machine commonly reports ~31 GiB usable after firmware reserve.
    return value >= 30.0, value, "32 GB declared (30 GiB floor)"


def _size_from(value: Any) -> tuple[int, int] | None:
    if isinstance(value, (list, tuple)) and len(value) >= 2:
        try:
            return int(value[0]), int(value[1])
        except (TypeError, ValueError):
            return None
    if isinstance(value, dict):
        width = _integer(value.get("width"))
        height = _integer(value.get("height"))
        if width is not MISSING and height is not MISSING:
            return int(width), int(height)
        for key in ("resolution", "size", "window_size", "display"):
            if key in value:
                size = _size_from(value[key])
                if size:
                    return size
        return None
    if isinstance(value, str):
        match = re.search(r"(\d{3,5})\s*[x\u00d7]\s*(\d{3,5})", value, re.I)
        if match:
            return int(match.group(1)), int(match.group(2))
    return None


def _display_size(hardware: dict[str, Any], document: dict[str, Any]) -> Any:
    candidates = (
        _get(hardware, ("display", "display_resolution", "resolution", "window_size")),
        _get(document, ("display", "resolution", "window_size")),
    )
    for candidate in candidates:
        if candidate is not MISSING:
            size = _size_from(candidate)
            if size:
                return size
    settings = document.get("settings")
    if isinstance(settings, dict):
        for key in ("display", "resolution", "window_size"):
            if key in settings:
                size = _size_from(settings[key])
                if size:
                    return size
    return MISSING


def _save_size_mb(document: dict[str, Any], results: dict[str, Any]) -> Any:
    for source in (results, document):
        value = _number(_get(source, ("save_size_mb", "save_mb")))
        if value is not MISSING:
            return value
        value = _number(_get(source, ("save_size_gb", "save_gb")))
        if value is not MISSING:
            return value * GIB
        value = _number(_get(source, ("save_bytes", "world_save_bytes")))
        if value is not MISSING:
            return value / (GIB * GIB)
    return MISSING


def _spread_pct(document: dict[str, Any], results: dict[str, Any]) -> Any:
    field_value = _number(_get(results, LATE_PATHS["spread_pct"]))
    if field_value is not MISSING:
        return field_value
    field_value = _number(_get(document, LATE_PATHS["spread_pct"]))
    if field_value is not MISSING:
        return field_value
    per_run = document.get("per_run")
    if isinstance(per_run, list) and len(per_run) > 1:
        values: list[float] = []
        for run in per_run:
            if not isinstance(run, dict):
                return MISSING
            run_results = run.get("results")
            if not isinstance(run_results, dict):
                return MISSING
            value = _number(_get(run_results, LATE_PATHS["fps_avg"]))
            if value is MISSING:
                return MISSING
            values.append(value)
        mean = sum(values) / len(values)
        if mean <= 0:
            return MISSING
        return (max(values) - min(values)) / mean * 100.0
    return MISSING


def _duration(document: dict[str, Any]) -> Any:
    value = _number(
        _get(document, ("duration_s", "duration_seconds", "route_duration_s"))
    )
    if value is not MISSING:
        return value
    route = document.get("route")
    if isinstance(route, dict):
        value = _number(_get(route, ("duration_s", "duration_seconds")))
        if value is not MISSING:
            return value
    settings = document.get("settings")
    if isinstance(settings, dict):
        return _number(_get(settings, ("duration_s", "duration_seconds")))
    return MISSING


def _warmup(document: dict[str, Any]) -> Any:
    value = _number(
        _get(document, ("warmup_s", "warmup_seconds", "route_warmup_s"))
    )
    if value is not MISSING:
        return value
    route = document.get("route")
    if isinstance(route, dict):
        value = _number(_get(route, ("warmup_s", "warmup_seconds")))
        if value is not MISSING:
            return value
    settings = document.get("settings")
    if isinstance(settings, dict):
        return _number(_get(settings, ("warmup_s", "warmup_seconds")))
    return MISSING


def _runs(document: dict[str, Any]) -> Any:
    value = _integer(_get(document, ("runs", "measured_runs")))
    if value is not MISSING:
        return value
    settings = document.get("settings")
    if isinstance(settings, dict):
        return _integer(_get(settings, ("runs", "measured_runs")))
    return MISSING


def _eval_common(ev: _Evaluator, document: dict[str, Any], scene: str) -> None:
    settings = document.get("settings")
    if isinstance(settings, dict) and settings:
        ev.required_text("settings", "object", "settings profile object")
    else:
        ev.required_text(
            "settings",
            _text(settings),
            "settings id (standard_1080p_high_v1) or object",
        )

    route = document.get("route")
    if isinstance(route, dict):
        route = _text(_get(route, ("id", "name", "route")))
    ev.required_text("route", _text(route), "fixed benchmark route id")

    runs = _runs(document)
    ev.number(
        "runs",
        runs,
        minimum=float(LATE["runs_min"]),
        required=True,
        source="docs/68:5.5",
        detail="median of three valid runs",
    )
    if scene == LATE_SCENE:
        ev.number(
            "duration_s",
            _duration(document),
            minimum=LATE["duration_s_min"],
            required=True,
            source="docs/68:5.5",
            detail="90-120 s Late route",
        )
        ev.number(
            "warmup_s",
            _warmup(document),
            minimum=LATE["warmup_s_min"],
            required=True,
            source="docs/68:5.5",
            detail="warm-up is never measured",
        )
    else:
        ev.number(
            "duration_s",
            _duration(document),
            required=True,
            source="docs/68:5.3",
            detail="headless run length (ticks carry the real budget)",
        )
        ev.number(
            "warmup_s",
            _warmup(document),
            required=True,
            source="docs/68:5.3",
            detail="warm-up ticks are discarded",
        )


def _eval_hardware(
    ev: _Evaluator,
    document: dict[str, Any],
    scene: str,
) -> None:
    hardware = document.get("hardware")
    if not isinstance(hardware, dict) or not hardware:
        ev.missing("hardware", "hardware object (GPU/CPU/RAM/display)")
        ev.result.hardware_match = None
        return

    matches: list[bool] = []
    unknown = False

    cpu_ok, cpu_text, cpu_detail = _cpu_match(hardware)
    if _get(hardware, ("cpu", "processor", "cpu_model")) is MISSING and _get(
        hardware, ("cpu_cores", "cores", "physical_cores")
    ) is MISSING:
        ev.missing("hardware.cpu", "CPU model and 8c/16t counts")
        unknown = True
    else:
        ev.hardware("hardware.cpu", cpu_ok, cpu_text, "8c/16t", cpu_detail)
        matches.append(cpu_ok)

    ram_ok, ram_value, ram_detail = _ram_match(hardware)
    if ram_value is MISSING:
        ev.missing("hardware.ram_gb", "RAM in GB")
        unknown = True
    else:
        ev.hardware("hardware.ram_gb", ram_ok, ram_value, ">=32", ram_detail)
        matches.append(ram_ok)

    if scene == COHORT_SCENE:
        ev.result.hardware_match = None if unknown or not matches else all(matches)
        return

    gpu_raw = _get(hardware, ("gpu", "video_adapter", "gpu_model"))
    if gpu_raw is MISSING:
        ev.missing("hardware.gpu", "GPU model (RTX 3060 12 GB)")
        unknown = True
    else:
        gpu_ok, gpu_text, gpu_detail = _gpu_match(hardware)
        ev.hardware(
            "hardware.gpu", gpu_ok, gpu_text, "RTX 3060 12 GB", gpu_detail
        )
        matches.append(gpu_ok)

    display = _display_size(hardware, document)
    if display is MISSING:
        ev.missing("hardware.display", "1920x1080 display/window size")
        unknown = True
    else:
        width, height = display
        display_ok = (width, height) == (1920, 1080)
        ev.hardware(
            "hardware.display",
            display_ok,
            f"{width}x{height}",
            "1920x1080",
            "render resolution without upscaling",
        )
        matches.append(display_ok)

    ev.result.hardware_match = None if unknown or not matches else all(matches)


def _optional(*sources: dict[str, Any], key: str) -> Any:
    for source in sources:
        if key in source and source[key] is not None:
            return source[key]
    return MISSING


def _eval_correctness(ev: _Evaluator, document: dict[str, Any], results: dict[str, Any]) -> None:
    valid = _optional(document, results, key="valid")
    if valid is not MISSING:
        ev.flag(
            "valid",
            valid,
            expected=True,
            source="docs/68:8.2#19",
            detail="run must not be marked invalid",
        )
    for name in ("crashed", "oom", "out_of_memory"):
        value = _optional(document, results, key=name)
        if value is MISSING:
            continue
        ev.flag(
            name,
            value,
            expected=False,
            source="docs/68:8.2#19",
            detail="no crash or OOM",
        )
    hashes = _optional(document, results, key="hashes_match")
    if hashes is MISSING:
        hashes = _optional(document, results, key="save_load_hash_equal")
    if hashes is not MISSING:
        ev.flag(
            "hashes_match",
            hashes,
            expected=True,
            source="docs/68:8.2#15",
            detail="save -> load -> save hash equal",
        )


def _eval_late(
    ev: _Evaluator,
    document: dict[str, Any],
    results: dict[str, Any],
) -> None:
    _eval_common(ev, document, LATE_SCENE)

    headless = document.get("headless")
    if headless is MISSING:
        ev.add(
            "headless",
            "pass",
            False,
            "false",
            "docs/68:5.1",
            "render scene must not be headless (flag absent)",
        )
    else:
        ev.flag(
            "headless",
            headless,
            expected=False,
            required=True,
            source="docs/68:5.1",
            detail="a headless run cannot produce FPS evidence",
        )

    _eval_hardware(ev, document, LATE_SCENE)

    values: dict[str, Any] = {}
    for name, paths in LATE_PATHS.items():
        values[name] = _number(_get(results, paths))
        if values[name] is MISSING:
            values[name] = _number(_get(document, paths))

    ev.number(
        "fps_avg",
        values["fps_avg"],
        minimum=LATE["fps_avg_min"],
        required=True,
        source="docs/68:8.2#1",
        detail="median average FPS over three runs",
    )
    ev.number(
        "fps_low_1pct",
        values["fps_low_1pct"],
        minimum=LATE["fps_low_1pct_min"],
        required=True,
        source="docs/68:8.2#2",
        detail="1000 / p99 frame time",
    )
    ev.number(
        "fps_low_0_1pct",
        values["fps_low_0_1pct"],
        minimum=LATE["fps_low_0_1pct_min"],
        source="docs/68:8.2#3",
        detail="checked when p99.9 is recorded (G-08)",
    )
    ev.number(
        "frametime_ms_p99",
        values["frametime_ms_p99"],
        maximum=LATE["frametime_ms_p99_max"],
        source="docs/68:8.2#4",
    )
    ev.number(
        "frametime_ms_p99_9",
        values["frametime_ms_p99_9"],
        maximum=LATE["frametime_ms_p99_9_max"],
        source="docs/68:8.2#4",
        detail="checked when p99.9 is recorded (G-08)",
    )
    ev.number(
        "frametime_ms_max",
        values["frametime_ms_max"],
        maximum=LATE["frametime_ms_max_max"],
        required=True,
        source="docs/68:8.2#5",
        detail="single frame after warm-up; a stall fails even if 1% low holds",
    )
    ev.number(
        "hitches_50ms",
        values["hitches_50ms"],
        maximum=float(LATE["hitches_50ms_max"]),
        required=True,
        source="docs/68:8.2#5",
        detail=">50 ms frames after warm-up",
    )
    ev.number(
        "stalls_250ms",
        values["stalls_250ms"],
        maximum=float(LATE["stalls_250ms_max"]),
        required=True,
        source="docs/68:8.2#5",
        detail=">250 ms frames after warm-up",
    )
    ev.number(
        "streaming_stalls_100ms",
        values["streaming_stalls_100ms"],
        maximum=float(LATE["streaming_stalls_100ms_max"]),
        required=True,
        source="docs/68:8.2#16",
        detail="streaming stalls >100 ms",
    )
    ev.number(
        "dropped_chunks",
        values["dropped_chunks"],
        maximum=float(LATE["dropped_chunks_max"]),
        source="docs/68:8.2#16",
        detail="checked when the streaming counters are recorded",
    )

    ev.number(
        "draw_calls_p99",
        values["draw_calls_p99"],
        maximum=LATE["draw_calls_p99_max"],
        strict_max=True,
        source="docs/68:8.2#8",
    )
    ev.number(
        "triangles",
        values["triangles"],
        minimum=LATE["triangles_min"],
        maximum=LATE["triangles_max"],
        source="docs/68:8.2#8",
        detail="median visible triangles on Late",
    )

    ev.number(
        "vram_mb_peak",
        values["vram_mb_peak"],
        maximum=LATE["vram_mb_peak_max"],
        required=True,
        source="docs/68:8.2#9",
        detail="10.5 GB of the 12 GB card",
    )
    ev.number(
        "ram_mb_peak",
        values["ram_mb_peak"],
        maximum=LATE["ram_mb_peak_max"],
        required=True,
        source="docs/68:8.2#9",
        detail="client RSS peak",
    )
    ev.number(
        "vbus_mb_peak",
        values["vbus_mb_peak"],
        maximum=LATE["vbus_mb_peak_max"],
        source="gate-tooling",
        detail="optional asset/GPU-buffer budget; docs/68 has no VBUS row",
    )

    ev.number(
        "save_ms",
        values["save_ms"],
        maximum=LATE["save_ms_max"],
        required=True,
        source="docs/68:8.2#15",
    )
    ev.number(
        "load_ms",
        values["load_ms"],
        maximum=LATE["load_ms_max"],
        source="docs/68:8.2#15",
    )
    save_size = _save_size_mb(document, results)
    if save_size is MISSING:
        ev.missing("save_size", "save size in MB or bytes (provenance only)")
    else:
        ev.add(
            "save_size_mb",
            "pass" if save_size > 0 else "fail",
            save_size,
            ">0",
            "docs/68:8.2#15",
            "recorded size; docs/68 sets no size cap",
        )

    ev.number(
        "overhead_pct",
        values["overhead_pct"],
        maximum=LATE["overhead_pct_max"],
        strict_max=True,
        source="docs/68:8.2#17",
        detail="recording off vs on",
    )
    ev.number(
        "fps_avg_spread_pct",
        _spread_pct(document, results),
        maximum=LATE["spread_pct_max"],
        strict_max=True,
        source="docs/68:8.2#17",
        detail="3-run average FPS spread",
    )

    settings = document.get("settings")
    if isinstance(settings, dict) and "renderer" in settings:
        renderer = str(settings["renderer"]).strip().lower()
        ev.add(
            "renderer",
            "pass" if renderer in ("forward_plus", "forward+") else "fail",
            renderer,
            "forward_plus",
            "docs/68:8.1",
            "Compatibility is a fallback tier, not the gate renderer",
        )

    _eval_correctness(ev, document, results)


def _eval_cohort(
    ev: _Evaluator,
    document: dict[str, Any],
    results: dict[str, Any],
) -> None:
    _eval_common(ev, document, COHORT_SCENE)

    ev.flag(
        "headless",
        document.get("headless"),
        expected=True,
        required=True,
        source="docs/68:4",
        detail="cohort_1m_v1 is a headless scene",
    )

    _eval_hardware(ev, document, COHORT_SCENE)

    values: dict[str, Any] = {}
    for name, paths in COHORT_PATHS.items():
        values[name] = _number(_get(results, paths))
        if values[name] is MISSING:
            values[name] = _number(_get(document, paths))

    ev.number(
        "ticks",
        values["ticks"],
        minimum=float(COHORT["ticks_min"]),
        source="docs/68:4",
        detail="3,600 ticks at 1 Hz",
    )
    ev.number(
        "cohort_tick_ms_p99",
        values["cohort_tick_ms_p99"],
        maximum=COHORT["cohort_tick_ms_p99_max"],
        required=True,
        source="docs/68:8.2#10",
        detail="10k cohorts at 1 Hz",
    )
    ev.number(
        "full_tick_ms_p99",
        values["full_tick_ms_p99"],
        maximum=COHORT["full_tick_ms_p99_max"],
        required=True,
        source="docs/68:8.2#10",
        detail="districts + dirty buildings + economy aggregate",
    )
    ev.number(
        "active_agents_ms_p99",
        values["active_agents_ms_p99"],
        maximum=COHORT["active_agents_ms_p99_max"],
        source="docs/68:8.2#11",
        detail="checked when the 2k-agent tier is recorded",
    )
    ev.number(
        "pathfinding_ms_p99",
        values["pathfinding_ms_p99"],
        maximum=COHORT["pathfinding_ms_p99_max"],
        source="docs/68:8.2#11",
        detail="checked when pathfinding is recorded",
    )
    ev.number(
        "sim_cpu_ms_per_s",
        values["sim_cpu_ms_per_s"],
        maximum=COHORT["sim_cpu_ms_per_s_max"],
        source="docs/68:8.2#12",
        detail="total simulation CPU per wall-clock second",
    )
    ev.number(
        "server_rss_mb_peak",
        values["server_rss_mb_peak"],
        maximum=COHORT["server_rss_mb_peak_max"],
        source="docs/68:5.3",
        detail="dedicated server RSS",
    )
    ev.number(
        "network_stall_ms_max",
        values["network_stall_ms_max"],
        maximum=COHORT["network_stall_ms_max"],
        source="docs/68:5.3",
        detail="no network-loop stall >16 ms",
    )

    ev.number(
        "save_ms",
        values["save_ms"],
        maximum=COHORT["save_ms_max"],
        required=True,
        source="docs/68:8.2#15",
    )
    ev.number(
        "load_ms",
        values["load_ms"],
        maximum=COHORT["load_ms_max"],
        source="docs/68:8.2#15",
    )
    save_size = _save_size_mb(document, results)
    if save_size is MISSING:
        ev.missing("save_size", "save size in MB or bytes (provenance only)")
    else:
        ev.add(
            "save_size_mb",
            "pass" if save_size > 0 else "fail",
            save_size,
            ">0",
            "docs/68:8.2#15",
            "recorded size; docs/68 sets no size cap",
        )

    ev.number(
        "traffic_build_ms",
        values["traffic_build_ms"],
        maximum=COHORT["traffic_build_ms_max"],
        source="docs/68:8.2#14",
        detail="1M grid build on a worker",
    )
    ev.number(
        "traffic_incremental_ms_p99",
        values["traffic_incremental_ms_p99"],
        maximum=COHORT["traffic_incremental_ms_p99_max"],
        source="docs/68:8.2#14",
    )
    ev.number(
        "traffic_flow_ms_p99",
        values["traffic_flow_ms_p99"],
        maximum=COHORT["traffic_flow_ms_p99_max"],
        source="docs/68:8.2#14",
    )
    ev.number(
        "traffic_queries_5000_ms",
        values["traffic_queries_5000_ms"],
        maximum=COHORT["traffic_queries_5000_ms_max"],
        source="docs/68:8.2#14",
    )

    _eval_correctness(ev, document, results)


def evaluate_document(
    document: Any,
    path: Path,
    expected_scene: str | None = None,
    require_hardware: bool = False,
) -> ResultEval:
    if not isinstance(document, dict):
        ev = _Evaluator(path, expected_scene or "unknown", require_hardware)
        ev.missing("document", "JSON root must be an object")
        return ev.result

    scene = _text(document.get("scene"))
    if scene is MISSING:
        ev = _Evaluator(path, expected_scene or "unknown", require_hardware)
        ev.missing("scene", "scene id (late_metropolis_v1|cohort_1m_v1)")
        return ev.result

    ev = _Evaluator(path, scene, require_hardware)
    if expected_scene and scene != expected_scene:
        ev.add(
            "scene",
            "fail",
            scene,
            expected_scene,
            "docs/68:4",
            "result scene does not match --scene",
        )
        return ev.result
    if scene not in SUPPORTED_SCENES:
        ev.result.insufficient = True
        ev.add(
            "scene",
            "missing",
            scene,
            "|".join(SUPPORTED_SCENES),
            "docs/68:4",
            "unsupported scene id; only the 1M gate scenes are evaluated",
        )
        return ev.result

    results = document.get("results")
    if not isinstance(results, dict) or not results:
        ev.missing("results", "results object with frame/sim metrics")
        return ev.result

    if scene == LATE_SCENE:
        _eval_late(ev, document, results)
    else:
        _eval_cohort(ev, document, results)
    return ev.result


def evaluate_file(
    path: Path,
    expected_scene: str | None = None,
    require_hardware: bool = False,
) -> ResultEval:
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return ResultEval(path=path, error=f"file not found: {path}")
    except json.JSONDecodeError as error:
        return ResultEval(path=path, error=f"invalid JSON in {path}: {error}")
    except OSError as error:
        return ResultEval(path=path, error=f"cannot read {path}: {error}")
    return evaluate_document(document, path, expected_scene, require_hardware)


def _print_check(result: ResultEval, check: Check) -> None:
    line = (
        f"METROPOLIS_GATE_CHECK result={result.path} "
        f"scene={result.scene or 'unknown'} check={check.name} "
        f"status={check.status} value={_fmt(check.value)}"
    )
    if check.threshold:
        line += f' threshold="{check.threshold}"'
    if check.source:
        line += f" source={check.source}"
    if check.detail:
        line += f' detail="{_clean(check.detail)}"'
    print(line)


def _aggregate_hardware(results: Sequence[ResultEval]) -> str:
    values = [result.hardware_match for result in results if result.hardware_match is not None]
    if not values:
        return "unknown"
    if any(value is False for value in values):
        return "false"
    return "true"


def _overall_status(results: Sequence[ResultEval]) -> str:
    if any(result.failed for result in results):
        return "fail"
    if any(result.insufficient or result.error for result in results):
        return "insufficient"
    return "pass"


def _report(
    results: Sequence[ResultEval],
    require_hardware: bool,
) -> dict[str, Any]:
    status = _overall_status(results)
    checks = sum(len(result.checks) for result in results)
    failed = sum(result.failed for result in results)
    return {
        "format_version": 1,
        "gate": "G5",
        "evaluator": "check_metropolis_gate.py",
        "require_hardware": require_hardware,
        "status": status,
        "scenes": len({result.scene for result in results if result.scene}),
        "checks": checks,
        "failed": failed,
        "hardware_match": _aggregate_hardware(results),
        "thresholds": {
            "late_metropolis_v1": LATE,
            "cohort_1m_v1": COHORT,
        },
        "results": [
            {
                "path": str(result.path),
                "scene": result.scene,
                "status": result.status,
                "hardware_match": result.hardware_match,
                "insufficient": result.insufficient,
                "error": result.error,
                "checks": [
                    {
                        "name": check.name,
                        "status": check.status,
                        "value": None
                        if check.value is MISSING
                        else check.value,
                        "threshold": check.threshold,
                        "source": check.source,
                        "detail": check.detail,
                    }
                    for check in result.checks
                ],
            }
            for result in results
        ],
    }


def _late_fixture() -> dict[str, Any]:
    return {
        "format_version": 1,
        "gate": "G5",
        "config": "C1_local_server",
        "scene": LATE_SCENE,
        "route": "late_flyover_v1",
        "route_hash": "blake3:route",
        "settings": "standard_1080p_high_v1",
        "settings_hash": "blake3:settings",
        "duration_s": 210,
        "warmup_s": 15,
        "runs": 3,
        "headless": False,
        "hardware": {
            "cpu": "AMD Ryzen 7 5800X 8-Core Processor",
            "cpu_cores": 8,
            "cpu_threads": 16,
            "gpu": "NVIDIA GeForce RTX 3060 12 GB",
            "driver": "580.00",
            "ram_gb": 32,
            "display": "1920x1080",
            "os": "Windows 11",
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
            "streaming_stalls_100ms": 0,
            "dropped_chunks": 0,
            "draw_calls_p99": 1840,
            "triangles_avg": 7_600_000,
            "vram_mb_peak": 9800,
            "ram_mb_peak": 12000,
            "save_ms": 24000,
            "load_ms": 21000,
            "save_bytes": 182_808_978,
            "overhead_pct": 1.2,
            "fps_avg_spread_pct": 2.4,
            "sim_tick_ms_p99": 8.1,
            "sim_cpu_ms_per_s": 180.0,
            "valid": True,
        },
        "valid": True,
    }


def _cohort_fixture() -> dict[str, Any]:
    return {
        "format_version": 1,
        "gate": "G5",
        "config": "C2_dedicated_server",
        "scene": COHORT_SCENE,
        "route": "cohort_3600_ticks_v1",
        "settings": "headless_1hz_v1",
        "duration_s": 3600,
        "warmup_s": 0,
        "runs": 3,
        "headless": True,
        "hardware": {
            "cpu": "AMD Ryzen 7 5800X 8-Core Processor",
            "cpu_cores": 8,
            "cpu_threads": 16,
            "ram_gb": 32,
            "os": "Linux",
        },
        "results": {
            "ticks": 3600,
            "cohort_tick_ms_p99": 8.1,
            "full_tick_ms_p99": 40.0,
            "active_agents_ms_p99": 8.5,
            "pathfinding_ms_p99": 2.0,
            "sim_cpu_ms_per_s": 180.0,
            "server_rss_mb_peak": 3500,
            "server_network_stall_ms_max": 4.0,
            "save_ms": 24000,
            "load_ms": 25000,
            "save_bytes": 182_808_978,
            "traffic_build_ms": 200,
            "traffic_incremental_ms_p99": 10,
            "traffic_flow_ms_p99": 3,
            "traffic_queries_5000_ms": 8,
            "valid": True,
        },
        "valid": True,
    }


def _self_test_case(
    directory: Path,
    name: str,
    fixture: Any,
    *,
    expect_status: str,
    expect_hardware: str | None = None,
    expect_check: str | None = None,
    expect_check_status: str | None = None,
    expected_scene: str | None = None,
    require_hardware: bool = False,
) -> str | None:
    path = directory / f"{name}.json"
    if isinstance(fixture, str):
        path.write_text(fixture, encoding="utf-8")
    else:
        path.write_text(json.dumps(fixture), encoding="utf-8")
    result = evaluate_file(path, expected_scene, require_hardware)
    status = _overall_status([result])
    if status != expect_status:
        return (
            f"{name}: expected status {expect_status}, got {status} "
            f"(failed={result.failed}, insufficient={result.insufficient}, "
            f"error={result.error})"
        )
    hardware = (
        "unknown"
        if result.hardware_match is None
        else ("true" if result.hardware_match else "false")
    )
    if expect_hardware is not None and hardware != expect_hardware:
        return f"{name}: expected hardware_match={expect_hardware}, got {hardware}"
    if expect_check:
        matches = [check for check in result.checks if check.name == expect_check]
        if not matches:
            names = sorted({check.name for check in result.checks})
            return f"{name}: check {expect_check!r} not emitted (checks: {names})"
        actual = matches[0].status
        if expect_check_status and actual != expect_check_status:
            return (
                f"{name}: expected check {expect_check} status "
                f"{expect_check_status}, got {actual}"
            )
    return None


def run_self_test() -> int:
    failures: list[str] = []
    cases = 0

    with tempfile.TemporaryDirectory(prefix="townsmith-gate-") as raw:
        directory = Path(raw)

        def case(name: str, fixture: Any, **kwargs: Any) -> None:
            nonlocal cases
            cases += 1
            failure = _self_test_case(directory, name, fixture, **kwargs)
            if failure:
                failures.append(failure)

        late = _late_fixture()
        case(
            "late_pass",
            late,
            expect_status="pass",
            expect_hardware="true",
            expect_check="fps_avg",
            expect_check_status="pass",
        )

        fps_fail = _late_fixture()
        fps_fail["results"]["fps_avg"] = 28.0
        case(
            "late_fps_fail",
            fps_fail,
            expect_status="fail",
            expect_check="fps_avg",
            expect_check_status="fail",
        )

        missing = _late_fixture()
        del missing["results"]["fps_low_1pct"]
        case(
            "late_missing_field",
            missing,
            expect_status="insufficient",
            expect_check="fps_low_1pct",
            expect_check_status="missing",
        )

        headless = _late_fixture()
        headless["headless"] = True
        case(
            "late_headless",
            headless,
            expect_status="fail",
            expect_check="headless",
            expect_check_status="fail",
        )

        non_reference = _late_fixture()
        non_reference["hardware"]["gpu"] = "NVIDIA GeForce RTX 4080 16 GB"
        case(
            "late_non_reference_pass",
            non_reference,
            expect_status="pass",
            expect_hardware="false",
            expect_check="hardware.gpu",
            expect_check_status="warn",
        )
        case(
            "late_non_reference_required",
            non_reference,
            expect_status="fail",
            expect_hardware="false",
            expect_check="hardware.gpu",
            expect_check_status="fail",
            require_hardware=True,
        )

        wrong_scene = _late_fixture()
        wrong_scene["scene"] = "mid_city_v1"
        case(
            "late_wrong_scene",
            wrong_scene,
            expect_status="fail",
            expect_check="scene",
            expect_check_status="fail",
            expected_scene=LATE_SCENE,
        )

        stall = _late_fixture()
        stall["results"]["frametime_ms_max"] = 320.0
        case(
            "late_max_frame",
            stall,
            expect_status="fail",
            expect_check="frametime_ms_max",
            expect_check_status="fail",
        )

        streaming = _late_fixture()
        streaming["results"]["streaming_stalls_100ms"] = 1
        case(
            "late_streaming_stall",
            streaming,
            expect_status="fail",
            expect_check="streaming_stalls_100ms",
            expect_check_status="fail",
        )

        vbus = _late_fixture()
        vbus["results"]["vbus_mb_peak"] = 9000
        case(
            "late_vbus_over",
            vbus,
            expect_status="fail",
            expect_check="vbus_mb_peak",
            expect_check_status="fail",
        )

        cohort = _cohort_fixture()
        case(
            "cohort_pass",
            cohort,
            expect_status="pass",
            expect_hardware="true",
            expect_check="cohort_tick_ms_p99",
            expect_check_status="pass",
        )

        cohort_over = _cohort_fixture()
        cohort_over["results"]["cohort_tick_ms_p99"] = 12.0
        case(
            "cohort_over_budget",
            cohort_over,
            expect_status="fail",
            expect_check="cohort_tick_ms_p99",
            expect_check_status="fail",
        )

        cohort_invalid = _cohort_fixture()
        cohort_invalid["headless"] = False
        case(
            "cohort_not_headless",
            cohort_invalid,
            expect_status="fail",
            expect_check="headless",
            expect_check_status="fail",
        )

        case(
            "invalid_json",
            "{",
            expect_status="insufficient",
        )

        missing_file = directory / "does_not_exist.json"
        result = evaluate_file(missing_file)
        if _overall_status([result]) != "insufficient":
            failures.append("missing_file: expected insufficient")

    if failures:
        for failure in failures:
            print(f"METROPOLIS_GATE_SELFTEST_FAIL {failure}")
        print(
            f"METROPOLIS_GATE_SELFTEST cases={cases} failures={len(failures)} "
            "status=failed"
        )
        return 1

    print(f"METROPOLIS_GATE_SELFTEST cases={cases} status=ok")
    return 0


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Evaluate a docs/68 Metropolis gate (G5) result JSON against the "
            "late-scene and headless 1M thresholds. Measurement-free; stdlib only."
        ),
        epilog=(
            "Exit 0 = pass, 1 = fail, 2 = insufficient/bad input. "
            "Example: check_metropolis_gate.py --result "
            "benchmarks/results/gate/late_metropolis_v1.json"
        ),
    )
    parser.add_argument(
        "--result",
        action="append",
        default=[],
        metavar="PATH",
        help="gate result JSON (repeatable)",
    )
    parser.add_argument(
        "--scene",
        choices=SUPPORTED_SCENES,
        help="require every result to be this scene",
    )
    parser.add_argument(
        "--require-hardware",
        action="store_true",
        help=(
            "treat a non-reference CPU/GPU/RAM/display as a failure; without "
            "it the mismatch is a warning and hardware_match=false"
        ),
    )
    parser.add_argument(
        "--report",
        metavar="PATH",
        help="write the per-check evaluation as JSON",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="run built-in pass/fail/missing/headless/hardware fixtures",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)

    if args.self_test:
        return run_self_test()

    if not args.result:
        parser.error("at least one --result PATH is required (or --self-test)")

    results = [
        evaluate_file(Path(path), args.scene, args.require_hardware)
        for path in args.result
    ]

    for result in results:
        if result.error:
            print(
                f"METROPOLIS_GATE_ERROR result={result.path} "
                f'error="{_clean(result.error)}"'
            )
            continue
        for check in result.checks:
            _print_check(result, check)

    status = _overall_status(results)
    checks = sum(len(result.checks) for result in results)
    failed = sum(result.failed for result in results)
    scenes = len({result.scene for result in results if result.scene})
    print(
        f"METROPOLIS_GATE status={status} scenes={scenes} checks={checks} "
        f"failed={failed} hardware_match={_aggregate_hardware(results)}"
    )

    if args.report:
        report = _report(results, args.require_hardware)
        try:
            path = Path(args.report)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        except OSError as error:
            print(f'METROPOLIS_GATE_ERROR cannot write report: "{_clean(error)}"')
            return 2

    if status == "fail":
        return 1
    if status == "insufficient":
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
