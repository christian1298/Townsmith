extends Node3D

## Display-backed metropolis-gate frame-budget route (G5, docs/68).
##
## Builds a deterministic box metropolis as one `MultiMeshInstance3D`, then
## flies a deterministic warm-up + measured camera route and records frame
## times from the real render loop. It is a **synthetic box city**, not the
## gate fixture: no real voxel buildings, LOD/impostors, streaming, GI, SSAO,
## or 1M simulation. See docs/68 §7.5 and the doc note added with this route.
##
## Headless mode (`--headless ... -- --setup-only`) validates the layout and
## upload path, prints a checksum, and exits without claiming FPS.
##
## Usage:
##   godot --path godot res://metropolis_gate.tscn -- --output /tmp/gate.json
##   godot --headless --path godot res://metropolis_gate.tscn -- --setup-only
##
## Options after `--`:
##   --buildings N       City building count (default 250000)
##   --duration S        Measured seconds (default 120, route value otherwise)
##   --warmup S          Discarded warm-up seconds (default 15)
##   --route PATH        Route JSON (default benchmarks/routes/metropolis_gate_v1.json)
##   --output PATH       Result JSON
##   --setup-only        Build, validate, print setup JSON, exit 0
##   --shadows           Let the MultiMesh cast directional shadows (off by default)
##   --git-commit HASH   Provenance, recorded verbatim
##   --git-dirty BOOL    Provenance, recorded verbatim

const DEFAULT_BUILDINGS := 250_000
const DEFAULT_ROUTE := "benchmarks/routes/metropolis_gate_v1.json"
const GRID_SPACING_M := 10.0
const BOX_EDGE_M := 4.0
const STORY_M := 4.0
const MAX_STORIES := 15
const FLOATS_PER_INSTANCE := 12  # 3x4 transform, no color
const FLOAT_BYTES := 4
const FNV_OFFSET_BASIS := 2166136261
const FNV_PRIME := 16777619

# docs/68 §5.1 — late_metropolis_v1 frame thresholds.
const GATE_MIN_FPS_AVG := 30.0
const GATE_MIN_FPS_LOW_1PCT := 22.0
const GATE_MIN_FPS_LOW_0_1PCT := 15.0
const GATE_MAX_P99_MS := 45.0
const GATE_MAX_P99_9_MS := 67.0
const GATE_MAX_HITCHES_50MS := 3
const GATE_MAX_STALLS_250MS := 0

var building_count := DEFAULT_BUILDINGS
var duration_s := 120.0
var warmup_s := 15.0
var route: Dictionary = {}
var waypoints: Array = []
var route_id := "unknown"
var route_path := ""
var output_path := ""
var git_commit := ""
var git_dirty := ""
var setup_only := false
var shadows_enabled := false

var camera: Camera3D
var light: DirectionalLight3D
var multimesh: MultiMesh
var multimesh_instance: MultiMeshInstance3D
var _container: Node3D
var _city_stats: Dictionary = {}
var _settings: Dictionary = {}
var _hardware: Dictionary = {}
var _route_hash := 0

var elapsed := 0.0
var measured_s := 0.0
var measuring := false
var finished := false
var samples: Array[float] = []
var last_usec := 0

