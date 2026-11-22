#!/usr/bin/env python3
"""Compare benchmark baseline and candidate JSON (D-020, stdlib only).

The comparator is intentionally deterministic and boring:

* strings, booleans, nulls, and checksums must match exactly;
* numbers may drift within ``--tolerance`` percent (default 5), with an
  absolute fallback near zero;
* dictionaries and lists are compared recursively; missing/extra keys and
  list-length changes are failures;
* list-of-dictionary values are matched by a stable identity key (``scenario``,
  ``degraded``, ``tier``, ...) when possible, so reordering is not a false
  regression;
* volatile provenance (``generated_at``, ``git_commit``, ...) is ignored by
  default.

Named result profiles add required-key checks so a benchmark cannot silently
drop a metric. Unknown files are compared generically.

Exit status: 0 on success, 1 on a benchmark mismatch, 2 on usage/input errors.
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import math
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Sequence

DEFAULT_TOLERANCE = 5.0
DEFAULT_ABS_TOLERANCE = 1e-6

# Timestamps, commit ids, and free-form provenance change on every run/checkout.
# They are not benchmark evidence and must not make a comparison fail.
DEFAULT_IGNORE_PATTERNS: tuple[str, ...] = (
    "generated_at",
    "generated_on",
    "git_commit",
    "git_dirty",
    "hostname",
    "timestamp",
    "run_id",
    "started_at",
    "finished_at",
)

DEFAULT_CHECKSUM_PATTERNS: tuple[str, ...] = (
    "checksum",
    "*_checksum",
    "hash",
    "*_hash",
    "sha",
    "*_sha",
    "digest",
    "*_digest",
    "fingerprint",
    "*_fingerprint",
)

# Stable identity keys for list-of-dict results, in preference order. The first
# key that exists and is unique across every item is used.
IDENTITY_KEYS: tuple[str, ...] = (
    "id",
    "name",
    "scenario",
    "tier",
    "degraded",
    "age",
    "scene",
    "route",
)


@dataclass(frozen=True)
class Profile:
    """A named result shape with required metric paths."""

    name: str
    required: tuple[str, ...] = ()
    nonempty: tuple[str, ...] = ()
    optional: tuple[str, ...] = ()
    ignore: tuple[str, ...] = ()


# Path syntax: dots for object keys, ``key[]`` for every list item, and
# ``key{}`` for every dictionary value.
PROFILES: dict[str, Profile] = {
    "city_loop": Profile(
        name="city_loop",
        required=(
            "scenarios",
            "scenarios[].scenario",
            "scenarios[].checksum",
            "scenarios[].population",
            "scenarios[].population_checksum",
            "scenarios[].jobs",
            "scenarios[].employed",
            "scenarios[].construction_checksum",
            "scenarios[].economy_checksum",
            "scenarios[].industry_checksum",
            "scenarios[].utility_checksum",
            "scenarios[].traffic_checksum",
            "scenarios[].days",
            "scenarios[].seed",
            "scenarios[].step_seconds",
        ),
        nonempty=("scenarios",),
        # The --matrix runner emits scene/days/step_seconds at the top level and
        # omits format_version/suite/seed; the committed baseline is the other
        # way around. The scenario records carry the same settings, so the
        # top-level copies are optional metadata, not evidence.
        optional=(
            "format_version",
            "suite",
            "scene",
            "seed",
            "days",
            "step_seconds",
        ),
    ),
    "city_loop_degraded": Profile(
        name="city_loop_degraded",
        required=(
            "scenario",
            "days",
            "step_seconds",
            "variants",
            "control",
            "variants[].degraded",
            "variants[].checksum",
            "variants[].population",
            "variants[].population_checksum",
            "variants[].construction_checksum",
            "variants[].economy_checksum",
            "variants[].industry_checksum",
            "variants[].utility_checksum",
            "variants[].traffic_checksum",
        ),
        nonempty=("variants",),
        optional=("format_version",),
    ),
    "industrial_balance": Profile(
        name="industrial_balance",
        required=(
            "scenarios",
            "city",
            "scenarios[].scenario",
            "scenarios[].checksum",
            "scenarios[].population",
            "scenarios[].population_checksum",
            "scenarios[].construction_checksum",
            "scenarios[].economy_checksum",
            "scenarios[].industry_checksum",
            "scenarios[].utility_checksum",
            "scenarios[].placements_checksum",
        ),
        nonempty=("scenarios",),
        optional=("format_version",),
    ),
    "core": Profile(
        name="core",
        required=(
            "format_version",
            "suite",
            "runs",
            "results",
            "results.mesh_checkerboard_ms",
            "results.mesh_hut_ms",
            "results.worldgen_load_ms",
            "results.worldgen_reload_ms",
            "results.worldgen_edit_ms",
            "results.terrain_mesh_ms",
            "results.terrain_lod_2_ms",
            "results.terrain_lod_4_ms",
            "results.cohort_tick_ms",
            "results.cohort_tick_p95_ms",
            "results.rooms_detection_ms",
            "results.city_early_ms",
            "results.city_mid_ms",
            "results.city_late_ms",
            "results.city_checksum",
            "results.active_tick_ms",
            "results.active_tick_p95_ms",
            "results.active_dense_tick_ms",
            "results.active_dense_tick_p95_ms",
            "results.volume_mesh_ms",
            "results.volume_solid_voxels",
            "results.volume_quads",
            "results.volume_checksum",
        ),
    ),
    "metropolis_data_1m": Profile(
        name="metropolis_data_1m",
        required=(
            "scene",
            "format_version",
            "population",
            "cohorts",
            "active_agents",
            "buildings",
            "ticks",
            "checksum",
            "tick_ms_median",
            "tick_ms_p95",
        ),
        optional=("elapsed_ms", "setup_ms", "tick_loop_ms"),
    ),
    "placements": Profile(
        name="placements",
        required=(
            "scene",
            "placements",
            "houses",
            "workplaces",
            "index_entries",
            "build_ms",
            "serialize_ms",
            "parse_ms",
            "checksum",
        ),
        optional=(
            "seed",
            "grid_side",
            "spacing_m",
            "bounds_m",
            "per_placement_us",
            "serialized_bytes",
            "deterministic",
            "status",
        ),
    ),
    "godot_phase0_summary": Profile(
        name="godot_phase0_summary",
        required=(
            "format_version",
            "suite",
            "runs",
            "route",
            "results",
            "results.fps_avg",
            "results.fps_low_1pct",
            "results.frametime_ms_avg",
            "results.frametime_ms_p95",
            "results.frametime_ms_p99",
            "results.hitches_16ms",
        ),
    ),
    "godot_phase0_matrix": Profile(
        name="godot_phase0_matrix",
        required=(
            "format_version",
            "suite",
            "settings",
            "routes",
            "routes{}.route",
            "routes{}.fps_avg",
            "routes{}.fps_low_1pct",
            "routes{}.frametime_ms_avg",
            "routes{}.frametime_ms_p99",
            "routes{}.hitches_16ms",
        ),
        nonempty=("routes",),
    ),
}


class CompareError(RuntimeError):
    """A bad input pair, not a benchmark regression."""


@dataclass
class CompareResult:
    checks: int = 0
    mismatches: list[str] = field(default_factory=list)
    profile: str | None = None

    @property
    def ok(self) -> bool:
        return not self.mismatches


@dataclass(frozen=True)
class CompareOptions:
    tolerance: float = DEFAULT_TOLERANCE
    abs_tolerance: float = DEFAULT_ABS_TOLERANCE
    checksum_keys: tuple[str, ...] = ()
    ignore: tuple[str, ...] = ()


def _split_path(path: str) -> list[str]:
    return [segment for segment in path.split(".") if segment]


def _join(path: str, segment: str | int) -> str:
    return f"{path}.{segment}" if path else str(segment)


def _compact(value: Any, limit: int = 140) -> str:
    try:
        text = json.dumps(value, ensure_ascii=True, sort_keys=True)
    except (TypeError, ValueError):
        text = repr(value)
    text = " ".join(text.split())
    if len(text) > limit:
        text = text[: limit - 3] + "..."
    return text


def _is_checksum_key(key: str, path: str, extra: Sequence[str]) -> bool:
    lowered = key.lower()
    lowered_extra = {item.lower() for item in extra}
    if lowered in lowered_extra:
        return True
    for pattern in (*DEFAULT_CHECKSUM_PATTERNS, *extra):
        if fnmatch.fnmatchcase(lowered, pattern.lower()):
            return True
        if fnmatch.fnmatchcase(path.lower(), pattern.lower()):
            return True
    return False


def _has_identity(items: Sequence[Any], key: str) -> bool:
    if not items or not all(isinstance(item, dict) and key in item for item in items):
        return False
    values = [str(item[key]) for item in items if isinstance(item, dict)]
    return len(values) == len(set(values))


def _identity_key(items: Sequence[Any]) -> str | None:
    for key in IDENTITY_KEYS:
        if _has_identity(items, key):
            return key
    return None


def _path_present(value: Any, segments: Sequence[str]) -> bool:
    if not segments:
        return True
    head, rest = segments[0], segments[1:]
    if head.endswith("[]"):
        key = head[:-2]
        if key:
            if not isinstance(value, dict) or key not in value:
                return False
            value = value[key]
        if not isinstance(value, list):
            return False
        return all(_path_present(item, rest) for item in value)
    if head.endswith("{}"):
        key = head[:-2]
        if key:
            if not isinstance(value, dict) or key not in value:
                return False
            value = value[key]
        if not isinstance(value, dict):
            return False
        return all(_path_present(item, rest) for item in value.values())
    if not isinstance(value, dict) or head not in value:
        return False
    return _path_present(value[head], rest)


def _nonempty_present(value: Any, path: str) -> bool:
    current = value
    for segment in _split_path(path):
        if not isinstance(current, dict) or segment not in current:
            return False
        current = current[segment]
    return isinstance(current, (list, dict)) and len(current) > 0


def _detect_profile(document: Any, path: Path) -> str | None:
    name = path.name.lower()
    if not isinstance(document, dict):
        return None
    scene = str(document.get("scene", "")).lower()
    suite = str(document.get("suite", "")).lower()

    if (
        "city_loop_degraded" in name
        or scene == "city_loop_degraded_matrix"
        or suite == "city_loop_degraded"
    ):
        return "city_loop_degraded"
    if (
        "phase3_city_loop" in name
        or scene in {"city_loop_matrix", "city_loop_balance"}
        or suite == "phase3_city_loop"
    ):
        return "city_loop"
    if (
        "industrial_balance" in name
        or scene in {"phase3_industrial_balance", "industrial_balance"}
        or suite == "phase3_industrial_balance"
    ):
        return "industrial_balance"
    if "phase0_core_summary" in name or suite == "phase0_core":
        return "core"
    if "metropolis_data_1m" in name or scene == "metropolis_data_1m":
        return "metropolis_data_1m"
    if name.startswith("placements") or scene == "placements":
        return "placements"
    if (
        "godot_phase0" in name
        or suite in {"phase0_godot_route", "phase0_godot_route_baseline"}
    ):
        if "routes" in document and "settings" in document:
            return "godot_phase0_matrix"
        return "godot_phase0_summary"
    if "routes" in document and "settings" in document and "hardware" in document:
        return "godot_phase0_matrix"
    return None


class _Comparator:
    def __init__(
        self,
        options: CompareOptions,
        profile: Profile | None,
        result: CompareResult,
    ) -> None:
        self.options = options
        self.profile = profile
        self.result = result
        self.ignore_patterns = (*DEFAULT_IGNORE_PATTERNS, *options.ignore)
        if profile:
            self.ignore_patterns = (*self.ignore_patterns, *profile.ignore)
        self.optional_patterns = profile.optional if profile else ()
        self.checksum_patterns = options.checksum_keys

    # -- reporting ---------------------------------------------------------
    def mismatch(self, message: str) -> None:
        self.result.mismatches.append(" ".join(str(message).splitlines()))

    def ignored(self, path: str) -> bool:
        if any(fnmatch.fnmatchcase(path, pattern) for pattern in self.ignore_patterns):
            return True
        return any(
            fnmatch.fnmatchcase(segment, pattern)
            for segment in _split_path(path)
            for pattern in self.ignore_patterns
        )

    def optional(self, path: str) -> bool:
        return any(
            fnmatch.fnmatchcase(path, pattern) for pattern in self.optional_patterns
        )

    # -- recursion ---------------------------------------------------------
    def compare(self, baseline: Any, candidate: Any, path: str, key: str = "") -> None:
        if path and self.ignored(path):
            return

        if isinstance(baseline, dict):
            if not isinstance(candidate, dict):
                self.result.checks += 1
                self.mismatch(
                    f"{path or '<root>'}: type mismatch: baseline is object, "
                    f"candidate is {type(candidate).__name__}"
                )
                return
            self._compare_dict(baseline, candidate, path)
            return

        if isinstance(baseline, list):
            if not isinstance(candidate, list):
                self.result.checks += 1
                self.mismatch(
                    f"{path or '<root>'}: type mismatch: baseline is list, "
                    f"candidate is {type(candidate).__name__}"
                )
                return
            self._compare_list(baseline, candidate, path)
            return

        self._compare_scalar(baseline, candidate, path, key)

    def _compare_dict(
        self, baseline: dict[str, Any], candidate: dict[str, Any], path: str
    ) -> None:
        baseline_keys = list(baseline)
        candidate_only = [key for key in candidate if key not in baseline]
        candidate_keys = baseline_keys + candidate_only

        for key in candidate_keys:
            child = _join(path, key)
            if self.ignored(child):
                continue
            if key not in baseline:
                if self.optional(child):
                    continue
                self.result.checks += 1
                self.mismatch(
                    f"{child}: extra key in candidate (value {_compact(candidate[key])})"
                )
                continue
            if key not in candidate:
                if self.optional(child):
                    continue
                self.result.checks += 1
                self.mismatch(
                    f"{child}: missing key in candidate "
                    f"(baseline value {_compact(baseline[key])})"
                )
                continue
            self.compare(baseline[key], candidate[key], child, key)

    def _compare_list(
        self, baseline: list[Any], candidate: list[Any], path: str
    ) -> None:
        identity = _identity_key(baseline)
        if identity:
            if not _has_identity(candidate, identity):
                identity = None
        else:
            candidate_identity = _identity_key(candidate)
            if candidate_identity and _has_identity(baseline, candidate_identity):
                identity = candidate_identity
        if identity:
            baseline_map = {str(item[identity]): item for item in baseline}
            candidate_map = {str(item[identity]): item for item in candidate}
            for ident in baseline_map:
                child = _join(path, ident)
                if ident not in candidate_map:
                    self.result.checks += 1
                    self.mismatch(f"{child}: missing list entry '{ident}'")
                    continue
                self.compare(
                    baseline_map[ident], candidate_map[ident], child, identity
                )
            for ident in candidate_map:
                if ident not in baseline_map:
                    child = _join(path, ident)
                    self.result.checks += 1
                    self.mismatch(f"{child}: extra list entry '{ident}'")
            return

        if len(baseline) != len(candidate):
            self.result.checks += 1
            self.mismatch(
                f"{path or '<root>'}: list length {len(baseline)} != {len(candidate)}"
            )
        for index in range(min(len(baseline), len(candidate))):
            self.compare(
                baseline[index], candidate[index], _join(path, index), identity or ""
            )

    def _compare_scalar(
        self, baseline: Any, candidate: Any, path: str, key: str
    ) -> None:
        self.result.checks += 1
        label = path or "<root>"
        checksum = _is_checksum_key(key, path, self.checksum_patterns)

        if isinstance(baseline, bool) or isinstance(candidate, bool):
            if baseline is not candidate:
                kind = "checksum" if checksum else "boolean"
                self.mismatch(
                    f"{label}: {kind} mismatch: baseline={_compact(baseline)} "
                    f"candidate={_compact(candidate)}"
                )
            return

        if isinstance(baseline, (int, float)) and isinstance(
            candidate, (int, float)
        ):
            self._compare_number(float(baseline), float(candidate), label, checksum)
            return

        if baseline is None or candidate is None:
            if baseline is not candidate:
                self.mismatch(
                    f"{label}: null mismatch: baseline={_compact(baseline)} "
                    f"candidate={_compact(candidate)}"
                )
            return

        if isinstance(baseline, str) and isinstance(candidate, str):
            # Godot summaries record the route as a path. Compare the file name
            # for that profile so a different checkout directory is not a false
            # regression; everything else stays an exact string comparison.
            if (
                key == "route"
                and self.profile
                and self.profile.name.startswith("godot_phase0")
            ):
                baseline = Path(baseline).name
                candidate = Path(candidate).name
            if baseline != candidate:
                kind = "checksum" if checksum else "string"
                self.mismatch(
                    f"{label}: {kind} mismatch: baseline={_compact(baseline)} "
                    f"candidate={_compact(candidate)}"
                )
            return

        if type(baseline) is not type(candidate):
            self.mismatch(
                f"{label}: type mismatch: baseline={type(baseline).__name__} "
                f"candidate={type(candidate).__name__}"
            )
            return

        if baseline != candidate:
            self.mismatch(
                f"{label}: value mismatch: baseline={_compact(baseline)} "
                f"candidate={_compact(candidate)}"
            )

    def _compare_number(
        self, baseline: float, candidate: float, path: str, checksum: bool
    ) -> None:
        if checksum:
            if baseline != candidate:
                self.mismatch(
                    f"{path}: checksum mismatch: baseline={_compact(baseline)} "
                    f"candidate={_compact(candidate)} (exact)"
                )
            return

        if math.isnan(baseline) or math.isnan(candidate):
            if not (math.isnan(baseline) and math.isnan(candidate)):
                self.mismatch(
                    f"{path}: numeric mismatch: baseline={baseline} candidate={candidate}"
                )
            return

        if baseline == candidate:
            return

        distance = abs(candidate - baseline)
        if abs(baseline) <= self.options.abs_tolerance:
            if distance <= self.options.abs_tolerance:
                return
            self.mismatch(
                f"{path}: numeric mismatch near zero: baseline={baseline:g} "
                f"candidate={candidate:g} (absolute delta {distance:g} > "
                f"{self.options.abs_tolerance:g})"
            )
            return

        percent = distance / abs(baseline) * 100.0
        if percent > self.options.tolerance:
            self.mismatch(
                f"{path}: numeric mismatch: baseline={baseline:g} "
                f"candidate={candidate:g} ({percent:.2f}% > "
                f"{self.options.tolerance:g}%)"
            )


def _check_profile(
    baseline: Any,
    candidate: Any,
    profile: Profile,
    result: CompareResult,
) -> None:
    """Require the profile shape on the candidate without double-reporting.

    If the baseline carries the key too, the recursive comparison has already
    reported the missing/extra key or list entry. Profile checks only add the
    cases the recursive comparison cannot see: both documents missing a
    required metric, or both carrying an empty result list.
    """
    for path in profile.required:
        segments = _split_path(path)
        if _path_present(candidate, segments):
            continue
        if _path_present(baseline, segments):
            continue
        result.checks += 1
        result.mismatches.append(
            f"candidate: missing profile key '{path}' (profile {profile.name})"
        )
    for path in profile.nonempty:
        segments = _split_path(path)
        if _nonempty_present(candidate, path):
            continue
        # If the baseline has entries, the list comparison already reported the
        # missing entries. If the baseline lacks the field, it reported the
        # candidate's extra key.
        if not _path_present(baseline, segments) or _nonempty_present(
            baseline, path
        ):
            continue
        result.checks += 1
        result.mismatches.append(
            f"candidate: profile key '{path}' must be a non-empty object or list "
            f"(profile {profile.name})"
        )


def compare_documents(
    baseline: Any,
    candidate: Any,
    baseline_path: Path,
    candidate_path: Path,
    options: CompareOptions = CompareOptions(),
) -> CompareResult:
    """Compare two decoded documents and return checks/mismatches."""
    baseline_profile = _detect_profile(baseline, baseline_path)
    candidate_profile = _detect_profile(candidate, candidate_path)

    if (
        baseline_profile
        and candidate_profile
        and baseline_profile != candidate_profile
    ):
        raise CompareError(
            f"profile mismatch: baseline is {baseline_profile}, "
            f"candidate is {candidate_profile}"
        )

    profile_name = baseline_profile or candidate_profile
    profile = PROFILES.get(profile_name) if profile_name else None
    result = CompareResult(profile=profile_name)
    comparator = _Comparator(options, profile, result)
    comparator.compare(baseline, candidate, "", "")

    if profile:
        # Enforce the shape on the candidate (the document CI just produced).
        _check_profile(baseline, candidate, profile, result)

    return result


def _load_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as error:
        raise CompareError(f"file not found: {path}") from error
    except OSError as error:
        raise CompareError(f"cannot read {path}: {error}") from error
    except json.JSONDecodeError as error:
        raise CompareError(f"invalid JSON in {path}: {error}") from error


def _pairs_from_args(args: argparse.Namespace, parser: argparse.ArgumentParser) -> list[tuple[Path, Path]]:
    pairs: list[tuple[Path, Path]] = [
        (Path(baseline), Path(candidate)) for baseline, candidate in args.pair
    ]
    if len(args.baseline) != len(args.candidate):
        parser.error(
            "--baseline and --candidate must be repeated the same number of times "
            f"(got {len(args.baseline)} and {len(args.candidate)})"
        )
    pairs.extend(
        (Path(baseline), Path(candidate))
        for baseline, candidate in zip(args.baseline, args.candidate)
    )
    if args.paths:
        if len(args.paths) % 2:
            parser.error("positional paths must be BASELINE CANDIDATE pairs")
        pairs.extend(
            (Path(args.paths[index]), Path(args.paths[index + 1]))
            for index in range(0, len(args.paths), 2)
        )
    if not pairs:
        parser.error("no baseline/candidate pair given")
    return pairs


def _self_test_case(
    directory: Path,
    name: str,
    baseline: Any,
    candidate: Any,
    *,
    expect_ok: bool,
    expect_mismatches: int | None,
    baseline_name: str = "fixture_baseline.json",
    candidate_name: str = "fixture_candidate.json",
    options: CompareOptions = CompareOptions(),
) -> str | None:
    baseline_path = directory / f"{name}_{baseline_name}"
    candidate_path = directory / f"{name}_{candidate_name}"
    baseline_path.write_text(json.dumps(baseline), encoding="utf-8")
    candidate_path.write_text(json.dumps(candidate), encoding="utf-8")
    try:
        result = compare_documents(
            _load_json(baseline_path),
            _load_json(candidate_path),
            baseline_path,
            candidate_path,
            options,
        )
    except CompareError as error:
        return f"{name}: unexpected error: {error}"

    if result.ok != expect_ok:
        return (
            f"{name}: expected {'ok' if expect_ok else 'failed'}, "
            f"got {'ok' if result.ok else 'failed'} "
            f"({len(result.mismatches)} mismatches)"
        )
    if expect_mismatches is not None and len(result.mismatches) != expect_mismatches:
        return (
            f"{name}: expected {expect_mismatches} mismatches, "
            f"got {len(result.mismatches)}: {result.mismatches}"
        )
    return None


def run_self_test() -> int:
    failures: list[str] = []
    cases = 0
    with tempfile.TemporaryDirectory(prefix="townsmith-bench-compare-") as raw:
        directory = Path(raw)

        def case(*args: Any, **kwargs: Any) -> None:
            nonlocal cases
            cases += 1
            failure = _self_test_case(directory, *args, **kwargs)
            if failure:
                failures.append(failure)

        case(
            "equal",
            {"suite": "x", "results": {"fps": 100.0, "checksum": "abc"}},
            {"suite": "x", "results": {"fps": 100.0, "checksum": "abc"}},
            expect_ok=True,
            expect_mismatches=0,
        )
        case(
            "within",
            {"fps": 100.0},
            {"fps": 104.0},
            expect_ok=True,
            expect_mismatches=0,
        )
        case(
            "over",
            {"fps": 100.0},
            {"fps": 106.0},
            expect_ok=False,
            expect_mismatches=1,
        )
        case(
            "checksum",
            {"checksum": "aaaa"},
            {"checksum": "bbbb"},
            expect_ok=False,
            expect_mismatches=1,
            options=CompareOptions(tolerance=50.0),
        )
        case(
            "list_length",
            {"values": [1, 2, 3]},
            {"values": [1, 2]},
            expect_ok=False,
            expect_mismatches=1,
        )
        case(
            "missing_key",
            {"a": 1, "b": 2},
            {"a": 1},
            expect_ok=False,
            expect_mismatches=1,
        )
        case(
            "extra_key",
            {"a": 1},
            {"a": 1, "b": 2},
            expect_ok=False,
            expect_mismatches=1,
        )
        case(
            "nested_within",
            {"results": {"fps": 100.0}},
            {"results": {"fps": 103.0}},
            expect_ok=True,
            expect_mismatches=0,
        )
        case(
            "near_zero",
            {"x": 0.0},
            {"x": 5e-7},
            expect_ok=True,
            expect_mismatches=0,
        )
        case(
            "near_zero_over",
            {"x": 0.0},
            {"x": 1e-5},
            expect_ok=False,
            expect_mismatches=1,
        )
        case(
            "reordered_scenarios",
            {"scenarios": [{"scenario": "early", "v": 1}, {"scenario": "late", "v": 2}]},
            {"scenarios": [{"scenario": "late", "v": 2}, {"scenario": "early", "v": 1}]},
            expect_ok=True,
            expect_mismatches=0,
        )
        case(
            "volatile_ignored",
            {"generated_at": "2026-01-01T00:00:00Z", "x": 1},
            {"generated_at": "2026-11-22T00:00:00Z", "x": 1},
            expect_ok=True,
            expect_mismatches=0,
        )
        case(
            "profile_required",
            {"scenarios": [{"scenario": "early"}]},
            {"scene": "city_loop_matrix", "scenarios": [{"scenario": "early"}]},
            expect_ok=False,
            expect_mismatches=None,
            candidate_name="phase3_city_loop_matrix.json",
        )

    if failures:
        for failure in failures:
            print(f"BENCH_COMPARE_SELFTEST_FAIL {failure}")
        print(
            f"BENCH_COMPARE_SELFTEST cases={cases} failures={len(failures)} "
            "status=failed"
        )
        return 1
    print(f"BENCH_COMPARE_SELFTEST cases={cases} status=ok")
    return 0


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Compare benchmark baseline and candidate JSON with exact checksums, "
            "numeric tolerances, and profile-aware required metrics (D-020)."
        ),
        epilog=(
            "Example: compare_bench.py --baseline benchmarks/baselines/"
            "phase3_city_loop.json --candidate benchmarks/results/"
            "phase3_city_loop_matrix.json"
        ),
    )
    parser.add_argument(
        "--baseline",
        action="append",
        default=[],
        metavar="PATH",
        help="baseline JSON (repeat with --candidate for multiple pairs)",
    )
    parser.add_argument(
        "--candidate",
        action="append",
        default=[],
        metavar="PATH",
        help="candidate JSON (paired with --baseline by position)",
    )
    parser.add_argument(
        "--pair",
        action="append",
        nargs=2,
        default=[],
        metavar=("BASELINE", "CANDIDATE"),
        help="explicit baseline/candidate pair (repeatable)",
    )
    parser.add_argument(
        "--tolerance",
        type=float,
        default=DEFAULT_TOLERANCE,
        metavar="PCT",
        help=f"numeric tolerance in percent (default {DEFAULT_TOLERANCE:g})",
    )
    parser.add_argument(
        "--abs-tolerance",
        type=float,
        default=DEFAULT_ABS_TOLERANCE,
        metavar="VALUE",
        help=(
            "absolute tolerance used when the baseline magnitude is at or near "
            f"zero (default {DEFAULT_ABS_TOLERANCE:g})"
        ),
    )
    parser.add_argument(
        "--checksum-key",
        action="append",
        default=[],
        metavar="KEY",
        help=(
            "extra key/glob treated as an exact checksum (e.g. population_checksum); "
            "checksum names and *_checksum are always exact"
        ),
    )
    parser.add_argument(
        "--ignore",
        action="append",
        default=[],
        metavar="PATH",
        help="dotted path or glob to ignore (e.g. hardware.display_server)",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="run built-in fixtures in the system temp directory and exit",
    )
    parser.add_argument(
        "paths",
        nargs="*",
        metavar="BASELINE CANDIDATE",
        help="optional positional baseline/candidate pairs",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)

    if args.self_test:
        return run_self_test()

    if not math.isfinite(args.tolerance) or args.tolerance < 0:
        parser.error("--tolerance must be a finite number >= 0")
    if not math.isfinite(args.abs_tolerance) or args.abs_tolerance < 0:
        parser.error("--abs-tolerance must be a finite number >= 0")

    pairs = _pairs_from_args(args, parser)
    options = CompareOptions(
        tolerance=args.tolerance,
        abs_tolerance=args.abs_tolerance,
        checksum_keys=tuple(args.checksum_key),
        ignore=tuple(args.ignore),
    )

    total_checks = 0
    total_mismatches = 0
    failed = False
    for baseline_path, candidate_path in pairs:
        try:
            baseline = _load_json(baseline_path)
            candidate = _load_json(candidate_path)
            result = compare_documents(
                baseline, candidate, baseline_path, candidate_path, options
            )
        except CompareError as error:
            print(
                f"BENCH_COMPARE_ERROR baseline={baseline_path} "
                f"candidate={candidate_path} error={error}"
            )
            failed = True
            continue

        for mismatch in result.mismatches:
            print(f"MISMATCH {mismatch}")
        total_checks += result.checks
        total_mismatches += len(result.mismatches)
        if not result.ok:
            failed = True
            print(
                f"BENCH_COMPARE_PAIR baseline={baseline_path} "
                f"candidate={candidate_path} profile={result.profile or 'generic'} "
                f"checks={result.checks} mismatches={len(result.mismatches)} "
                "status=failed"
            )

    print(
        f"BENCH_COMPARE pairs={len(pairs)} checks={total_checks} "
        f"mismatches={total_mismatches} status={'failed' if failed else 'ok'}"
    )
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
