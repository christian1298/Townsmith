extends MeshInstance3D

## Lightweight stylized water plane for the Phase 0 preview.
##
## Owns a single large PlaneMesh and the shader material from water.gdshader.
## The streamer calls follow() every frame so the plane stays centered on the
## camera in XZ while sitting at the terrain water level.

const WATER_SHADER := preload("res://water.gdshader")
const PLANE_SIZE := 8192.0
const PLANE_SUBDIVISIONS := 128

@export var height := 24.1:
	set(value):
		height = value
		if is_inside_tree():
			global_position.y = value

@export var shallow_color := Color(0.24, 0.58, 0.60, 1.0):
	set(value):
		shallow_color = value
		_set_shader_parameter("shallow_color", value)

@export var deep_color := Color(0.05, 0.17, 0.28, 1.0):
	set(value):
		deep_color = value
		_set_shader_parameter("deep_color", value)

@export var sky_color := Color(0.62, 0.80, 0.92, 1.0):
	set(value):
		sky_color = value
		_set_shader_parameter("sky_color", value)

@export_range(0.0, 1.0, 0.01) var opacity := 0.74:
	set(value):
		opacity = value
		_set_shader_parameter("opacity", value)

@export_range(0.0, 4.0, 0.01) var wave_speed := 0.6:
	set(value):
		wave_speed = value
		_set_shader_parameter("wave_speed", value)

@export_range(0.1, 4.0, 0.01) var wave_scale := 1.0:
	set(value):
		wave_scale = value
		_set_shader_parameter("wave_scale", value)

@export_range(0.0, 0.5, 0.005) var wave_height := 0.05:
	set(value):
		wave_height = value
		_set_shader_parameter("wave_height", value)

@export_range(1.0, 8.0, 0.1) var fresnel_power := 4.0:
	set(value):
		fresnel_power = value
		_set_shader_parameter("fresnel_power", value)

@export_range(25.0, 600.0, 5.0) var detail_distance := 160.0:
	set(value):
		detail_distance = value
		_set_shader_parameter("detail_distance", value)

@export_range(50.0, 2000.0, 10.0) var depth_distance := 420.0:
	set(value):
		depth_distance = value
		_set_shader_parameter("depth_distance", value)

var shader_material: ShaderMaterial

func _ready() -> void:
	var plane := PlaneMesh.new()
	plane.size = Vector2(PLANE_SIZE, PLANE_SIZE)
	plane.subdivide_width = PLANE_SUBDIVISIONS
	plane.subdivide_depth = PLANE_SUBDIVISIONS
	mesh = plane
	cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF

	shader_material = ShaderMaterial.new()
	shader_material.shader = WATER_SHADER
	material_override = shader_material
	_apply_all_parameters()
	position.y = height

## Centers the plane on a target in XZ and keeps it at the water height.
func follow(target: Node3D) -> void:
	if not is_instance_valid(target):
		return
	global_position = Vector3(target.global_position.x, height, target.global_position.z)

func _apply_all_parameters() -> void:
	_set_shader_parameter("shallow_color", shallow_color)
	_set_shader_parameter("deep_color", deep_color)
	_set_shader_parameter("sky_color", sky_color)
	_set_shader_parameter("opacity", opacity)
	_set_shader_parameter("wave_speed", wave_speed)
	_set_shader_parameter("wave_scale", wave_scale)
	_set_shader_parameter("wave_height", wave_height)
	_set_shader_parameter("fresnel_power", fresnel_power)
	_set_shader_parameter("detail_distance", detail_distance)
	_set_shader_parameter("depth_distance", depth_distance)

func _set_shader_parameter(name: StringName, value: Variant) -> void:
	if shader_material != null:
		shader_material.set_shader_parameter(name, value)
