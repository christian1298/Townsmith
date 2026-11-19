extends Node3D

## Renders a deterministic synthetic city as one `MultiMeshInstance3D`.
##
## The Rust core generates the layout; this node only uploads it. There is one
## instance per box building, one `MultiMesh`, and one ground plane. No
## per-building nodes are created, so draw calls stay constant while the
## building count changes by orders of magnitude across tiers.
##
## The instance buffer is written directly through `MultiMesh.buffer` instead of
## calling `set_instance_transform` / `set_instance_color` per building. That is
## the bulk-upload path and keeps a 100,000-building build in the tens of
## milliseconds instead of hundreds of thousands of scripted property calls.
##
## The renderer is intentionally cheap for Phase 0: opaque vertex-colored
## material, no shadows, no GI, no LOD. It answers "how much does the box city
## cost on this machine", not "what will the final voxel city cost".

const TIER_NAMES := ["early", "mid", "late"]
const DEFAULT_SEED := 2026
const FLOATS_PER_INSTANCE := 16  # 12 transform + 4 color
const GROUND_COLOR := Color(0.26, 0.30, 0.26)
const GROUND_MARGIN := 2.0

var multimesh: MultiMesh
var multimesh_instance: MultiMeshInstance3D
var ground: MeshInstance3D
var _stats: Dictionary = {}
var _built := false

## Build (or rebuild) the city for `tier` (0 = Early, 1 = Mid, 2 = Late).
##
## `seed` is passed straight to `TownsmithCore.build_city_layout`. The layout is
## deterministic for a given tier, seed, and core binary.
func build(tier: int, seed: int) -> void:
	_clear()
	var started_usec := Time.get_ticks_usec()
	var core := TownsmithCore.new()
	var layout: Dictionary = core.build_city_layout(tier, seed)
	var layout_done_usec := Time.get_ticks_usec()

	if layout.has("error"):
		var message := str(layout.get("error", "unknown city layout error"))
		_stats = {
			"tier": _tier_name(tier),
			"seed": seed,
			"error": message,
			"build_ms": _ms_between(started_usec, layout_done_usec),
		}
		push_error("City layout failed for tier %s: %s" % [_tier_name(tier), message])
		return

	var positions: PackedVector3Array = layout.get("positions", PackedVector3Array())
	var sizes: PackedVector3Array = layout.get("sizes", PackedVector3Array())
	var colors: PackedColorArray = layout.get("colors", PackedColorArray())
	var count := positions.size()
	if sizes.size() != count or colors.size() != count:
		_stats = {
			"tier": str(layout.get("tier", _tier_name(tier))),
			"seed": seed,
			"error": "layout arrays disagree: positions=%d sizes=%d colors=%d" % [
				count, sizes.size(), colors.size()],
			"build_ms": _ms_between(started_usec, layout_done_usec),
		}
		push_error("City layout arrays disagree for tier %s: %s" % [
			_stats.get("tier", "?"), _stats["error"]])
		return

	# Keep the city's own bounds; they are authoritative and avoid a second
	# min/max pass over the instance data.
	var bounds: PackedFloat32Array = layout.get("bounds", PackedFloat32Array())
	var min_x := 0.0
	var min_z := 0.0
	var max_x := 0.0
	var max_z := 0.0
	if bounds.size() >= 4:
		min_x = bounds[0]
		min_z = bounds[1]
		max_x = bounds[2]
		max_z = bounds[3]

	var upload_started_usec := Time.get_ticks_usec()
	var data := PackedFloat32Array()
	data.resize(count * FLOATS_PER_INSTANCE)
	var max_height := 0.0
	for index in count:
		var position := positions[index]
		var size := sizes[index]
		var color := colors[index]
		var offset := index * FLOATS_PER_INSTANCE
		# Row-major 3x4 transform: unit X, Y, Z axes scaled to the box extent,
		# then the box center. The BoxMesh is centered on its origin, so the
		# resulting box spans position.y +/- size.y/2.
		data[offset + 0] = size.x
		data[offset + 1] = 0.0
		data[offset + 2] = 0.0
		data[offset + 3] = position.x
		data[offset + 4] = 0.0
		data[offset + 5] = size.y
		data[offset + 6] = 0.0
		data[offset + 7] = position.y
		data[offset + 8] = 0.0
		data[offset + 9] = 0.0
		data[offset + 10] = size.z
		data[offset + 11] = position.z
		data[offset + 12] = color.r
		data[offset + 13] = color.g
		data[offset + 14] = color.b
		data[offset + 15] = 1.0
		if position.y + size.y * 0.5 > max_height:
			max_height = position.y + size.y * 0.5

	var box := BoxMesh.new()
	box.size = Vector3.ONE

	multimesh = MultiMesh.new()
	multimesh.mesh = box
	# transform_format and use_colors must be set before instance_count: Godot
	# rejects both after instances exist.
	multimesh.transform_format = MultiMesh.TRANSFORM_3D
	multimesh.use_colors = true
	multimesh.instance_count = count
	multimesh.buffer = data
	var width := maxf(max_x - min_x, 0.0)
	var depth := maxf(max_z - min_z, 0.0)
	multimesh.custom_aabb = AABB(
		Vector3(min_x, 0.0, min_z),
		Vector3(width, maxf(max_height, 1.0), depth))

	multimesh_instance = MultiMeshInstance3D.new()
	multimesh_instance.name = "CityBuildings"
	multimesh_instance.multimesh = multimesh
	multimesh_instance.material_override = _building_material()
	multimesh_instance.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	add_child(multimesh_instance)

	ground = _create_ground(maxf(width, depth), (min_x + max_x) * 0.5, (min_z + max_z) * 0.5)
	add_child(ground)

	var finished_usec := Time.get_ticks_usec()
	_built = true
	_stats = {
		"tier": str(layout.get("tier", _tier_name(tier))),
		"seed": int(layout.get("seed", seed)),
		"population": int(layout.get("population", 0)),
		"buildings": int(layout.get("buildings", count)),
		"instances": count,
		"bounds": [min_x, min_z, max_x, max_z],
		"extent_m": maxf(width, depth),
		"max_height_m": max_height,
		"ground_size_m": _ground_size(maxf(width, depth)),
		"buffer_bytes": data.size() * 4,
		"layout_ms": _ms_between(started_usec, layout_done_usec),
		"buffer_ms": _ms_between(upload_started_usec, finished_usec),
		"build_ms": _ms_between(started_usec, finished_usec),
	}