func _ready() -> void:
	_configure_target_window()
	var args := OS.get_cmdline_user_args()
	setup_only = args.has("--setup-only")
	shadows_enabled = args.has("--shadows")
	git_commit = _arg(args, "--git-commit", "")
	git_dirty = _arg(args, "--git-dirty", "")
	var buildings_arg := _arg(args, "--buildings", str(DEFAULT_BUILDINGS))
	if not buildings_arg.is_valid_int() or int(buildings_arg) < 1:
		_fail("--buildings expects a positive integer (got '%s')" % buildings_arg)
		return
	building_count = int(buildings_arg)
	route_path = _arg(args, "--route", _project_root().path_join(DEFAULT_ROUTE))
	output_path = _arg(args, "--output", _project_root().path_join(
		"benchmarks/results/metropolis_gate_latest.json"))
	if not _load_route(route_path):
		_fail("could not load route: " + route_path)
		return
	var duration_override := _arg(args, "--duration", "")
	var warmup_override := _arg(args, "--warmup", "")
	duration_s = float(duration_override) if duration_override != "" else float(route.get("duration_seconds", 120.0))
	warmup_s = float(warmup_override) if warmup_override != "" else float(route.get("warmup_seconds", 15.0))
	if duration_s <= 0.0 or warmup_s < 0.0:
		_fail("--duration must be > 0 and --warmup must be >= 0")
		return

	_build_city()
	if _city_stats.has("error"):
		_fail(str(_city_stats["error"]))
		return
	_add_environment()
	_apply_route(0.0)
	_hardware = _hardware_info()
	_settings = _settings_info()
	_route_hash = _hash_route()
	last_usec = Time.get_ticks_usec()

	if setup_only:
		_write_setup()
		return

func _process(delta: float) -> void:
	if finished or camera == null:
		return
	elapsed += delta
	var progress := clampf((elapsed - warmup_s) / maxf(duration_s, 0.001), 0.0, 1.0)
	_apply_route(progress)

	var now := Time.get_ticks_usec()
	if not measuring and elapsed >= warmup_s:
		# Discard everything up to and including the warm-up boundary frame.
		measuring = true
		samples.clear()
		measured_s = 0.0
		last_usec = now
		return
	if measuring:
		var frame_ms := float(now - last_usec) / 1000.0
		last_usec = now
		if frame_ms > 0.0 and frame_ms < 10_000.0:
			samples.append(frame_ms)
			measured_s += frame_ms / 1000.0
	if elapsed >= warmup_s + duration_s:
		_finish()

