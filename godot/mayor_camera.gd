extends Camera3D

## RTS-style Mayor Mode camera.
##
## The camera orbits a terrain target: WASD pans the target, Q/E rotates the
## view, the mouse wheel zooms, and Shift boosts pan. The scene keeps the
## target's Y on the sampled terrain. There is no collision or edge clamp yet.

@export var move_speed_mps := 80.0
@export var boost_multiplier := 3.0
@export var zoom_step := 1.12
@export var min_distance_m := 10.0
@export var max_distance_m := 420.0
@export var min_pitch_deg := 20.0
@export var max_pitch_deg := 82.0

var target := Vector3.ZERO
var yaw_deg := 0.0
var pitch_deg := 55.0
var distance := 80.0

func _ready() -> void:
	refresh()

func focus(point: Vector3, new_distance: float = -1.0) -> void:
	target = point
	if new_distance > 0.0:
		distance = clampf(new_distance, min_distance_m, max_distance_m)
	refresh()

func refresh() -> void:
	distance = clampf(distance, min_distance_m, max_distance_m)
	pitch_deg = clampf(pitch_deg, min_pitch_deg, max_pitch_deg)
	var yaw := deg_to_rad(yaw_deg)
	var pitch := deg_to_rad(pitch_deg)
	var offset := Vector3(
		sin(yaw) * cos(pitch),
		sin(pitch),
		cos(yaw) * cos(pitch)
	) * distance
	global_position = target + offset
	look_at(target, Vector3.UP if absf(pitch_deg) < 89.0 else Vector3.FORWARD)

func _process(delta: float) -> void:
	if not current or Input.mouse_mode == Input.MOUSE_MODE_CAPTURED:
		return
	var yaw := deg_to_rad(yaw_deg)
	var forward := Vector3(-sin(yaw), 0.0, -cos(yaw))
	var right := Vector3(cos(yaw), 0.0, -sin(yaw))
	var direction := Vector3.ZERO
	if Input.is_physical_key_pressed(KEY_W):
		direction += forward
	if Input.is_physical_key_pressed(KEY_S):
		direction -= forward
	if Input.is_physical_key_pressed(KEY_D):
		direction += right
	if Input.is_physical_key_pressed(KEY_A):
		direction -= right
	var changed := false
	if direction != Vector3.ZERO:
		var speed := move_speed_mps
		if Input.is_physical_key_pressed(KEY_SHIFT):
			speed *= boost_multiplier
		target += direction.normalized() * speed * minf(delta, 0.1)
		changed = true
	if Input.is_physical_key_pressed(KEY_Q):
		yaw_deg = wrapf(yaw_deg - 45.0 * delta, -180.0, 180.0)
		changed = true
	if Input.is_physical_key_pressed(KEY_E):
		yaw_deg = wrapf(yaw_deg + 45.0 * delta, -180.0, 180.0)
		changed = true
	if changed:
		refresh()

func _unhandled_input(event: InputEvent) -> void:
	if not current:
		return
	if event is InputEventMouseButton and event.pressed:
		if event.button_index == MOUSE_BUTTON_WHEEL_UP:
			distance /= zoom_step
			refresh()
		elif event.button_index == MOUSE_BUTTON_WHEEL_DOWN:
			distance *= zoom_step
			refresh()
