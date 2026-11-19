extends Node3D

## Renders the deterministic Phase 0 greedy-meshed hut as instanced geometry.
##
## `TownsmithCore.build_demo_mesh()` runs once. The resulting `ArrayMesh` is
## assigned to a single `MultiMesh`, and `count` copies are placed on a
## deterministic 15 x 14 grid. The instance buffer is written through
## `MultiMesh.buffer` in bulk (12 floats per instance: one row-major 3x4
## transform, no color), not through per-instance `set_instance_transform`
## calls.
##
## This is the first benchmark that renders real voxel-derived geometry instead
## of boxes. It is still intentionally cheap: one opaque material, no shadows,
## no GI, no LOD. It answers "what do a few hundred greedy-meshed huts cost on
## this machine", not "what will the final city cost".

const GRID_COLUMNS := 15
const GRID_ROWS := 14
const GRID_CAPACITY := GRID_COLUMNS * GRID_ROWS
const SPACING_M := 10.0
const DEFAULT_COUNT := 200
const DEFAULT_SEED := 2026
const FLOATS_PER_INSTANCE := 12  # 12 transform, no color
const GROUND_COLOR := Color(0.26, 0.30, 0.26)
const GROUND_SIZE_FACTOR := 1.5
const MIN_GROUND_SIZE_M := 128.0

var multimesh: MultiMesh
var multimesh_instance: MultiMeshInstance3D
var ground: MeshInstance3D
var _stats: Dictionary = {}
var _built := false

## Build (or rebuild) `count` hut instances with a deterministic layout.
##
## `seed` only varies per-instance yaw quarter turns; the grid, spacing, and
## geometry are identical for every seed. Counts above the 210-cell grid are
## rejected rather than silently wrapped or stacked.
func build(count: int = DEFAULT_COUNT, seed: int = DEFAULT_SEED) -> void:
	_clear()
	var started_usec := Time.get_ticks_usec()
	if count < 1 or count > GRID_CAPACITY:
		_stats = {
			"error": "count_out_of_range: %d (expected 1..%d)" % [count, GRID_CAPACITY],
			"count": count,
			"seed": seed,
			"build_ms": 0.0,
		}
		push_error("Voxel instance count must be 1..%d (got %d)" % [GRID_CAPACITY, count])
		return

	var core := TownsmithCore.new()
	var mesh: ArrayMesh = core.build_demo_mesh()
	var mesh_done_usec := Time.get_ticks_usec()
	var surfaces := mesh.get_surface_count()
	if surfaces < 1:
		_stats = {
			"error": "demo_mesh_empty",
			"count": count,
			"seed": seed,
			"surfaces": surfaces,
			"build_ms": _ms_between(started_usec, mesh_done_usec),
		}
		push_error("Phase 0 demo mesh has no surfaces")
		return

	# The hut mesh is not centered on the origin (it spans voxels 4..28), so
	# subtract its XZ center to put each instance's footprint around its grid
	# cell. The Y origin stays at the mesh floor so huts sit on the ground.
	var mesh_aabb := mesh.get_aabb()
	var mesh_center := mesh_aabb.get_center()
	var half_width := mesh_aabb.size.x * 0.5
	var half_depth := mesh_aabb.size.z * 0.5
	var height := mesh_aabb.size.y
	var x0 := -float(GRID_COLUMNS - 1) * SPACING_M * 0.5
	var z0 := -float(GRID_ROWS - 1) * SPACING_M * 0.5
	# Instances are translated by -mesh_center on X/Z, and quarter-turn yaw can
	# swap width and depth. Use the larger half-span and the actual instance
	# centers so the AABB covers the whole grid (an undersized AABB can cull
	# instances at the edges).
	var first_x := x0 - mesh_center.x
	var last_x := x0 + float(GRID_COLUMNS - 1) * SPACING_M - mesh_center.x
	var first_z := z0 - mesh_center.z
	var last_z := z0 + float(GRID_ROWS - 1) * SPACING_M - mesh_center.z
	var half_span := maxf(half_width, half_depth)
	var min_x := minf(first_x, last_x) - half_span
	var max_x := maxf(first_x, last_x) + half_span
	var min_z := minf(first_z, last_z) - half_span
	var max_z := maxf(first_z, last_z) + half_span

	var upload_started_usec := Time.get_ticks_usec()
	var data := PackedFloat32Array()
	data.resize(count * FLOATS_PER_INSTANCE)
	for index in count:
		var column := index % GRID_COLUMNS
		var row := index / GRID_COLUMNS
		var x := x0 + float(column) * SPACING_M
		var z := z0 + float(row) * SPACING_M
		# Quarter-turn yaw from a pure integer expression: stable across Godot
		# versions and independent of the global RNG state.
		var angle := float(posmod(seed + index * 7, 4)) * PI * 0.5
		var c := cos(angle)
		var s := sin(angle)
		var offset := index * FLOATS_PER_INSTANCE
		# Row-major 3x4 transform. The basis columns are the rotated unit axes.
		data[offset + 0] = c
		data[offset + 1] = 0.0
		data[offset + 2] = -s
		data[offset + 3] = x - mesh_center.x
		data[offset + 4] = 0.0
		data[offset + 5] = 1.0
		data[offset + 6] = 0.0
		data[offset + 7] = 0.0
		data[offset + 8] = s
		data[offset + 9] = 0.0
		data[offset + 10] = c
		data[offset + 11] = z - mesh_center.z

	multimesh = MultiMesh.new()
	multimesh.mesh = mesh
	# transform_format and use_colors must be set before instance_count: Godot
	# rejects both after instances exist.
	multimesh.transform_format = MultiMesh.TRANSFORM_3D
	multimesh.use_colors = false
	multimesh.instance_count = count
	# Set the AABB before the bulk upload so Godot skips its own instance scan.
	# The mesh floor is at y = 0, so the AABB starts there.
	multimesh.custom_aabb = AABB(
		Vector3(min_x, mesh_aabb.position.y, min_z),
		Vector3(max_x - min_x, maxf(height, 1.0), max_z - min_z))
	multimesh.buffer = data

	multimesh_instance = MultiMeshInstance3D.new()
	multimesh_instance.name = "VoxelHuts"
	multimesh_instance.multimesh = multimesh
	multimesh_instance.material_override = _hut_material()
	multimesh_instance.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	add_child(multimesh_instance)

	var extent := maxf(max_x - min_x, max_z - min_z)
	ground = _create_ground(extent)
	add_child(ground)

	var finished_usec := Time.get_ticks_usec()
	_built = true
	_stats = {
		"count": count,
		"instances": count,
		"seed": seed,
		"surfaces": surfaces,
		"grid": [GRID_COLUMNS, GRID_ROWS],
		"spacing_m": SPACING_M,
		"bounds": [min_x, min_z, max_x, max_z],
		"extent_m": extent,
		"height_m": height,
		"mesh_aabb": [
			mesh_aabb.position.x, mesh_aabb.position.y, mesh_aabb.position.z,
			mesh_aabb.size.x, mesh_aabb.size.y, mesh_aabb.size.z,
		],
		"ground_size_m": _ground_size(extent),
		"buffer_bytes": data.size() * 4,
		"mesh_build_ms": _ms_between(started_usec, mesh_done_usec),
		"buffer_ms": _ms_between(upload_started_usec, finished_usec),
		"build_ms": _ms_between(started_usec, finished_usec),
	}
	# Drop the GDScript reference to the source mesh; the MultiMesh owns it.
	mesh = null

