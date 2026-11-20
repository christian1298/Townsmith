extends Node3D

const TerrainStreamer = preload("res://terrain_streamer.gd")
const SMOKE_CITY_TIER := 2
const SMOKE_CITY_SEED := 2026
const SMOKE_CITY_BUILDINGS := 100000
const SMOKE_CITY_CHECKSUM := "2595171946372580485"

var streamer

func _ready() -> void:
	var core = TownsmithCore.new()
	print("Townsmith core version: ", core.version())

	var demo: ArrayMesh = core.build_demo_mesh()
	print("Demo mesh surfaces: ", demo.get_surface_count())

	if OS.get_cmdline_user_args().has("--volume-smoke"):
		_run_volume_smoke(core)
		return

	streamer = TerrainStreamer.new()
	streamer.seed = 2026
	streamer.view_radius = 1
	streamer.keep_radius = 2
	streamer.builds_per_frame = 2
	add_child(streamer)

	var camera := Camera3D.new()
	camera.position = Vector3(128.0, 92.0, 244.0)
	camera.far = 1500.0
	add_child(camera)
	camera.look_at(Vector3(128.0, 28.0, 128.0))
	streamer.bind_camera(camera)

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

	if OS.get_cmdline_user_args().has("--smoke"):
		_run_smoke()

func _run_volume_smoke(core) -> void:
	var status := "ok"
	if not core.create_editor_volume(0, 0, 0):
		status = "create_failed"
	elif not core.editor_fill_box(0, 0, 0, 160, 1, 160, 1):
		status = "fill_failed"
	elif not core.editor_set_voxel(10, 1, 10, 5):
		status = "set_failed"
	elif core.editor_get_voxel(10, 1, 10) != 5:
		status = "get_mismatch"
	elif core.editor_set_voxel(9999, 0, 0, 1):
		status = "oob_set_allowed"
	elif core.editor_set_voxel(0, 0, 0, 65536):
		status = "bad_material_allowed"
	elif not core.editor_clear():
		status = "clear_failed"
	elif not core.editor_fill_box(0, 0, 0, 160, 1, 160, 1):
		status = "refill_failed"
	elif not core.editor_set_voxel(10, 1, 10, 5):
		status = "reset_failed"

	var mesh: ArrayMesh = core.build_editor_volume_mesh()
	var stats: Dictionary = core.editor_volume_stats()
	var bounds: PackedFloat32Array = core.editor_volume_bounds()
	var solid := int(stats.get("solid_voxels", -1))
	var quads := int(stats.get("last_mesh_quads", -1))
	var vertices := int(stats.get("last_mesh_vertices", -1))
	var surfaces := mesh.get_surface_count()
	if solid != 25601:
		status = "solid_mismatch"
	elif surfaces != 1:
		status = "surface_mismatch"
	elif quads <= 0 or vertices != quads * 4:
		status = "mesh_mismatch"
	elif bounds.size() != 3 or absf(bounds[0] - 16.0) > 0.001 or absf(bounds[1] - 9.6) > 0.001 or absf(bounds[2] - 16.0) > 0.001:
		status = "bounds_mismatch"

	print("VOLUME_SMOKE solid=%d quads=%d vertices=%d surfaces=%d bounds=%.1f,%.1f,%.1f mesh_ms=%.1f status=%s" % [
		solid,
		quads,
		vertices,
		surfaces,
		bounds[0] if bounds.size() > 0 else -1.0,
		bounds[1] if bounds.size() > 1 else -1.0,
		bounds[2] if bounds.size() > 2 else -1.0,
		float(stats.get("mesh_ms", 0.0)),
		status,
	])
	get_tree().quit(0 if status == "ok" else 1)

func _run_smoke() -> void:
	var started := Time.get_ticks_msec()
	var deadline := started + 15000
	while streamer.chunk_count() < 9 and Time.get_ticks_msec() < deadline:
		await get_tree().process_frame

	var lods: Array[int] = streamer.loaded_lods()
	var lod_text := ""
	for lod in lods:
		if lod_text != "":
			lod_text += ","
		lod_text += str(lod)
	var city_core := TownsmithCore.new()
	var city: Dictionary = city_core.build_city_layout(SMOKE_CITY_TIER, SMOKE_CITY_SEED)
	var city_error := str(city.get("error", ""))
	var city_tier := str(city.get("tier", ""))
	var city_buildings := int(city.get("buildings", 0))
	var city_positions: PackedVector3Array = city.get("positions", PackedVector3Array())
	var city_sizes: PackedVector3Array = city.get("sizes", PackedVector3Array())
	var city_colors: PackedColorArray = city.get("colors", PackedColorArray())
	var city_kinds: PackedInt32Array = city.get("kinds", PackedInt32Array())
	var city_bounds: PackedFloat32Array = city.get("bounds", PackedFloat32Array())
	var city_checksum := str(city.get("checksum", ""))
	print("CITY tier=%s buildings=%d positions=%d sizes=%d colors=%d kinds=%d bounds=%d checksum=%s error=%s" % [
		city_tier,
		city_buildings,
		city_positions.size(),
		city_sizes.size(),
		city_colors.size(),
		city_kinds.size(),
		city_bounds.size(),
		city_checksum,
		city_error,
	])

	var status := "ok"
	if streamer.chunk_count() < 9:
		status = "timeout"
	elif not lods.has(1) or not lods.has(2):
		status = "missing_lods"
	elif city_error != "":
		status = "city_error"
	elif city_tier != "late" or city_buildings != SMOKE_CITY_BUILDINGS:
		status = "city_failed"
	elif city_positions.size() != SMOKE_CITY_BUILDINGS:
		status = "city_positions"
	elif city_sizes.size() != SMOKE_CITY_BUILDINGS:
		status = "city_sizes"
	elif city_colors.size() != SMOKE_CITY_BUILDINGS:
		status = "city_colors"
	elif city_kinds.size() != SMOKE_CITY_BUILDINGS:
		status = "city_kinds"
	elif city_bounds.size() != 4:
		status = "city_bounds"
	elif city_checksum != SMOKE_CITY_CHECKSUM:
		status = "city_checksum"
	print("SMOKE chunks=%d lods=%s status=%s elapsed_ms=%d" % [
		streamer.chunk_count(),
		lod_text,
		status,
		Time.get_ticks_msec() - started,
	])
	get_tree().quit(0 if status == "ok" else 1)
