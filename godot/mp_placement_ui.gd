extends RefCounted

## Deterministic helpers for the opt-in Godot placement UI.
##
## This class owns the client-side catalog shape, entry normalization, rotated
## footprints, the ghost transform, and the scripted-action parser. It never
## touches the network, the server, or `TownsmithCore` statically: the caller
## hands over an already-instantiated bridge object, so a headless run without
## the extension still starts and fails with a stable marker instead of a parse
## error.
##
## Catalog source: `blueprint_library_entries(root_path)` returns the library
## summary (`count`, `total_bytes`, `checksum`), `entry_count`, `skipped`, and
## `entries[{hash, id, name, bytes, bounds_m, footprint_m}]` sorted by hash.
## Every entry is validated here; a malformed record fails the catalog instead
## of being silently trusted. Only the bridge `error_code` is surfaced, never
## the bridge message, because messages can contain local paths.
##
## Script grammar (one action per `;`-separated segment, empty segments ignored,
## maximum 256 actions):
##   select:<index>     non-negative entry index (wraps at playback)
##   move:<dx>,<dz>     relative integer metre offset, snapped to the 1 m grid
##   rotate:<deg>       multiple of 90, normalized to 0/90/180/270
##   place              send `place_blueprint` for the selected entry
##   cancel             leave placement mode and disconnect
## Unknown or malformed actions return a stable `error` code and no actions.

const BLUEPRINT_HASH_PREFIX := "blake3:"
const MAX_SCRIPT_ACTIONS := 256
const MAX_SCRIPT_INDEX := 4096
const MAX_MOVE_M := 1000000
const VALID_ROTATIONS := [0, 90, 180, 270]

## Canonicalize a content hash: `blake3:` + 64 lowercase hex, or bare 64
## lowercase hex. Anything else returns an empty string.
static func canonical_hash(value: String) -> String:
	var hex := value
	if hex.begins_with(BLUEPRINT_HASH_PREFIX):
		hex = hex.substr(BLUEPRINT_HASH_PREFIX.length())
	if hex.length() != 64:
		return ""
	for index in range(hex.length()):
		var character := hex[index]
		if not (
			(character >= "0" and character <= "9")
			or (character >= "a" and character <= "f")
		):
			return ""
	return BLUEPRINT_HASH_PREFIX + hex

## Normalize a rotation in degrees to 0/90/180/270, or -1 when it is not a
## multiple of 90.
static func canonical_rotation(rotation_deg: int) -> int:
	var rotation := rotation_deg % 360
	if rotation < 0:
		rotation += 360
	if not VALID_ROTATIONS.has(rotation):
		return -1
	return rotation

## Load and normalize the local catalog. `core` is the instantiated bridge; the
## caller owns it. The result is `{ok, error, entries, count, entry_count,
## total_bytes, checksum, skipped}` and carries no path or raw message.
static func load_catalog(core: Object, library_root: String) -> Dictionary:
	if core == null:
		return _catalog_error("core_missing")
	if library_root.strip_edges().is_empty():
		return _catalog_error("library_missing")
	if not core.has_method("blueprint_library_entries"):
		return _catalog_error("bridge_missing")
	var raw: Variant = core.call("blueprint_library_entries", library_root)
	if typeof(raw) != TYPE_DICTIONARY:
		return _catalog_error("catalog_not_dictionary")
	var catalog: Dictionary = raw
	if not bool(catalog.get("ok", false)):
		var code := str(catalog.get("error_code", "catalog_failed"))
		return _catalog_error(code if not code.is_empty() else "catalog_failed")
	var checksum := str(catalog.get("checksum", ""))
	if not _is_hex16(checksum):
		return _catalog_error("catalog_checksum")
	var count := int(catalog.get("count", -1))
	var total_bytes := int(catalog.get("total_bytes", -1))
	var skipped := int(catalog.get("skipped", -1))
	if count < 0 or total_bytes < 0 or skipped < 0:
		return _catalog_error("catalog_summary")
	var entries_raw: Variant = catalog.get("entries", null)
	if typeof(entries_raw) != TYPE_ARRAY:
		return _catalog_error("catalog_entries")
	var entries: Array = []
	for index in range(entries_raw.size()):
		var normalized := normalize_entry(entries_raw[index], index)
		if not bool(normalized.get("ok", false)):
			return normalized
		entries.append(normalized["entry"])
	if int(catalog.get("entry_count", -1)) != entries.size():
		return _catalog_error("catalog_entry_count")
	return {
		"ok": true,
		"error": "",
		"entries": entries,
		"count": count,
		"entry_count": entries.size(),
		"total_bytes": total_bytes,
		"checksum": checksum,
		"skipped": skipped,
	}

## Normalize one bridge entry into
## `{hash, id, name, bytes, bounds_m: Array, footprint_m: Array}`. `bounds_m` is
## `[x, y, z]` and `footprint_m` is `[x, z]`, both finite-positive metres.
static func normalize_entry(raw: Variant, index: int) -> Dictionary:
	if typeof(raw) != TYPE_DICTIONARY:
		return _entry_error(index, "type")
	var entry: Dictionary = raw
	var hash := canonical_hash(str(entry.get("hash", "")))
	if hash.is_empty():
		return _entry_error(index, "hash")
	var id := str(entry.get("id", ""))
	if id.is_empty() or id.contains(" ") or id.length() > 128:
		return _entry_error(index, "id")
	var name := str(entry.get("name", ""))
	if name.is_empty():
		name = id
	if name.length() > 128:
		return _entry_error(index, "name")
	var bytes := int(entry.get("bytes", -1))
	if bytes < 0:
		return _entry_error(index, "bytes")
	var bounds := _finite_array(entry.get("bounds_m", null), 3)
	if bounds.is_empty() or not _all_positive(bounds):
		return _entry_error(index, "bounds")
	var footprint := _finite_array(entry.get("footprint_m", null), 2)
	if footprint.is_empty() or not _all_positive(footprint):
		return _entry_error(index, "footprint")
	return {
		"ok": true,
		"error": "",
		"entry": {
			"hash": hash,
			"id": id,
			"name": name,
			"bytes": bytes,
			"bounds_m": [float(bounds[0]), float(bounds[1]), float(bounds[2])],
			"footprint_m": [float(footprint[0]), float(footprint[1])],
		},
	}

