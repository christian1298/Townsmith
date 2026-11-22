#!/usr/bin/env python3
"""Summarize one or more Q-101 playtest capture JSON reports (stdlib only).

Reads reports written by ``godot/playtest_capture.gd`` (see
``docs/66 - Playtest Kit.md`` §9), validates the export schema, and prints an
aggregate plus the machine-readable marker

    PLAYTEST_SUMMARY reports=N completed=N issues=N status=ok|partial

``status`` is ``partial`` when any report carries ``validation.warnings`` or
has incomplete stages. Partial reports are valid input: the tool exits 0 and
still writes the requested ``--markdown``/``--json`` outputs. Malformed JSON,
a missing schema field, inconsistent counters, or an unsupported format
version are hard errors (exit 2), and no aggregate is printed.

The export field is ``schema`` (currently ``1``). ``format_version`` is
accepted as an alias; if both are present they must agree. Empty issue rows
(a blank ``description``) are ignored, matching the capture form, and counted
so a hand-edited report does not inflate the issue totals.

Usage:
    ./scripts/summarize_playtest.py REPORT... [--markdown PATH] [--json PATH]
    ./scripts/summarize_playtest.py REPORT_DIR/
    ./scripts/summarize_playtest.py --self-test

Exit status:
    0  every report was read; the marker reports ok or partial
    2  bad input: unreadable file, invalid JSON, schema/version mismatch,
       inconsistent counters, or an unwritable output path
"""

from __future__ import annotations

import argparse
import io
import json
import math
import re
import sys
import tempfile
from collections import Counter
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from typing import Any, Iterable, Sequence

SUITE = "q101_playtest_capture"
SUPPORTED_SCHEMA_VERSIONS = (1,)
ROUTE_ID = "q101"
TOP_ISSUES = 5
SUMMARY_SCHEMA = 1

# Route order and labels from godot/playtest_capture.gd. Reports may carry a
# different stage list (a future schema bump), but known ids keep a canonical
# order in the aggregate.
STAGE_ORDER = (
    "orient",
    "starter_build",
    "house",
    "industry",
    "utility",
    "service",
    "save_load",
    "ratings",
)
STAGE_LABELS = {
    "orient": "Orient",
    "starter_build": "Starter build",
    "house": "House",
    "industry": "Industry",
    "utility": "Utility",
    "service": "Service",
    "save_load": "Save/load",
    "ratings": "Ratings",
}
SEVERITY_ORDER = ("high", "medium", "low")
SEVERITY_RANK = {name: index for index, name in enumerate(SEVERITY_ORDER)}

REQUIRED_ROOT_FIELDS = (
    "suite",
    "status",
    "captured_at",
    "route",
    "tester",
    "timer",
    "stages_completed",
    "stages_total",
    "stages",
    "issues",
    "notes",
    "validation",
)

CAPTURED_AT_RE = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")

MARKER_PREFIX = "PLAYTEST_SUMMARY"


class ReportError(Exception):
    """Input or output problem that must exit 2 with a clear message."""

    def __init__(self, label: str, message: str) -> None:
        super().__init__(message)
        self.label = label
        self.message = message


# --- validation -----------------------------------------------------------


