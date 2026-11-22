#!/usr/bin/env python3
"""Pseudo-localization gate for the three runtime EN/DE locale tables.

Reads the same GDScript tables as ``scripts/check_locale.sh``:

- ``TABLES`` in ``godot/editor_i18n.gd`` (editor)
- ``TABLES`` in ``godot/world_i18n.gd`` (world)
- ``DISPLAY`` in ``godot/content_i18n.gd`` (content)

For every English key it generates a pseudo-localized string:

- placeholders (``{0}``, ``{name}``) stay byte-identical;
- vowels are accented (``a -> á``, ``e -> é``, ``i -> í``, ``o -> ó``,
  ``u -> ú``, ``y -> ý``, upper case too);
- the result is wrapped in the visible marker ``« »`` and padded with ``·``
  to 130% of the English length;
- a pseudo string longer than ``--max-length`` fails unless its key is in the
  explicit ``LONG_ALLOWLIST`` below.

It also re-asserts the structural checks a German review depends on: EN/DE
key parity (both directions), no duplicate keys, no empty values, and
placeholder parity. The pseudo locale is not a translation; it is a
structural/layout proxy only.

Usage:
    scripts/check_pseudo_locale.py
    scripts/check_pseudo_locale.py --max-length 140
    scripts/check_pseudo_locale.py --report build/pseudo_locale.json
    scripts/check_pseudo_locale.py --self-test

Success prints one summary line and a JSON report is written only when
``--report PATH`` is passed:

    PSEUDO_LOCALE editor=111 world=211 content=34 long=10 status=ok

``status=failed`` exits 1 and prints the issues on stderr. Bad arguments exit
2. ``--self-test`` writes no repository files and checks the generator and the
audit failure paths.
"""

import argparse
import json
import math
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, Iterable, List, Optional, Sequence, Set, Tuple

ROOT = Path(__file__).resolve().parent.parent

# Same basename-based floors live in scripts/check_locale.sh. This gate does
# not enforce floors; it reuses the file list and reports the current counts.
TABLES: Tuple[Tuple[str, Path, str], ...] = (
    ("editor", ROOT / "godot/editor_i18n.gd", "TABLES"),
    ("world", ROOT / "godot/world_i18n.gd", "TABLES"),
    ("content", ROOT / "godot/content_i18n.gd", "DISPLAY"),
)

DEFAULT_MAX_LENGTH = 120
MARK_START = "«"
MARK_STOP = "»"
FILLER = "·"
PLACEHOLDER_RE = re.compile(r"\{[A-Za-z0-9_]+\}")

# Accents only vowels; placeholders are protected before this table is used.
ACCENTS = str.maketrans(
    {
        "a": "á",
        "e": "é",
        "i": "í",
        "o": "ó",
        "u": "ú",
        "y": "ý",
        "A": "Á",
        "E": "É",
        "I": "Í",
        "O": "Ó",
        "U": "Ú",
        "Y": "Ý",
    }
)

# Deliberately long composite/help lines. These are one-line control cheat
# sheets or summaries with many parameters where the German layout budget
# already expects wrapping. Everything else must stay within --max-length so
# new prose does not silently outgrow the HUD/panels.
LONG_ALLOWLIST: Dict[str, str] = {
    "base.editor.help.tools": "control cheat sheet, one dense line by design",
    "base.ui.hud.structure_valid": "composite structure summary with seven parameters",
    "base.ui.hud.selected": "composite selection summary with five parameters",
    "base.ui.hud.placements": "composite counter row with six counters",
    "base.ui.hud.city": "composite city summary",
    "base.ui.hud.construction": "composite construction summary",
    "base.ui.hud.civic_value": "composite civic summary",
    "base.ui.modal.saved": "save summary with six counts and a path",
    "base.ui.modal.architect_view": "explanatory hint for the read-only view",
    "base.ui.dashboard.body.civic": "composite civic body line for the dashboard",
    "base.ui.help.tools": "control cheat sheet, one dense line by design",
}


class ParseError(Exception):
    """The table does not match the contract shared with check_locale.sh."""


@dataclass
class TableResult:
    name: str
    file: str
    en: int = 0
    de: int = 0
    long: int = 0
    long_keys: List[str] = field(default_factory=list)
    issues: List[str] = field(default_factory=list)

    @property
    def failed(self) -> bool:
        return bool(self.issues)