## Benchmark metadata. Required keys are `count`, `seed`, `surfaces`, `bounds`
## (`[min_x, min_z, max_x, max_z]`), and `build_ms`.
func stats() -> Dictionary:
	return _stats.duplicate(true)

func is_built() -> bool:
	return _built

## Longest grid edge in meters, or 0 before a successful build.
func extent_m() -> float:
	return float(_stats.get("extent_m", 0.0))

func instance_count() -> int:
	return int(_stats.get("count", 0))

func _hut_material() -> StandardMaterial3D:
	var material := StandardMaterial3D.new()
	material.albedo_color = Color(0.72, 0.62, 0.48)
	material.roughness = 1.0
	material.metallic = 0.0
	material.transparency = BaseMaterial3D.TRANSPARENCY_DISABLED
	return material

func _create_ground(extent: float) -> MeshInstance3D:
	var plane := PlaneMesh.new()
	var size := _ground_size(extent)
	plane.size = Vector2(size, size)
	var material := StandardMaterial3D.new()
	material.albedo_color = GROUND_COLOR
	material.roughness = 1.0
	material.metallic = 0.0
	material.transparency = BaseMaterial3D.TRANSPARENCY_DISABLED
	var instance := MeshInstance3D.new()
	instance.name = "VoxelGround"
	instance.mesh = plane
	instance.material_override = material
	instance.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	instance.position = Vector3(0.0, -0.05, 0.0)
	return instance

func _ground_size(extent: float) -> float:
	return maxf(extent * GROUND_SIZE_FACTOR, MIN_GROUND_SIZE_M)

func _clear() -> void:
	if multimesh_instance != null:
		remove_child(multimesh_instance)
		multimesh_instance.queue_free()
		multimesh_instance = null
	if ground != null:
		remove_child(ground)
		ground.queue_free()
		ground = null
	multimesh = null
	_stats = {}
	_built = false

func _ms_between(start_usec: int, end_usec: int) -> float:
	return float(end_usec - start_usec) / 1000.0