def _mapping(value: Any, field: str, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ReportError(label, f"field {field!r} must be a JSON object")
    return value


def _string(value: Any, field: str, label: str, *, nonempty: bool = False) -> str:
    if not isinstance(value, str):
        raise ReportError(label, f"field {field!r} must be a string")
    if nonempty and not value.strip():
        raise ReportError(label, f"field {field!r} must not be empty")
    return value


def _boolean(value: Any, field: str, label: str) -> bool:
    if not isinstance(value, bool):
        raise ReportError(label, f"field {field!r} must be true or false")
    return value


def _integer(value: Any, field: str, label: str, *, minimum: int | None = None) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise ReportError(label, f"field {field!r} must be an integer")
    if minimum is not None and value < minimum:
        raise ReportError(label, f"field {field!r} must be >= {minimum} (got {value})")
    return value


def _number(value: Any, field: str, label: str, *, minimum: float | None = None) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ReportError(label, f"field {field!r} must be a number")
    result = float(value)
    if not math.isfinite(result):
        raise ReportError(label, f"field {field!r} must be finite")
    if minimum is not None and result < minimum:
        raise ReportError(label, f"field {field!r} must be >= {minimum} (got {value})")
    return result


def _optional_string(mapping: dict[str, Any], key: str, label: str) -> str:
    value = mapping.get(key, "")
    if value is None:
        return ""
    return _string(value, key, label)


def _format_version(data: dict[str, Any], label: str) -> int:
    schema = data.get("schema")
    alias = data.get("format_version")
    if schema is None and alias is None:
        raise ReportError(
            label,
            "missing field 'schema' (or alias 'format_version'); expected 1",
        )
    if schema is not None:
        schema = _integer(schema, "schema", label)
    if alias is not None:
        alias = _integer(alias, "format_version", label)
    if schema is not None and alias is not None and schema != alias:
        raise ReportError(
            label,
            f"schema={schema} and format_version={alias} disagree",
        )
    version = schema if schema is not None else alias
    assert version is not None
    if version not in SUPPORTED_SCHEMA_VERSIONS:
        supported = ", ".join(str(item) for item in SUPPORTED_SCHEMA_VERSIONS)
        raise ReportError(
            label,
            f"unsupported format_version {version}; supported: {supported}",
        )
    return version


def validate_report(raw: Any, label: str) -> dict[str, Any]:
    """Validate one parsed report and return the normalized aggregate shape."""
    data = _mapping(raw, "<root>", label)
    if data.get("tool") == "summarize_playtest":
        raise ReportError(
            label,
            "this is a summarize_playtest aggregate, not a capture report; "
            "write --json/--markdown outputs outside the report directory",
        )
    for field in REQUIRED_ROOT_FIELDS:
        if field not in data:
            raise ReportError(label, f"missing required field {field!r}")

    version = _format_version(data, label)

    suite = _string(data["suite"], "suite", label, nonempty=True)
    if suite != SUITE:
        raise ReportError(label, f"unsupported suite {suite!r}; expected {SUITE!r}")

    status = _string(data["status"], "status", label, nonempty=True)
    if status not in ("ok", "partial"):
        raise ReportError(label, f"field 'status' must be 'ok' or 'partial' (got {status!r})")

    captured_at = _string(data["captured_at"], "captured_at", label, nonempty=True)
    if not CAPTURED_AT_RE.match(captured_at):
        raise ReportError(
            label,
            "field 'captured_at' must be UTC ISO 8601 like 2026-01-31T09:05:00Z",
        )

    route = _mapping(data["route"], "route", label)
    route_id = _string(route.get("id"), "route.id", label, nonempty=True)
    if route_id != ROUTE_ID:
        raise ReportError(label, f"unsupported route.id {route_id!r}; expected {ROUTE_ID!r}")
    route_total_minutes = _integer(
        route.get("total_minutes", 30), "route.total_minutes", label, minimum=0
    )

    tester = _mapping(data["tester"], "tester", label)
    tester_name = _string(tester.get("name", ""), "tester.name", label)
    tester_fields = {
        "name": tester_name,
        "experience": _optional_string(tester, "experience", label),
        "build": _optional_string(tester, "build", label),
        "seed": _optional_string(tester, "seed", label),
        "hardware": _optional_string(tester, "hardware", label),
        "display": _optional_string(tester, "display", label),
        "fps_notes": _optional_string(tester, "fps_notes", label),
    }

    timer = _mapping(data["timer"], "timer", label)
    elapsed_s = _number(timer.get("elapsed_s"), "timer.elapsed_s", label, minimum=0.0)

    stages_raw = data["stages"]
    if not isinstance(stages_raw, list):
        raise ReportError(label, "field 'stages' must be an array")
    if not stages_raw:
        raise ReportError(label, "field 'stages' must not be empty")
    stages: list[dict[str, Any]] = []
    seen_stages: set[str] = set()
    for index, item in enumerate(stages_raw):
        stage = _mapping(item, f"stages[{index}]", label)
        stage_id = _string(stage.get("id"), f"stages[{index}].id", label, nonempty=True)
        if stage_id in seen_stages:
            raise ReportError(label, f"duplicate stage id {stage_id!r}")
        seen_stages.add(stage_id)
        label_text = _optional_string(stage, "label", label) or stage_id
        complete = _boolean(stage.get("complete"), f"stages[{index}].complete", label)
        completed_at = stage.get("completed_at_s")
        if completed_at is not None:
            completed_at = _number(
                completed_at,
                f"stages[{index}].completed_at_s",
                label,
                minimum=0.0,
            )
        elif complete:
            raise ReportError(
                label,
                f"stage {stage_id!r} is complete but completed_at_s is null",
            )
        stages.append(
            {
                "id": stage_id,
                "label": label_text,
                "complete": complete,
                "completed_at_s": completed_at,
            }
        )

    stages_total = _integer(data["stages_total"], "stages_total", label, minimum=1)
    if stages_total != len(stages):
        raise ReportError(
            label,
            f"stages_total={stages_total} but stages has {len(stages)} entries",
        )
    route_stages_total = route.get("stages_total")
    if route_stages_total is not None:
        route_stages_total = _integer(
            route_stages_total, "route.stages_total", label, minimum=1
        )
        if route_stages_total != stages_total:
            raise ReportError(
                label,
                f"route.stages_total={route_stages_total} but stages_total={stages_total}",
            )

    stages_completed = _integer(
        data["stages_completed"], "stages_completed", label, minimum=0
    )
    counted_complete = sum(1 for stage in stages if stage["complete"])
    if stages_completed != counted_complete:
        raise ReportError(
            label,
            f"stages_completed={stages_completed} but {counted_complete} stages "
            "are marked complete",
        )

    issues_raw = data["issues"]
    if not isinstance(issues_raw, list):
        raise ReportError(label, "field 'issues' must be an array")
    issues: list[dict[str, Any]] = []
    ignored_empty_issues = 0
    for index, item in enumerate(issues_raw):
        issue = _mapping(item, f"issues[{index}]", label)
        if "description" not in issue:
            raise ReportError(label, f"missing field 'issues[{index}].description'")
        description = _string(
            issue["description"], f"issues[{index}].description", label
        ).strip()
        if not description:
            # The capture form omits rows with an empty description. Tolerate
            # one in a hand-edited report, but never count it as an issue.
            ignored_empty_issues += 1
            continue
        area = _string(issue.get("area", ""), f"issues[{index}].area", label)
        severity = _string(
            issue.get("severity"), f"issues[{index}].severity", label, nonempty=True
        ).lower()
        if severity not in SEVERITY_RANK:
            allowed = ", ".join(SEVERITY_ORDER)
            raise ReportError(
                label,
                f"issues[{index}].severity {severity!r} is not one of: {allowed}",
            )
        row = issue.get("row", index + 1)
        row = _integer(row, f"issues[{index}].row", label, minimum=1)
        reproducible = _boolean(
            issue.get("reproducible", False),
            f"issues[{index}].reproducible",
            label,
        )
        issues.append(
            {
                "row": row,
                "area": area,
                "severity": severity,
                "description": description,
                "reproducible": reproducible,
            }
        )

    notes = _string(data["notes"], "notes", label)

    validation = _mapping(data["validation"], "validation", label)
    validation_complete = _boolean(
        validation.get("complete"), "validation.complete", label
    )
    warnings_raw = validation.get("warnings")
    if not isinstance(warnings_raw, list):
        raise ReportError(label, "field 'validation.warnings' must be an array")
    warnings: list[str] = []
    for index, warning in enumerate(warnings_raw):
        warnings.append(
            _string(
                warning,
                f"validation.warnings[{index}]",
                label,
                nonempty=True,
            )
        )
    if validation_complete and warnings:
        raise ReportError(
            label,
            "validation.complete is true but validation.warnings is not empty",
        )
    expected_status = "ok" if validation_complete else "partial"
    if status != expected_status:
        raise ReportError(
            label,
            f"status={status!r} does not match validation.complete="
            f"{str(validation_complete).lower()}",
        )

    partial_reasons: list[str] = []
    if warnings:
        partial_reasons.append("validation_warnings")
    if stages_completed < stages_total:
        partial_reasons.append("incomplete_stages")
    if not validation_complete:
        partial_reasons.append("validation_incomplete")
    complete = not partial_reasons

    return {
        "source": label,
        "schema": version,
        "captured_at": captured_at,
        "route": route_id,
        "route_total_minutes": route_total_minutes,
        "status": status,
        "complete": complete,
        "partial_reasons": partial_reasons,
        "tester": tester_fields,
        "elapsed_s": elapsed_s,
        "stages_completed": stages_completed,
        "stages_total": stages_total,
        "stages": stages,
        "issues": issues,
        "ignored_empty_issues": ignored_empty_issues,
        "warnings": warnings,
        "notes": notes,
    }


# --- loading --------------------------------------------------------------


def _read_report(path: Path) -> dict[str, Any]:
    try:
        text = path.read_text(encoding="utf-8-sig")
    except (OSError, UnicodeDecodeError) as error:
        detail = getattr(error, "strerror", None) or str(error)
        raise ReportError(str(path), f"cannot read report: {detail}") from error
    try:
        raw = json.loads(text)
    except json.JSONDecodeError as error:
        raise ReportError(
            str(path),
            f"invalid JSON at line {error.lineno} column {error.colno}: {error.msg}",
        ) from error
    return validate_report(raw, str(path))


def collect_report_paths(
    raw_args: Sequence[str],
    label: str = "arguments",
    exclude: Iterable[Path] = (),
) -> list[Path]:
    """Expand directories to their sorted ``*.json`` files, preserving order."""
    excluded = {path.resolve() for path in exclude}
    paths: list[Path] = []
    seen: set[Path] = set()
    for raw in raw_args:
        path = Path(raw)
        if path.is_dir():
            found = sorted(
                candidate
                for candidate in path.iterdir()
                if candidate.is_file()
                and candidate.suffix.lower() == ".json"
                and candidate.resolve() not in excluded
            )
            if not found:
                raise ReportError(str(path), "directory contains no capture reports")
            candidates = found
        elif path.is_file():
            candidates = [path]
        else:
            raise ReportError(str(path), "report not found")
        for candidate in candidates:
            key = candidate.resolve()
            if key in seen or key in excluded:
                continue
            seen.add(key)
            paths.append(candidate)
    if not paths:
        raise ReportError(label, "no reports given")
    return paths


def load_reports(
    raw_args: Sequence[str], exclude: Iterable[Path] = ()
) -> list[dict[str, Any]]:
    return [
        _read_report(path)
        for path in collect_report_paths(raw_args, exclude=exclude)
    ]


# --- aggregation ----------------------------------------------------------


def _stage_sort_key(stage_id: str) -> tuple[int, Any]:
    try:
        return (0, STAGE_ORDER.index(stage_id))
    except ValueError:
        return (1, stage_id)


def _stage_label(stage_id: str, reports: Iterable[dict[str, Any]]) -> str:
    if stage_id in STAGE_LABELS:
        return STAGE_LABELS[stage_id]
    for report in reports:
        for stage in report["stages"]:
            if stage["id"] == stage_id and stage["label"]:
                return str(stage["label"])
    return stage_id


def summarize(reports: Sequence[dict[str, Any]]) -> dict[str, Any]:
    if not reports:
        raise ReportError("reports", "no reports to summarize")

    total_issues = sum(len(report["issues"]) for report in reports)
    completed = sum(1 for report in reports if report["complete"])
    status = "ok" if completed == len(reports) else "partial"

    stage_counts: Counter[str] = Counter()
    for report in reports:
        for stage in report["stages"]:
            if stage["complete"]:
                stage_counts[stage["id"]] += 1
    all_stage_ids = {stage["id"] for report in reports for stage in report["stages"]}
    stages = [
        {
            "id": stage_id,
            "label": _stage_label(stage_id, reports),
            "completed": stage_counts.get(stage_id, 0),
            "reports": len(reports),
        }
        for stage_id in sorted(all_stage_ids, key=_stage_sort_key)
    ]

    warning_counts: Counter[str] = Counter()
    severity_counts: Counter[str] = Counter()
    area_counts: Counter[str] = Counter()
    issue_groups: dict[str, dict[str, Any]] = {}
    for report in reports:
        warning_counts.update(report["warnings"])
        for issue in report["issues"]:
            severity_counts[issue["severity"]] += 1
            area_counts[issue["area"]] += 1
            group = issue_groups.setdefault(
                issue["description"],
                {
                    "description": issue["description"],
                    "severity": issue["severity"],
                    "areas": set(),
                    "count": 0,
                    "reproducible_count": 0,
                },
            )
            if SEVERITY_RANK[issue["severity"]] < SEVERITY_RANK[group["severity"]]:
                group["severity"] = issue["severity"]
            group["areas"].add(issue["area"])
            group["count"] += 1
            if issue["reproducible"]:
                group["reproducible_count"] += 1

    top_issue_rows = sorted(
        issue_groups.values(),
        key=lambda group: (
            SEVERITY_RANK[group["severity"]],
            -group["count"],
            group["description"].casefold(),
            group["description"],
        ),
    )[:TOP_ISSUES]
    top_issues = [
        {
            "description": group["description"],
            "severity": group["severity"],
            "areas": sorted(group["areas"]),
            "count": group["count"],
            "reproducible_count": group["reproducible_count"],
        }
        for group in top_issue_rows
    ]

    elapsed_values = [report["elapsed_s"] for report in reports]
    elapsed_total = math.fsum(elapsed_values)
    elapsed_mean = elapsed_total / len(reports)

    def unique(field: str) -> list[str]:
        values = {report["tester"][field] for report in reports}
        values.discard("")
        return sorted(values)

    partial_reports = [report for report in reports if not report["complete"]]
    summary = {
        "summary_schema": SUMMARY_SCHEMA,
        "tool": "summarize_playtest",
        "status": status,
        "marker": (
            f"{MARKER_PREFIX} reports={len(reports)} completed={completed} "
            f"issues={total_issues} status={status}"
        ),
        "totals": {
            "reports": len(reports),
            "completed": completed,
            "partial": len(partial_reports),
            "issues": total_issues,
            "ignored_empty_issues": sum(
                report["ignored_empty_issues"] for report in reports
            ),
            "elapsed_s": round(elapsed_total, 3),
            "elapsed_mean_s": round(elapsed_mean, 3),
            "elapsed_min_s": round(min(elapsed_values), 3),
            "elapsed_max_s": round(max(elapsed_values), 3),
            "testers": sorted(
                {report["tester"]["name"] or "not recorded" for report in reports}
            ),
            "builds": unique("build"),
            "seeds": unique("seed"),
            "hardware": unique("hardware"),
        },
        "stages": stages,
        "warnings": [
            {"code": code, "count": count}
            for code, count in sorted(
                warning_counts.items(), key=lambda item: (-item[1], item[0])
            )
        ],
        "issues_by_severity": [
            {"severity": severity, "count": severity_counts[severity]}
            for severity in SEVERITY_ORDER
            if severity_counts[severity]
        ],
        "issues_by_area": [
            {"area": area or "not_recorded", "count": count}
            for area, count in sorted(
                area_counts.items(), key=lambda item: (-item[1], item[0])
            )
        ],
        "top_issues": top_issues,
        "reports": list(reports),
    }
    return summary


# --- rendering ------------------------------------------------------------


def format_seconds(seconds: float) -> str:
    total = int(math.floor(seconds + 0.5))
    hours, remainder = divmod(total, 3600)
    minutes, secs = divmod(remainder, 60)
    if hours:
        return f"{hours}:{minutes:02d}:{secs:02d}"
    return f"{minutes:02d}:{secs:02d}"


def _value_or_dash(value: str) -> str:
    return value if value else "-"


def _record_stages(report: dict[str, Any]) -> str:
    return f"{report['stages_completed']}/{report['stages_total']}"


def _report_status(report: dict[str, Any]) -> str:
    return "ok" if report["complete"] else "partial"


def render_text(summary: dict[str, Any]) -> str:
    totals = summary["totals"]
    reports = summary["reports"]
    lines: list[str] = [
        "Q-101 playtest summary",
        (
            f"Reports: {totals['reports']}  complete: {totals['completed']}  "
            f"partial: {totals['partial']}  issues: {totals['issues']}"
        ),
        "",
    ]

    headers = (
        "Tester",
        "Build",
        "Seed",
        "Hardware",
        "Stages",
        "Elapsed",
        "Issues",
        "Status",
    )
    rows = [
        (
            report["tester"]["name"] or "not recorded",
            _value_or_dash(report["tester"]["build"]),
            _value_or_dash(report["tester"]["seed"]),
            _value_or_dash(report["tester"]["hardware"]),
            _record_stages(report),
            format_seconds(report["elapsed_s"]),
            str(len(report["issues"])),
            _report_status(report),
        )
        for report in reports
    ]
    alignments = ("l", "l", "l", "l", "r", "r", "r", "l")
    widths = [len(header) for header in headers]
    for row in rows:
        for index, cell in enumerate(row):
            widths[index] = max(widths[index], len(cell))

    def pad(value: str, index: int) -> str:
        if alignments[index] == "r":
            return value.rjust(widths[index])
        return value.ljust(widths[index])

    lines.append(
        "  ".join(pad(header, index) for index, header in enumerate(headers)).rstrip()
    )
    for row in rows:
        lines.append(
            "  ".join(pad(cell, index) for index, cell in enumerate(row)).rstrip()
        )

    builds = ", ".join(totals["builds"]) or "not recorded"
    seeds = ", ".join(totals["seeds"]) or "not recorded"
    hardware = ", ".join(totals["hardware"]) or "not recorded"
    lines.append(f"Builds: {builds}  Seeds: {seeds}  Hardware: {hardware}")
    lines.append("")

    lines.append("Stages (complete across reports):")
    stage_width = max(
        (len(stage["label"]) for stage in summary["stages"]), default=0
    )
    for stage in summary["stages"]:
        lines.append(
            f"  {stage['label'].ljust(stage_width)}  "
            f"{stage['completed']}/{stage['reports']}"
        )
    if not summary["stages"]:
        lines.append("  none recorded")

    lines.append("")
    if summary["warnings"]:
        lines.append("Warnings:")
        for warning in summary["warnings"]:
            lines.append(f"  {warning['code']}  {warning['count']}")
    else:
        lines.append("Warnings: none")

    lines.append("")
    if summary["issues_by_severity"]:
        lines.append("Issues by severity:")
        for row in summary["issues_by_severity"]:
            lines.append(f"  {row['severity']}  {row['count']}")
    else:
        lines.append("Issues by severity: none")

    lines.append("")
    if summary["issues_by_area"]:
        lines.append("Issues by area:")
        for row in summary["issues_by_area"]:
            area = "(not recorded)" if row["area"] == "not_recorded" else row["area"]
            lines.append(f"  {area}  {row['count']}")
    else:
        lines.append("Issues by area: none")

    lines.append("")
    if summary["top_issues"]:
        lines.append("Top issues:")
        for index, issue in enumerate(summary["top_issues"], start=1):
            areas = ", ".join(
                "(not recorded)" if area == "" else area for area in issue["areas"]
            )
            repro = f"{issue['reproducible_count']}/{issue['count']} reproducible"
            lines.append(
                f"  {index}. [{issue['severity']}] {areas} x{issue['count']}: "
                f"{issue['description']} ({repro})"
            )
    else:
        lines.append("Top issues: none recorded")

    if totals["ignored_empty_issues"]:
        lines.append("")
        lines.append(f"Ignored empty issue rows: {totals['ignored_empty_issues']}")

    lines.append("")
    lines.append(
        "Elapsed: "
        f"total={format_seconds(totals['elapsed_s'])} "
        f"mean={format_seconds(totals['elapsed_mean_s'])} "
        f"min={format_seconds(totals['elapsed_min_s'])} "
        f"max={format_seconds(totals['elapsed_max_s'])}"
    )

    if summary["status"] == "partial":
        warning_total = sum(row["count"] for row in summary["warnings"])
        incomplete = [
            report
            for report in reports
            if report["stages_completed"] < report["stages_total"]
        ]
        reasons: list[str] = []
        if warning_total:
            reasons.append(f"{warning_total} validation warning(s)")
        if incomplete:
            reasons.append(f"{len(incomplete)} report(s) with incomplete stages")
        if not reasons:
            reasons.append("validation.complete=false")
        lines.append("Partial reasons: " + "; ".join(reasons))
    else:
        lines.append("All reports complete.")

    lines.append(summary["marker"])
    return "\n".join(lines)


def _md_cell(value: str) -> str:
    return value.replace("|", "\\|").replace("\n", " ").strip()


def render_markdown(summary: dict[str, Any]) -> str:
    totals = summary["totals"]
    lines: list[str] = [
        "# Q-101 playtest summary",
        "",
        (
            f"`{summary['marker']}`  "
            f"({totals['reports']} report(s), generated by "
            "`scripts/summarize_playtest.py`)."
        ),
        "",
        "## Reports",
        "",
        "| Tester | Build | Seed | Hardware | Stages | Elapsed | Issues | Status |",
        "|---|---|---|---|---:|---:|---:|---|",
    ]
    for report in summary["reports"]:
        tester = report["tester"]
        lines.append(
            "| "
            + " | ".join(
                _md_cell(value)
                for value in (
                    tester["name"] or "not recorded",
                    tester["build"] or "-",
                    tester["seed"] or "-",
                    tester["hardware"] or "-",
                    _record_stages(report),
                    format_seconds(report["elapsed_s"]),
                    str(len(report["issues"])),
                    _report_status(report),
                )
            )
            + " |"
        )

    lines.extend(["", "## Stage completion", "", "| Stage | Complete |", "|---|---:|"])
    for stage in summary["stages"]:
        lines.append(
            f"| {_md_cell(stage['label'])} | {stage['completed']}/{stage['reports']} |"
        )

    lines.extend(["", "## Warnings", ""])
    if summary["warnings"]:
        lines.extend(["| Warning | Reports |", "|---|---:|"])
        for warning in summary["warnings"]:
            lines.append(f"| `{warning['code']}` | {warning['count']} |")
    else:
        lines.append("None.")

    lines.extend(["", "## Issues", "", "### By severity", ""])
    if summary["issues_by_severity"]:
        lines.extend(["| Severity | Count |", "|---|---:|"])
        for row in summary["issues_by_severity"]:
            lines.append(f"| {row['severity']} | {row['count']} |")
    else:
        lines.append("None recorded.")

    lines.extend(["", "### By area", ""])
    if summary["issues_by_area"]:
        lines.extend(["| Area | Count |", "|---|---:|"])
        for row in summary["issues_by_area"]:
            area = "(not recorded)" if row["area"] == "not_recorded" else row["area"]
            lines.append(f"| {_md_cell(area)} | {row['count']} |")
    else:
        lines.append("None recorded.")

    lines.extend(["", "### Top issues", ""])
    if summary["top_issues"]:
        for index, issue in enumerate(summary["top_issues"], start=1):
            areas = ", ".join(
                "(not recorded)" if area == "" else area for area in issue["areas"]
            )
            lines.append(
                f"{index}. **{issue['severity']}** - {areas} "
                f"(x{issue['count']}, {issue['reproducible_count']} reproducible): "
                f"{issue['description']}"
            )
    else:
        lines.append("None recorded.")

    lines.extend(["", "## Per-report detail", ""])
    for report in summary["reports"]:
        tester = report["tester"]
        lines.extend(
            [
                f"### {tester['name'] or 'not recorded'}",
                "",
                f"- Source: `{report['source']}`",
                f"- Captured: {report['captured_at']}",
                f"- Build / seed: {tester['build'] or '-'} / {tester['seed'] or '-'}",
                f"- Hardware / display: {tester['hardware'] or '-'} / "
                f"{tester['display'] or '-'}",
                f"- Stages: {_record_stages(report)}",
                f"- Elapsed: {format_seconds(report['elapsed_s'])}",
                f"- Status: {_report_status(report)}",
                f"- Warnings: {', '.join(report['warnings']) or 'none'}",
                f"- Notes: {report['notes'] or '-'}",
            ]
        )
        if report["issues"]:
            lines.append("- Issues:")
            for issue in report["issues"]:
                area = issue["area"] or "(not recorded)"
                repro = "reproducible" if issue["reproducible"] else "not reproducible"
                lines.append(
                    f"  - [{issue['severity']}] {area}: {issue['description']} "
                    f"({repro}, row {issue['row']})"
                )
        else:
            lines.append("- Issues: none recorded")
        if report["ignored_empty_issues"]:
            lines.append(
                f"- Ignored empty issue rows: {report['ignored_empty_issues']}"
            )
        lines.append("")

    lines.append(
        "Elapsed total: "
        f"{format_seconds(totals['elapsed_s'])} "
        f"(mean {format_seconds(totals['elapsed_mean_s'])}, "
        f"min {format_seconds(totals['elapsed_min_s'])}, "
        f"max {format_seconds(totals['elapsed_max_s'])})."
    )
    lines.append("")
    return "\n".join(lines)


def render_json(summary: dict[str, Any]) -> str:
    return json.dumps(summary, indent=2, sort_keys=True, ensure_ascii=False) + "\n"


def _write_output(path_text: str, text: str) -> None:
    path = Path(path_text)
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
    except OSError as error:
        detail = getattr(error, "strerror", None) or str(error)
        raise ReportError(str(path), f"cannot write output: {detail}") from error


# --- self-test ------------------------------------------------------------

STAGE_SPECS = (
    ("orient", "Orient", 0, 3),
    ("starter_build", "Starter build", 3, 8),
    ("house", "House", 8, 13),
    ("industry", "Industry", 13, 19),
    ("utility", "Utility", 19, 22),
    ("service", "Service", 22, 25),
    ("save_load", "Save/load", 25, 29),
    ("ratings", "Ratings", 29, 30),
)


def _fixture(
    *,
    name: str = "Smoke Tester",
    build: str = "smoke-build-822c509",
    seed: str = "2026",
    hardware: str = "headless",
    stages_complete: int = 8,
    elapsed_s: float = 120.0,
    issues: list[dict[str, Any]] | None = None,
    status: str | None = None,
    warnings: list[str] | None = None,
    schema: int | None = 1,
    format_version: int | None = None,
    captured_at: str = "2026-01-31T09:05:00Z",
    notes: str = "Deterministic fixture; no human findings.",
) -> dict[str, Any]:
    if warnings is None:
        warnings = []
        if not name.strip():
            warnings.append("tester_name_missing")
        if stages_complete == 0:
            warnings.append("no_completed_stage")
    if issues is None:
        issues = [
            {
                "row": 1,
                "area": "starter_build",
                "severity": "high",
                "description": "Placement ghost showed green over an occupied cell.",
                "reproducible": True,
            },
            {
                "row": 2,
                "area": "save_load",
                "severity": "medium",
                "description": "Load modal did not say what was restored.",
                "reproducible": False,
            },
        ]
    validation_complete = not warnings
    if status is None:
        status = "ok" if validation_complete else "partial"
    stages = []
    for index, (stage_id, label, start_min, end_min) in enumerate(STAGE_SPECS):
        complete = index < stages_complete
        stages.append(
            {
                "index": index + 1,
                "id": stage_id,
                "label": label,
                "target_start_min": start_min,
                "target_end_min": end_min,
                "complete": complete,
                "completed_at_s": round((index + 1) * 15.0, 3) if complete else None,
            }
        )
    payload: dict[str, Any] = {
        "suite": SUITE,
        "generated_by": "playtest_capture",
        "status": status,
        "captured_at": captured_at,
        "smoke": False,
        "engine_version": "4.7.stable",
        "route": {
            "id": ROUTE_ID,
            "title": "Q-101 30-minute vertical-slice route",
            "source": "docs/66 - Playtest Kit.md",
            "total_minutes": 30,
            "stages_total": len(STAGE_SPECS),
        },
        "tester": {
            "name": name,
            "experience": "some",
            "build": build,
            "seed": seed,
            "hardware": hardware,
            "display": "headless 1280x720",
            "fps_notes": "No hitches observed.",
        },
        "timer": {
            "started": True,
            "running": False,
            "elapsed_s": elapsed_s,
            "route_minutes": 30,
        },
        "stages_completed": stages_complete,
        "stages_total": len(STAGE_SPECS),
        "stages": stages,
        "issues": issues,
        "notes": notes,
        "validation": {
            "complete": validation_complete,
            "warnings": warnings,
            "policy": (
                "partial reports are written; an empty tester name or zero "
                "completed stages warns only"
            ),
        },
        "limits": "Observer tool only.",
    }
    if schema is not None:
        payload["schema"] = schema
    if format_version is not None:
        payload["format_version"] = format_version
    return payload


def _run_cli(argv: Sequence[str]) -> tuple[int, str, str]:
    out = io.StringIO()
    err = io.StringIO()
    with redirect_stdout(out), redirect_stderr(err):
        try:
            code = main(argv)
        except SystemExit as error:  # argparse errors are exit 2 as well
            code = int(error.code) if isinstance(error.code, int) else 2
    return code, out.getvalue(), err.getvalue()


def _last_marker(text: str) -> str:
    markers = [
        line for line in text.splitlines() if line.startswith(MARKER_PREFIX + " ")
    ]
    return markers[-1] if markers else ""


def run_self_test() -> int:
    failures: list[str] = []
    cases = 0

    def check(name: str, condition: bool, detail: str = "") -> None:
        if not condition:
            failures.append(f"{name}: {detail}" if detail else name)

    with tempfile.TemporaryDirectory(prefix="townsmith-playtest-summary-") as raw:
        directory = Path(raw)

        def write(name: str, payload: Any) -> Path:
            path = directory / name
            if isinstance(payload, str):
                path.write_text(payload, encoding="utf-8")
            else:
                path.write_text(json.dumps(payload), encoding="utf-8")
            return path

        # 1. Valid complete report, with both derived outputs.
        complete = write("complete.json", _fixture())
        markdown_path = directory / "out" / "complete.md"
        json_path = directory / "out" / "complete.json"
        code, out, err = _run_cli(
            [
                str(complete),
                "--markdown",
                str(markdown_path),
                "--json",
                str(json_path),
            ]
        )
        cases += 1
        check(
            "complete_exit",
            code == 0,
            f"exit {code}, stderr={err.strip()}",
        )
        expected_marker = (
            "PLAYTEST_SUMMARY reports=1 completed=1 issues=2 status=ok"
        )
        check(
            "complete_marker",
            _last_marker(out) == expected_marker,
            f"got {_last_marker(out)!r}",
        )
        check("complete_markdown", markdown_path.is_file(), "markdown not written")
        check("complete_json", json_path.is_file(), "json not written")
        if json_path.is_file():
            aggregate = json.loads(json_path.read_text(encoding="utf-8"))
            check(
                "complete_json_totals",
                aggregate["totals"]["reports"] == 1
                and aggregate["totals"]["completed"] == 1
                and aggregate["totals"]["issues"] == 2,
                str(aggregate.get("totals")),
            )
            check(
                "complete_json_marker",
                aggregate.get("marker") == expected_marker,
                str(aggregate.get("marker")),
            )
        if markdown_path.is_file():
            markdown = markdown_path.read_text(encoding="utf-8")
            check(
                "complete_markdown_content",
                "Q-101 playtest summary" in markdown
                and "Placement ghost showed green" in markdown,
                "missing aggregate content",
            )

        # 2. Valid complete report with no issues (all issue rows empty).
        no_issues = write(
            "complete_no_issues.json",
            _fixture(issues=[], name="No Issues"),
        )
        code, out, err = _run_cli([str(no_issues)])
        cases += 1
        check("no_issues_exit", code == 0, f"exit {code}, stderr={err.strip()}")
        check(
            "no_issues_marker",
            _last_marker(out)
            == "PLAYTEST_SUMMARY reports=1 completed=1 issues=0 status=ok",
            f"got {_last_marker(out)!r}",
        )

        # 3. Partial report: missing tester name and no completed stage.
        partial = write(
            "partial_warnings.json",
            _fixture(name="", stages_complete=0, elapsed_s=4.5),
        )
        code, out, err = _run_cli([str(partial)])
        cases += 1
        check("partial_exit", code == 0, f"exit {code}, stderr={err.strip()}")
        check(
            "partial_marker",
            _last_marker(out)
            == "PLAYTEST_SUMMARY reports=1 completed=0 issues=2 status=partial",
            f"got {_last_marker(out)!r}",
        )
        check(
            "partial_warnings_rendered",
            "tester_name_missing" in out and "no_completed_stage" in out,
            "warning codes missing from text output",
        )

        # 4. Validation passes but the route is incomplete: still partial.
        stages_only = write(
            "partial_stages.json",
            _fixture(name="Halfway", stages_complete=4, elapsed_s=900.0),
        )
        code, out, err = _run_cli([str(stages_only)])
        cases += 1
        check("partial_stages_exit", code == 0, f"exit {code}, stderr={err.strip()}")
        check(
            "partial_stages_marker",
            _last_marker(out)
            == "PLAYTEST_SUMMARY reports=1 completed=0 issues=2 status=partial",
            f"got {_last_marker(out)!r}",
        )

        # 5. Empty issue description is ignored, not counted.
        empty_row = write(
            "empty_issue_row.json",
            _fixture(
                name="Empty Row",
                issues=[
                    {
                        "row": 1,
                        "area": "orient",
                        "severity": "low",
                        "description": "   ",
                        "reproducible": False,
                    },
                    {
                        "row": 2,
                        "area": "industry",
                        "severity": "high",
                        "description": "Job modal listed lumberjack twice.",
                        "reproducible": True,
                    },
                ],
            ),
        )
        empty_json = directory / "out" / "empty_row.json"
        code, out, err = _run_cli([str(empty_row), "--json", str(empty_json)])
        cases += 1
        check("empty_row_exit", code == 0, f"exit {code}, stderr={err.strip()}")
        check(
            "empty_row_marker",
            _last_marker(out)
            == "PLAYTEST_SUMMARY reports=1 completed=1 issues=1 status=ok",
            f"got {_last_marker(out)!r}",
        )
        if empty_json.is_file():
            aggregate = json.loads(empty_json.read_text(encoding="utf-8"))
            check(
                "empty_row_ignored",
                aggregate["totals"]["ignored_empty_issues"] == 1,
                str(aggregate["totals"]),
            )

        # 6. Malformed JSON.
        malformed = write("malformed.json", "{")
        code, out, err = _run_cli([str(malformed)])
        cases += 1
        check("malformed_exit", code == 2, f"got exit {code}")
        check(
            "malformed_message",
            "invalid JSON" in err and str(malformed) in err,
            f"stderr={err.strip()!r}",
        )
        check("malformed_no_marker", _last_marker(out) == "", "marker printed on error")

        # 7. Missing required schema field.
        missing = _fixture()
        del missing["issues"]
        missing_path = write("missing_field.json", missing)
        code, out, err = _run_cli([str(missing_path)])
        cases += 1
        check("missing_field_exit", code == 2, f"got exit {code}")
        check(
            "missing_field_message",
            "issues" in err and "missing required field" in err,
            f"stderr={err.strip()!r}",
        )

        # 8. Unsupported schema version.
        new_schema = write("unsupported_schema.json", _fixture(schema=2))
        code, out, err = _run_cli([str(new_schema)])
        cases += 1
        check("unsupported_schema_exit", code == 2, f"got exit {code}")
        check(
            "unsupported_schema_message",
            "unsupported format_version 2" in err,
            f"stderr={err.strip()!r}",
        )

        # 9. Unsupported format_version alias (no schema field).
        new_alias = write(
            "unsupported_alias.json",
            _fixture(schema=None, format_version=99),
        )
        code, out, err = _run_cli([str(new_alias)])
        cases += 1
        check("unsupported_alias_exit", code == 2, f"got exit {code}")
        check(
            "unsupported_alias_message",
            "unsupported format_version 99" in err,
            f"stderr={err.strip()!r}",
        )

        # 10. Multiple reports: aggregate counts and partial status.
        second = write(
            "second.json",
            _fixture(
                name="Second Tester",
                build="other-build-1234",
                seed="7",
                hardware="RTX 3060",
                stages_complete=6,
                elapsed_s=1810.0,
                issues=[
                    {
                        "row": 1,
                        "area": "house",
                        "severity": "low",
                        "description": "Longhouse tooltip clipped.",
                        "reproducible": False,
                    }
                ],
            ),
        )
        code, out, err = _run_cli([str(complete), str(second)])
        cases += 1
        check("multiple_exit", code == 0, f"exit {code}, stderr={err.strip()}")
        check(
            "multiple_marker",
            _last_marker(out)
            == "PLAYTEST_SUMMARY reports=2 completed=1 issues=3 status=partial",
            f"got {_last_marker(out)!r}",
        )
        check(
            "multiple_testers",
            "Smoke Tester" in out and "Second Tester" in out,
            "tester rows missing",
        )
        check(
            "multiple_unique_builds",
            "smoke-build-822c509" in out and "other-build-1234" in out,
            "build aggregate missing",
        )

        # 11. Directory input expands sorted *.json files.
        directory_case = directory / "dir_case"
        directory_case.mkdir()
        (directory_case / "b.json").write_text(
            json.dumps(_fixture(name="B Tester", stages_complete=4)), encoding="utf-8"
        )
        (directory_case / "a.json").write_text(
            json.dumps(_fixture(name="A Tester")), encoding="utf-8"
        )
        (directory_case / "ignore.txt").write_text("not json", encoding="utf-8")
        code, out, err = _run_cli([str(directory_case)])
        cases += 1
        check("directory_exit", code == 0, f"exit {code}, stderr={err.strip()}")
        check(
            "directory_marker",
            _last_marker(out)
            == "PLAYTEST_SUMMARY reports=2 completed=1 issues=4 status=partial",
            f"got {_last_marker(out)!r}",
        )
        check(
            "directory_sorted",
            out.find("A Tester") < out.find("B Tester"),
            "directory reports were not summarized in sorted order",
        )

        # 12. Same input produces identical text and JSON output.
        first_code, first_out, _ = _run_cli([str(complete)])
        second_code, second_out, _ = _run_cli([str(complete)])
        cases += 1
        check(
            "deterministic_text",
            first_code == second_code == 0 and first_out == second_out,
            "text output changed between identical runs",
        )
        det_one = directory / "out" / "det_one.json"
        det_two = directory / "out" / "det_two.json"
        _run_cli([str(complete), "--json", str(det_one)])
        _run_cli([str(complete), "--json", str(det_two)])
        cases += 1
        check(
            "deterministic_json",
            det_one.is_file()
            and det_two.is_file()
            and det_one.read_text(encoding="utf-8")
            == det_two.read_text(encoding="utf-8"),
            "JSON output changed between identical runs",
        )

        # 13. Missing file and inconsistent counters are hard errors.
        code, _, err = _run_cli([str(directory / "does_not_exist.json")])
        cases += 1
        check("missing_file_exit", code == 2, f"got exit {code}")
        check(
            "missing_file_message",
            "report not found" in err,
            f"stderr={err.strip()!r}",
        )

        inconsistent = _fixture()
        inconsistent["stages_completed"] = 7
        inconsistent_path = write("inconsistent.json", inconsistent)
        code, _, err = _run_cli([str(inconsistent_path)])
        cases += 1
        check("inconsistent_exit", code == 2, f"got exit {code}")
        check(
            "inconsistent_message",
            "stages_completed=7" in err and "marked complete" in err,
            f"stderr={err.strip()!r}",
        )

    if failures:
        for failure in failures:
            print(f"{MARKER_PREFIX}_SELFTEST_FAIL {failure}")
        print(
            f"{MARKER_PREFIX}_SELFTEST cases={cases} failures={len(failures)} "
            "status=failed"
        )
        return 1

    print(f"{MARKER_PREFIX}_SELFTEST cases={cases} status=ok")
    return 0


# --- CLI ------------------------------------------------------------------


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Validate and summarize Q-101 playtest capture JSON reports from "
            "godot/playtest_capture.gd. Stdlib only; does not modify reports."
        ),
        epilog=(
            "Exit 0 = reports read (marker status is ok or partial); "
            "2 = malformed/unsupported input or unwritable output. "
            "Example: summarize_playtest.py playtest_reports/ "
            "--markdown playtest_reports/summary.md"
        ),
    )
    parser.add_argument(
        "reports",
        nargs="*",
        metavar="REPORT",
        help="capture JSON file or a directory of *.json reports (repeatable)",
    )
    parser.add_argument(
        "--markdown",
        metavar="PATH",
        help="write the aggregate as a Markdown document",
    )
    parser.add_argument(
        "--json",
        metavar="PATH",
        help="write the aggregate as deterministic JSON",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="run built-in complete/partial/malformed/version/multi-report fixtures",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)

    if args.self_test:
        return run_self_test()

    if not args.reports:
        parser.error("at least one REPORT (or a directory) is required, or --self-test")

    try:
        outputs = [Path(path) for path in (args.markdown, args.json) if path]
        reports = load_reports(args.reports, exclude=outputs)
        summary = summarize(reports)
        text = render_text(summary)
        if args.markdown:
            _write_output(args.markdown, render_markdown(summary))
        if args.json:
            _write_output(args.json, render_json(summary))
    except ReportError as error:
        print(f"{MARKER_PREFIX}_ERROR {error.label}: {error.message}", file=sys.stderr)
        return 2

    print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
