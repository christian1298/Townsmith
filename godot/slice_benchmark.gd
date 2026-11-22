extends Node3D

## Display-backed vertical-slice benchmark.
##
## One deterministic two-story house mesh (built once in Rust) is reused by
## 21 `MultiMesh` instances on a 7 x 3 grid, 20 m apart. The route orbits the
## row so all 21 houses stay in view. The acceptance target is 60 FPS at 1080p
## with the test house plus 20 copies.
##
## `--smoke` skips the route and asserts the structural contract instead:
## 21 instances, one surface, positive quads, status ok.

const BenchRoute = preload("res://bench_route.gd")

const GRID_COLUMNS := 7
const GRID_ROWS := 3
const INSTANCE_COUNT := GRID_COLUMNS * GRID_ROWS
const SPACING_M := 20.0
const FLOATS_PER_INSTANCE := 12  # 12 transform, no color
const GROUND_COLOR := Color(0.26, 0.30, 0.26)
const GROUND_SIZE_FACTOR := 1.5
const MIN_GROUND_SIZE_M := 128.0
const DEFAULT_ROUTE := "benchmarks/routes/phase0_slice_houses.json"
const DEFAULT_OUTPUT := "benchmarks/results/godot_phase0_slice_houses_latest.json"

var multimesh: MultiMesh
var multimesh_instance: MultiMeshInstance3D
var ground: MeshInstance3D
var camera: Camera3D
var route: Dictionary = {}
var waypoints: Array = []
var route_id := "unknown"
var route_path := ""
var output_path := ""
var duration_s := 30.0
var warmup_s := 3.0
var elapsed := 0.0
var measuring := false
var finished := false
var smoke_mode := false
var samples: Array[float] = []
var last_usec := 0
var _stats: Dictionary = {}

func _ready() -> void:
	_configure_target_window()
	var args := OS.get_cmdline_user_args()
	smoke_mode = args.has("--smoke")
	route_path = _arg(args, "--route", _project_root().path_join(DEFAULT_ROUTE))
	output_path = _arg(args, "--output", _project_root().path_join(DEFAULT_OUTPUT))

	if not smoke_mode:
		if not _load_route(route_path):
			get_tree().quit(1)
			return
		var duration_override := _arg(args, "--duration", "")
		var warmup_override := _arg(args, "--warmup", "")
		if duration_override != "":
			duration_s = float(duration_override)
		else:
			duration_s = float(route.get("duration_seconds", 30.0))
		if warmup_override != "":
			warmup_s = float(warmup_override)
		else:
			warmup_s = float(route.get("warmup_seconds", 3.0))

	_build_scene()
	if _stats.has("error"):
		get_tree().quit(1)
		return
	if smoke_mode:
		_run_smoke()
		return
	_add_environment()
	last_usec = Time.get_ticks_usec()

func _arg(args: PackedStringArray, name: String, fallback: String) -> String:
	var index := args.find(name)
	if index >= 0 and index + 1 < args.size():
		return args[index + 1]
	return fallback

func _project_root() -> String:
	return ProjectSettings.globalize_path("res://").path_join("..").simplify_path()

func _load_route(path: String) -> bool:
	var text := FileAccess.get_file_as_string(path)
	if text.is_empty():
		push_error("Could not read benchmark route: " + path)
		return false
	var parsed: Variant = JSON.parse_string(text)
	if typeof(parsed) != TYPE_DICTIONARY:
		push_error("Benchmark route is not a JSON object: " + path)
		return false
	route = parsed
	waypoints = route.get("waypoints", [])
	if waypoints.size() < 2:
		push_error("Benchmark route needs at least two waypoints")
		return false
	route_id = str(route.get("id", "unknown"))
	return true

