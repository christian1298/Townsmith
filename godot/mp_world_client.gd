extends Node3D

## Debug 3D multiplayer world client.
##
## This is the first client render integration for the live protocol v0 path:
## it connects to a loopback `townsmith-server`, sends `hello` and a
## deterministic `set_interest`, then draws the server's bounded `interest_view`
## road and building records in 3D world space (1 unit = 1 m). It is a debug
## client, not gameplay networking: there is no prediction, interpolation,
## terrain/chunk/entity/agent streaming, delta snapshot, or production camera.
##
## Flow:
##   hello -> welcome
##   six deterministic place_road commands -> six snapshots (server-authoritative)
##   set_interest {center 0,0 radius 96 m} -> snapshot + interest_view
##   build the 3D scene from the received view
##   --smoke only: set_interest {center 32,32 radius 16 m} -> snapshot +
##     interest_view with zero roads/buildings, proving the render clears
##   disconnect -> goodbye
##
## The scene draws a ground plane, a world-aligned grid with world axes, the
## interest ring, one flat box per road, and one box per bounded building
## record. Roads are 6 m wide and 0.24 m thick at their centreline; building
## boxes use the server's `radius_m` render hint as a footprint diameter (not an
## authoritative footprint). A HUD shows tick, population, balance, road and
## building counts, both truncation flags, and the server checksum.
##
## `godot/mp_interest_render.gd` is reused for road parsing, the 64-road wire
## cap, and the deterministic 16-hex road checksum. Buildings are parsed
## defensively here (cap 64, ascending id, malformed entries skipped) with a
## separate 16-hex checksum. Headless Godot returns identity transforms from
## `MultiMesh`, so the smoke asserts CPU-side transform mirrors, never the
## GPU-side copies.
##
## Reconnect/resume is deliberately out of scope: `godot/mp_client.gd`
## (`--reconnect`) already covers the resume wire path, and this scene is a
## one-shot render probe. It does not fake or replay authoritative state.
##
## Run:
##   godot --path godot res://mp_world_client.tscn -- --port 47015
##   godot --headless --path godot res://mp_world_client.tscn -- --smoke --port 47015
##
## `scripts/smoke_mp_world_client.sh` starts a fresh `--sessions 1` server per
## run and asserts identical `MP_WORLD_CLIENT` / `MP_WORLD_CLIENT_ZERO` markers
## across two runs.
##
## See docs/05 - Multiplayer.md §Interest view 3D world client.

const InterestRender = preload("res://mp_interest_render.gd")

enum Phase {
	CONNECTING,
	HELLO_SENT,
	ROAD_SENT,
	INTEREST_SENT,
	INTEREST_VIEW_WAIT,
	ZERO_INTEREST_SENT,
	ZERO_VIEW_WAIT,
	DISCONNECT_SENT,
}

const PROTOCOL_VERSION := 0
const MAX_FRAME_LEN := 1024 * 1024
const DEFAULT_HOST := "127.0.0.1"
const DEFAULT_PORT := 47015
const DEFAULT_NAME := "mp-world-client"
const CONNECT_TIMEOUT_MS := 5000
const STEP_TIMEOUT_MS := 5000
# Same 64-character placeholder hash the Rust loopback harness uses.
const CONTENT_HASH := "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"

const DEFAULT_CENTER := Vector2(0.0, 0.0)
const DEFAULT_RADIUS_M := 96.0
# Zero/narrow-viewport probe: no road midpoint or synthetic building centre is
# within 16 m of this point.
const ZERO_CENTER := Vector2(32.0, 32.0)
const ZERO_RADIUS_M := 16.0
const MAX_RADIUS_M := 2048.0

# Six axis-aligned roads forming a square with a cross. Every midpoint is
# inside the 96 m interest circle and each segment is a legal 128-cell road.
const ROAD_PLAN := [
	{"start": [-64, 0], "end": [64, 0]},
	{"start": [0, -64], "end": [0, 64]},
	{"start": [-64, -64], "end": [-64, 64]},
	{"start": [64, -64], "end": [64, 64]},
	{"start": [-64, -64], "end": [64, -64]},
	{"start": [-64, 64], "end": [64, 64]},
]

const MAX_BUILDINGS := 64
const DEFAULT_BUILDING_RADIUS_M := 8.0
const MIN_BUILDING_RADIUS_M := 0.5
const MAX_BUILDING_RADIUS_M := 256.0
const BUILDING_HEIGHT_RATIO := 0.5
const MIN_BUILDING_HEIGHT_M := 1.0
const MAX_BUILDING_HEIGHT_M := 12.0

const MIN_WIDTH_M := 0.5
const MAX_WIDTH_M := 64.0
const ROAD_HEIGHT_M := 0.24
const ROAD_Y_M := 0.12
const MIN_ROAD_LENGTH_M := 0.5
const GROUND_MARGIN := 3.0
const GRID_STEP_M := 16.0
const GRID_MIN_EXTENT_M := 64.0
const RING_SEGMENTS := 96
const CAMERA_FOV := 50.0

const COLOR_GROUND := Color(0.065, 0.082, 0.105)
const COLOR_GRID := Color(0.18, 0.23, 0.29)
const COLOR_AXIS_X := Color(0.90, 0.34, 0.30)
const COLOR_AXIS_Y := Color(0.36, 0.86, 0.46)
const COLOR_AXIS_Z := Color(0.32, 0.56, 0.96)
const COLOR_RING := Color(0.32, 0.44, 0.58)
const COLOR_CENTER := Color(1.0, 0.92, 0.62)
const COLOR_INSIDE := Color(0.96, 0.78, 0.32)
const COLOR_OUTSIDE := Color(0.92, 0.28, 0.22)
const COLOR_BUILDING_INSIDE := Color(0.30, 0.82, 0.92)
const COLOR_BUILDING_OUTSIDE := Color(0.86, 0.36, 0.94)

var _peer: StreamPeerTCP
var _phase: int = Phase.CONNECTING
var _rx := PackedByteArray()
var _deadline_ms := 0
var _finished := false
var _status_text := "Connecting..."

