extends Node3D

## Streams deterministic 256 m terrain chunks around a camera.
##
## The first Phase 0 version builds a small ring of chunks and uses a coarser
## mesh stride for distant rings. Chunk generation is still synchronous; the
## per-frame build budget keeps the hitching bounded until worker-thread
## generation lands.

const CHUNK_METERS := 256.0
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
var pending: Dictionary = {}
var queue: Array[Vector2i] = []
var camera: Camera3D
var water: MeshInstance3D
var last_center := Vector2i(999999, 999999)
var chunks_built := 0

func _ready() -> void:
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

func _process(_delta: float) -> void:
	if camera == null:
		return

	var center := _camera_chunk()
	if center != last_center:
		last_center = center
		_refill_queue(center)

	for _i in range(builds_per_frame):
		if queue.is_empty():
			break
		var coord: Vector2i = queue.pop_front()
		pending.erase(coord)
		if not chunks.has(coord):
			_build_chunk(coord, center)

	_unload_outside(center)
	if water != null:
		water.position.x = camera.global_position.x
		water.position.z = camera.global_position.z

func _camera_chunk() -> Vector2i:
	return Vector2i(
		floori(camera.global_position.x / CHUNK_METERS),
		floori(camera.global_position.z / CHUNK_METERS)
	)

func _refill_queue(center: Vector2i) -> void:
	queue.clear()
	pending.clear()
	for dz in range(-view_radius, view_radius + 1):
		for dx in range(-view_radius, view_radius + 1):
			var coord := center + Vector2i(dx, dz)
			if not chunks.has(coord):
				queue.append(coord)
				pending[coord] = true
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
	var instance := MeshInstance3D.new()
	instance.mesh = mesh
	instance.position = Vector3(coord.x * CHUNK_METERS, 0.0, coord.y * CHUNK_METERS)
	instance.material_override = terrain_material
	add_child(instance)
	chunks[coord] = instance
	chunks_built += 1
	if chunks_built <= 3 or chunks_built % 8 == 0:
		print("Terrain chunk ", coord, " LOD ", step, " loaded; total ", chunks.size())

func _unload_outside(center: Vector2i) -> void:
	for key in chunks.keys():
		var coord: Vector2i = key
		var distance := maxi(absi(coord.x - center.x), absi(coord.y - center.y))
		if distance > keep_radius:
			var instance: MeshInstance3D = chunks[key]
			instance.queue_free()
			chunks.erase(key)