func _build_city() -> void:
	var started_usec := Time.get_ticks_usec()
	var columns := maxi(1, int(ceil(sqrt(float(building_count)))))
	var rows := maxi(1, int(ceil(float(building_count) / float(columns))))
	var extent_x := float(columns - 1) * GRID_SPACING_M
	var extent_z := float(rows - 1) * GRID_SPACING_M
	var origin_x := -extent_x * 0.5
	var origin_z := -extent_z * 0.5
	var half_box := BOX_EDGE_M * 0.5
	var mem_before_kb := OS.get_static_memory_usage() / 1024

	var data := PackedFloat32Array()
	data.resize(building_count * FLOATS_PER_INSTANCE)
	var checksum := FNV_OFFSET_BASIS
	checksum = _mix(checksum, building_count)
	checksum = _mix(checksum, columns)
	checksum = _mix(checksum, rows)

	var max_height := 0.0
	var stories_total := 0
	for index in building_count:
		var column := index % columns
		var row := index / columns
		# Deterministic downtown profile: taller near the centre, plus a
		# per-building pseudo-random floor count. Integer-only, so the layout
		# checksum is identical across platforms and processes.
		var core_x := absf(float(column) - float(columns - 1) * 0.5) / maxf(float(columns) * 0.5, 1.0)
		var core_z := absf(float(row) - float(rows - 1) * 0.5) / maxf(float(rows) * 0.5, 1.0)
		var core := clampf(1.0 - sqrt(core_x * core_x + core_z * core_z), 0.0, 1.0)
		var jitter := _mix(FNV_OFFSET_BASIS, index)
		var stories := 1 + int(jitter % 5) + int(core * core * 10.0)
		stories = clampi(stories, 1, MAX_STORIES)
		stories_total += stories
		var height := float(stories) * STORY_M
		if height > max_height:
			max_height = height
		checksum = _mix(checksum, stories)

		var offset := index * FLOATS_PER_INSTANCE
		# Row-major 3x4 transform: scale a unit X/Z footprint and the full
		# building height, then place the box centre half the height up.
		data[offset + 0] = BOX_EDGE_M
		data[offset + 1] = 0.0
		data[offset + 2] = 0.0
		data[offset + 3] = origin_x + float(column) * GRID_SPACING_M
		data[offset + 4] = 0.0
		data[offset + 5] = height
		data[offset + 6] = 0.0
		data[offset + 7] = height * 0.5
		data[offset + 8] = 0.0
		data[offset + 9] = 0.0
		data[offset + 10] = BOX_EDGE_M
		data[offset + 11] = origin_z + float(row) * GRID_SPACING_M
	var layout_usec := Time.get_ticks_usec() - started_usec

	var box := BoxMesh.new()
	box.size = Vector3(BOX_EDGE_M, BOX_EDGE_M, BOX_EDGE_M)
	multimesh = MultiMesh.new()
	multimesh.mesh = box
	multimesh.transform_format = MultiMesh.TRANSFORM_3D
	multimesh.instance_count = building_count
	multimesh.custom_aabb = AABB(
		Vector3(origin_x - half_box, 0.0, origin_z - half_box),
		Vector3(extent_x + BOX_EDGE_M, max_height + BOX_EDGE_M, extent_z + BOX_EDGE_M))
	var buffer_started_usec := Time.get_ticks_usec()
	multimesh.buffer = data
	var buffer_usec := Time.get_ticks_usec() - buffer_started_usec

	multimesh_instance = MultiMeshInstance3D.new()
	multimesh_instance.name = "MetropolisGateBuildings"
	multimesh_instance.multimesh = multimesh
	# Match the existing box-city harness: no shadow casting unless asked.
	multimesh_instance.cast_shadow = (
		GeometryInstance3D.SHADOW_CASTING_SETTING_ON if shadows_enabled
		else GeometryInstance3D.SHADOW_CASTING_SETTING_OFF)
	_container = Node3D.new()
	_container.name = "MetropolisGate"
	add_child(_container)
	_container.add_child(multimesh_instance)

	var mem_after_kb := OS.get_static_memory_usage() / 1024
	_city_stats = {
		"instances": building_count,
		"columns": columns,
		"rows": rows,
		"spacing_m": GRID_SPACING_M,
		"box_edge_m": BOX_EDGE_M,
		"story_m": STORY_M,
		"max_stories": MAX_STORIES,
		"stories_total": stories_total,
		"max_height_m": _round2(max_height),
		"build_ms": _ms(Time.get_ticks_usec() - started_usec),
		"layout_ms": _ms(layout_usec),
		"buffer_ms": _ms(buffer_usec),
		"buffer_bytes": data.size() * FLOAT_BYTES,
		"buffer_mb": _round2(float(data.size() * FLOAT_BYTES) / (1024.0 * 1024.0)),
		"cpu_mem_kb_delta": mem_after_kb - mem_before_kb,
		"checksum": _checksum_hex(checksum),
	}

func _add_environment() -> void:
	light = DirectionalLight3D.new()
	light.rotation_degrees = Vector3(-50.0, -30.0, 0.0)
	light.light_energy = 1.2
	light.shadow_enabled = shadows_enabled
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

	# One ground quad so the skyline sits on a surface. Two triangles; it does
	# not change the instancing comparison.
	var ground := PlaneMesh.new()
	ground.size = Vector2(
		float(_city_stats.get("columns", 1)) * GRID_SPACING_M + 4000.0,
		float(_city_stats.get("rows", 1)) * GRID_SPACING_M + 4000.0)
	var material := StandardMaterial3D.new()
	material.albedo_color = Color(0.26, 0.30, 0.26)
	material.roughness = 1.0
	ground.material = material
	var ground_node := MeshInstance3D.new()
	ground_node.name = "Ground"
	ground_node.mesh = ground
	ground_node.position.y = -0.05
	ground_node.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	add_child(ground_node)

	camera = Camera3D.new()
	camera.current = true
	camera.near = 1.0
	var extent := maxf(
		float(_city_stats.get("columns", 1)) * GRID_SPACING_M,
		float(_city_stats.get("rows", 1)) * GRID_SPACING_M)
	camera.far = maxf(12_000.0, extent * 2.5)
	add_child(camera)