var _smoke := false
var _host := DEFAULT_HOST
var _port := DEFAULT_PORT
var _player_name := DEFAULT_NAME
var _timeout_ms := STEP_TIMEOUT_MS
var _center := DEFAULT_CENTER
var _radius_m := DEFAULT_RADIUS_M

var _frames_sent := 0
var _frames_received := 0
var _road_index := 0
var _session_id := ""
var _session_token := ""
var _last_tick := -1
var _last_population := -1
var _last_balance := 0
var _last_checksum := ""
var _visible_roads := -1

var _container: Node3D
var _hud_layer: CanvasLayer
var _hud_label: Label
var _camera: Camera3D
var _road_multimesh: MultiMesh
var _building_multimesh: MultiMesh
var _roads: Array = []
var _buildings: Array = []
var _road_transforms: Array = []
var _building_transforms: Array = []
var _truncated := false
var _buildings_truncated := false
var _road_checksum := ""
var _building_checksum := ""
var _inside := 0
var _outside := 0
var _buildings_inside := 0
var _buildings_outside := 0
var _view_tick := -1
var _view_center := Vector2.ZERO
var _view_radius_m := 0.0
var _has_view := false
var _grid_lines := 0
var _main_marker := ""
var _zero_marker := ""

func _ready() -> void:
	var args := OS.get_cmdline_user_args()
	_smoke = args.has("--smoke")
	_host = _arg(args, "--host", DEFAULT_HOST)
	_player_name = _arg(args, "--name", DEFAULT_NAME)
	_timeout_ms = _positive_int(_arg(args, "--timeout-ms", ""), STEP_TIMEOUT_MS)
	_center = Vector2(
		_float_arg(args, "--center-x", DEFAULT_CENTER.x),
		_float_arg(args, "--center-z", DEFAULT_CENTER.y)
	)
	_radius_m = _float_arg(args, "--radius-m", DEFAULT_RADIUS_M)

	_container = Node3D.new()
	_container.name = "WorldClientRoot"
	add_child(_container)
	_build_hud()

	var port_text := _arg(args, "--port", str(DEFAULT_PORT))
	if not port_text.is_valid_int():
		_fail("invalid_port")
		return
	_port = port_text.to_int()
	if _port < 1 or _port > 65535:
		_fail("port_out_of_range")
		return
	if _player_name.strip_edges().is_empty() or _player_name.length() > 32:
		_fail("invalid_player_name")
		return
	if not _is_valid_view(_center, _radius_m):
		_fail("invalid_interest")
		return

	_peer = StreamPeerTCP.new()
	var error := _peer.connect_to_host(_host, _port)
	if error != OK:
		_fail("connect_start_%s" % error_string(error))
		return
	_arm_deadline(CONNECT_TIMEOUT_MS)
	_set_status("Connecting to %s:%d..." % [_host, _port])

func _process(_delta: float) -> void:
	if _finished:
		return
	if Time.get_ticks_msec() > _deadline_ms:
		_fail("timeout_%s" % _phase_name())
		return
	if _peer == null:
		_fail("peer_missing")
		return

	_peer.poll()
	if _phase == Phase.CONNECTING:
		match _peer.get_status():
			StreamPeerTCP.STATUS_CONNECTED:
				_on_connected()
			StreamPeerTCP.STATUS_ERROR:
				_fail("connect_failed")
			StreamPeerTCP.STATUS_NONE:
				_fail("connect_lost")
		return

	_drain()
	if _finished:
		return
	if _peer.get_status() != StreamPeerTCP.STATUS_CONNECTED:
		# Read the goodbye or final reject before treating FIN as a failure.
		_drain()
		if not _finished:
			_fail("connection_lost_%s" % _phase_name())

func _on_connected() -> void:
	_set_status("Connected; sending hello.")
	if not _send({
		"type": "hello",
		"protocol_version": PROTOCOL_VERSION,
		"content_hash": CONTENT_HASH,
		"player_name": _player_name,
	}):
		return
	_phase = Phase.HELLO_SENT
	_arm_deadline(_timeout_ms)

func _drain() -> void:
	var available := _peer.get_available_bytes()
	if available > 0:
		var result := _peer.get_partial_data(available)
		if result[0] != OK:
			_fail("recv_%s" % error_string(result[0]))
			return
		_rx.append_array(result[1])

	while not _finished and _rx.size() >= 4:
		var length := _read_u32_be(_rx, 0)
		if length == 0:
			_fail("frame.zero_length")
			return
		if length > MAX_FRAME_LEN:
			_fail("frame.oversized")
			return
		if _rx.size() < 4 + length:
			return
		var payload := _rx.slice(4, 4 + length)
		_rx = _rx.slice(4 + length)
		_frames_received += 1
		var parsed: Variant = JSON.parse_string(payload.get_string_from_utf8())
		if typeof(parsed) != TYPE_DICTIONARY:
			_fail("frame.invalid_json")
			return
		_handle_message(parsed)

func _handle_message(message: Dictionary) -> void:
	var tag := str(message.get("type", ""))
	match tag:
		"welcome":
			_on_welcome(message)
		"snapshot":
			_on_snapshot(message)
		"interest_view":
			_on_interest_view(message)
		"reject":
			_on_reject(message)
		"goodbye":
			_on_goodbye()
		_:
			_fail("unexpected_message_%s" % (tag if tag != "" else "missing_type"))

func _on_welcome(message: Dictionary) -> void:
	if _phase != Phase.HELLO_SENT:
		_fail("welcome_in_%s" % _phase_name())
		return
	var protocol_version := int(message.get("protocol_version", -1))
	if protocol_version != PROTOCOL_VERSION:
		_fail("protocol_version_%d" % protocol_version)
		return
	_session_id = str(message.get("session_id", ""))
	if _session_id.is_empty():
		_fail("welcome_session")
		return
	_session_token = str(message.get("session_token", ""))
	if bool(message.get("resumed", false)):
		_fail("unexpected_resumed_welcome")
		return
	var tick := int(message.get("tick", -1))
	if tick < 0:
		_fail("welcome_tick")
		return
	_last_tick = tick
	_set_status("Welcome %s; placing deterministic roads." % _session_id)
	_refresh_hud()
	_send_next_road()