## Ground footprint of `entry` after a 0/90/180/270 rotation. Invalid rotations
## fall back to 0 so a caller bug cannot produce a half-rotated box.
static func rotated_footprint(entry: Dictionary, rotation_deg: int) -> Vector2:
	var footprint: Array = entry.get("footprint_m", [1.0, 1.0])
	var rotation := canonical_rotation(rotation_deg)
	if rotation < 0:
		rotation = 0
	if rotation == 90 or rotation == 270:
		return Vector2(float(footprint[1]), float(footprint[0]))
	return Vector2(float(footprint[0]), float(footprint[1]))

## Ghost transform for a unit `BoxMesh`: scaled by `bounds_m`, rotated about the
## world Y axis, and centred on `(center_x, center_z)` with its bottom at y=0.
## The CPU-side mirror the smoke asserts is exactly this value.
static func ghost_transform(
	entry: Dictionary, center_x: float, center_z: float, rotation_deg: int
) -> Transform3D:
	var bounds: Array = entry.get("bounds_m", [1.0, 1.0, 1.0])
	var size := Vector3(float(bounds[0]), float(bounds[1]), float(bounds[2]))
	var rotation := canonical_rotation(rotation_deg)
	if rotation < 0:
		rotation = 0
	var basis := (
		Basis(Vector3.UP, deg_to_rad(float(rotation)))
		* Basis.from_scale(size)
	)
	return Transform3D(
		basis,
		Vector3(center_x, size.y * 0.5, center_z)
	)

## Parse a deterministic action script. Returns `{ok, error, actions}` where
## each action is `{kind, index}` / `{kind, dx, dz}` / `{kind, rotation}` /
## `{kind}`. Errors name the segment index and a reason, never the text.
static func parse_script(text: String) -> Dictionary:
	var actions: Array = []
	var segments := text.split(";", true)
	for segment_index in range(segments.size()):
		var token := segments[segment_index].strip_edges()
		if token.is_empty():
			continue
		if actions.size() >= MAX_SCRIPT_ACTIONS:
			return _script_error(segment_index, "too_many_actions")
		var parsed := _parse_action(token)
		if not bool(parsed.get("ok", false)):
			return _script_error(segment_index, str(parsed.get("error", "invalid")))
		actions.append(parsed["action"])
	return {"ok": true, "error": "", "actions": actions}

static func _parse_action(token: String) -> Dictionary:
	if token == "place":
		return {"ok": true, "action": {"kind": "place"}}
	if token == "cancel":
		return {"ok": true, "action": {"kind": "cancel"}}
	if token.begins_with("select:"):
		var raw := token.substr("select:".length())
		if not raw.is_valid_int():
			return {"ok": false, "error": "select_value"}
		var index := raw.to_int()
		if index < 0 or index > MAX_SCRIPT_INDEX:
			return {"ok": false, "error": "select_range"}
		return {"ok": true, "action": {"kind": "select", "index": index}}
	if token.begins_with("move:"):
		var raw := token.substr("move:".length())
		var parts := raw.split(",", false)
		if parts.size() != 2:
			return {"ok": false, "error": "move_parts"}
		for part in parts:
			if not part.is_valid_int():
				return {"ok": false, "error": "move_value"}
			var value := part.to_int()
			if value < -MAX_MOVE_M or value > MAX_MOVE_M:
				return {"ok": false, "error": "move_range"}
		return {
			"ok": true,
			"action": {"kind": "move", "dx": parts[0].to_int(), "dz": parts[1].to_int()},
		}
	if token.begins_with("rotate:"):
		var raw := token.substr("rotate:".length())
		if not raw.is_valid_int():
			return {"ok": false, "error": "rotate_value"}
		var rotation := canonical_rotation(raw.to_int())
		if rotation < 0:
			return {"ok": false, "error": "rotate_multiple"}
		return {"ok": true, "action": {"kind": "rotate", "rotation": rotation}}
	return {"ok": false, "error": "unknown_action"}

static func _catalog_error(code: String) -> Dictionary:
	return {
		"ok": false,
		"error": code,
		"entries": [],
		"count": 0,
		"entry_count": 0,
		"total_bytes": 0,
		"checksum": "",
		"skipped": 0,
	}

static func _entry_error(index: int, field: String) -> Dictionary:
	return {"ok": false, "error": "entry_%d_%s" % [index, field], "entry": {}}

static func _script_error(index: int, reason: String) -> Dictionary:
	return {
		"ok": false,
		"error": "action_%d_%s" % [index, reason],
		"actions": [],
	}

static func _is_hex16(value: String) -> bool:
	if value.length() != 16:
		return false
	for index in range(value.length()):
		var character := value[index]
		if not (
			(character >= "0" and character <= "9")
			or (character >= "a" and character <= "f")
		):
			return false
	return true

static func _finite_array(value: Variant, size: int) -> Array:
	if typeof(value) != TYPE_ARRAY or value.size() != size:
		return []
	for item in value:
		if typeof(item) != TYPE_INT and typeof(item) != TYPE_FLOAT:
			return []
		if not is_finite(float(item)):
			return []
	return value

static func _all_positive(values: Array) -> bool:
	for value in values:
		if float(value) <= 0.0:
			return false
	return true
