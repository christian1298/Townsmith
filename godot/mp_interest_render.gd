extends RefCounted

## Pure geometry and checksum helpers for the multiplayer interest view.
##
## The client smoke renders the server's bounded `interest_view` in a flat
## top-down `Control`. This file holds the parts that do not need a scene tree:
## parsing/limiting the road list, fitting the viewport circle into a panel,
## projecting metre coordinates to pixels, and a deterministic 16-hex checksum
## over road ids and endpoints.
##
## It is intentionally not a renderer: no world streaming, terrain, camera, or
## entity data is involved. `godot/mp_interest_canvas.gd` consumes these values.
##
## See docs/05 - Multiplayer.md §Protocol v0.

## Wire bound from `crates/server/src/protocol.rs::MAX_INTEREST_ROADS`.
const MAX_ROADS := 64
const DEFAULT_WIDTH_M := 6.0
const DEFAULT_MARGIN_PX := 12.0
const MIN_LINE_WIDTH_PX := 1.0
const MAX_LINE_WIDTH_PX := 32.0
const FNV_OFFSET_BASIS := 0x811C9DC5
const FNV_PRIME := 16777619
const EPSILON_M := 0.001

## Parse and bound the `interest_view.roads` payload.
##
## Returns deterministic dictionaries `{id, start: Vector2, end: Vector2,
## width_m}` sorted by id then endpoints. Malformed entries are skipped, and the
## result is capped at [`MAX_ROADS`] even if a future server or mod sends more.
static func parse_roads(raw: Variant) -> Array:
	var roads: Array = []
	if typeof(raw) != TYPE_ARRAY:
		return roads
	for entry in raw:
		if roads.size() >= MAX_ROADS:
			break
		if typeof(entry) != TYPE_DICTIONARY:
			continue
		var start: Variant = entry.get("start", null)
		var end: Variant = entry.get("end", null)
		if not is_world_pair(start) or not is_world_pair(end):
			continue
		var width_m := DEFAULT_WIDTH_M
		var width_raw: Variant = entry.get("width_m", DEFAULT_WIDTH_M)
		if typeof(width_raw) == TYPE_INT or typeof(width_raw) == TYPE_FLOAT:
			var candidate := float(width_raw)
			if is_finite(candidate) and candidate > 0.0:
				width_m = candidate
		roads.append(
			{
				"id": int(entry.get("id", 0)),
				"start": Vector2(float(start[0]), float(start[1])),
				"end": Vector2(float(end[0]), float(end[1])),
				"width_m": width_m,
			}
		)
	roads.sort_custom(
		func(a: Dictionary, b: Dictionary) -> bool:
			var a_id := int(a.get("id", 0))
			var b_id := int(b.get("id", 0))
			if a_id != b_id:
				return a_id < b_id
			var a_start: Vector2 = a.get("start", Vector2.ZERO)
			var b_start: Vector2 = b.get("start", Vector2.ZERO)
			if a_start.x != b_start.x:
				return a_start.x < b_start.x
			if a_start.y != b_start.y:
				return a_start.y < b_start.y
			var a_end: Vector2 = a.get("end", Vector2.ZERO)
			var b_end: Vector2 = b.get("end", Vector2.ZERO)
			if a_end.x != b_end.x:
				return a_end.x < b_end.x
			return a_end.y < b_end.y
	)
	return roads

## Accept a `[x, z]` pair of finite numbers, matching the wire format.
static func is_world_pair(value: Variant) -> bool:
	if typeof(value) != TYPE_ARRAY:
		return false
	if value.size() != 2:
		return false
	for index in range(2):
		var component: Variant = value[index]
		if typeof(component) != TYPE_INT and typeof(component) != TYPE_FLOAT:
			return false
		if not is_finite(float(component)):
			return false
	return true

## Pixel radius of the fitted interest circle inside `size`.
##
## The circle always fits in the smaller panel axis; the caller can draw it
## directly and use [`pixels_per_metre`] for road widths.
static func circle_radius_px(radius_m: float, size: Vector2, margin: float = DEFAULT_MARGIN_PX) -> float:
	var usable := maxf(minf(size.x, size.y) - margin * 2.0, 1.0)
	return usable * 0.5

## Pixels per world metre for a viewport of `radius_m` fitted into `size`.
static func pixels_per_metre(
	radius_m: float, size: Vector2, margin: float = DEFAULT_MARGIN_PX
) -> float:
	var fitted_radius := circle_radius_px(radius_m, size, margin)
	return fitted_radius / maxf(radius_m, EPSILON_M)

## Project a world `(x, z)` point to panel pixels. `+z` points down.
static func project_point(
	point: Vector2, center: Vector2, radius_m: float, size: Vector2, margin: float = DEFAULT_MARGIN_PX
) -> Vector2:
	return project_with_scale(point, center, size, pixels_per_metre(radius_m, size, margin))

static func project_with_scale(point: Vector2, center: Vector2, size: Vector2, scale: float) -> Vector2:
	return size * 0.5 + (point - center) * scale

## Build draw-ready line segments for every road in the bounded list.
##
## Each entry is `{start: Vector2, end: Vector2, width_px: float}`. The width is
## the server's `width_m` scaled into the panel and clamped so a distant or
## very narrow road stays visible and a wide one cannot swamp the panel.
static func segments(
	roads: Array,
	center: Vector2,
	radius_m: float,
	size: Vector2,
	margin: float = DEFAULT_MARGIN_PX
) -> Array:
	var scale := pixels_per_metre(radius_m, size, margin)
	var result: Array = []
	for road in roads:
		var start: Vector2 = road.get("start", Vector2.ZERO)
		var end: Vector2 = road.get("end", Vector2.ZERO)
		var width_m := float(road.get("width_m", DEFAULT_WIDTH_M))
		result.append(
			{
				"start": project_with_scale(start, center, size, scale),
				"end": project_with_scale(end, center, size, scale),
				"width_px":
				clampf(width_m * scale, MIN_LINE_WIDTH_PX, MAX_LINE_WIDTH_PX),
			}
		)
	return result

## Deterministic 16-hex checksum over the bounded road ids and endpoints.
##
## Two independent 32-bit FNV-1a lanes are concatenated, so the result is
## stable across platforms and processes and independent of JSON key order.
static func checksum(roads: Array) -> String:
	var low := FNV_OFFSET_BASIS
	var high := FNV_OFFSET_BASIS ^ FNV_PRIME
	low = _mix_low(low, roads.size())
	high = _mix_high(high, roads.size())
	for road in roads:
		for field in _road_fields(road):
			low = _mix_low(low, field)
			high = _mix_high(high, field)
	return "%08x%08x" % [low & 0xFFFFFFFF, high & 0xFFFFFFFF]

static func _road_fields(road: Dictionary) -> Array:
	var id := int(road.get("id", 0))
	var start: Vector2 = road.get("start", Vector2.ZERO)
	var end: Vector2 = road.get("end", Vector2.ZERO)
	return [
		(id >> 32) & 0xFFFFFFFF,
		id & 0xFFFFFFFF,
		int(start.x) & 0xFFFFFFFF,
		int(start.y) & 0xFFFFFFFF,
		int(end.x) & 0xFFFFFFFF,
		int(end.y) & 0xFFFFFFFF,
	]

static func _mix_low(state: int, value: int) -> int:
	return ((state ^ (value & 0xFFFFFFFF)) * FNV_PRIME) & 0xFFFFFFFF

static func _mix_high(state: int, value: int) -> int:
	return ((state + (value & 0xFFFFFFFF) + 1) * FNV_PRIME) & 0xFFFFFFFF