## Send the next road in the deterministic plan, or the main set_interest once
## every road has been acknowledged.
func _send_next_road() -> void:
	if _road_index >= ROAD_PLAN.size():
		_set_status("Roads placed; requesting the %.0f m interest view." % _radius_m)
		if not _send({
			"type": "set_interest",
			"center_x": _center.x,
			"center_z": _center.y,
			"radius_m": _radius_m,
		}):
			return
		_phase = Phase.INTEREST_SENT
		_arm_deadline(_timeout_ms)
		return
	var plan: Dictionary = ROAD_PLAN[_road_index]
	var start: Array = plan.get("start", [0, 0])
	var end: Array = plan.get("end", [0, 0])
	if not _send({
		"type": "place_road",
		"start": [int(start[0]), int(start[1])],
		"end": [int(end[0]), int(end[1])],
	}):
		return
	_phase = Phase.ROAD_SENT
	_arm_deadline(_timeout_ms)

func _on_snapshot(message: Dictionary) -> void:
	var tick := int(message.get("tick", -1))
	var population := int(message.get("population", -1))
	var balance := int(message.get("balance", 0))
	var checksum := str(message.get("checksum", ""))
	if tick < 0 or population < 1 or not _is_hex16(checksum):
		_fail("snapshot_fields")
		return

	if _phase == Phase.ROAD_SENT:
		if tick != _last_tick + 1:
			_fail("road_tick_%d_after_%d" % [tick, _last_tick])
			return
		_store_snapshot(tick, population, balance, checksum)
		_road_index += 1
		_send_next_road()
		return

	if _phase == Phase.INTEREST_SENT:
		if not _check_view_snapshot(message, tick, checksum, _center, _radius_m):
			return
		_visible_roads = int(message.get("visible_roads", -1))
		if _visible_roads != ROAD_PLAN.size():
			_fail("interest_visible_roads_%d" % _visible_roads)
			return
		_store_snapshot(tick, population, balance, checksum)
		_set_status("Interest snapshot received; awaiting the detail view.")
		_refresh_hud()
		_phase = Phase.INTEREST_VIEW_WAIT
		_arm_deadline(_timeout_ms)
		return

	if _phase == Phase.ZERO_INTEREST_SENT:
		if not _check_view_snapshot(message, tick, checksum, ZERO_CENTER, ZERO_RADIUS_M):
			return
		_visible_roads = int(message.get("visible_roads", -1))
		if _visible_roads != 0:
			_fail("zero_visible_roads_%d" % _visible_roads)
			return
		_store_snapshot(tick, population, balance, checksum)
		_set_status("Zero-viewport snapshot received; awaiting the detail view.")
		_refresh_hud()
		_phase = Phase.ZERO_VIEW_WAIT
		_arm_deadline(_timeout_ms)
		return

	_fail("snapshot_in_%s" % _phase_name())

## Shared checks for a set_interest snapshot: the viewport is view-only, so the
## tick and checksum must not move and the echoed centre/radius must match.
func _check_view_snapshot(
	message: Dictionary, tick: int, checksum: String, center: Vector2, radius_m: float
) -> bool:
	if tick != _last_tick:
		_fail("interest_tick_%d_after_%d" % [tick, _last_tick])
		return false
	if checksum != _last_checksum:
		_fail("interest_checksum_changed")
		return false
	if not _is_pair_close(message.get("interest_center", null), center, 0.001):
		_fail("interest_center")
		return false
	var echoed_radius := float(message.get("interest_radius_m", -1.0))
	if absf(echoed_radius - radius_m) > 0.001:
		_fail("interest_radius_%.3f" % echoed_radius)
		return false
	return true

func _on_interest_view(message: Dictionary) -> void:
	if _phase != Phase.INTEREST_VIEW_WAIT and _phase != Phase.ZERO_VIEW_WAIT:
		_fail("interest_view_in_%s" % _phase_name())
		return
	var tick := int(message.get("tick", -1))
	var center_raw: Variant = message.get("center", null)
	var radius_m := float(message.get("radius_m", -1.0))
	if tick != _last_tick:
		_fail("interest_view_tick_%d" % tick)
		return
	if typeof(center_raw) != TYPE_ARRAY or center_raw.size() != 2:
		_fail("interest_view_center")
		return
	var center := Vector2(float(center_raw[0]), float(center_raw[1]))
	var expected_center := _center if _phase == Phase.INTEREST_VIEW_WAIT else ZERO_CENTER
	var expected_radius := _radius_m if _phase == Phase.INTEREST_VIEW_WAIT else ZERO_RADIUS_M
	if not center.is_equal_approx(expected_center):
		_fail("interest_view_center_value")
		return
	if absf(radius_m - expected_radius) > 0.001:
		_fail("interest_view_radius_%.3f" % radius_m)
		return

	var roads_raw: Variant = message.get("roads", null)
	if typeof(roads_raw) != TYPE_ARRAY:
		_fail("interest_view_roads")
		return
	var roads: Array = InterestRender.parse_roads(roads_raw)
	if roads.size() != roads_raw.size():
		_fail("interest_view_road_parse_%d" % roads.size())
		return
	var truncated := bool(message.get("truncated", false))

	var buildings_raw: Variant = message.get("buildings", null)
	if buildings_raw != null and typeof(buildings_raw) != TYPE_ARRAY:
		_fail("interest_view_buildings")
		return
	var building_info := _parse_buildings(buildings_raw)
	var buildings: Array = building_info.get("buildings", [])
	var buildings_truncated := bool(
		message.get("buildings_truncated", false)
	) or bool(building_info.get("truncated", false))

	if not _build_scene(tick, center, radius_m, roads, truncated, buildings, buildings_truncated):
		return

	if _phase == Phase.INTEREST_VIEW_WAIT:
		if not _assert_main_view():
			return
		_main_marker = _marker_text("MP_WORLD_CLIENT", false)
		print(_main_marker)
		if _smoke:
			_set_status("Main view rendered; probing the zero-viewport case.")
			if not _send({
				"type": "set_interest",
				"center_x": ZERO_CENTER.x,
				"center_z": ZERO_CENTER.y,
				"radius_m": ZERO_RADIUS_M,
			}):
				return
			_phase = Phase.ZERO_INTEREST_SENT
			_arm_deadline(_timeout_ms)
			return
		_set_status("View rendered; disconnecting.")
		if not _send({"type": "disconnect"}):
			return
		_phase = Phase.DISCONNECT_SENT
		_arm_deadline(_timeout_ms)
		return

	if not _assert_zero_view():
		return
	_zero_marker = _marker_text("MP_WORLD_CLIENT_ZERO", true)
	print(_zero_marker)
	_set_status("Zero viewport rendered; disconnecting.")
	if not _send({"type": "disconnect"}):
		return
	_phase = Phase.DISCONNECT_SENT
	_arm_deadline(_timeout_ms)