func _build_scene() -> void:
	var started_usec := Time.get_ticks_usec()
	var core := TownsmithCore.new()
	var mesh: ArrayMesh = core.build_slice_house_mesh()
	var core_stats: Dictionary = core.slice_house_stats()
	var mesh_done_usec := Time.get_ticks_usec()

	if core_stats.has("error"):
		_fail("slice_house_stats_error: " + str(core_stats.get("error", "")))
		return
	var surfaces := mesh.get_surface_count()
	if surfaces != 1:
		_fail("slice_house_surfaces=%d" % surfaces)
		return
	var quads := int(core_stats.get("quads", 0))
	if quads <= 0:
		_fail("slice_house_quads=0")
		return

	# The house mesh is not centered on the origin (it spans volume voxels
	# 6..154), so subtract its XZ center to put each instance's footprint
	# around its grid cell. The Y origin stays at the mesh floor so the houses
	# sit on the ground plane.
	var mesh_aabb := mesh.get_aabb()
	var mesh_center := mesh_aabb.get_center()
	var half_width := mesh_aabb.size.x * 0.5
	var half_depth := mesh_aabb.size.z * 0.5
	var x0 := -float(GRID_COLUMNS - 1) * SPACING_M * 0.5
	var z0 := -float(GRID_ROWS - 1) * SPACING_M * 0.5
	var last_x := x0 + float(GRID_COLUMNS - 1) * SPACING_M
	var last_z := z0 + float(GRID_ROWS - 1) * SPACING_M
	var min_x := x0 - half_width
	var max_x := last_x + half_width
	var min_z := z0 - half_depth
	var max_z := last_z + half_depth

	var upload_started_usec := Time.get_ticks_usec()
	var data := PackedFloat32Array()
	data.resize(INSTANCE_COUNT * FLOATS_PER_INSTANCE)
	for index in INSTANCE_COUNT:
		var column := index % GRID_COLUMNS
		var row := index / GRID_COLUMNS
		var x := x0 + float(column) * SPACING_M
		var z := z0 + float(row) * SPACING_M
		var offset := index * FLOATS_PER_INSTANCE
		# Row-major 3x4 identity basis with the mesh centered on its cell.
		data[offset + 0] = 1.0
		data[offset + 1] = 0.0
		data[offset + 2] = 0.0
		data[offset + 3] = x - mesh_center.x
		data[offset + 4] = 0.0
		data[offset + 5] = 1.0
		data[offset + 6] = 0.0
		data[offset + 7] = 0.0
		data[offset + 8] = 0.0
		data[offset + 9] = 0.0
		data[offset + 10] = 1.0
		data[offset + 11] = z - mesh_center.z

	multimesh = MultiMesh.new()
	multimesh.mesh = mesh
	# transform_format and use_colors must be set before instance_count: Godot
	# rejects both after instances exist.
	multimesh.transform_format = MultiMesh.TRANSFORM_3D
	multimesh.use_colors = false
	multimesh.instance_count = INSTANCE_COUNT
	# Set the AABB before the bulk upload so Godot skips its own instance scan.
	# The mesh floor is at y = 0, so the AABB starts there.
	multimesh.custom_aabb = AABB(
		Vector3(min_x, mesh_aabb.position.y, min_z),
		Vector3(max_x - min_x, maxf(mesh_aabb.size.y, 1.0), max_z - min_z))
	multimesh.buffer = data

	multimesh_instance = MultiMeshInstance3D.new()
	multimesh_instance.name = "SliceHouses"
	multimesh_instance.multimesh = multimesh
	multimesh_instance.material_override = _house_material()
	multimesh_instance.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	add_child(multimesh_instance)

	var extent := maxf(max_x - min_x, max_z - min_z)
	ground = _create_ground(extent)
	add_child(ground)

	camera = Camera3D.new()
	camera.current = true
	camera.near = 0.1
	camera.far = maxf(1500.0, extent * 8.0)
	add_child(camera)
	if waypoints.size() >= 2:
		_apply_route(0.0)
	else:
		camera.position = Vector3(0.0, 10.0, 100.0)
		camera.look_at(Vector3(0.0, 3.0, 0.0))

	var finished_usec := Time.get_ticks_usec()
	var instance_buffer_bytes := data.size() * 4
	_stats = {
		"instances": INSTANCE_COUNT,
		"grid": [GRID_COLUMNS, GRID_ROWS],
		"spacing_m": SPACING_M,
		"surfaces": surfaces,
		"solid_voxels": int(core_stats.get("solid_voxels", 0)),
		"quads": quads,
		"vertices": int(core_stats.get("vertices", 0)),
		"indices": int(core_stats.get("indices", 0)),
		"checksum": str(core_stats.get("checksum", "")),
		"bounds": [min_x, min_z, max_x, max_z],
		"extent_m": extent,
		"mesh_aabb": [
			mesh_aabb.position.x, mesh_aabb.position.y, mesh_aabb.position.z,
			mesh_aabb.size.x, mesh_aabb.size.y, mesh_aabb.size.z,
		],
		"ground_size_m": _ground_size(extent),
		"buffer_bytes": instance_buffer_bytes,
		"mesh_buffer_bytes": int(core_stats.get("buffer_bytes", 0)),
		"mesh_build_ms": _ms_between(started_usec, mesh_done_usec),
		"buffer_ms": _ms_between(upload_started_usec, finished_usec),
		"build_ms": _ms_between(started_usec, finished_usec),
	}
	print("SLICE_HOUSES instances=%d solid_voxels=%d quads=%d vertices=%d surfaces=%d build_ms=%.1f buffer_bytes=%d" % [
		INSTANCE_COUNT,
		int(_stats.get("solid_voxels", 0)),
		quads,
		int(_stats.get("vertices", 0)),
		surfaces,
		float(_stats.get("build_ms", 0.0)),
		instance_buffer_bytes,
	])

