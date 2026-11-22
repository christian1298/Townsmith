extends Node3D

## Streams deterministic 256 m terrain chunks around a camera.
##
## Terrain generation and meshing run on Rust worker threads. This node queues
## chunk requests, polls finished meshes on the main thread, and stages them in
## a ready map. Each frame it uploads at most `COMMIT_BUDGET` ready chunks in a
## deterministic priority order, so worker completion order never decides which
## chunk is committed first. LOD is distance-based.

const WaterBody = preload("res://water_body.gd")

const CHUNK_METERS := 256.0
const CHUNK_CELLS := 256
const WATER_HEIGHT := 24.1
const MAX_COORD_ATTEMPTS := 3
const MAX_IN_FLIGHT := 32
## Maximum ready chunks applied to the scene per frame. Commits are selected
## nearest-ring first, then by (distance, coord.x, coord.z).
const COMMIT_BUDGET := 4
## 32-bit FNV-1a: deterministic over the ordered commit sequence.
const CHECKSUM_OFFSET := 2166136261
const CHECKSUM_PRIME := 16777619

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
## Finished meshes keyed by chunk coord, waiting for a commit slot.
var ready_chunks: Dictionary = {}
var queue: Array[Vector2i] = []
var camera: Camera3D
var water: WaterBody
var last_center := Vector2i.ZERO
var has_center := false
var chunks_built := 0
var dirty: Dictionary = {}
var coord_attempts: Dictionary = {}
var chunks_requested := 0
var chunks_applied := 0
var chunks_dropped := 0
var bridge_failures := 0
var commit_checksum := CHECKSUM_OFFSET
## Hold commits until the request queue and worker pool drain. Benchmark smoke
## paths set this so every stage has the same ready set before committing.
var commit_barrier := false
## Optional full commit log for deterministic smoke comparisons.
var track_commit_sequence := false
var commit_sequence: Array[Vector2i] = []

func _ready() -> void:
	view_radius = maxi(view_radius, 0)
	keep_radius = maxi(keep_radius, view_radius)
	builds_per_frame = clampi(builds_per_frame, 1, 8)
	max_in_flight = clampi(max_in_flight, 1, MAX_IN_FLIGHT)
	full_detail_radius = maxi(full_detail_radius, 0)
	half_detail_radius = maxi(half_detail_radius, full_detail_radius)
	far_detail_step = clampi(far_detail_step, 1, CHUNK_CELLS)

	if core == null:
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

## Cells along one chunk edge. A terrain edit on a border cell only remeshes
## the neighbour when the caller knows the last local index, so callers derive
## their edge test from this instead of copying `CHUNK_CELLS - 1`.
func chunk_cell_count() -> int:
	return CHUNK_CELLS

## Mark a loaded or queued chunk for remeshing after a terrain edit.
func invalidate_chunk(coord: Vector2i) -> void:
	ready_chunks.erase(coord)
	dirty[coord] = int(dirty.get(coord, 0)) + 1
	_enqueue(coord)

## Mark every loaded chunk for remeshing after loading a world save.
func invalidate_all() -> void:
	for key in chunks.keys():
		invalidate_chunk(key)
	for job in in_flight.values():
		invalidate_chunk(job["coord"])

func dirty_count() -> int:
	return dirty.size()

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
	_commit_ready(center)
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
			if ready_chunks.has(coord) and not dirty.has(coord):
				continue
			if not chunks.has(coord) or int(chunks_lod.get(coord, 0)) != _lod_step(coord, center) or dirty.has(coord):
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

		_stage_chunk(coord, step, mesh, int(job.get("generation", 0)))

## Hold a finished mesh until a deterministic commit slot opens.
func _stage_chunk(coord: Vector2i, step: int, mesh: ArrayMesh, generation: int) -> void:
	ready_chunks[coord] = {
		"step": step,
		"mesh": mesh,
		"generation": generation,
	}