func _on_reject(message: Dictionary) -> void:
	_fail("reject_%s_code=%s" % [_phase_name(), str(message.get("code", "missing"))])

func _on_goodbye() -> void:
	if _phase != Phase.DISCONNECT_SENT:
		_fail("goodbye_in_%s" % _phase_name())
		return
	_finish_ok()

func _finish_ok() -> void:
	if _finished:
		return
	_finished = true
	set_process(false)
	_set_status("3D world client exchange complete.")
	_refresh_hud()
	if _peer != null:
		_peer.disconnect_from_host()
	print(
		"MP_WORLD_CLIENT_DONE status=ok session=%s tick=%d frames_sent=%d frames_received=%d"
		% [_session_id, _last_tick, _frames_sent, _frames_received]
	)
	if _smoke:
		get_tree().quit(0)

func _send(message: Dictionary) -> bool:
	if _peer == null or _peer.get_status() != StreamPeerTCP.STATUS_CONNECTED:
		_fail("send_not_connected")
		return false
	var payload := JSON.stringify(message).to_utf8_buffer()
	if payload.is_empty():
		_fail("encode_empty")
		return false
	if payload.size() > MAX_FRAME_LEN:
		_fail("encode_oversized")
		return false
	var frame := PackedByteArray()
	frame.resize(4)
	_write_u32_be(frame, 0, payload.size())
	frame.append_array(payload)
	var error := _peer.put_data(frame)
	if error != OK:
		_fail("send_%s" % error_string(error))
		return false
	_frames_sent += 1
	return true

func _fail(reason: String) -> void:
	if _finished:
		return
	_finished = true
	set_process(false)
	_set_status("3D world client failed: " + reason)
	if _peer != null:
		_peer.disconnect_from_host()
	print(
		"MP_WORLD_CLIENT status=fail error=%s session=%s frames_sent=%d frames_received=%d"
		% [reason, _session_id, _frames_sent, _frames_received]
	)
	if _smoke:
		get_tree().quit(1)

func _store_snapshot(tick: int, population: int, balance: int, checksum: String) -> void:
	_last_tick = tick
	_last_population = population
	_last_balance = balance
	_last_checksum = checksum

## Validate and build the scene from one interest view. Returns false after
## calling `_fail` on an invalid viewport or render setup.
func _build_scene(
	tick: int,
	center: Vector2,
	radius_m: float,
	roads: Array,
	truncated: bool,
	buildings: Array,
	buildings_truncated: bool
) -> bool:
	if not _is_valid_view(center, radius_m):
		_fail("render_invalid_view")
		return false
	_clear_scene()
	_view_tick = tick
	_view_center = center
	_view_radius_m = radius_m
	_roads = roads
	_buildings = buildings
	_truncated = truncated
	_buildings_truncated = buildings_truncated
	_road_checksum = InterestRender.checksum(_roads)
	_building_checksum = _checksum_buildings(_buildings)
	_classify_roads()
	_classify_buildings()

	_add_environment()
	_add_ground()
	_add_grid()
	_add_interest_ring()
	_add_center_marker()
	_add_roads()
	_add_buildings()
	_add_camera()
	_has_view = true
	_refresh_hud()
	return true

func _clear_scene() -> void:
	if _container == null:
		return
	for child in _container.get_children():
		_container.remove_child(child)
		child.free()
	_road_multimesh = null
	_building_multimesh = null
	_camera = null
	_roads.clear()
	_buildings.clear()
	_road_transforms.clear()
	_building_transforms.clear()
	_inside = 0
	_outside = 0
	_buildings_inside = 0
	_buildings_outside = 0
	_road_checksum = ""
	_building_checksum = ""
	_grid_lines = 0
	_has_view = false

func _classify_roads() -> void:
	_inside = 0
	_outside = 0
	for road in _roads:
		var midpoint := _road_midpoint(road)
		if midpoint.distance_to(_view_center) <= _view_radius_m + 0.0001:
			_inside += 1
		else:
			_outside += 1

func _classify_buildings() -> void:
	_buildings_inside = 0
	_buildings_outside = 0
	for building in _buildings:
		var center: Vector2 = building.get("center", Vector2.ZERO)
		if center.distance_to(_view_center) <= _view_radius_m + 0.0001:
			_buildings_inside += 1
		else:
			_buildings_outside += 1

func _road_midpoint(road: Dictionary) -> Vector2:
	var start: Vector2 = road.get("start", Vector2.ZERO)
	var end: Vector2 = road.get("end", Vector2.ZERO)
	return (start + end) * 0.5

## One flat box per road, sized from its width and centreline length. The
## per-instance colour marks a road the server should already have filtered.
func _road_transform(road: Dictionary) -> Transform3D:
	var start: Vector2 = road.get("start", Vector2.ZERO)
	var end: Vector2 = road.get("end", Vector2.ZERO)
	var width := clampf(float(road.get("width_m", 6.0)), MIN_WIDTH_M, MAX_WIDTH_M)
	var delta := end - start
	var length := maxf(delta.length(), MIN_ROAD_LENGTH_M)
	var yaw := atan2(delta.x, delta.y)
	var midpoint := (start + end) * 0.5
	var basis := Basis(Vector3.UP, yaw) * Basis.from_scale(
		Vector3(width, ROAD_HEIGHT_M, length)
	)
	return Transform3D(basis, Vector3(midpoint.x, ROAD_Y_M, midpoint.y))

