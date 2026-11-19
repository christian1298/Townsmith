extends Node3D

func _ready() -> void:
	var core = TownsmithCore.new()
	print("Townsmith core version: ", core.version())

	var demo: ArrayMesh = core.build_demo_mesh()
	print("Demo mesh surfaces: ", demo.get_surface_count())

	var terrain: ArrayMesh = core.build_terrain_mesh(2026, 0, 0)
	print("Terrain mesh surfaces: ", terrain.get_surface_count())

	var terrain_instance := MeshInstance3D.new()
	terrain_instance.mesh = terrain
	var terrain_material := StandardMaterial3D.new()
	terrain_material.vertex_color_use_as_albedo = true
	terrain_material.roughness = 1.0
	terrain_material.cull_mode = BaseMaterial3D.CULL_DISABLED
	terrain_instance.material_override = terrain_material
	add_child(terrain_instance)

	var water := MeshInstance3D.new()
	var water_plane := PlaneMesh.new()
	water_plane.size = Vector2(256.0, 256.0)
	water.mesh = water_plane
	water.position = Vector3(128.0, 24.0, 128.0)
	var water_material := StandardMaterial3D.new()
	water_material.albedo_color = Color(0.16, 0.38, 0.62, 0.55)
	water_material.transparency = BaseMaterial3D.TRANSPARENCY_ALPHA
	water_material.cull_mode = BaseMaterial3D.CULL_DISABLED
	water.material_override = water_material
	add_child(water)

	var camera := Camera3D.new()
	camera.position = Vector3(128.0, 92.0, 244.0)
	camera.far = 1000.0
	add_child(camera)
	camera.look_at(Vector3(128.0, 28.0, 128.0))

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
