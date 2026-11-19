extends Node3D

func _ready() -> void:
	var core = TownsmithCore.new()
	print("Townsmith core version: ", core.version())

	var mesh: ArrayMesh = core.build_demo_mesh()
	print("Demo mesh surfaces: ", mesh.get_surface_count())

	var mesh_instance := MeshInstance3D.new()
	mesh_instance.mesh = mesh
	var material := StandardMaterial3D.new()
	material.albedo_color = Color(0.62, 0.48, 0.33)
	material.roughness = 0.9
	mesh_instance.material_override = material
	add_child(mesh_instance)

	var ground := MeshInstance3D.new()
	var plane := PlaneMesh.new()
	plane.size = Vector2(12.0, 12.0)
	ground.mesh = plane
	var ground_material := StandardMaterial3D.new()
	ground_material.albedo_color = Color(0.27, 0.45, 0.22)
	ground.material_override = ground_material
	add_child(ground)

	var camera := Camera3D.new()
	camera.position = Vector3(5.0, 4.2, 5.0)
	add_child(camera)
	camera.look_at(Vector3(0.0, 1.2, 0.0))

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