## One box per building record at its centre. `radius_m` is a render hint, so
## the footprint is shown as a diameter with a height derived from it.
func _building_transform(building: Dictionary) -> Transform3D:
	var center: Vector2 = building.get("center", Vector2.ZERO)
	var radius_m := float(building.get("radius_m", DEFAULT_BUILDING_RADIUS_M))
	var footprint := radius_m * 2.0
	var height := clampf(
		radius_m * BUILDING_HEIGHT_RATIO, MIN_BUILDING_HEIGHT_M, MAX_BUILDING_HEIGHT_M
	)
	var basis := Basis.from_scale(Vector3(footprint, height, footprint))
	return Transform3D(basis, Vector3(center.x, height * 0.5, center.y))

func _add_environment() -> void:
	var env := Environment.new()
	env.background_mode = Environment.BG_COLOR
	env.background_color = Color(0.035, 0.045, 0.060)
	env.ambient_light_source = Environment.AMBIENT_SOURCE_COLOR
	env.ambient_light_color = Color(0.55, 0.62, 0.72)
	env.ambient_light_energy = 0.85
	var world_env := WorldEnvironment.new()
	world_env.name = "WorldClientEnvironment"
	world_env.environment = env
	_container.add_child(world_env)

	var light := DirectionalLight3D.new()
	light.name = "WorldClientLight"
	light.rotation_degrees = Vector3(-55.0, -40.0, 0.0)
	light.light_energy = 1.15
	light.shadow_enabled = false
	_container.add_child(light)

func _add_ground() -> void:
	var plane := PlaneMesh.new()
	var extent := maxf(_view_radius_m * GROUND_MARGIN, GRID_MIN_EXTENT_M)
	plane.size = Vector2(extent * 2.0, extent * 2.0)
	var material := StandardMaterial3D.new()
	material.albedo_color = COLOR_GROUND
	material.roughness = 1.0
	plane.material = material

	var ground := MeshInstance3D.new()
	ground.name = "WorldClientGround"
	ground.mesh = plane
	ground.position = Vector3(_view_center.x, -0.06, _view_center.y)
	ground.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	_container.add_child(ground)

## World-aligned 16 m grid plus X/Z/Y axis lines through the world origin when
## they intersect the framed extent.
func _add_grid() -> void:
	var extent := maxf(_view_radius_m * GROUND_MARGIN, GRID_MIN_EXTENT_M)
	var min_ix := floori((_view_center.x - extent) / GRID_STEP_M)
	var max_ix := ceili((_view_center.x + extent) / GRID_STEP_M)
	var min_iz := floori((_view_center.y - extent) / GRID_STEP_M)
	var max_iz := ceili((_view_center.y + extent) / GRID_STEP_M)
	var min_x := float(min_ix) * GRID_STEP_M
	var max_x := float(max_ix) * GRID_STEP_M
	var min_z := float(min_iz) * GRID_STEP_M
	var max_z := float(max_iz) * GRID_STEP_M
	var vertices: Array = []
	var colors: Array = []

	for ix in range(min_ix, max_ix + 1):
		var x := float(ix) * GRID_STEP_M
		_append_line(
			vertices,
			colors,
			Vector3(x, 0.02, min_z),
			Vector3(x, 0.02, max_z),
			COLOR_GRID
		)
	for iz in range(min_iz, max_iz + 1):
		var z := float(iz) * GRID_STEP_M
		_append_line(
			vertices,
			colors,
			Vector3(min_x, 0.02, z),
			Vector3(max_x, 0.02, z),
			COLOR_GRID
		)
	if min_z <= 0.0 and max_z >= 0.0:
		_append_line(
			vertices, colors, Vector3(0.0, 0.05, min_z), Vector3(0.0, 0.05, max_z), COLOR_AXIS_Z
		)
	if min_x <= 0.0 and max_x >= 0.0:
		_append_line(
			vertices, colors, Vector3(min_x, 0.05, 0.0), Vector3(max_x, 0.05, 0.0), COLOR_AXIS_X
		)
	if min_x <= 0.0 and max_x >= 0.0 and min_z <= 0.0 and max_z >= 0.0:
		_append_line(
			vertices, colors, Vector3(0.0, 0.0, 0.0), Vector3(0.0, 12.0, 0.0), COLOR_AXIS_Y
		)

	_grid_lines = vertices.size() / 2
	var arrays := []
	arrays.resize(Mesh.ARRAY_MAX)
	arrays[Mesh.ARRAY_VERTEX] = PackedVector3Array(vertices)
	arrays[Mesh.ARRAY_COLOR] = PackedColorArray(colors)
	var mesh := ArrayMesh.new()
	mesh.add_surface_from_arrays(Mesh.PRIMITIVE_LINES, arrays)
	var material := StandardMaterial3D.new()
	material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	material.vertex_color_use_as_albedo = true
	mesh.surface_set_material(0, material)

	var grid := MeshInstance3D.new()
	grid.name = "WorldClientGrid"
	grid.mesh = mesh
	grid.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	_container.add_child(grid)

func _append_line(
	vertices: Array, colors: Array, from: Vector3, to: Vector3, color: Color
) -> void:
	vertices.append(from)
	vertices.append(to)
	colors.append(color)
	colors.append(color)

func _add_interest_ring() -> void:
	if _view_radius_m <= 0.0:
		return
	var torus := TorusMesh.new()
	var tube := clampf(_view_radius_m * 0.012, 0.25, 1.0)
	torus.inner_radius = maxf(_view_radius_m - tube, 0.01)
	torus.outer_radius = _view_radius_m + tube
	torus.rings = RING_SEGMENTS
	torus.ring_segments = 8
	var material := StandardMaterial3D.new()
	material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	material.albedo_color = COLOR_RING
	torus.material = material

	var ring := MeshInstance3D.new()
	ring.name = "WorldClientInterestRing"
	ring.mesh = torus
	ring.position = Vector3(_view_center.x, 0.04, _view_center.y)
	ring.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	_container.add_child(ring)

