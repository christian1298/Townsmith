extends Node3D

## Streams deterministic 256 m terrain chunks around a camera.
##
## Terrain generation and meshing run on Rust worker threads. This node queues
## chunk requests, polls finished meshes on the main thread, and uploads them to
## MeshInstance3D nodes. LOD is distance-based.

const WaterBody = preload("res://water_body.gd")

const CHUNK_METERS := 256.0
const CHUNK_CELLS := 256
const WATER_HEIGHT := 24.1
const MAX_COORD_ATTEMPTS := 3
const MAX_IN_FLIGHT := 32

@export var seed: int = 2026
@export var view_radius: int = 1
@export var keep_radius: int = 2
@export var builds_per_frame: int = 2
@export var max_in_flight: int = 4
@export var full_detail_radius: int = 0
@export var half_detail_radius: int = 1
@export var far_detail_step: int = 4

var core: TownsmithCore
var terrain_material: StandardMaterial3D
var chunks: Dictionary = {}
var chunks_lod: Dictionary = {}
var in_flight: Dictionary = {}
var queue: Array[Vector2i] = []
var camera: Camera3D
var water: WaterBody
var last_center := Vector2i.ZERO
var has_center := false
var chunks_built := 0
var coord_attempts: Dictionary = {}
var chunks_requested := 0
var chunks_applied := 0
var chunks_dropped := 0
var bridge_failures := 0

func _ready() -> void:
	view_radius = maxi(view_radius, 0)
	keep_radius = maxi(keep_radius, view_radius)
	builds_per_frame = clampi(builds_per_frame, 1, 8)
	max_in_flight = clampi(max_in_flight, 1, MAX_IN_FLIGHT)
	full_detail_radius = maxi(full_detail_radius, 0)
	half_detail_radius = maxi(half_detail_radius, full_detail_radius)
	far_detail_step = clampi(far_detail_step, 1, CHUNK_CELLS)

	core = TownsmithCore.new()
	terrain_material = StandardMaterial3D.new()
	terrain_material.vertex_color_use_as_albedo = true
	terrain_material.roughness = 1.0
	terrain_material.cull_mode = BaseMaterial3D.CULL_DISABLED

	water = WaterBody.new()
	water.height = WATER_HEIGHT
	add_child(water)

func bind_camera(camera_node: Camera3D) -> void:
	camera = camera_node
	if is_instance_valid(water):
		water.follow(camera)

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

	_poll_results(center)
	_request_jobs(center)
	_unload_outside(center)
	if is_instance_valid(water):
		water.follow(camera)

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

func _is_desired(coord: Vector2i, center: Vector2i) -> bool:
	return maxi(absi(coord.x - center.x), absi(coord.y - center.y)) <= view_radius

func _has_job(coord: Vector2i, step: int) -> bool:
	for job in in_flight.values():
		if job["coord"] == coord and int(job["step"]) == step:
			return true
	return false

func _poll_results(center: Vector2i) -> void:
	for _i in range(builds_per_frame):
		var result: Dictionary = core.poll_terrain_chunk()
		if result.is_empty():
			return

		var error := str(result.get("error", ""))
		if error == "worker_pool_disconnected":
			_handle_disconnected()
			return

		var id := int(result.get("id", -1))
		var job: Dictionary = in_flight.get(id, {})
		if job.is_empty():
			continue
		in_flight.erase(id)

		var coord: Vector2i = job["coord"]
		var step := int(result.get("step", job["step"]))
		if error != "":
			_retry_coord(coord, error)
			continue

		var mesh := result.get("mesh") as ArrayMesh
		if mesh == null or mesh.get_surface_count() == 0:
			_retry_coord(coord, "empty mesh")
			continue

		if not _is_desired(coord, center) or _lod_step(coord, center) != step:
			chunks_dropped += 1
			if _is_desired(coord, center):
				_enqueue(coord)
			continue

		_apply_chunk(coord, step, mesh)

func _request_jobs(center: Vector2i) -> void:
	while in_flight.size() < max_in_flight and not queue.is_empty():
		var coord: Vector2i = queue.pop_front()
		if not _is_desired(coord, center):
			continue
		var step := _lod_step(coord, center)
		if chunks.has(coord) and int(chunks_lod.get(coord, 0)) == step:
			continue
		if _has_job(coord, step):
			continue
		var id := core.request_terrain_chunk(seed, coord.x, coord.y, step)
		if id < 0:
			push_error("Terrain request failed for %s; resetting workers" % coord)
			_handle_disconnected()
			return
		in_flight[id] = {"coord": coord, "step": step}
		chunks_requested += 1

func _enqueue(coord: Vector2i) -> void:
	if not queue.has(coord):
		queue.append(coord)

func _retry_coord(coord: Vector2i, reason: String) -> void:
	var attempts := int(coord_attempts.get(coord, 0)) + 1
	coord_attempts[coord] = attempts
	if attempts <= MAX_COORD_ATTEMPTS:
		_enqueue(coord)
	else:
		push_error("Terrain chunk %s failed after %d attempts: %s" % [coord, attempts, reason])

func _handle_disconnected() -> void:
	in_flight.clear()
	bridge_failures += 1
	if bridge_failures > 2:
		push_error("Terrain worker pool disconnected repeatedly; disabling streaming")
		set_process(false)
		return
	push_error("Terrain worker pool disconnected; resetting")
	core = TownsmithCore.new()
	if has_center:
		_refill_queue(last_center)

func stats() -> Dictionary:
	return {
		"chunks_loaded": chunks.size(),
		"chunks_requested": chunks_requested,
		"chunks_applied": chunks_applied,
		"chunks_dropped": chunks_dropped,
		"in_flight": in_flight.size(),
		"queue": queue.size(),
	}

func _apply_chunk(coord: Vector2i, step: int, mesh: ArrayMesh) -> void:
	var instance := MeshInstance3D.new()
	instance.mesh = mesh
	instance.position = Vector3(coord.x * CHUNK_METERS, 0.0, coord.y * CHUNK_METERS)
	instance.material_override = terrain_material
	add_child(instance)
	if chunks.has(coord):
		var previous: MeshInstance3D = chunks[coord]
		if is_instance_valid(previous):
			previous.mesh = null
			previous.queue_free()
	chunks[coord] = instance
	chunks_lod[coord] = step
	coord_attempts.erase(coord)
	chunks_applied += 1
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
				instance.mesh = null
				instance.queue_free()
			chunks.erase(key)
			chunks_lod.erase(key)
			coord_attempts.erase(key)