## Benchmark metadata. The required keys are `tier`, `population`, `buildings`,
## and `bounds` as `[min_x, min_z, max_x, max_z]`. Timing keys are informational
## and differ between headless and a real renderer.
func stats() -> Dictionary:
	return _stats.duplicate(true)

func is_built() -> bool:
	return _built

## Longest city edge in meters, or 0 before a successful build.
func extent_m() -> float:
	return float(_stats.get("extent_m", 0.0))

func building_count() -> int:
	return int(_stats.get("buildings", 0))

func _building_material() -> StandardMaterial3D:
	var material := StandardMaterial3D.new()
	material.vertex_color_use_as_albedo = true
	# Core city colors are linear RGB, so do not apply another sRGB conversion.
	material.vertex_color_is_srgb = false
	material.roughness = 1.0
	material.metallic = 0.0
	material.transparency = BaseMaterial3D.TRANSPARENCY_DISABLED
	return material

func _create_ground(extent: float, center_x: float, center_z: float) -> MeshInstance3D:
	var plane := PlaneMesh.new()
	var size := _ground_size(extent)
	plane.size = Vector2(size, size)
	var material := StandardMaterial3D.new()
	material.albedo_color = GROUND_COLOR
	material.roughness = 1.0
	material.metallic = 0.0
	material.transparency = BaseMaterial3D.TRANSPARENCY_DISABLED
	var instance := MeshInstance3D.new()
	instance.name = "CityGround"
	instance.mesh = plane
	instance.material_override = material
	instance.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	instance.position = Vector3(center_x, -0.05, center_z)
	return instance

func _ground_size(extent: float) -> float:
	return maxf(extent * GROUND_MARGIN, 128.0)

func _clear() -> void:
	if multimesh_instance != null:
		multimesh_instance.queue_free()
		multimesh_instance = null
	if ground != null:
		ground.queue_free()
		ground = null
	multimesh = null
	_stats = {}
	_built = false

func _tier_name(tier: int) -> String:
	if tier >= 0 and tier < TIER_NAMES.size():
		return TIER_NAMES[tier]
	return "late"

func _ms_between(start_usec: int, end_usec: int) -> float:
	return float(end_usec - start_usec) / 1000.0