func _load_route(path: String) -> bool:
	var text := FileAccess.get_file_as_string(path)
	if text.is_empty():
		push_error("Could not read route: " + path)
		return false
	var parsed: Variant = JSON.parse_string(text)
	if typeof(parsed) != TYPE_DICTIONARY:
		push_error("Route is not a JSON object: " + path)
		return false
	route = parsed
	waypoints = route.get("waypoints", [])
	if waypoints.size() < 2:
		push_error("Route needs at least two waypoints: " + path)
		return false
	route_id = str(route.get("id", "unknown"))
	return true

func _apply_route(t: float) -> void:
	if camera == null or waypoints.size() < 2:
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

func _finish() -> void:
	if finished:
		return
	finished = true
	var sorted := samples.duplicate()
	sorted.sort()
	var count := sorted.size()
	var frame_total := 0.0
	for value in sorted:
		frame_total += value
	var frame_avg := frame_total / maxf(float(count), 1.0)
	var p95 := _percentile(sorted, 0.95)
	var p99 := _percentile(sorted, 0.99)
	var p999 := _percentile(sorted, 0.999)
	var hitches_16 := 0
	var hitches_33 := 0
	var hitches_50 := 0
	var stalls_250 := 0
	for value in sorted:
		if value > 250.0:
			stalls_250 += 1
		if value > 50.0:
			hitches_50 += 1
		if value > 33.0:
			hitches_33 += 1
		if value > 16.0:
			hitches_16 += 1
	var headless := DisplayServer.get_name() == "headless"
	var fps_avg := 1000.0 / maxf(frame_avg, 0.001)
	var fps_low_1 := 1000.0 / maxf(p99, 0.001)
	var fps_low_0_1 := 1000.0 / maxf(p999, 0.001)
	var checks := {
		"fps_avg": fps_avg >= GATE_MIN_FPS_AVG,
		"fps_low_1pct": fps_low_1 >= GATE_MIN_FPS_LOW_1PCT,
		"fps_low_0_1pct": fps_low_0_1 >= GATE_MIN_FPS_LOW_0_1PCT,
		"frametime_ms_p99": p99 <= GATE_MAX_P99_MS,
		"frametime_ms_p99_9": p999 <= GATE_MAX_P99_9_MS,
		"hitches_50ms": hitches_50 <= GATE_MAX_HITCHES_50MS,
		"stalls_250ms": stalls_250 <= GATE_MAX_STALLS_250MS,
	}
	var all_checks := true
	for key in checks:
		if not bool(checks[key]):
			all_checks = false
	var verdict := "not_measured"
	var reason := "headless run: the dummy renderer does not measure FPS"
	if not headless:
		verdict = "fps_targets_met" if all_checks else "not_met"
		reason = (
			"all docs/68 late frame thresholds met on this run; this is still not "
			+ "the full gate (synthetic boxes, one run, not a verified reference rig)"
			) if all_checks else "one or more docs/68 late frame thresholds missed"
	var result := {
		"format_version": 1,
		"suite": "metropolis_gate",
		"gate": "G5",
		"scene": "metropolis_gate_boxes_%dk" % int(building_count / 1000),
		"scene_kind": "synthetic_boxes",
		"run_kind": "fps",
		"route": route_id,
		"route_path": route_path,
		"route_hash": _checksum_hex(_route_hash),
		"duration_s": duration_s,
		"warmup_s": warmup_s,
		"measured_s": _round2(measured_s),
		"headless": headless,
		"engine": Engine.get_version_info(),
		"hardware": _hardware,
		"settings": _settings,
		"settings_hash": _checksum_hex(_hash_settings()),
		"city": _city_stats,
		"git_commit": git_commit,
		"git_dirty": git_dirty,
		"frames": count,
		"results": {
			"fps_avg": _round2(fps_avg),
			"fps_low_1pct": _round2(fps_low_1),
			"fps_low_0_1pct": _round2(fps_low_0_1),
			"frametime_ms_avg": _round2(frame_avg),
			"frametime_ms_median": _round2(_percentile(sorted, 0.50)),
			"frametime_ms_p95": _round2(p95),
			"frametime_ms_p99": _round2(p99),
			"frametime_ms_p99_9": _round2(p999),
			"frametime_ms_min": _round2(sorted[0]) if count > 0 else 0.0,
			"frametime_ms_max": _round2(sorted[count - 1]) if count > 0 else 0.0,
			"hitches_16ms": hitches_16,
			"hitches_33ms": hitches_33,
			"hitches_50ms": hitches_50,
			"stalls_250ms": stalls_250,
		},
		"thresholds": {
			"fps_avg_min": GATE_MIN_FPS_AVG,
			"fps_low_1pct_min": GATE_MIN_FPS_LOW_1PCT,
			"fps_low_0_1pct_min": GATE_MIN_FPS_LOW_0_1PCT,
			"frametime_ms_p99_max": GATE_MAX_P99_MS,
			"frametime_ms_p99_9_max": GATE_MAX_P99_9_MS,
			"hitches_50ms_max": GATE_MAX_HITCHES_50MS,
			"stalls_250ms_max": GATE_MAX_STALLS_250MS,
		},
		"checks": checks,
		"gate_result": verdict,
		"gate_scope": "frame_budget_only",
		"gate_reason": reason,
	}
	_write_result(result, "METROPOLIS_GATE_RESULT")

