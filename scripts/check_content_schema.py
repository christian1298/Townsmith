#!/usr/bin/env python3
"""Validate the Townsmith v1 content schemas, base data, and mod packs.

The runtime registry loads the base documents, the manifest v0 validator
checks manifest structure, IDs, versions, and safe relative paths, and the
in-memory content pack validates one manifest with its materials, rooms, and
props. This gate keeps those data contracts structurally valid, namespaced,
and cross-reference clean. It reads the example pack by directory convention;
it does not resolve arbitrary manifest content paths on disk.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
from pathlib import Path

ID_RE = re.compile(r"^[a-z0-9_]+:[a-z0-9_./-]+$")
MOD_ID_RE = re.compile(r"^[a-z0-9_]+(:[a-z0-9_]+)?$")
PATH_COMPONENT_RE = re.compile(r"^[A-Za-z0-9_-][A-Za-z0-9._-]*$")
TAG_RE = re.compile(r"^[a-z0-9_./-]+$")
MATERIAL_CATEGORIES = {"wood", "masonry", "concrete", "metal", "glass", "earth", "other"}
ROOM_CATEGORIES = {"living", "service", "work", "circulation", "storage", "other"}
SCHEMA_FILES = ("material.schema.json", "room.schema.json", "prop.schema.json", "mod.schema.json")


class Checker:
    def __init__(self) -> None:
        self.errors: list[str] = []
        self.ids: dict[str, str] = {}
        self.room_ids: set[str] = set()
        self.mod_ids: dict[str, str] = {}

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


def semver_like(value: str) -> bool:
    """Mirror the Rust manifest's semver-like check."""
    indexes = [index for index in (value.find("-"), value.find("+")) if index != -1]
    split = min(indexes) if indexes else -1
    core = value if split == -1 else value[:split]
    suffix = None if split == -1 else value[split + 1:]
    parts = core.split(".")
    if len(parts) != 3 or any(not part.isascii() or not part.isdigit() for part in parts):
        return False
    if suffix is None:
        return True
    return (
        bool(suffix)
        and not suffix.startswith(".")
        and not suffix.endswith(".")
        and ".." not in suffix
        and all(character.isascii() and (character.isalnum() or character in ".-+") for character in suffix)
    )


def content_path_problem(value: object) -> str | None:
    """Return a short reason when a manifest content path is unsafe."""
    if not isinstance(value, str) or not value:
        return "must be a non-empty string"
    if value.startswith("/"):
        return "must be relative"
    if "\\" in value:
        return "must use '/' separators"
    if ":" in value:
        return "must not contain ':'"
    if not value.endswith(".json"):
        return "must end with .json"
    for component in value.split("/"):
        if not component:
            return "must not contain empty components"
        if component in (".", ".."):
            return "must not contain '.' or '..' components"
        if not PATH_COMPONENT_RE.match(component):
            return "must use ASCII letters, digits, '_', '-', and '.'"
    return None


def check_manifest(checker: Checker, path: Path, doc: dict) -> int:
    where = str(path)
    required = ["format_version", "id", "name", "version", "game_version", "content"]
    checker.object(where, doc, required)
    allowed = {
        "format_version",
        "id",
        "name",
        "version",
        "game_version",
        "description",
        "content",
        "dependencies",
        "draft",
    }
    for key in sorted(set(doc) - allowed):
        checker.error(where, f"unexpected field '{key}'")
    if doc.get("format_version") != 1:
        checker.error(where, "format_version must be 1")

    ident = doc.get("id")
    if not isinstance(ident, str) or not MOD_ID_RE.match(ident):
        checker.error(where, f"id must be namespaced (got {ident!r})")
    elif ident in checker.mod_ids:
        checker.error(where, f"duplicate mod id '{ident}' (already in {checker.mod_ids[ident]})")
    else:
        checker.mod_ids[ident] = where

    checker.string(where, doc.get("name"), "name")
    version = doc.get("version")
    if not isinstance(version, str) or not semver_like(version):
        checker.error(where, f"version must be semver-like (got {version!r})")
    checker.string(where, doc.get("game_version"), "game_version")

    if "description" in doc:
        checker.string(where, doc.get("description"), "description")

    content = doc.get("content")
    if not isinstance(content, dict):
        checker.error(where, "content must be an object")
    else:
        for key in sorted(set(content) - {"materials", "rooms", "props"}):
            checker.error(where, f"unexpected content field '{key}'")
        for field in ("materials", "rooms", "props"):
            if field not in content:
                checker.error(where, f"content.{field} is required")
                continue
            problem = content_path_problem(content[field])
            if problem:
                checker.error(where, f"content.{field} path {content[field]!r} {problem}")

    dependencies = doc.get("dependencies", [])
    if not isinstance(dependencies, list):
        checker.error(where, "dependencies must be an array")
    else:
        seen: set[str] = set()
        for dependency in dependencies:
            if not isinstance(dependency, str) or not MOD_ID_RE.match(dependency):
                checker.error(where, f"dependency {dependency!r} is not a valid mod id")
            elif dependency in seen:
                checker.error(where, f"dependency '{dependency}' is duplicated")
            else:
                seen.add(dependency)

    if "draft" in doc and not isinstance(doc["draft"], bool):
        checker.error(where, "draft must be a boolean")
    return 1