func _add_center_marker() -> void:
	var box := BoxMesh.new()
	box.size = Vector3.ONE
	var material := StandardMaterial3D.new()
	material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	material.albedo_color = COLOR_CENTER
	box.material = material

	var marker := MeshInstance3D.new()
	marker.name = "WorldClientCenter"
	marker.mesh = box
	marker.position = Vector3(_view_center.x, 0.6, _view_center.y)
	marker.scale = Vector3(1.2, 1.2, 1.2)
	marker.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	_container.add_child(marker)

func _add_roads() -> void:
	if _roads.is_empty():
		return
	var box := BoxMesh.new()
	box.size = Vector3.ONE
	var material := StandardMaterial3D.new()
	material.vertex_color_use_as_albedo = true
	material.roughness = 0.9
	box.material = material

	_road_multimesh = MultiMesh.new()
	_road_multimesh.transform_format = MultiMesh.TRANSFORM_3D
	_road_multimesh.use_colors = true
	_road_multimesh.mesh = box
	_road_multimesh.instance_count = _roads.size()
	_road_transforms.clear()

	for index in range(_roads.size()):
		var road: Dictionary = _roads[index]
		var transform := _road_transform(road)
		_road_transforms.append(transform)
		_road_multimesh.set_instance_transform(index, transform)
		var color := (
			COLOR_OUTSIDE
			if _road_midpoint(road).distance_to(_view_center) > _view_radius_m + 0.0001
			else COLOR_INSIDE
		)
		_road_multimesh.set_instance_color(index, color)

	var node := MultiMeshInstance3D.new()
	node.name = "WorldClientRoads"
	node.multimesh = _road_multimesh
	node.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	_container.add_child(node)

func _add_buildings() -> void:
	if _buildings.is_empty():
		return
	var box := BoxMesh.new()
	box.size = Vector3.ONE
	var material := StandardMaterial3D.new()
	material.vertex_color_use_as_albedo = true
	material.roughness = 0.85
	box.material = material

	_building_multimesh = MultiMesh.new()
	_building_multimesh.transform_format = MultiMesh.TRANSFORM_3D
	_building_multimesh.use_colors = true
	_building_multimesh.mesh = box
	_building_multimesh.instance_count = _buildings.size()
	_building_transforms.clear()

	for index in range(_buildings.size()):
		var building: Dictionary = _buildings[index]
		var transform := _building_transform(building)
		_building_transforms.append(transform)
		_building_multimesh.set_instance_transform(index, transform)
		var center: Vector2 = building.get("center", Vector2.ZERO)
		var color := (
			COLOR_BUILDING_OUTSIDE
			if center.distance_to(_view_center) > _view_radius_m + 0.0001
			else COLOR_BUILDING_INSIDE
		)
		_building_multimesh.set_instance_color(index, color)

	var node := MultiMeshInstance3D.new()
	node.name = "WorldClientBuildings"
	node.multimesh = _building_multimesh
	node.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	_container.add_child(node)

func _add_camera() -> void:
	var framed_radius := maxf(_view_radius_m, 8.0)
	_camera = Camera3D.new()
	_camera.name = "WorldClientCamera"
	_camera.fov = CAMERA_FOV
	_camera.near = 0.1
	_camera.far = maxf(framed_radius * 12.0, 256.0)
	_camera.position = (
		Vector3(_view_center.x, 0.0, _view_center.y)
		+ Vector3(framed_radius * 0.85, framed_radius * 1.15, framed_radius * 0.85)
	)
	_camera.current = true
	_container.add_child(_camera)
	_camera.look_at(Vector3(_view_center.x, 0.0, _view_center.y), Vector3.UP)

func _road_count() -> int:
	return _road_multimesh.instance_count if _road_multimesh != null else 0

func _building_count() -> int:
	return _building_multimesh.instance_count if _building_multimesh != null else 0

## Parse and bound the optional `interest_view.buildings` payload. Entries are
## `{id, center: Vector2, radius_m}` sorted by ascending id, capped at
## [`MAX_BUILDINGS`]; malformed entries are skipped.
func _parse_buildings(raw: Variant) -> Dictionary:
	var buildings: Array = []
	var raw_count := 0
	if typeof(raw) == TYPE_ARRAY:
		raw_count = raw.size()
		for entry in raw:
			if buildings.size() >= MAX_BUILDINGS:
				break
			if typeof(entry) != TYPE_DICTIONARY:
				continue
			var center_raw: Variant = entry.get("center", null)
			if not InterestRender.is_world_pair(center_raw):
				continue
			var radius_m := DEFAULT_BUILDING_RADIUS_M
			var radius_raw: Variant = entry.get("radius_m", DEFAULT_BUILDING_RADIUS_M)
			if typeof(radius_raw) == TYPE_INT or typeof(radius_raw) == TYPE_FLOAT:
				var candidate := float(radius_raw)
				if is_finite(candidate) and candidate > 0.0:
					radius_m = clampf(candidate, MIN_BUILDING_RADIUS_M, MAX_BUILDING_RADIUS_M)
			buildings.append(
				{
					"id": int(entry.get("id", 0)),
					"center": Vector2(float(center_raw[0]), float(center_raw[1])),
					"radius_m": radius_m,
				}
			)
	buildings.sort_custom(
		func(a: Dictionary, b: Dictionary) -> bool:
			var a_id := int(a.get("id", 0))
			var b_id := int(b.get("id", 0))
			if a_id != b_id:
				return a_id < b_id
			var a_center: Vector2 = a.get("center", Vector2.ZERO)
			var b_center: Vector2 = b.get("center", Vector2.ZERO)
			if a_center.x != b_center.x:
				return a_center.x < b_center.x
			if a_center.y != b_center.y:
				return a_center.y < b_center.y
			return float(a.get("radius_m", 0.0)) < float(b.get("radius_m", 0.0))
	)
	return {
		"buildings": buildings,
		"truncated": raw_count > MAX_BUILDINGS,
	}

