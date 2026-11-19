extends Node3D

const TerrainStreamer = preload("res://terrain_streamer.gd")

var streamer

func _ready() -> void:
	var core = TownsmithCore.new()
	print("Townsmith core version: ", core.version())

	var demo: ArrayMesh = core.build_demo_mesh()
	print("Demo mesh surfaces: ", demo.get_surface_count())

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
	var status := "ok"
	if streamer.chunk_count() < 9:
		status = "timeout"
	elif not lods.has(1) or not lods.has(2):
		status = "missing_lods"
	print("SMOKE chunks=%d lods=%s status=%s elapsed_ms=%d" % [
		streamer.chunk_count(),
		lod_text,
		status,
		Time.get_ticks_msec() - started,
	])
	get_tree().quit(0 if status == "ok" else 1)