def check_pack_namespace(checker: Checker, path: Path, doc: dict, collection: str, namespace: str) -> None:
    """Require every well-formed ID in a pack document to use the manifest namespace."""
    entries = doc.get(collection)
    if not isinstance(entries, list):
        return
    for index, entry in enumerate(entries):
        if not isinstance(entry, dict):
            continue
        ident = entry.get("id")
        if isinstance(ident, str) and ID_RE.match(ident) and ident.split(":", 1)[0] != namespace:
            checker.error(
                f"{path} {collection}[{index}]",
                f"id '{ident}' is outside the mod namespace '{namespace}'",
            )


def check_mod_content(checker: Checker, manifest_path: Path, manifest_doc: dict) -> int:
    """Validate the example pack directory that sits next to a manifest.

    Content is discovered by convention: `example.mod.json` maps to
    `example/{materials,rooms,props}.json`. Props may only reference rooms
    defined by the same pack, matching `ContentPack::from_json`.
    """
    ident = manifest_doc.get("id")
    if not isinstance(ident, str) or not MOD_ID_RE.match(ident):
        return 0
    namespace = ident.split(":", 1)[0]
    pack_dir = manifest_path.with_name(manifest_path.name.removesuffix(".mod.json"))
    total = 0
    pack_room_ids: set[str] = set()
    for collection in ("materials", "rooms", "props"):
        path = pack_dir / f"{collection}.json"
        if not path.is_file():
            checker.error(str(path), "missing mod content file")
            continue
        doc = load_json(checker, path)
        if doc is None:
            continue
        if collection == "materials":
            total += check_materials(checker, path, doc)
        elif collection == "rooms":
            total += check_rooms(checker, path, doc)
            pack_room_ids = {
                entry["id"]
                for entry in doc.get("rooms", [])
                if isinstance(entry, dict)
                and isinstance(entry.get("id"), str)
                and ID_RE.match(entry["id"])
            }
        else:
            total += check_props(checker, path, doc, room_ids=pack_room_ids)
        check_pack_namespace(checker, path, doc, collection, namespace)
    return total


def check_props(checker: Checker, path: Path, doc: dict, *, room_ids: set[str] | None = None) -> int:
    entries = check_header(checker, path, doc, "props")
    if entries is None:
        return 0
    known_rooms = checker.room_ids if room_ids is None else room_ids
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
            if room not in known_rooms:
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
    mod_count = 0
    mods_dir = root / "content" / "mods"
    if not mods_dir.is_dir():
        checker.error(str(mods_dir), "missing mods directory")
    else:
        for manifest_path in sorted(mods_dir.glob("*.mod.json")):
            manifest = load_json(checker, manifest_path)
            if manifest is not None:
                mod_count += check_manifest(checker, manifest_path, manifest)
                mod_count += check_mod_content(checker, manifest_path, manifest)
    return checker.errors, schema_count, material_count + room_count + prop_count + mod_count