## Apply at most COMMIT_BUDGET ready chunks. The selection order depends only
## on the current center and chunk coords, never on which worker finished first:
## nearest Chebyshev ring, then squared distance, then coord.x, then coord.z.
func _commit_ready(center: Vector2i) -> void:
	if ready_chunks.is_empty():
		return
	if commit_barrier and (not in_flight.is_empty() or not queue.is_empty()):
		return

	var candidates: Array[Dictionary] = []
	for key in ready_chunks.keys():
		var coord: Vector2i = key
		var dx := coord.x - center.x
		var dz := coord.y - center.y
		candidates.append({
			"coord": coord,
			"ring": maxi(absi(dx), absi(dz)),
			"distance": dx * dx + dz * dz,
		})
	candidates.sort_custom(func(a: Dictionary, b: Dictionary) -> bool:
		if int(a["ring"]) != int(b["ring"]):
			return int(a["ring"]) < int(b["ring"])
		if int(a["distance"]) != int(b["distance"]):
			return int(a["distance"]) < int(b["distance"])
		var ca: Vector2i = a["coord"]
		var cb: Vector2i = b["coord"]
		if ca.x != cb.x:
			return ca.x < cb.x
		return ca.y < cb.y
	)

	var committed := 0
	for candidate in candidates:
		if committed >= COMMIT_BUDGET:
			return
		var coord: Vector2i = candidate["coord"]
		if not ready_chunks.has(coord):
			continue
		var data: Dictionary = ready_chunks[coord]
		var step := int(data["step"])
		if not _is_desired(coord, center) or _lod_step(coord, center) != step:
			ready_chunks.erase(coord)
			chunks_dropped += 1
			if _is_desired(coord, center):
				_enqueue(coord)
			continue
		ready_chunks.erase(coord)
		_apply_chunk(coord, step, data["mesh"], int(data["generation"]))
		committed += 1

func _request_jobs(center: Vector2i) -> void:
	var deferred: Array[Vector2i] = []
	while in_flight.size() < max_in_flight and not queue.is_empty():
		var coord: Vector2i = queue.pop_front()
		if not _is_desired(coord, center):
			continue
		if ready_chunks.has(coord) and not dirty.has(coord):
			continue
		var step := _lod_step(coord, center)
		if chunks.has(coord) and int(chunks_lod.get(coord, 0)) == step and not dirty.has(coord):
			continue
		if _has_job(coord, step):
			deferred.append(coord)
			continue
		var id := core.request_terrain_chunk(seed, coord.x, coord.y, step)
		if id < 0:
			push_error("Terrain request failed for %s; resetting workers" % coord)
			_handle_disconnected()
			return
		in_flight[id] = {
			"coord": coord,
			"step": step,
			"generation": int(dirty.get(coord, 0)),
		}
		chunks_requested += 1
	for coord in deferred:
		_enqueue(coord)

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
	if core.has_method("reset_terrain_workers"):
		core.reset_terrain_workers()
	if has_center:
		_refill_queue(last_center)

func is_drained() -> bool:
	return in_flight.is_empty() and ready_chunks.is_empty() and queue.is_empty()

func commit_checksum_hex() -> String:
	return "%08x" % commit_checksum

func commit_sequence_text() -> String:
	var parts: PackedStringArray = []
	for coord in commit_sequence:
		parts.append("%d,%d" % [coord.x, coord.y])
	return ";".join(parts)

func stats() -> Dictionary:
	return {
		"chunks_loaded": chunks.size(),
		"chunks_ready": ready_chunks.size(),
		"chunks_pending": in_flight.size() + ready_chunks.size(),
		"chunks_requested": chunks_requested,
		"chunks_applied": chunks_applied,
		"chunks_dropped": chunks_dropped,
		"in_flight": in_flight.size(),
		"queue": queue.size(),
		"dirty": dirty.size(),
		"commit_count": chunks_applied,
		"commit_checksum": commit_checksum_hex(),
	}

func _hash_commit(coord: Vector2i, step: int) -> void:
	for value in [coord.x, coord.y, step]:
		var bits: int = value & 0xFFFFFFFF
		for shift in range(0, 32, 8):
			commit_checksum = ((commit_checksum ^ ((bits >> shift) & 0xFF)) * CHECKSUM_PRIME) & 0xFFFFFFFF

func _apply_chunk(coord: Vector2i, step: int, mesh: ArrayMesh, generation: int) -> void:
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
	if int(dirty.get(coord, 0)) <= generation:
		dirty.erase(coord)
	else:
		_enqueue(coord)
	coord_attempts.erase(coord)
	_hash_commit(coord, step)
	if track_commit_sequence:
		commit_sequence.append(coord)
	chunks_applied += 1
	chunks_built += 1
	if chunks_built <= 3 or chunks_built % 8 == 0:
		print("Terrain chunk ", coord, " LOD ", step, " loaded; total ", chunks.size())

func _unload_outside(center: Vector2i) -> void:
	for key in ready_chunks.keys():
		var coord: Vector2i = key
		if maxi(absi(coord.x - center.x), absi(coord.y - center.y)) > keep_radius:
			ready_chunks.erase(key)
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
			dirty.erase(key)
			coord_attempts.erase(key)
