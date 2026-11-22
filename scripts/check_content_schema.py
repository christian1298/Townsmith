#!/usr/bin/env python3
"""Validate the Townsmith v1 material/room/prop content schemas and base data.

The runtime registry and mod loader are not implemented yet. This gate keeps
the first data files structurally valid, namespaced, and cross-reference clean
so a later registry starts from a known contract.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
from pathlib import Path

ID_RE = re.compile(r"^[a-z0-9_]+:[a-z0-9_./-]+$")
TAG_RE = re.compile(r"^[a-z0-9_./-]+$")
MATERIAL_CATEGORIES = {"wood", "masonry", "concrete", "metal", "glass", "earth", "other"}
ROOM_CATEGORIES = {"living", "service", "work", "circulation", "storage", "other"}
SCHEMA_FILES = ("material.schema.json", "room.schema.json", "prop.schema.json")


class Checker:
    def __init__(self) -> None:
        self.errors: list[str] = []
        self.ids: dict[str, str] = {}
        self.room_ids: set[str] = set()

    def error(self, where: str, message: str) -> None:
        self.errors.append(f"{where}: {message}")

    def object(self, where: str, value: object, required: list[str]) -> dict | None:
        if not isinstance(value, dict):
            self.error(where, "expected an object")
            return None
        for key in required:
            if key not in value:
                self.error(where, f"missing required field '{key}'")
        return value

    def ident(self, where: str, value: object) -> str | None:
        if not isinstance(value, str) or not ID_RE.match(value):
            self.error(where, f"id must be namespaced (got {value!r})")
            return None
        if value in self.ids:
            self.error(where, f"duplicate id '{value}' (already in {self.ids[value]})")
            return None
        self.ids[value] = where
        return value

    def string(self, where: str, value: object, field: str) -> None:
        if not isinstance(value, str) or not value.strip():
            self.error(where, f"{field} must be a non-empty string")

    def number(self, where: str, value: object, field: str, *, minimum: float = 0.0, exclusive: bool = False) -> None:
        if not isinstance(value, (int, float)) or isinstance(value, bool):
            self.error(where, f"{field} must be a number")
            return
        if exclusive and value <= minimum:
            self.error(where, f"{field} must be > {minimum}")
        if not exclusive and value < minimum:
            self.error(where, f"{field} must be >= {minimum}")

    def tags(self, where: str, value: object, field: str) -> None:
        if not isinstance(value, list):
            self.error(where, f"{field} must be an array")
            return
        seen: set[str] = set()
        for tag in value:
            if not isinstance(tag, str) or not TAG_RE.match(tag):
                self.error(where, f"{field} tag {tag!r} is invalid")
            elif tag in seen:
                self.error(where, f"{field} tag '{tag}' is duplicated")
            seen.add(tag)


def load_json(checker: Checker, path: Path) -> dict | None:
    try:
        value = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        checker.error(str(path), f"cannot read JSON: {error}")
        return None
    if not isinstance(value, dict):
        checker.error(str(path), "top level must be an object")
        return None
    return value


def check_header(checker: Checker, path: Path, doc: dict, collection: str) -> list | None:
    if doc.get("format_version") != 1:
        checker.error(str(path), "format_version must be 1")
    entries = doc.get(collection)
    if not isinstance(entries, list) or not entries:
        checker.error(str(path), f"{collection} must be a non-empty array")
        return None
    unexpected = set(doc) - {"format_version", collection}
    for key in sorted(unexpected):
        checker.error(str(path), f"unexpected top-level field '{key}'")
    return entries


def check_materials(checker: Checker, path: Path, doc: dict) -> int:
    entries = check_header(checker, path, doc, "materials")
    if entries is None:
        return 0
    allowed = {"id", "name_key", "category", "density_kg_m3", "color", "cost_per_litre", "tags", "draft"}
    for index, raw in enumerate(entries):
        where = f"{path} materials[{index}]"
        entry = checker.object(where, raw, ["id", "name_key", "category", "density_kg_m3", "color", "cost_per_litre", "tags"])
        if entry is None:
            continue
        for key in sorted(set(entry) - allowed):
            checker.error(where, f"unexpected field '{key}'")
        checker.ident(where, entry.get("id"))
        checker.string(where, entry.get("name_key"), "name_key")
        if entry.get("category") not in MATERIAL_CATEGORIES:
            checker.error(where, f"category must be one of {sorted(MATERIAL_CATEGORIES)}")
        checker.number(where, entry.get("density_kg_m3"), "density_kg_m3", exclusive=True)
        checker.number(where, entry.get("cost_per_litre"), "cost_per_litre")
        color = entry.get("color")
        if not isinstance(color, list) or len(color) != 3:
            checker.error(where, "color must be [r, g, b]")
        else:
            for channel in color:
                checker.number(where, channel, "color channel")
                if isinstance(channel, (int, float)) and not 0 <= channel <= 1:
                    checker.error(where, "color channels must be within 0..=1")
        checker.tags(where, entry.get("tags"), "tags")
        if "draft" in entry and not isinstance(entry["draft"], bool):
            checker.error(where, "draft must be a boolean")
    return len(entries)


def check_rooms(checker: Checker, path: Path, doc: dict) -> int:
    entries = check_header(checker, path, doc, "rooms")
    if entries is None:
        return 0
    allowed = {"id", "name_key", "category", "min_area_m2", "required_tags", "draft"}
    for index, raw in enumerate(entries):
        where = f"{path} rooms[{index}]"
        entry = checker.object(where, raw, ["id", "name_key", "category", "min_area_m2", "required_tags"])
        if entry is None:
            continue
        for key in sorted(set(entry) - allowed):
            checker.error(where, f"unexpected field '{key}'")
        ident = checker.ident(where, entry.get("id"))
        if ident:
            checker.room_ids.add(ident)
        checker.string(where, entry.get("name_key"), "name_key")
        if entry.get("category") not in ROOM_CATEGORIES:
            checker.error(where, f"category must be one of {sorted(ROOM_CATEGORIES)}")
        checker.number(where, entry.get("min_area_m2"), "min_area_m2", exclusive=True)
        checker.tags(where, entry.get("required_tags"), "required_tags")
        if "draft" in entry and not isinstance(entry["draft"], bool):
            checker.error(where, "draft must be a boolean")
    return len(entries)


def check_props(checker: Checker, path: Path, doc: dict) -> int:
    entries = check_header(checker, path, doc, "props")
    if entries is None:
        return 0
    allowed = {"id", "name_key", "footprint_m", "height_m", "cost", "allowed_rooms", "tags", "draft"}
    pending: list[tuple[str, list[str]]] = []
    for index, raw in enumerate(entries):
        where = f"{path} props[{index}]"
        entry = checker.object(where, raw, ["id", "name_key", "footprint_m", "height_m", "cost", "allowed_rooms", "tags"])
        if entry is None:
            continue
        for key in sorted(set(entry) - allowed):
            checker.error(where, f"unexpected field '{key}'")
        checker.ident(where, entry.get("id"))
        checker.string(where, entry.get("name_key"), "name_key")
        footprint = entry.get("footprint_m")
        if not isinstance(footprint, list) or len(footprint) != 2:
            checker.error(where, "footprint_m must be [width, depth]")
        else:
            for value in footprint:
                checker.number(where, value, "footprint_m", exclusive=True)
        checker.number(where, entry.get("height_m"), "height_m", exclusive=True)
        checker.number(where, entry.get("cost"), "cost")
        rooms = entry.get("allowed_rooms")
        if not isinstance(rooms, list):
            checker.error(where, "allowed_rooms must be an array")
            rooms = []
        for room in rooms:
            if not isinstance(room, str) or not ID_RE.match(room):
                checker.error(where, f"allowed_rooms entry {room!r} is not a namespaced id")
        pending.append((where, [room for room in rooms if isinstance(room, str)]))
        checker.tags(where, entry.get("tags"), "tags")
        if "draft" in entry and not isinstance(entry["draft"], bool):
            checker.error(where, "draft must be a boolean")
    for where, rooms in pending:
        for room in rooms:
            if room not in checker.room_ids:
                checker.error(where, f"allowed_rooms references unknown room '{room}'")
    return len(entries)


def validate_root(root: Path) -> tuple[list[str], int, int]:
    checker = Checker()
    schema_dir = root / "content" / "schema"
    schema_count = 0
    for name in SCHEMA_FILES:
        path = schema_dir / name
        if not path.is_file():
            checker.error(str(path), "missing schema file")
            continue
        schema = load_json(checker, path)
        if schema is None:
            continue
        schema_count += 1
        if not str(schema.get("$schema", "")).startswith("https://json-schema.org/draft/2020-12/"):
            checker.error(str(path), "expected a draft 2020-12 $schema")
        if not isinstance(schema.get("$defs"), dict):
            checker.error(str(path), "expected $defs")
    material_count = 0
    material = load_json(checker, root / "content/base/materials.json")
    if material is not None:
        material_count = check_materials(checker, root / "content/base/materials.json", material)
    room_count = 0
    room = load_json(checker, root / "content/base/rooms.json")
    if room is not None:
        room_count = check_rooms(checker, root / "content/base/rooms.json", room)
    prop_count = 0
    prop = load_json(checker, root / "content/base/props.json")
    if prop is not None:
        prop_count = check_props(checker, root / "content/base/props.json", prop)
    return checker.errors, schema_count, material_count + room_count + prop_count


def write_fixture(root: Path, *, duplicate: bool = False, bad_room_ref: bool = False, bad_density: bool = False, bad_version: bool = False) -> None:
    schema = root / "content" / "schema"
    schema.mkdir(parents=True)
    for name in SCHEMA_FILES:
        (schema / name).write_text(json.dumps({"$schema": "https://json-schema.org/draft/2020-12/schema", "$defs": {}}))
    material = {"format_version": 1 if not bad_version else 2, "materials": [
        {"id": "base:oak", "name_key": "content.material.oak", "category": "wood", "density_kg_m3": -1 if bad_density else 700, "color": [0.5, 0.4, 0.2], "cost_per_litre": 0.1, "tags": ["timber"]},
    ]}
    if duplicate:
        material["materials"].append(dict(material["materials"][0]))
    room = {"format_version": 1, "rooms": [
        {"id": "base:bedroom", "name_key": "content.room.bedroom", "category": "living", "min_area_m2": 9, "required_tags": ["bed"]},
    ]}
    prop = {"format_version": 1, "props": [
        {"id": "base:bed", "name_key": "content.prop.bed", "footprint_m": [1.6, 2.0], "height_m": 0.6, "cost": 10, "allowed_rooms": ["base:missing" if bad_room_ref else "base:bedroom"], "tags": ["sleep"]},
    ]}
    (root / "content" / "base").mkdir()
    (root / "content" / "base" / "materials.json").write_text(json.dumps(material))
    (root / "content" / "base" / "rooms.json").write_text(json.dumps(room))
    (root / "content" / "base" / "props.json").write_text(json.dumps(prop))


def self_test() -> int:
    cases = 0
    with tempfile.TemporaryDirectory(prefix="townsmith_content_") as tmp:
        root = Path(tmp)
        write_fixture(root)
        errors, schemas, entries = validate_root(root)
        cases += 1
        if errors or schemas != 3 or entries != 3:
            print(f"self-test valid fixture failed: {errors}", file=sys.stderr)
            return 1
        for name, kwargs in (
            ("duplicate", {"duplicate": True}),
            ("bad_room_ref", {"bad_room_ref": True}),
            ("bad_density", {"bad_density": True}),
            ("bad_version", {"bad_version": True}),
        ):
            case_root = root / name
            write_fixture(case_root, **kwargs)
            errors, _, _ = validate_root(case_root)
            cases += 1
            if not errors:
                print(f"self-test {name} fixture unexpectedly passed", file=sys.stderr)
                return 1
    print(f"CONTENT_SCHEMA_SELFTEST cases={cases} status=ok")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    errors, schemas, entries = validate_root(args.root)
    for error in errors:
        print(f"CONTENT_SCHEMA_ERROR {error}", file=sys.stderr)
    status = "ok" if not errors else "failed"
    print(f"CONTENT_SCHEMA files={schemas} entries={entries} errors={len(errors)} status={status}")
    return 0 if not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