def write_fixture(
    root: Path,
    *,
    duplicate: bool = False,
    bad_room_ref: bool = False,
    bad_density: bool = False,
    bad_version: bool = False,
    bad_manifest_id: bool = False,
    bad_manifest_version: bool = False,
    unsafe_manifest_path: bool = False,
    duplicate_manifest_dep: bool = False,
    mod_wrong_namespace: bool = False,
    mod_duplicate_with_base: bool = False,
    mod_missing_content: bool = False,
    mod_bad_room_ref: bool = False,
) -> None:
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

    manifest_id = "Example:Bad" if bad_manifest_id else ("base:example" if mod_duplicate_with_base else "example:copper_age")
    manifest = {
        "format_version": 1,
        "id": manifest_id,
        "name": "Copper Age example",
        "version": "1.0" if bad_manifest_version else "0.1.0",
        "game_version": ">=0.1.0 <0.2.0",
        "content": {
            "materials": "../materials.json" if unsafe_manifest_path else "content/materials.json",
            "rooms": "content/rooms.json",
            "props": "content/props.json",
        },
        "dependencies": ["example:core", "example:core"] if duplicate_manifest_dep else [],
        "draft": True,
    }
    (root / "content" / "mods").mkdir()
    (root / "content" / "mods" / "example.mod.json").write_text(json.dumps(manifest))

    pack_namespace = "base" if mod_duplicate_with_base else "example"
    material_id = f"{pack_namespace}:oak" if mod_duplicate_with_base else (
        "other:plank" if mod_wrong_namespace else f"{pack_namespace}:plank"
    )
    room_id = f"{pack_namespace}:room"
    prop_id = f"{pack_namespace}:prop"
    pack = root / "content" / "mods" / "example"
    pack.mkdir()
    pack_materials = {"format_version": 1, "materials": [
        {"id": material_id, "name_key": "content.material.plank", "category": "wood", "density_kg_m3": 700, "color": [0.5, 0.4, 0.2], "cost_per_litre": 0.1, "tags": ["timber"]},
    ]}
    pack_rooms = {"format_version": 1, "rooms": [
        {"id": room_id, "name_key": "content.room.test", "category": "living", "min_area_m2": 9, "required_tags": ["bed"]},
    ]}
    pack_props = {"format_version": 1, "props": [
        {"id": prop_id, "name_key": "content.prop.test", "footprint_m": [1.6, 2.0], "height_m": 0.6, "cost": 10, "allowed_rooms": ["base:bedroom" if mod_bad_room_ref else room_id], "tags": ["sleep"]},
    ]}
    (pack / "materials.json").write_text(json.dumps(pack_materials))
    (pack / "rooms.json").write_text(json.dumps(pack_rooms))
    if not mod_missing_content:
        (pack / "props.json").write_text(json.dumps(pack_props))


def self_test() -> int:
    cases = 0
    with tempfile.TemporaryDirectory(prefix="townsmith_content_") as tmp:
        root = Path(tmp)
        write_fixture(root)
        errors, schemas, entries = validate_root(root)
        cases += 1
        if errors or schemas != 4 or entries != 7:
            print(f"self-test valid fixture failed: {errors}", file=sys.stderr)
            return 1
        for name, kwargs in (
            ("duplicate", {"duplicate": True}),
            ("bad_room_ref", {"bad_room_ref": True}),
            ("bad_density", {"bad_density": True}),
            ("bad_version", {"bad_version": True}),
            ("bad_manifest_id", {"bad_manifest_id": True}),
            ("bad_manifest_version", {"bad_manifest_version": True}),
            ("unsafe_manifest_path", {"unsafe_manifest_path": True}),
            ("duplicate_manifest_dep", {"duplicate_manifest_dep": True}),
            ("mod_wrong_namespace", {"mod_wrong_namespace": True}),
            ("mod_duplicate_with_base", {"mod_duplicate_with_base": True}),
            ("mod_missing_content", {"mod_missing_content": True}),
            ("mod_bad_room_ref", {"mod_bad_room_ref": True}),
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
    if errors:
        print(f"CONTENT_SCHEMA files={schemas} entries={entries} errors={len(errors)} status=failed")
        return 1
    print(f"CONTENT_SCHEMA files={schemas} entries={entries} status=ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