## Deterministic 16-hex checksum over the bounded building ids, centres, and
## millimetre-quantized radii. Same two-lane FNV-1a shape as the road checksum.
func _checksum_buildings(buildings: Array) -> String:
	var low := InterestRender.FNV_OFFSET_BASIS
	var high := InterestRender.FNV_OFFSET_BASIS ^ InterestRender.FNV_PRIME
	low = _mix_low(low, buildings.size())
	high = _mix_high(high, buildings.size())
	for building in buildings:
		var id := int(building.get("id", 0))
		var center: Vector2 = building.get("center", Vector2.ZERO)
		var radius_mm := int(round(float(building.get("radius_m", 0.0)) * 1000.0))
		for field in [
			(id >> 32) & 0xFFFFFFFF,
			id & 0xFFFFFFFF,
			int(center.x) & 0xFFFFFFFF,
			int(center.y) & 0xFFFFFFFF,
			radius_mm & 0xFFFFFFFF,
		]:
			low = _mix_low(low, int(field))
			high = _mix_high(high, int(field))
	return "%08x%08x" % [low & 0xFFFFFFFF, high & 0xFFFFFFFF]

func _mix_low(state: int, value: int) -> int:
	return ((state ^ (value & 0xFFFFFFFF)) * InterestRender.FNV_PRIME) & 0xFFFFFFFF

func _mix_high(state: int, value: int) -> int:
	return ((state + (value & 0xFFFFFFFF) + 1) * InterestRender.FNV_PRIME) & 0xFFFFFFFF

## Deterministic smoke assertions on the CPU-side data, never on GPU-side
## `MultiMesh` transforms (headless Godot returns identity there).
func _assert_main_view() -> bool:
	if _roads.size() != ROAD_PLAN.size():
		_fail("assert_main_roads_%d" % _roads.size())
		return false
	if _truncated:
		_fail("assert_main_truncated")
		return false
	if _inside != ROAD_PLAN.size() or _outside != 0:
		_fail("assert_main_road_classify_%d_%d" % [_inside, _outside])
		return false
	if _road_count() != ROAD_PLAN.size() or _road_transforms.size() != ROAD_PLAN.size():
		_fail("assert_main_road_render_%d" % _road_count())
		return false
	if int(_roads[0].get("id", -1)) != 1:
		_fail("assert_main_road_order")
		return false
	var first_road: Transform3D = _road_transforms[0]
	if not first_road.origin.is_equal_approx(Vector3(0.0, ROAD_Y_M, 0.0)):
		_fail("assert_main_road_origin")
		return false
	if not first_road.basis.get_scale().is_equal_approx(Vector3(6.0, ROAD_HEIGHT_M, 128.0)):
		_fail("assert_main_road_scale")
		return false

	if _buildings.size() != 4:
		_fail("assert_main_buildings_%d" % _buildings.size())
		return false
	if _buildings_truncated:
		_fail("assert_main_buildings_truncated")
		return false
	if _buildings_inside != 4 or _buildings_outside != 0:
		_fail("assert_main_building_classify_%d_%d" % [_buildings_inside, _buildings_outside])
		return false
	if _building_count() != 4 or _building_transforms.size() != 4:
		_fail("assert_main_building_render_%d" % _building_count())
		return false
	if int(_buildings[0].get("id", -1)) != 1 or int(_buildings[-1].get("id", -1)) != 18:
		_fail("assert_main_building_ids")
		return false
	var first_building: Transform3D = _building_transforms[0]
	if not first_building.origin.is_equal_approx(Vector3(0.0, 2.0, 0.0)):
		_fail("assert_main_building_origin")
		return false
	if not first_building.basis.get_scale().is_equal_approx(Vector3(16.0, 4.0, 16.0)):
		_fail("assert_main_building_scale")
		return false

	if _camera == null or _grid_lines <= 0:
		_fail("assert_main_scene_nodes")
		return false
	if not _assert_hud_fields():
		return false
	return true

func _assert_zero_view() -> bool:
	if not _roads.is_empty() or not _buildings.is_empty():
		_fail("assert_zero_records_%d_%d" % [_roads.size(), _buildings.size()])
		return false
	if _truncated or _buildings_truncated:
		_fail("assert_zero_truncated")
		return false
	if _road_count() != 0 or _building_count() != 0:
		_fail("assert_zero_instances_%d_%d" % [_road_count(), _building_count()])
		return false
	if _road_multimesh != null or _building_multimesh != null:
		_fail("assert_zero_multimesh")
		return false
	if not _road_transforms.is_empty() or not _building_transforms.is_empty():
		_fail("assert_zero_transforms")
		return false
	if _camera == null or _grid_lines <= 0:
		_fail("assert_zero_scene_nodes")
		return false
	if not _hud_label.text.contains("roads=0") or not _hud_label.text.contains("buildings=0"):
		_fail("assert_zero_hud")
		return false
	return true

func _assert_hud_fields() -> bool:
	if _hud_label == null:
		_fail("assert_hud_missing")
		return false
	var text := _hud_label.text
	for field in [
		"tick=%d" % _last_tick,
		"population=%d" % _last_population,
		"balance=%d" % _last_balance,
		"roads=%d" % _roads.size(),
		"buildings=%d" % _buildings.size(),
		"truncated=%s" % str(_truncated),
		"buildings_truncated=%s" % str(_buildings_truncated),
		"checksum=%s" % _last_checksum,
	]:
		if not text.contains(field):
			_fail("assert_hud_field_%s" % field)
			return false
	return true

func _marker_text(prefix: String, zero: bool) -> String:
	var rendered_roads := _road_count()
	var rendered_buildings := _building_count()
	if zero:
		return (
			"%s status=ok tick=%d population=%d balance=%d roads=%d buildings=%d"
			+ " truncated=%s buildings_truncated=%s checksum=%s"
			+ " rendered_roads=%d rendered_buildings=%d"
			+ " center=%.1f,%.1f radius=%.1f"
		) % [
			prefix,
			_last_tick,
			_last_population,
			_last_balance,
			_roads.size(),
			_buildings.size(),
			str(_truncated),
			str(_buildings_truncated),
			_last_checksum,
			rendered_roads,
			rendered_buildings,
			_view_center.x,
			_view_center.y,
			_view_radius_m,
		]
	return (
		"%s status=ok tick=%d population=%d balance=%d roads=%d buildings=%d"
		+ " truncated=%s buildings_truncated=%s checksum=%s"
		+ " road_checksum=%s building_checksum=%s"
		+ " rendered_roads=%d rendered_buildings=%d center=%.1f,%.1f radius=%.1f"
		+ " session=%s"
	) % [
		prefix,
		_last_tick,
		_last_population,
		_last_balance,
		_roads.size(),
		_buildings.size(),
		str(_truncated),
		str(_buildings_truncated),
		_last_checksum,
		_road_checksum,
		_building_checksum,
		rendered_roads,
		rendered_buildings,
		_view_center.x,
		_view_center.y,
		_view_radius_m,
		_session_id,
	]