def parse_locale(text: str, marker: str, locale: str) -> List[Tuple[str, str]]:
    """Parse one locale block with the same line-oriented contract as the shell gate.

    The parser is intentionally strict: two-tab entries, ``"key": "value",``
    separator, comments/blank lines skipped, and the block closes at a single
    tab + ``},``. Values are kept raw (``\\n`` stays two characters) so the
    Python and awk parsers see exactly the same bytes.
    """
    lines = text.splitlines()
    header = "const %s := {" % marker
    try:
        index = lines.index(header)
    except ValueError:
        raise ParseError("missing const %s := {" % marker)

    index += 1
    locale_header = '\t"%s": {' % locale
    while index < len(lines) and lines[index] != locale_header:
        index += 1
    if index >= len(lines):
        raise ParseError("missing '%s' locale block" % locale)
    index += 1

    rows: List[Tuple[str, str]] = []
    while index < len(lines):
        line = lines[index]
        line_number = index + 1
        if line == "\t},":
            return rows
        if line.lstrip().startswith("#") or not line.strip():
            index += 1
            continue
        if not line.startswith('\t\t"'):
            raise ParseError(
                "unexpected line in locale block (line %d: %s)" % (line_number, line)
            )
        rest = line[3:]
        separator = rest.find('": "')
        if separator < 0:
            raise ParseError(
                "missing key/value separator (line %d: %s)" % (line_number, line)
            )
        key = rest[:separator]
        value = rest[separator + 4 :]
        if value.endswith('",'):
            value = value[:-2]
        elif value.endswith('"'):
            value = value[:-1]
        rows.append((key, value))
        index += 1

    raise ParseError("unterminated '%s' locale block" % locale)


def split_placeholders(text: str) -> List[str]:
    return PLACEHOLDER_RE.findall(text)


def accent_segments(text: str) -> str:
    """Accent vowels outside placeholders, keeping every placeholder intact."""
    pieces: List[str] = []
    last = 0
    for match in PLACEHOLDER_RE.finditer(text):
        pieces.append(text[last : match.start()].translate(ACCENTS))
        pieces.append(match.group(0))
        last = match.end()
    pieces.append(text[last:].translate(ACCENTS))
    return "".join(pieces)


def pseudo(text: str) -> str:
    """Return the deterministic pseudo-localized form of an English string."""
    body = accent_segments(text)
    target = max(len(text) + len(MARK_START) + len(MARK_STOP), math.ceil(len(text) * 1.3))
    pad = target - len(body) - len(MARK_START) - len(MARK_STOP)
    if pad > 0:
        body += FILLER * pad
    return MARK_START + body + MARK_STOP


def key_set(rows: Iterable[Tuple[str, str]]) -> Set[str]:
    return {key for key, _ in rows}


def value_map(rows: Iterable[Tuple[str, str]]) -> Dict[str, str]:
    return {key: value for key, value in rows}


def audit_table(
    name: str,
    file: Path,
    en_rows: List[Tuple[str, str]],
    de_rows: List[Tuple[str, str]],
    en_parsed: bool,
    de_parsed: bool,
    max_length: int,
    allowlist: Dict[str, str],
) -> TableResult:
    result = TableResult(name=name, file=str(file.relative_to(ROOT)))
    result.en = len(en_rows)
    result.de = len(de_rows)

    for locale, rows, parsed in (("en", en_rows, en_parsed), ("de", de_rows, de_parsed)):
        if not parsed:
            continue
        seen: Dict[str, int] = {}
        for key, value in rows:
            seen[key] = seen.get(key, 0) + 1
            if not value.strip():
                result.issues.append("empty value: %s %s" % (locale, key))
        for key in sorted(seen):
            if seen[key] > 1:
                result.issues.append("duplicate key in %s: %s" % (locale, key))

    if en_parsed and de_parsed:
        en_keys = key_set(en_rows)
        de_keys = key_set(de_rows)
        missing_de = sorted(en_keys - de_keys)
        missing_en = sorted(de_keys - en_keys)
        if missing_de or missing_en:
            result.issues.append("EN and DE key sets differ")
            for key in missing_de:
                result.issues.append("  missing in DE: %s" % key)
            for key in missing_en:
                result.issues.append("  missing in EN: %s" % key)

        en_values = value_map(en_rows)
        de_values = value_map(de_rows)
        for key in sorted(en_values):
            if key not in de_values:
                continue
            # Empty values are reported above; do not double-report them.
            if not en_values[key].strip() or not de_values[key].strip():
                continue
            en_placeholders = sorted(set(split_placeholders(en_values[key])))
            de_placeholders = sorted(set(split_placeholders(de_values[key])))
            if en_placeholders != de_placeholders:
                result.issues.append(
                    "placeholder mismatch: %s en=(%s) de=(%s)"
                    % (key, ",".join(en_placeholders), ",".join(de_placeholders))
                )

    if en_parsed:
        long_keys: Set[str] = set()
        for key, value in en_rows:
            generated = pseudo(value)
            if not generated.startswith(MARK_START) or not generated.endswith(MARK_STOP):
                result.issues.append("pseudo marker missing: %s" % key)
            if split_placeholders(generated) != split_placeholders(value):
                result.issues.append(
                    "pseudo placeholder mismatch: %s en=(%s) pseudo=(%s)"
                    % (
                        key,
                        ",".join(split_placeholders(value)),
                        ",".join(split_placeholders(generated)),
                    )
                )
            if len(generated) > max_length:
                long_keys.add(key)
                if key not in allowlist:
                    result.issues.append(
                        "pseudo too long: %s len=%d max=%d"
                        % (key, len(generated), max_length)
                    )
        result.long = len(long_keys)
        ordered: List[str] = []
        seen_long: Set[str] = set()
        for key, _ in en_rows:
            if key in long_keys and key not in seen_long:
                seen_long.add(key)
                ordered.append(key)
        result.long_keys = ordered

    return result


