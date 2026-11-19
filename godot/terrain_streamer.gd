extends Node3D

## Streams deterministic 256 m terrain chunks around a camera.
##
## Chunks are built at a distance-based mesh LOD. When the camera crosses a
## chunk border, chunks whose desired LOD changed are rebuilt (both finer and
## coarser), subject to the per-frame build budget. Chunk generation is still
## synchronous; the budget keeps the hitching bounded until worker-thread
## generation lands.

const CHUNK_METERS := 256.0
const CHUNK_CELLS := 256
const WATER_HEIGHT := 24.1

@export var seed: int = 2026
@export var view_radius: int = 1
@export var keep_radius: int = 2
@export var builds_per_frame: int = 2
@export var full_detail_radius: int = 0
@export var half_detail_radius: int = 1
@export var far_detail_step: int = 4

var core: TownsmithCore
var terrain_material: StandardMaterial3D
var chunks: Dictionary = {}
var chunks_lod: Dictionary = {}
var empty_reported: Dictionary = {}
var queue: Array[Vector2i] = []
var camera: Camera3D
var water: MeshInstance3D
var last_center := Vector2i.ZERO
var has_center := false
var chunks_built := 0

func _ready() -> void:
	view_radius = maxi(view_radius, 0)
	keep_radius = maxi(keep_radius, view_radius)
	builds_per_frame = maxi(builds_per_frame, 1)
	full_detail_radius = maxi(full_detail_radius, 0)
	half_detail_radius = maxi(half_detail_radius, full_detail_radius)
	far_detail_step = clampi(far_detail_step, 1, CHUNK_CELLS)

	core = TownsmithCore.new()
	terrain_material = StandardMaterial3D.new()
	terrain_material.vertex_color_use_as_albedo = true
	terrain_material.roughness = 1.0
	terrain_material.cull_mode = BaseMaterial3D.CULL_DISABLED

	var water_plane := PlaneMesh.new()
	water_plane.size = Vector2(8192.0, 8192.0)
	water = MeshInstance3D.new()
	water.mesh = water_plane
	water.position = Vector3(0.0, WATER_HEIGHT, 0.0)
	var water_material := StandardMaterial3D.new()
	water_material.albedo_color = Color(0.16, 0.38, 0.62, 0.55)
	water_material.transparency = BaseMaterial3D.TRANSPARENCY_ALPHA
	water_material.cull_mode = BaseMaterial3D.CULL_DISABLED
	water.material_override = water_material
	add_child(water)

func bind_camera(camera_node: Camera3D) -> void:
	camera = camera_node

func chunk_count() -> int:
	return chunks.size()

func loaded_lods() -> Array[int]:
	var lods: Array[int] = []
	for value in chunks_lod.values():
		var step := int(value)
		if not lods.has(step):
			lods.append(step)
	lods.sort()
	return lods

func _process(_delta: float) -> void:
	if not is_instance_valid(camera):
		return

	var center := _camera_chunk()
	if not has_center or center != last_center:
		has_center = true
		last_center = center
		_refill_queue(center)

	for _i in range(builds_per_frame):
		if queue.is_empty():
			break
		var coord: Vector2i = queue.pop_front()
		if chunks.has(coord) and int(chunks_lod.get(coord, 0)) == _lod_step(coord, center):
			continue
		_build_chunk(coord, center)

	_unload_outside(center)
	if is_instance_valid(water):
		water.global_position = Vector3(
			camera.global_position.x,
			WATER_HEIGHT,
			camera.global_position.z
		)

func _camera_chunk() -> Vector2i:
	var position := camera.global_position
	return Vector2i(
		floori(position.x / CHUNK_METERS),
		floori(position.z / CHUNK_METERS)
	)

func _refill_queue(center: Vector2i) -> void:
	queue.clear()
	for dz in range(-view_radius, view_radius + 1):
		for dx in range(-view_radius, view_radius + 1):
			var coord := center + Vector2i(dx, dz)
			if not chunks.has(coord) or int(chunks_lod.get(coord, 0)) != _lod_step(coord, center):
				queue.append(coord)
	queue.sort_custom(func(a: Vector2i, b: Vector2i) -> bool:
		return _distance2(a, center) < _distance2(b, center)
	)

func _distance2(a: Vector2i, b: Vector2i) -> int:
	var dx := a.x - b.x
	var dz := a.y - b.y
	return dx * dx + dz * dz

func _lod_step(coord: Vector2i, center: Vector2i) -> int:
	var distance := maxi(absi(coord.x - center.x), absi(coord.y - center.y))
	if distance <= full_detail_radius:
		return 1
	if distance <= half_detail_radius:
		return 2
	return far_detail_step

func _build_chunk(coord: Vector2i, center: Vector2i) -> void:
	var step := _lod_step(coord, center)
	var mesh: ArrayMesh = core.build_terrain_mesh_lod(seed, coord.x, coord.y, step)
	if mesh.get_surface_count() == 0:
		if not empty_reported.has(coord):
			empty_reported[coord] = true
			push_warning("Terrain chunk %s produced no geometry; not cached, will retry" % coord)
		return
	var instance := MeshInstance3D.new()
	instance.mesh = mesh
	instance.position = Vector3(coord.x * CHUNK_METERS, 0.0, coord.y * CHUNK_METERS)
	instance.material_override = terrain_material
	add_child(instance)
	if chunks.has(coord):
		var previous: MeshInstance3D = chunks[coord]
		if is_instance_valid(previous):
			previous.queue_free()
	chunks[coord] = instance
	chunks_lod[coord] = step
	chunks_built += 1
	if chunks_built <= 3 or chunks_built % 8 == 0:
		print("Terrain chunk ", coord, " LOD ", step, " loaded; total ", chunks.size())

func _unload_outside(center: Vector2i) -> void:
	for key in chunks.keys():
		var coord: Vector2i = key
		var distance := maxi(absi(coord.x - center.x), absi(coord.y - center.y))
		if distance > keep_radius:
			var instance: MeshInstance3D = chunks[key]
			if is_instance_valid(instance):
				instance.queue_free()
			chunks.erase(key)
			chunks_lod.erase(key)
			empty_reported.erase(key)
