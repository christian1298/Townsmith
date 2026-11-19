extends Node3D

const TerrainStreamer = preload("res://terrain_streamer.gd")
const CityRenderer = preload("res://city_renderer.gd")

const CITY_TIERS := {"early": 0, "mid": 1, "late": 2}
const CITY_SEED := 2026

var streamer
var city
var city_tier := ""
var city_tier_index := -1
var route: Dictionary = {}
var waypoints: Array = []
var route_id := "unknown"
var route_path := ""
var output_path := ""
var warmup_s := 3.0
var duration_s := 30.0
var elapsed := 0.0
var measuring := false
var finished := false
var samples: Array[float] = []
var last_usec := 0
var camera: Camera3D

func _ready() -> void:
	var args := OS.get_cmdline_user_args()
	route_path = _arg(args, "--route", _project_root().path_join("benchmarks/routes/phase0_flyover.json"))
	output_path = _arg(args, "--output", _project_root().path_join("benchmarks/results/godot_phase0_latest.json"))
	var duration_override := _arg(args, "--duration", "")
	var warmup_override := _arg(args, "--warmup", "")
	city_tier = _arg(args, "--city-tier", "")
	if city_tier != "":
		if not CITY_TIERS.has(city_tier):
			push_error("Unknown --city-tier '%s'; expected early, mid, or late" % city_tier)
			get_tree().quit(2)
			return
		city_tier_index = int(CITY_TIERS[city_tier])

	if not _load_route(route_path):
		get_tree().quit(1)
		return
	if duration_override != "":
		duration_s = float(duration_override)
	else:
		duration_s = float(route.get("duration_seconds", 30.0))
	if warmup_override != "":
		warmup_s = float(warmup_override)
	else:
		warmup_s = float(route.get("warmup_seconds", 3.0))

	_build_world()
	if city != null and city.stats().has("error"):
		get_tree().quit(1)
		return
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

func _build_world() -> void:
	if city_tier != "":
		_build_city()
	else:
		_build_terrain()
	_add_environment()

func _build_terrain() -> void:
	streamer = TerrainStreamer.new()
	streamer.seed = 2026
	streamer.view_radius = 2
	streamer.keep_radius = 3
	streamer.builds_per_frame = 2
	add_child(streamer)

	camera = Camera3D.new()
	camera.current = true
	camera.far = 1500.0
	add_child(camera)
	_apply_route(0.0)
	streamer.bind_camera(camera)

func _build_city() -> void:
	city = CityRenderer.new()
	city.name = "CityRenderer"
	add_child(city)
	city.build(city_tier_index, CITY_SEED)

	camera = Camera3D.new()
	camera.current = true
	camera.near = 1.0
	# Cover the whole city plus the route's outside-in approach. The Late tier
	# is 8 km across, so the corner-to-corner view is ~11 km.
	camera.far = maxf(1500.0, city.extent_m() * 2.5)
	add_child(camera)
	_apply_route(0.0)

	var stats: Dictionary = city.stats()
	if stats.has("error"):
		push_error("City benchmark build failed: " + str(stats["error"]))
		return
	print("CITY tier=%s seed=%d buildings=%d population=%d build_ms=%.1f layout_ms=%.1f buffer_ms=%.1f" % [
		stats.get("tier", city_tier),
		int(stats.get("seed", CITY_SEED)),
		int(stats.get("buildings", 0)),
		int(stats.get("population", 0)),
		float(stats.get("build_ms", 0.0)),
		float(stats.get("layout_ms", 0.0)),
		float(stats.get("buffer_ms", 0.0)),
	])

func _add_environment() -> void:
	var light := DirectionalLight3D.new()
	light.rotation_degrees = Vector3(-50.0, -30.0, 0.0)
	light.light_energy = 1.2
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
	if finished or camera == null:
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
	camera.position = pos
	camera.look_at(look)
	camera.fov = lerpf(float(left.get("fov", 70.0)), float(right.get("fov", 70.0)), f)

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
		"scene": "phase0_city" if city != null else "phase0_terrain",
		"city": city.stats() if city != null else null,
		"chunks_loaded": streamer.chunk_count() if streamer != null else null,
		"streaming": streamer.stats() if streamer != null else null,
		"route": route_id,
		"route_path": route_path,
		"duration_s": duration_s,
		"warmup_s": warmup_s,
		"headless": DisplayServer.get_name() == "headless",
		"engine": Engine.get_version_info(),
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