func _fail(message: String) -> void:
	_stats = {"error": message}
	push_error("Slice house benchmark build failed: " + message)

func _run_smoke() -> void:
	var status := "ok"
	var instances := multimesh.instance_count if multimesh != null else 0
	var surfaces := int(_stats.get("surfaces", 0))
	var quads := int(_stats.get("quads", 0))
	if _stats.has("error"):
		status = "error"
	elif instances != INSTANCE_COUNT:
		status = "instances=%d" % instances
	elif surfaces != 1:
		status = "surfaces=%d" % surfaces
	elif quads <= 0:
		status = "quads=0"
	print("SLICE_HOUSE_SMOKE instances=%d quads=%d status=%s" % [instances, quads, status])
	get_tree().quit(0 if status == "ok" else 1)

func _add_environment() -> void:
	var light := DirectionalLight3D.new()
	light.rotation_degrees = Vector3(-50.0, -30.0, 0.0)
	light.light_energy = 1.2
	# Shadows stay off in this benchmark: the slice acceptance measures the
	# base scene, and a shadow pass would measure different work.
	light.shadow_enabled = false
	add_child(light)

	var world_environment := WorldEnvironment.new()
	var environment := Environment.new()
	environment.background_mode = Environment.BG_COLOR
	environment.background_color = Color(0.55, 0.72, 0.9)
	environment.ambient_light_source = Environment.AMBIENT_SOURCE_COLOR
	environment.ambient_light_color = Color(0.75, 0.8, 0.9)
	environment.ambient_light_energy = 0.55
	world_environment.environment = environment
	add_child(world_environment)

func _process(delta: float) -> void:
	if finished or smoke_mode or camera == null:
		return
	elapsed += delta
	var t := clampf((elapsed - warmup_s) / maxf(duration_s, 0.001), 0.0, 1.0)
	_apply_route(t)

	var now := Time.get_ticks_usec()
	var frame_ms := float(now - last_usec) / 1000.0
	last_usec = now
	if elapsed >= warmup_s:
		if not measuring:
			measuring = true
			samples.clear()
		if frame_ms > 0.0 and frame_ms < 1000.0:
			samples.append(frame_ms)
	if elapsed >= warmup_s + duration_s:
		_finish()

func _apply_route(t: float) -> void:
	if waypoints.size() < 2:
		return
	var left: Dictionary = waypoints[0]
	var right: Dictionary = waypoints[waypoints.size() - 1]
	for index in range(waypoints.size() - 1):
		var a: Dictionary = waypoints[index]
		var b: Dictionary = waypoints[index + 1]
		if t <= float(b.get("t", 1.0)):
			left = a
			right = b
			break
	var span := maxf(float(right.get("t", 1.0)) - float(left.get("t", 0.0)), 0.0001)
	var f := clampf((t - float(left.get("t", 0.0))) / span, 0.0, 1.0)
	var a_pos: Array = left.get("position", [0, 50, 0])
	var b_pos: Array = right.get("position", [0, 50, 0])
	var a_look: Array = left.get("look_at", [0, 0, 0])
	var b_look: Array = right.get("look_at", [0, 0, 0])
	var pos := Vector3(float(a_pos[0]), float(a_pos[1]), float(a_pos[2])).lerp(
		Vector3(float(b_pos[0]), float(b_pos[1]), float(b_pos[2])), f)
	var look := Vector3(float(a_look[0]), float(a_look[1]), float(a_look[2])).lerp(
		Vector3(float(b_look[0]), float(b_look[1]), float(b_look[2])), f)
	BenchRoute.apply_pose(camera, pos, look)
	camera.fov = lerpf(float(left.get("fov", 70.0)), float(right.get("fov", 70.0)), f)