func _build_hud() -> void:
	_hud_layer = CanvasLayer.new()
	_hud_layer.name = "WorldClientHud"
	add_child(_hud_layer)

	var panel := PanelContainer.new()
	panel.position = Vector2(12.0, 12.0)
	var style := StyleBoxFlat.new()
	style.bg_color = Color(0.02, 0.03, 0.04, 0.78)
	style.border_color = Color(0.25, 0.32, 0.40)
	style.set_border_width_all(1)
	style.set_content_margin_all(10.0)
	panel.add_theme_stylebox_override("panel", style)
	_hud_layer.add_child(panel)

	_hud_label = Label.new()
	_hud_label.add_theme_color_override("font_color", Color(0.86, 0.90, 0.95))
	_hud_label.add_theme_font_size_override("font_size", 14)
	panel.add_child(_hud_label)
	_refresh_hud()

func _refresh_hud() -> void:
	if _hud_label == null:
		return
	var tick_text := str(_last_tick) if _last_tick >= 0 else "-"
	var population_text := str(_last_population) if _last_population >= 0 else "-"
	var balance_text := str(_last_balance) if _last_tick >= 0 else "-"
	var roads_text := str(_roads.size()) if _has_view else "-"
	var buildings_text := str(_buildings.size()) if _has_view else "-"
	var truncated_text := str(_truncated) if _has_view else "-"
	var buildings_truncated_text := str(_buildings_truncated) if _has_view else "-"
	var checksum_text := _last_checksum if not _last_checksum.is_empty() else "-"
	_hud_label.text = (
		"3D multiplayer world client (debug)\n"
		+ "status: %s\n"
		+ "session: %s\n"
		+ "tick=%s population=%s balance=%s\n"
		+ "interest: center=(%.1f, %.1f) radius=%.1f m\n"
		+ "roads=%s buildings=%s truncated=%s buildings_truncated=%s\n"
		+ "rendered: roads=%d buildings=%d grid_lines=%d camera=%s\n"
		+ "checksum=%s\n"
		+ "road_checksum=%s building_checksum=%s"
	) % [
		_status_text,
		_session_id if not _session_id.is_empty() else "-",
		tick_text,
		population_text,
		balance_text,
		_view_center.x,
		_view_center.y,
		_view_radius_m,
		roads_text,
		buildings_text,
		truncated_text,
		buildings_truncated_text,
		_road_count(),
		_building_count(),
		_grid_lines,
		"ok" if _camera != null else "-",
		checksum_text,
		_road_checksum if not _road_checksum.is_empty() else "-",
		_building_checksum if not _building_checksum.is_empty() else "-",
	]

func _set_status(text: String) -> void:
	_status_text = text
	_refresh_hud()

func _is_valid_view(center: Vector2, radius_m: float) -> bool:
	if not is_finite(center.x) or not is_finite(center.y):
		return false
	return is_finite(radius_m) and radius_m >= 0.0 and radius_m <= MAX_RADIUS_M

func _is_pair_close(value: Variant, expected: Vector2, epsilon: float) -> bool:
	if typeof(value) != TYPE_ARRAY or value.size() != 2:
		return false
	if typeof(value[0]) != TYPE_INT and typeof(value[0]) != TYPE_FLOAT:
		return false
	if typeof(value[1]) != TYPE_INT and typeof(value[1]) != TYPE_FLOAT:
		return false
	return (
		absf(float(value[0]) - expected.x) <= epsilon
		and absf(float(value[1]) - expected.y) <= epsilon
	)

func _is_hex16(value: String) -> bool:
	if value.length() != 16:
		return false
	for index in range(value.length()):
		var character := value[index]
		if not (
			(character >= "0" and character <= "9")
			or (character >= "a" and character <= "f")
		):
			return false
	return true

func _phase_name() -> String:
	match _phase:
		Phase.CONNECTING:
			return "connecting"
		Phase.HELLO_SENT:
			return "hello_sent"
		Phase.ROAD_SENT:
			return "road_sent"
		Phase.INTEREST_SENT:
			return "interest_sent"
		Phase.INTEREST_VIEW_WAIT:
			return "interest_view_wait"
		Phase.ZERO_INTEREST_SENT:
			return "zero_interest_sent"
		Phase.ZERO_VIEW_WAIT:
			return "zero_view_wait"
		Phase.DISCONNECT_SENT:
			return "disconnect_sent"
		_:
			return "unknown"

func _arm_deadline(duration_ms: int) -> void:
	_deadline_ms = Time.get_ticks_msec() + duration_ms

func _arg(args: PackedStringArray, name: String, fallback: String) -> String:
	var index := args.find(name)
	if index >= 0 and index + 1 < args.size():
		return args[index + 1]
	return fallback

func _float_arg(args: PackedStringArray, name: String, fallback: float) -> float:
	var text := _arg(args, name, "")
	if text.is_empty() or not text.is_valid_float():
		return fallback
	var value := text.to_float()
	return value if is_finite(value) else fallback

func _positive_int(value: String, fallback: int) -> int:
	if not value.is_valid_int():
		return fallback
	var number := value.to_int()
	return number if number > 0 else fallback

func _read_u32_be(buffer: PackedByteArray, offset: int) -> int:
	return (
		(int(buffer[offset]) << 24)
		| (int(buffer[offset + 1]) << 16)
		| (int(buffer[offset + 2]) << 8)
		| int(buffer[offset + 3])
	)

func _write_u32_be(buffer: PackedByteArray, offset: int, value: int) -> void:
	buffer[offset] = (value >> 24) & 0xff
	buffer[offset + 1] = (value >> 16) & 0xff
	buffer[offset + 2] = (value >> 8) & 0xff
	buffer[offset + 3] = value & 0xff