func _write_setup() -> void:
	var result := {
		"format_version": 1,
		"suite": "metropolis_gate",
		"gate": "G5",
		"scene": "metropolis_gate_boxes_%dk" % int(building_count / 1000),
		"scene_kind": "synthetic_boxes",
		"run_kind": "setup",
		"route": route_id,
		"route_path": route_path,
		"route_hash": _checksum_hex(_hash_route()),
		"duration_s": duration_s,
		"warmup_s": warmup_s,
		"headless": DisplayServer.get_name() == "headless",
		"engine": Engine.get_version_info(),
		"hardware": _hardware,
		"settings": _settings,
		"settings_hash": _checksum_hex(_hash_settings()),
		"city": _city_stats,
		"git_commit": git_commit,
		"git_dirty": git_dirty,
		"setup_status": "ok",
		"gate_result": "not_measured",
		"gate_scope": "setup_only",
		"gate_reason": "setup-only run: no frame samples, no FPS claim",
	}
	_write_result(result, "METROPOLIS_GATE_SETUP")
	# `_write_result` quits with 0.

func _write_result(result: Dictionary, marker: String) -> void:
	DirAccess.make_dir_recursive_absolute(output_path.get_base_dir())
	var file := FileAccess.open(output_path, FileAccess.WRITE)
	if file == null:
		push_error("Could not write result: " + output_path)
		get_tree().quit(1)
		return
	file.store_string(JSON.stringify(result, "  "))
	file.close()
	print(marker + " " + JSON.stringify(result))
	get_tree().quit(0)

