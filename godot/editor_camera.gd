extends Camera3D

## First-person fly camera for the voxel editor shell.
##
## The editor owns mouse capture and editing; this node only turns relative
## mouse motion into yaw/pitch and moves on WASD + Space/Ctrl. Horizontal
## movement uses the yaw-only basis so looking down does not slow the walk,
## and diagonal input is normalized so strafing is not faster than walking
## forward.
##
## Pitch is clamped just short of vertical to avoid a degenerate look basis.
## Movement is free flight: there is no gravity or collision yet.

const MOVE_SPEED_MPS := 6.0
const BOOST_MULTIPLIER := 3.0
const MOUSE_SENSITIVITY := 0.0022
const PITCH_LIMIT := 1.5533  # 89 degrees

var _yaw := 0.0
var _pitch := 0.0

func _ready() -> void:
	sync_look_from_rotation()

## Read the node's current rotation into the internal yaw/pitch pair.
##
## Call this after `look_at` so the first mouse motion continues from the
## authored view instead of snapping back to zero.
func sync_look_from_rotation() -> void:
	_yaw = rotation.y
	_pitch = clampf(rotation.x, -PITCH_LIMIT, PITCH_LIMIT)

func _unhandled_input(event: InputEvent) -> void:
	if event is InputEventMouseMotion and Input.mouse_mode == Input.MOUSE_MODE_CAPTURED:
		_yaw = wrapf(_yaw - event.relative.x * MOUSE_SENSITIVITY, -PI, PI)
		_pitch = clampf(_pitch - event.relative.y * MOUSE_SENSITIVITY, -PITCH_LIMIT, PITCH_LIMIT)
		rotation = Vector3(_pitch, _yaw, 0.0)

func _process(delta: float) -> void:
	if Input.mouse_mode != Input.MOUSE_MODE_CAPTURED:
		return

	var forward := Vector3(-sin(_yaw), 0.0, -cos(_yaw))
	var right := Vector3(cos(_yaw), 0.0, -sin(_yaw))
	var direction := Vector3.ZERO
	if Input.is_physical_key_pressed(KEY_W):
		direction += forward
	if Input.is_physical_key_pressed(KEY_S):
		direction -= forward
	if Input.is_physical_key_pressed(KEY_D):
		direction += right
	if Input.is_physical_key_pressed(KEY_A):
		direction -= right
	if Input.is_physical_key_pressed(KEY_SPACE):
		direction += Vector3.UP
	if Input.is_physical_key_pressed(KEY_CTRL):
		direction -= Vector3.UP

	if direction == Vector3.ZERO:
		return
	var speed := MOVE_SPEED_MPS
	if Input.is_physical_key_pressed(KEY_SHIFT):
		speed *= BOOST_MULTIPLIER
	# A synchronous remesh can stall one frame; clamp the step so the camera
	# does not teleport when the next frame arrives.
	global_position += direction.normalized() * speed * minf(delta, 0.1)