func _configure_target_window() -> void:
	if DisplayServer.get_name() == "headless":
		return
	DisplayServer.window_set_size(Vector2i(1920, 1080))
	DisplayServer.window_set_vsync_mode(DisplayServer.VSYNC_DISABLED)
	Engine.max_fps = 0

func _house_material() -> StandardMaterial3D:
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
	instance.name = "SliceGround"
	instance.mesh = plane
	instance.material_override = material
	instance.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	instance.position = Vector3(0.0, -0.05, 0.0)
	return instance

func _ground_size(extent: float) -> float:
	return maxf(extent * GROUND_SIZE_FACTOR, MIN_GROUND_SIZE_M)

func _ms_between(start_usec: int, end_usec: int) -> float:
	return float(end_usec - start_usec) / 1000.0

func _hardware_info() -> Dictionary:
	var window_size := DisplayServer.window_get_size()
	var info := {
		"os": OS.get_name(),
		"os_version": OS.get_version(),
		"processor": OS.get_processor_name(),
		"logical_processor_count": OS.get_processor_count(),
		"display_server": DisplayServer.get_name(),
		"window_size": [window_size.x, window_size.y],
		"vsync_mode": DisplayServer.window_get_vsync_mode(),
		"max_fps": Engine.max_fps,
		"refresh_rate_hz": DisplayServer.screen_get_refresh_rate(),
		"rendering_method": str(ProjectSettings.get_setting("rendering/renderer/rendering_method", "unknown")),
	}
	var adapter := RenderingServer.get_video_adapter_name()
	if adapter != "":
		info["video_adapter"] = adapter
	var vendor := RenderingServer.get_video_adapter_vendor()
	if vendor != "":
		info["video_vendor"] = vendor
	var api := RenderingServer.get_video_adapter_api_version()
	if api != "":
		info["video_api"] = api
	return info

func _percentile(sorted_values: Array[float], fraction: float) -> float:
	if sorted_values.is_empty():
		return 0.0
	var index := clampi(int(round(float(sorted_values.size() - 1) * fraction)), 0, sorted_values.size() - 1)
	return sorted_values[index]

func _finish() -> void:
	if finished:
		return
	finished = true
	var sorted := samples.duplicate()
	sorted.sort()
	var frame_total := 0.0
	for value in sorted:
		frame_total += value
	var frame_avg := frame_total / maxf(float(sorted.size()), 1.0)
	var hitches_16 := 0
	var hitches_33 := 0
	var hitches_50 := 0
	for value in sorted:
		if value > 50.0:
			hitches_50 += 1
		if value > 33.0:
			hitches_33 += 1
		if value > 16.0:
			hitches_16 += 1
	var p99 := _percentile(sorted, 0.99)
	var result := {
		"format_version": 1,
		"suite": "godot_route",
		"scene": "phase0_slice_houses",
		"slice_houses": _stats,
		"route": route_id,
		"route_path": route_path,
		"duration_s": duration_s,
		"warmup_s": warmup_s,
		"headless": DisplayServer.get_name() == "headless",
		"engine": Engine.get_version_info(),
		"hardware": _hardware_info(),
		"frames": sorted.size(),
		"results": {
			"fps_avg": 1000.0 / maxf(frame_avg, 0.001),
			"fps_low_1pct": 1000.0 / maxf(p99, 0.001),
			"frametime_ms_avg": frame_avg,
			"frametime_ms_median": _percentile(sorted, 0.50),
			"frametime_ms_p95": _percentile(sorted, 0.95),
			"frametime_ms_p99": p99,
			"frametime_ms_min": sorted[0] if sorted.size() > 0 else 0.0,
			"frametime_ms_max": sorted[sorted.size() - 1] if sorted.size() > 0 else 0.0,
			"hitches_16ms": hitches_16,
			"hitches_33ms": hitches_33,
			"hitches_50ms": hitches_50
		}
	}
	DirAccess.make_dir_recursive_absolute(output_path.get_base_dir())
	var file := FileAccess.open(output_path, FileAccess.WRITE)
	if file == null:
		push_error("Could not write benchmark result: " + output_path)
	else:
		file.store_string(JSON.stringify(result, "  "))
		file.close()
	print("BENCHMARK_RESULT " + JSON.stringify(result))
	get_tree().quit(0)