def load_tables(
    max_length: int, allowlist: Dict[str, str]
) -> Tuple[List[TableResult], List[str]]:
    results: List[TableResult] = []
    all_en_keys: Set[str] = set()
    parsed_any = False

    for name, path, marker in TABLES:
        en_rows: List[Tuple[str, str]] = []
        de_rows: List[Tuple[str, str]] = []
        en_parsed = False
        de_parsed = False

        if not path.is_file():
            result = TableResult(name=name, file=str(path.relative_to(ROOT)))
            result.issues.append("file not found: %s" % path)
            results.append(result)
            continue

        text = path.read_text(encoding="utf-8")
        try:
            en_rows = parse_locale(text, marker, "en")
            en_parsed = True
        except ParseError as error:
            result = TableResult(name=name, file=str(path.relative_to(ROOT)))
            result.issues.append(
                "could not parse the 'en' table in %s: %s" % (path, error)
            )
            results.append(result)
            continue
        try:
            de_rows = parse_locale(text, marker, "de")
            de_parsed = True
        except ParseError as error:
            de_rows = []
            de_parsed = False
            parse_error: Optional[str] = str(error)
        else:
            parse_error = None

        result = audit_table(
            name,
            path,
            en_rows,
            de_rows,
            en_parsed,
            de_parsed,
            max_length,
            allowlist,
        )
        if parse_error is not None:
            result.issues.append(
                "could not parse the 'de' table in %s: %s" % (path, parse_error)
            )
        results.append(result)
        all_en_keys.update(key_set(en_rows))
        parsed_any = True

    unknown = sorted(set(allowlist) - all_en_keys) if parsed_any else []
    issues = [issue for result in results for issue in result.issues]
    if unknown:
        for key in unknown:
            issues.append("allowlist key not found in EN: %s" % key)

    return results, issues


def build_report(
    results: Sequence[TableResult],
    issues: Sequence[str],
    max_length: int,
) -> Dict[str, object]:
    counts = {result.name: result.en for result in results}
    counts["long"] = sum(result.long for result in results)
    return {
        "status": "ok" if not issues else "failed",
        "max_length": max_length,
        "marker": MARK_START + MARK_STOP,
        "counts": counts,
        "allowlist": [
            {"key": key, "reason": LONG_ALLOWLIST[key]} for key in sorted(LONG_ALLOWLIST)
        ],
        "long_keys": sorted({key for result in results for key in result.long_keys}),
        "tables": {
            result.name: {
                "file": result.file,
                "en": result.en,
                "de": result.de,
                "long": result.long,
                "status": "failed" if result.failed else "ok",
            }
            for result in results
        },
        "issues": list(issues),
    }


def write_report(path: Path, report: Dict[str, object]) -> None:
    path.write_text(
        json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )


def self_test() -> int:
    cases = 0

    def check(condition: bool, case: str, reason: str) -> None:
        nonlocal cases
        cases += 1
        if not condition:
            print(
                "PSEUDO_SELF_TEST case=%s status=fail reason=%s" % (case, reason),
                file=sys.stderr,
            )
            raise SystemExit(1)

    def has_issue(result: TableResult, fragment: str) -> bool:
        return any(fragment in issue for issue in result.issues)

    # 1. Deterministic accenting and marker wrapping.
    check(pseudo("Undo") == "«Úndó»", "accent", "unexpected pseudo form")
    check(
        pseudo("Language") == "«Lángúágé·»",
        "accent_long",
        "unexpected accented form",
    )

    # 2. Placeholders survive byte-identical and in order.
    source = "Hello {name}, {0} left"
    generated = pseudo(source)
    check("{name}" in generated and "{0}" in generated, "placeholder", "placeholder lost")
    check(
        split_placeholders(generated) == ["{name}", "{0}"],
        "placeholder_order",
        "placeholder order changed",
    )
    check(
        generated.count("{") == 2 and generated.count("}") == 2,
        "placeholder_count",
        "placeholder count changed",
    )

    # 3. Target length is 130% (markers are included; short strings use +2).
    for sample in ("Hi", "No roads", "Place a voxel first", "x" * 200):
        target = max(len(sample) + 2, math.ceil(len(sample) * 1.3))
        check(
            len(pseudo(sample)) == target,
            "length",
            "length mismatch for %r" % sample,
        )

    # 4. Empty values and duplicates fail.
    result = audit_table(
        "fixture",
        ROOT / "fixture.gd",
        [("a", ""), ("dup", "one"), ("dup", "two")],
        [("a", "eins"), ("dup", "eins"), ("dup", "zwei")],
        True,
        True,
        DEFAULT_MAX_LENGTH,
        {},
    )
    check(
        has_issue(result, "empty value: en a"),
        "empty_value",
        "empty value not reported",
    )
    check(
        has_issue(result, "duplicate key in en: dup"),
        "duplicate_key",
        "duplicate key not reported",
    )

    # 5. Missing DE keys and placeholder mismatches fail.
    result = audit_table(
        "fixture",
        ROOT / "fixture.gd",
        [("a", "Hello {0}"), ("b", "Only EN")],
        [("a", "Hallo {1}"), ("c", "Only DE")],
        True,
        True,
        DEFAULT_MAX_LENGTH,
        {},
    )
    check(
        has_issue(result, "  missing in DE: b"),
        "missing_de",
        "missing DE key not reported",
    )
    check(
        has_issue(result, "  missing in EN: c"),
        "missing_en",
        "extra DE key not reported",
    )
    check(
        has_issue(result, "placeholder mismatch: a"),
        "placeholder_mismatch",
        "placeholder mismatch not reported",
    )

    # 6. Over-length strings fail unless explicitly allowlisted.
    long_text = "word " * 30
    result = audit_table(
        "fixture",
        ROOT / "fixture.gd",
        [("known", long_text), ("unknown", long_text)],
        [("known", long_text), ("unknown", long_text)],
        True,
        True,
        40,
        {"known": "fixture"},
    )
    check(
        has_issue(result, "pseudo too long: unknown"),
        "long_rejected",
        "over-length key not reported",
    )
    check(
        not has_issue(result, "pseudo too long: known"),
        "long_allowlisted",
        "allowlisted key still rejected",
    )
    check(result.long == 2, "long_count", "long count wrong")

    print("PSEUDO_SELF_TEST cases=%d status=ok" % cases)
    return 0


def main(argv: Optional[Sequence[str]] = None) -> int:
    parser = argparse.ArgumentParser(
        description="Pseudo-localization gate for the runtime EN/DE locale tables."
    )
    parser.add_argument(
        "--max-length",
        type=int,
        default=DEFAULT_MAX_LENGTH,
        metavar="N",
        help="maximum pseudo-localized string length in characters (default: %d)"
        % DEFAULT_MAX_LENGTH,
    )
    parser.add_argument(
        "--report",
        metavar="PATH",
        help="write a deterministic JSON report to PATH",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="check the generator and audit failure paths without touching the repo",
    )
    args = parser.parse_args(argv)

    if args.max_length <= 0:
        parser.error("--max-length must be positive")
    if args.self_test:
        return self_test()

    results, issues = load_tables(args.max_length, LONG_ALLOWLIST)
    counts = {result.name: result.en for result in results}
    long_count = sum(result.long for result in results)
    summary = "PSEUDO_LOCALE editor=%d world=%d content=%d long=%d status=%s" % (
        counts.get("editor", 0),
        counts.get("world", 0),
        counts.get("content", 0),
        long_count,
        "ok" if not issues else "failed",
    )
    print(summary)

    if issues:
        print("PSEUDO_LOCALE status=failed", file=sys.stderr)
        for issue in issues:
            print("  " + issue, file=sys.stderr)

    if args.report:
        report_path = Path(args.report)
        try:
            write_report(report_path, build_report(results, issues, args.max_length))
        except OSError as error:
            print("PSEUDO_LOCALE report error: %s" % error, file=sys.stderr)
            return 2

    return 0 if not issues else 1


if __name__ == "__main__":
    sys.exit(main())