func _settings_info() -> Dictionary:
	var viewport := get_viewport()
	var window_size := DisplayServer.window_get_size()
	return {
		"resolution": [window_size.x, window_size.y],
		"viewport_size": [
			int(viewport.get_visible_rect().size.x), int(viewport.get_visible_rect().size.y)],
		"window_mode": DisplayServer.window_get_mode(),
		"vsync_mode": DisplayServer.window_get_vsync_mode(),
		"max_fps": Engine.max_fps,
		"rendering_method": str(ProjectSettings.get_setting(
			"rendering/renderer/rendering_method", "unknown")),
		"msaa_3d": viewport.msaa_3d,
		"screen_space_aa": viewport.screen_space_aa,
		"use_taa": viewport.use_taa,
		"use_debanding": viewport.use_debanding,
		"scaling_3d_mode": viewport.scaling_3d_mode,
		"scaling_3d_scale": viewport.scaling_3d_scale,
		"shadows": "on" if shadows_enabled else "off",
		"shadow_enabled": light.shadow_enabled if light != null else false,
		"shadow_blur": light.shadow_blur if light != null else 0.0,
	}

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
		"rendering_method": str(ProjectSettings.get_setting(
			"rendering/renderer/rendering_method", "unknown")),
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

func _hash_route() -> int:
	var hash := FNV_OFFSET_BASIS
	hash = _mix_string(hash, route_id)
	hash = _mix(hash, int(duration_s * 1000.0))
	hash = _mix(hash, int(warmup_s * 1000.0))
	for waypoint in waypoints:
		var wp: Dictionary = waypoint
		hash = _mix(hash, int(float(wp.get("t", 0.0)) * 10_000.0))
		for key in ["position", "look_at"]:
			var values: Array = wp.get(key, [0, 0, 0])
			for value in values:
				hash = _mix(hash, int(round(float(value) * 1000.0)))
		hash = _mix(hash, int(round(float(wp.get("fov", 70.0)) * 100.0)))
	return hash

func _hash_settings() -> int:
	var hash := FNV_OFFSET_BASIS
	hash = _mix_string(hash, str(_settings.get("resolution", [])))
	hash = _mix(hash, int(_settings.get("vsync_mode", 0)))
	hash = _mix(hash, int(_settings.get("max_fps", 0)))
	hash = _mix_string(hash, str(_settings.get("rendering_method", "")))
	hash = _mix(hash, int(_settings.get("msaa_3d", 0)))
	hash = _mix(hash, int(_settings.get("screen_space_aa", 0)))
	hash = _mix(hash, 1 if bool(_settings.get("use_taa", false)) else 0)
	hash = _mix(hash, 1 if shadows_enabled else 0)
	return hash

func _configure_target_window() -> void:
	if DisplayServer.get_name() == "headless":
		return
	DisplayServer.window_set_size(Vector2i(1920, 1080))
	DisplayServer.window_set_vsync_mode(DisplayServer.VSYNC_DISABLED)
	Engine.max_fps = 0

func _percentile(sorted_values: Array[float], fraction: float) -> float:
	if sorted_values.is_empty():
		return 0.0
	var index := clampi(
		int(round(float(sorted_values.size() - 1) * fraction)),
		0, sorted_values.size() - 1)
	return sorted_values[index]

func _arg(args: PackedStringArray, name: String, fallback: String) -> String:
	var index := args.find(name)
	if index >= 0 and index + 1 < args.size():
		return args[index + 1]
	return fallback

func _project_root() -> String:
	return ProjectSettings.globalize_path("res://").path_join("..").simplify_path()

func _fail(message: String) -> void:
	push_error(message)
	print("METROPOLIS_GATE status=error message=%s" % message)
	get_tree().quit(1)

func _mix(hash: int, value: int) -> int:
	hash = (hash ^ (value & 0xFFFFFFFF)) & 0xFFFFFFFF
	return (hash * FNV_PRIME) & 0xFFFFFFFF

func _mix_string(hash: int, value: String) -> int:
	for index in value.length():
		hash = _mix(hash, value.unicode_at(index))
	return hash

func _checksum_hex(value: int) -> String:
	return "%08x" % (value & 0xFFFFFFFF)

func _ms(usec: int) -> float:
	return _round2(float(usec) / 1000.0)

func _round2(value: float) -> float:
	return round(value * 100.0) / 100.0
