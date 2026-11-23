extends Node3D

## Standalone 3D preview of one server `interest_view` road + building set.
##
## This is a visualization harness for the bounded `InterestView` payload
## (`tick`, `center`, `radius_m`, `roads`, `truncated`, `buildings`,
## `buildings_truncated`) from `crates/server/src/protocol.rs`. It reads a JSON
## fixture (or builds a small deterministic sample) and draws every road in the
## bounded list as a flat box at `y = 0`, plus one box per bounded building
## record at its centre, with a fixed overhead `Camera3D`, a ground plane, the
## interest ring, and reused `MultiMesh` nodes (one for roads, one for
## buildings).
##
## It deliberately reuses `godot/mp_interest_render.gd` for road parsing, the
## 64-road wire cap, and the deterministic road checksum, so the 2D client
## preview and this 3D harness agree on the road set. Building records are
## parsed defensively here: a missing, empty, wrong-typed, or malformed list is
## ignored, entries are sorted by ascending id, the list is clamped to 64, and
## a payload that carried more entries than the cap sets `truncated` even when
## the server omitted the flag. A building box uses a footprint of
## `2 * radius_m`, a render hint rather than an authoritative footprint.
##
## It is **not** client render integration and **not** a benchmark gate: it
## draws no terrain, chunks, citizens, or streaming, and it does not mutate
## anything. Out-of-view roads and buildings are still drawn (the server owns
## filtering); the reported `outside` counts expose a filter regression
## instead of hiding it.
##
## Usage:
##   godot --path godot res://interest_preview.tscn -- --input path/to/view.json
##   godot --headless --path godot res://interest_preview.tscn -- --smoke
##
## Fixture shape (matching the wire payload):
##   {"tick":42,"center":[120.0,-80.0],"radius_m":96.0,
##    "roads":[{"id":1,"start":[0,0],"end":[8,0],"width_m":6}],
##    "truncated":false,
##    "buildings":[{"id":1,"center":[4,0],"radius_m":8}],
##    "buildings_truncated":false}
##
## `--smoke` writes a deterministic fixture (60 in-view + 4 out-of-view roads,
## exactly at the 64-road wire cap, plus malformed entries and no building
## list), renders it twice with a teardown in between, checks the road/instance
## counts, centreline bounds, camera framing, checksum, and the over-cap parse
## probe. It then runs building-only probes: missing/empty/wrong-typed and
## malformed payloads, a 5-building synthetic view checked against CPU-side
## transform mirrors, and an 80-record over-cap payload. It prints the road
## marker twice plus a building marker for the zero-building and synthetic
## views with identical checksums:
##   INTEREST_PREVIEW status=ok roads=64 instances=64 truncated=false checksum=…
##   INTEREST_PREVIEW_BUILDINGS status=ok buildings=0 boxes=0 truncated=false checksum=…
##
## See `docs/05 - Multiplayer.md` §Interest view 3D preview and
## `godot/mp_interest_render.gd`.

const InterestRender = preload("res://mp_interest_render.gd")

const SUITE := "interest_preview"
const MAX_RADIUS_M := 2048.0
# Matches the 2D preview's readability clamp; the checksum still uses the
# server's raw width.
const MIN_WIDTH_M := 0.5
const MAX_WIDTH_M := 64.0
const ROAD_HEIGHT_M := 0.24
const ROAD_Y_M := 0.12
const MIN_ROAD_LENGTH_M := 0.5
const GROUND_MARGIN := 3.0
const RING_SEGMENTS := 96
const CAMERA_FOV := 50.0

# Wire bound from `crates/server/src/protocol.rs::MAX_INTEREST_BUILDINGS`. The
# client clamps again even though the server already caps: a mod, a proxy, or
# a future server revision must not be able to grow this harness.
const MAX_BUILDINGS := 64
const DEFAULT_BUILDING_RADIUS_M := 8.0
const MIN_BUILDING_RADIUS_M := 0.5
const MAX_BUILDING_RADIUS_M := 256.0
const BUILDING_HEIGHT_RATIO := 0.5
const MIN_BUILDING_HEIGHT_M := 1.0
const MAX_BUILDING_HEIGHT_M := 12.0

# `--smoke` fixture: 60 roads inside the inclusive circle, 4 outside it, all
# 64 kept (the wire cap), and 3 malformed entries that must be skipped.
const SMOKE_TICK := 42
const SMOKE_CENTER := Vector2(120.0, -80.0)
const SMOKE_RADIUS_M := 96.0
const SMOKE_INSIDE := 60
const SMOKE_OUTSIDE := 4
const SMOKE_ROADS := SMOKE_INSIDE + SMOKE_OUTSIDE
const SMOKE_MALFORMED := 3
const SMOKE_RAW := SMOKE_ROADS + SMOKE_MALFORMED
const CAP_PROBE_ROADS := 80
const SMOKE_PATH := "user://interest_preview_smoke.json"

# Building probes: a 5-record synthetic view and an 80-record over-cap payload.
const SMOKE_BUILDINGS := 5
const BUILDING_CAP_PROBE := 80

const COLOR_INSIDE := Color(0.96, 0.78, 0.32)
const COLOR_OUTSIDE := Color(0.92, 0.28, 0.22)
const COLOR_BUILDING_INSIDE := Color(0.30, 0.82, 0.92)
const COLOR_BUILDING_OUTSIDE := Color(0.86, 0.36, 0.94)
const COLOR_GROUND := Color(0.065, 0.082, 0.105)
const COLOR_RING := Color(0.32, 0.44, 0.58)
const COLOR_CENTER := Color(1.0, 0.92, 0.62)

var _container: Node3D
var _camera: Camera3D
var _label: Label
var _road_multimesh: MultiMesh
var _building_multimesh: MultiMesh

var _tick := 0
var _center := Vector2.ZERO
var _radius_m := 0.0
var _truncated := false
var _roads: Array = []
var _inside := 0
var _outside := 0
var _checksum := ""
var _bounds := Rect2()
var _buildings: Array = []
var _buildings_raw_count := 0
var _buildings_malformed := 0
var _buildings_truncated := false
var _buildings_inside := 0
var _buildings_outside := 0
var _building_checksum := ""
# CPU-side mirror of the transforms written to the building MultiMesh. Headless
# Godot returns identity transforms from `get_instance_transform`, so smoke
# assertions read this array instead of the GPU-side copy.
var _building_transforms: Array = []
var _last_error := ""

func _ready() -> void:
	_container = Node3D.new()
	_container.name = "PreviewRoot"
	add_child(_container)

	var args := OS.get_cmdline_user_args()
	if args.has("--smoke"):
		_run_smoke()
		return

	var input_arg := _arg(args, "--input", "")
	var fixture: Dictionary
	if input_arg != "":
		fixture = _load_fixture_file(_resolve_path(input_arg))
		if fixture.is_empty():
			_fail(_last_error)
			return
	else:
		fixture = _sample_fixture()

	if not _build(fixture):
		_fail(_last_error)
		return
	print(
		"INTEREST_PREVIEW_SOURCE path=%s tick=%d center=%.1f,%.1f radius=%.1f roads=%d inside=%d outside=%d truncated=%s buildings=%d buildings_inside=%d buildings_outside=%d buildings_truncated=%s"
		% [
			input_arg if input_arg != "" else "builtin",
			_tick,
			_center.x,
			_center.y,
			_radius_m,
			_roads.size(),
			_inside,
			_outside,
			str(_truncated),
			_buildings.size(),
			_buildings_inside,
			_buildings_outside,
			str(_buildings_truncated),
		]
	)
	_print_marker()
	if DisplayServer.get_name() == "headless":
		get_tree().quit(0)

func _arg(args: PackedStringArray, name: String, fallback: String) -> String:
	var index := args.find(name)
	if index >= 0 and index + 1 < args.size():
		return args[index + 1]
	return fallback

func _project_root() -> String:
	return ProjectSettings.globalize_path("res://").path_join("..").simplify_path()

func _resolve_path(path: String) -> String:
	if path.begins_with("res://") or path.begins_with("user://") or path.is_absolute_path():
		return path
	var repo_relative := _project_root().path_join(path).simplify_path()
	if FileAccess.file_exists(repo_relative):
		return repo_relative
	return path

func _load_fixture_file(path: String) -> Dictionary:
	_last_error = ""
	if not FileAccess.file_exists(path):
		_last_error = "missing_fixture:" + path
		return {}
	var file := FileAccess.open(path, FileAccess.READ)
	if file == null:
		_last_error = "open_fixture:%s:%s" % [path, error_string(FileAccess.get_open_error())]
		return {}
	var text := file.get_as_text()
	file.close()
	var json := JSON.new()
	if json.parse(text) != OK:
		_last_error = "fixture_json_line_%d:%s" % [json.get_error_line(), json.get_error_message()]
		return {}
	var data: Variant = json.data
	if typeof(data) != TYPE_DICTIONARY:
		_last_error = "fixture_not_object"
		return {}
	return data

func _write_fixture_file(path: String, fixture: Dictionary) -> bool:
	var file := FileAccess.open(path, FileAccess.WRITE)
	if file == null:
		_last_error = "write_fixture:%s" % error_string(FileAccess.get_open_error())
		return false
	file.store_string(JSON.stringify(fixture, "", true, true))
	file.close()
	return true

## Validate the payload and build the scene. Returns false on a bad fixture.
func _build(fixture: Dictionary) -> bool:
	_clear_scene()
	_last_error = ""

	var tick_raw: Variant = fixture.get("tick", null)
	if typeof(tick_raw) != TYPE_INT and typeof(tick_raw) != TYPE_FLOAT:
		_last_error = "fixture_tick"
		return false
	if float(tick_raw) < 0.0 or float(tick_raw) != floorf(float(tick_raw)):
		_last_error = "fixture_tick"
		return false
	var center_raw: Variant = fixture.get("center", null)
	if not InterestRender.is_world_pair(center_raw):
		_last_error = "fixture_center"
		return false
	var radius_raw: Variant = fixture.get("radius_m", null)
	if typeof(radius_raw) != TYPE_INT and typeof(radius_raw) != TYPE_FLOAT:
		_last_error = "fixture_radius"
		return false
	var radius := float(radius_raw)
	if not is_finite(radius) or radius <= 0.0 or radius > MAX_RADIUS_M:
		_last_error = "fixture_radius"
		return false
	var roads_raw: Variant = fixture.get("roads", null)
	if typeof(roads_raw) != TYPE_ARRAY:
		_last_error = "fixture_roads"
		return false
	var truncated_raw: Variant = fixture.get("truncated", false)
	if typeof(truncated_raw) != TYPE_BOOL:
		_last_error = "fixture_truncated"
		return false

	_tick = int(tick_raw)
	_center = Vector2(float(center_raw[0]), float(center_raw[1]))
	_radius_m = radius
	_truncated = bool(truncated_raw)
	_roads = InterestRender.parse_roads(roads_raw)
	_checksum = InterestRender.checksum(_roads)
	_classify_roads()
	_bounds = _road_bounds(_roads)

	# Buildings are a later, optional part of the payload: missing, empty,
	# wrong-typed, or malformed input must not fail the road preview.
	var building_info := _parse_buildings(fixture.get("buildings", null))
	_buildings = building_info.get("buildings", [])
	_buildings_raw_count = int(building_info.get("raw", 0))
	_buildings_malformed = int(building_info.get("malformed", 0))
	_buildings_truncated = _payload_buildings_truncated(fixture) or bool(
		building_info.get("clamped", false)
	)
	_building_checksum = _checksum_buildings(_buildings)
	_classify_buildings()

	_add_environment()
	_add_ground()
	_add_interest_ring()
	_add_center_marker()
	_add_roads()
	_add_buildings()
	_add_camera()
	_add_label()
	return true

## Count roads whose midpoint is inside the inclusive circle. The server owns
## this filter; matching it here only labels the list for the operator.
func _classify_roads() -> void:
	_inside = 0
	_outside = 0
	for road in _roads:
		var start: Vector2 = road.get("start", Vector2.ZERO)
		var end: Vector2 = road.get("end", Vector2.ZERO)
		var midpoint := (start + end) * 0.5
		if midpoint.distance_to(_center) <= _radius_m + 0.0001:
			_inside += 1
		else:
			_outside += 1

func _road_bounds(roads: Array) -> Rect2:
	if roads.is_empty():
		return Rect2(_center, Vector2.ZERO)
	var min_point := Vector2(INF, INF)
	var max_point := Vector2(-INF, -INF)
	for road in roads:
		for key in ["start", "end"]:
			var point: Vector2 = road.get(key, Vector2.ZERO)
			min_point.x = minf(min_point.x, point.x)
			min_point.y = minf(min_point.y, point.y)
			max_point.x = maxf(max_point.x, point.x)
			max_point.y = maxf(max_point.y, point.y)
	return Rect2(min_point, max_point - min_point)

## Parse and bound the optional `interest_view.buildings` payload.
##
## Returns `{buildings, raw, malformed, clamped, wrong_type}`. Entries are
## `{id, center: Vector2, radius_m}`, sorted by ascending id then centre and
## radius. Non-dictionary entries and entries without a finite `[x, z]` centre
## are skipped and counted as `malformed`; a bad `id` becomes `0` and a bad
## `radius_m` falls back to [`DEFAULT_BUILDING_RADIUS_M`]. The list is capped at
## [`MAX_BUILDINGS`] even when a future payload sends more.
func _parse_buildings(raw: Variant) -> Dictionary:
	var buildings: Array = []
	var malformed := 0
	var raw_count := 0
	var wrong_type := false
	if typeof(raw) == TYPE_ARRAY:
		raw_count = raw.size()
		for entry in raw:
			if buildings.size() >= MAX_BUILDINGS:
				break
			if typeof(entry) != TYPE_DICTIONARY:
				malformed += 1
				continue
			var center_raw: Variant = entry.get("center", null)
			if not InterestRender.is_world_pair(center_raw):
				malformed += 1
				continue
			var radius_m := DEFAULT_BUILDING_RADIUS_M
			var radius_raw: Variant = entry.get("radius_m", DEFAULT_BUILDING_RADIUS_M)
			if typeof(radius_raw) == TYPE_INT or typeof(radius_raw) == TYPE_FLOAT:
				var candidate := float(radius_raw)
				if is_finite(candidate) and candidate > 0.0:
					radius_m = clampf(candidate, MIN_BUILDING_RADIUS_M, MAX_BUILDING_RADIUS_M)
			var id := 0
			var id_raw: Variant = entry.get("id", 0)
			if typeof(id_raw) == TYPE_INT:
				id = int(id_raw)
			elif typeof(id_raw) == TYPE_FLOAT and is_finite(float(id_raw)):
				id = int(id_raw)
			buildings.append(
				{
					"id": id,
					"center": Vector2(float(center_raw[0]), float(center_raw[1])),
					"radius_m": radius_m,
				}
			)
	elif raw != null:
		wrong_type = true
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
		"raw": raw_count,
		"malformed": malformed,
		"clamped": raw_count > MAX_BUILDINGS,
		"wrong_type": wrong_type,
	}

func _payload_buildings_truncated(fixture: Dictionary) -> bool:
	var raw: Variant = fixture.get("buildings_truncated", false)
	return typeof(raw) == TYPE_BOOL and bool(raw)

## Deterministic 16-hex checksum over the bounded building ids, centres, and
## millimetre-quantized radii. Same two-lane FNV-1a shape as the road checksum,
## so it is stable across processes and independent of JSON key order.
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

func _classify_buildings() -> void:
	_buildings_inside = 0
	_buildings_outside = 0
	for building in _buildings:
		var center: Vector2 = building.get("center", Vector2.ZERO)
		if center.distance_to(_center) <= _radius_m + 0.0001:
			_buildings_inside += 1
		else:
			_buildings_outside += 1

func _add_environment() -> void:
	var env := Environment.new()
	env.background_mode = Environment.BG_COLOR
	env.background_color = Color(0.035, 0.045, 0.060)
	env.ambient_light_source = Environment.AMBIENT_SOURCE_COLOR
	env.ambient_light_color = Color(0.55, 0.62, 0.72)
	env.ambient_light_energy = 0.85
	var world_env := WorldEnvironment.new()
	world_env.name = "PreviewEnvironment"
	world_env.environment = env
	_container.add_child(world_env)

	var light := DirectionalLight3D.new()
	light.name = "PreviewLight"
	light.rotation_degrees = Vector3(-55.0, -40.0, 0.0)
	light.light_energy = 1.15
	light.shadow_enabled = false
	_container.add_child(light)

func _add_ground() -> void:
	var plane := PlaneMesh.new()
	var extent := maxf(_radius_m * GROUND_MARGIN, 16.0)
	plane.size = Vector2(extent, extent)
	var material := StandardMaterial3D.new()
	material.albedo_color = COLOR_GROUND
	material.roughness = 1.0
	plane.material = material

	var ground := MeshInstance3D.new()
	ground.name = "PreviewGround"
	ground.mesh = plane
	ground.position = Vector3(_center.x, -0.06, _center.y)
	ground.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	_container.add_child(ground)

func _add_interest_ring() -> void:
	var torus := TorusMesh.new()
	var tube := clampf(_radius_m * 0.012, 0.25, 1.0)
	torus.inner_radius = maxf(_radius_m - tube, 0.01)
	torus.outer_radius = _radius_m + tube
	torus.rings = RING_SEGMENTS
	torus.ring_segments = 8
	var material := StandardMaterial3D.new()
	material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	material.albedo_color = COLOR_RING
	torus.material = material

	var ring := MeshInstance3D.new()
	ring.name = "PreviewInterestRing"
	ring.mesh = torus
	ring.position = Vector3(_center.x, 0.04, _center.y)
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
	marker.name = "PreviewCenter"
	marker.mesh = box
	marker.position = Vector3(_center.x, 0.6, _center.y)
	marker.scale = Vector3(1.2, 1.2, 1.2)
	marker.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	_container.add_child(marker)

## One flat box per road, at the wire cap. Per-instance colour marks roads the
## server should already have filtered out.
func _add_roads() -> void:
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

	for index in range(_roads.size()):
		var road: Dictionary = _roads[index]
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
		_road_multimesh.set_instance_transform(
			index, Transform3D(basis, Vector3(midpoint.x, ROAD_Y_M, midpoint.y))
		)
		var midpoint_distance := midpoint.distance_to(_center)
		var color := (
			COLOR_OUTSIDE if midpoint_distance > _radius_m + 0.0001 else COLOR_INSIDE
		)
		_road_multimesh.set_instance_color(index, color)

	var node := MultiMeshInstance3D.new()
	node.name = "PreviewRoads"
	node.multimesh = _road_multimesh
	node.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	_container.add_child(node)

## One box per building record at its centre, sized from `radius_m` (footprint
## diameter), with a distinct teal/violet palette. No node is created for an
## empty list, so a roads-only payload keeps the original node budget.
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
		var center: Vector2 = building.get("center", Vector2.ZERO)
		var radius_m := float(building.get("radius_m", DEFAULT_BUILDING_RADIUS_M))
		var footprint := radius_m * 2.0
		var height := clampf(
			radius_m * BUILDING_HEIGHT_RATIO, MIN_BUILDING_HEIGHT_M, MAX_BUILDING_HEIGHT_M
		)
		var basis := Basis.from_scale(Vector3(footprint, height, footprint))
		var transform := Transform3D(basis, Vector3(center.x, height * 0.5, center.y))
		_building_transforms.append(transform)
		_building_multimesh.set_instance_transform(index, transform)
		var color := (
			COLOR_BUILDING_OUTSIDE
			if center.distance_to(_center) > _radius_m + 0.0001
			else COLOR_BUILDING_INSIDE
		)
		_building_multimesh.set_instance_color(index, color)

	var node := MultiMeshInstance3D.new()
	node.name = "PreviewBuildings"
	node.multimesh = _building_multimesh
	node.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	_container.add_child(node)

func _add_camera() -> void:
	_camera = Camera3D.new()
	_camera.name = "PreviewCamera"
	_camera.fov = CAMERA_FOV
	_camera.near = 0.1
	_camera.far = maxf(_radius_m * 12.0, 256.0)
	_camera.position = (
		Vector3(_center.x, 0.0, _center.y)
		+ Vector3(_radius_m * 0.85, _radius_m * 1.15, _radius_m * 0.85)
	)
	_camera.current = true
	_container.add_child(_camera)
	_camera.look_at(Vector3(_center.x, 0.0, _center.y), Vector3.UP)

func _add_label() -> void:
	var layer := CanvasLayer.new()
	layer.name = "PreviewUi"
	_label = Label.new()
	_label.name = "PreviewLabel"
	_label.position = Vector2(16.0, 12.0)
	_label.add_theme_color_override("font_color", Color(0.86, 0.90, 0.95))
	_label.text = (
		"interest_view tick=%d  center=(%.1f, %.1f)  radius=%.1f m\n"
		+ "roads=%d  instances=%d  inside=%d  outside=%d  truncated=%s\n"
		+ "buildings=%d  boxes=%d  inside=%d  outside=%d  truncated=%s\n"
		+ "road_checksum=%s  building_checksum=%s"
	) % [
		_tick,
		_center.x,
		_center.y,
		_radius_m,
		_roads.size(),
		_road_count(),
		_inside,
		_outside,
		str(_truncated),
		_buildings.size(),
		_building_count(),
		_buildings_inside,
		_buildings_outside,
		str(_buildings_truncated),
		_checksum,
		_building_checksum,
	]
	layer.add_child(_label)
	_container.add_child(layer)

func _road_count() -> int:
	return _road_multimesh.instance_count if _road_multimesh != null else 0

func _building_count() -> int:
	return _building_multimesh.instance_count if _building_multimesh != null else 0

func _render_node_counts() -> Dictionary:
	var multimesh_nodes := 0
	var mesh_nodes := 0
	if _container != null:
		for child in _container.get_children():
			if child is MultiMeshInstance3D:
				multimesh_nodes += 1
			elif child is MeshInstance3D:
				mesh_nodes += 1
	return {"multimesh": multimesh_nodes, "mesh": mesh_nodes}

func _clear_scene() -> void:
	if _container != null:
		for child in _container.get_children():
			_container.remove_child(child)
			child.free()
	_road_multimesh = null
	_building_multimesh = null
	_camera = null
	_label = null
	_roads.clear()
	_buildings.clear()
	_building_transforms.clear()
	_inside = 0
	_outside = 0
	_buildings_raw_count = 0
	_buildings_malformed = 0
	_buildings_inside = 0
	_buildings_outside = 0
	_buildings_truncated = false
	_building_checksum = ""
	_checksum = ""
	_bounds = Rect2()

func _print_marker() -> void:
	print(
		"INTEREST_PREVIEW status=ok roads=%d instances=%d truncated=%s checksum=%s"
		% [_roads.size(), _road_count(), str(_truncated), _checksum]
	)

## One-line building marker. `prefix` distinguishes the main zero-building
## build from the synthetic and over-cap probes while keeping the same fields.
func _print_buildings_marker(prefix: String) -> void:
	print(
		"%s status=ok buildings=%d boxes=%d truncated=%s checksum=%s"
		% [prefix, _buildings.size(), _building_count(), str(_buildings_truncated), _building_checksum]
	)

func _print_view() -> void:
	var counts := _render_node_counts()
	print(
		"INTEREST_PREVIEW_VIEW tick=%d center=%.1f,%.1f radius=%.1f raw=%d roads=%d inside=%d outside=%d truncated=%s"
		% [
			_tick,
			_center.x,
			_center.y,
			_radius_m,
			SMOKE_RAW,
			_roads.size(),
			_inside,
			_outside,
			str(_truncated),
		]
	)
	print(
		"INTEREST_PREVIEW_GEOMETRY instances=%d multimesh=%d mesh=%d bounds=%.1f,%.1f,%.1f,%.1f camera=ok"
		% [
			_road_count(),
			int(counts["multimesh"]),
			int(counts["mesh"]),
			_bounds.position.x,
			_bounds.position.y,
			_bounds.size.x,
			_bounds.size.y,
		]
	)

func _fail(message: String) -> void:
	push_error(message)
	print("INTEREST_PREVIEW status=error message=%s" % message)
	get_tree().quit(1)

func _run_smoke() -> void:
	if not _write_fixture_file(SMOKE_PATH, _smoke_fixture()):
		_fail(_last_error)
		return
	var loaded := _load_fixture_file(SMOKE_PATH)
	if loaded.is_empty():
		_fail(_last_error)
		return
	if not _build(loaded):
		_fail(_last_error)
		return
	if not _assert_smoke(loaded):
		return

	_print_view()
	_print_marker()
	_print_buildings_marker("INTEREST_PREVIEW_BUILDINGS")
	var first_checksum := _checksum
	var first_instances := _road_count()
	var first_bounds := _bounds
	var first_buildings := _buildings.size()
	var first_buildings_checksum := _building_checksum

	if not _assert_cap_probe():
		return
	if not _assert_buildings_tolerance_probe():
		return
	if not _assert_buildings_view_probe():
		return
	if not _assert_buildings_cap_probe():
		return

	_clear_scene()
	if _road_count() != 0 or _building_count() != 0 or _container.get_child_count() != 0:
		_fail("teardown_nodes=%d" % _container.get_child_count())
		return
	print("INTEREST_PREVIEW_TEARDOWN instances=0 nodes=0")

	# Re-read the same file through the same path the CLI uses, then require an
	# identical checksum, instance count, and bounds.
	var rebuilt := _load_fixture_file(SMOKE_PATH)
	if rebuilt.is_empty():
		_fail(_last_error)
		return
	if not _build(rebuilt):
		_fail(_last_error)
		return
	if _checksum != first_checksum or _road_count() != first_instances:
		_fail("rebuild_mismatch")
		return
	if _buildings.size() != first_buildings or _building_checksum != first_buildings_checksum:
		_fail("rebuild_buildings_mismatch")
		return
	if not _bounds.is_equal_approx(first_bounds):
		_fail("rebuild_bounds")
		return
	_print_marker()
	_print_buildings_marker("INTEREST_PREVIEW_BUILDINGS")

	DirAccess.remove_absolute(ProjectSettings.globalize_path(SMOKE_PATH))
	get_tree().quit(0)

func _assert_smoke(fixture: Dictionary) -> bool:
	if _tick != SMOKE_TICK:
		_fail("assert_tick_%d" % _tick)
		return false
	if not _center.is_equal_approx(SMOKE_CENTER):
		_fail("assert_center")
		return false
	if not is_equal_approx(_radius_m, SMOKE_RADIUS_M):
		_fail("assert_radius")
		return false
	if _roads.size() != SMOKE_ROADS:
		_fail("assert_roads_%d" % _roads.size())
		return false
	if _inside != SMOKE_INSIDE or _outside != SMOKE_OUTSIDE:
		_fail("assert_classify_inside_%d_outside_%d" % [_inside, _outside])
		return false
	if _road_count() != SMOKE_ROADS:
		_fail("assert_instances_%d" % _road_count())
		return false
	if _truncated:
		_fail("assert_truncated")
		return false
	if _checksum.length() != 16 or not _is_hex16(_checksum):
		_fail("assert_checksum_%s" % _checksum)
		return false
	# The road fixture carries no building list at all: the optional field must
	# parse as empty, stay untruncated, and create no building instances.
	if not _buildings.is_empty() or _building_count() != 0:
		_fail("assert_buildings_%d_boxes_%d" % [_buildings.size(), _building_count()])
		return false
	if _buildings_truncated:
		_fail("assert_buildings_truncated")
		return false
	if _buildings_raw_count != 0 or _buildings_malformed != 0:
		_fail("assert_buildings_raw")
		return false
	if _building_checksum.length() != 16 or not _is_hex16(_building_checksum):
		_fail("assert_building_checksum_%s" % _building_checksum)
		return false

	# Bounds are the centreline AABB of the bounded road list, recomputed from
	# the JSON fixture independently of the parser.
	var expected := _fixture_bounds(fixture)
	if not _bounds.is_equal_approx(expected):
		_fail("assert_bounds")
		return false
	if _bounds.size.x <= 0.0 or _bounds.size.y <= 0.0:
		_fail("assert_bounds_empty")
		return false
	var limit := _radius_m * GROUND_MARGIN
	if (
		absf(_bounds.position.x - _center.x) > limit
		or absf(_bounds.position.y - _center.y) > limit
		or _bounds.size.x > limit * 2.0
		or _bounds.size.y > limit * 2.0
	):
		_fail("assert_bounds_extent")
		return false

	# Every in-view midpoint is inside the inclusive circle; every out-of-view
	# midpoint is outside it.
	var inside_seen := 0
	var outside_seen := 0
	for road in _roads:
		var start: Vector2 = road.get("start", Vector2.ZERO)
		var end: Vector2 = road.get("end", Vector2.ZERO)
		var distance := ((start + end) * 0.5).distance_to(_center)
		if distance <= _radius_m + 0.0001:
			inside_seen += 1
		else:
			outside_seen += 1
	if inside_seen != SMOKE_INSIDE or outside_seen != SMOKE_OUTSIDE:
		_fail("assert_midpoints")
		return false

	var counts := _render_node_counts()
	if int(counts["multimesh"]) != 1 or int(counts["mesh"]) > 3:
		_fail("assert_node_budget_multimesh_%d_mesh_%d" % [counts["multimesh"], counts["mesh"]])
		return false
	if _camera == null:
		_fail("assert_camera")
		return false
	var to_center := (
		Vector3(_center.x, 0.0, _center.y) - _camera.position
	).normalized()
	if (-_camera.global_transform.basis.z).dot(to_center) < 0.999:
		_fail("assert_camera_aim")
		return false
	if _camera.position.distance_to(Vector3(_center.x, 0.0, _center.y)) <= _radius_m:
		_fail("assert_camera_distance")
		return false
	return true

## Prove the shared parser still caps an over-long list, even though the smoke
## fixture itself sits exactly at the 64-road wire cap.
func _assert_cap_probe() -> bool:
	var raw: Array = []
	for index in range(CAP_PROBE_ROADS):
		raw.append(
			{
				"id": index + 1,
				"start": [float(index), 0.0],
				"end": [float(index), 10.0],
				"width_m": 6,
			}
		)
	var parsed: Array = InterestRender.parse_roads(raw)
	print("INTEREST_PREVIEW_CAP raw=%d parsed=%d" % [raw.size(), parsed.size()])
	if parsed.size() != InterestRender.MAX_ROADS:
		_fail("assert_cap_%d" % parsed.size())
		return false
	return true

## A fixture with no road or building detail, plus caller overrides.
func _view_fixture(extra: Dictionary) -> Dictionary:
	var fixture := {
		"tick": 1,
		"center": [0.0, 0.0],
		"radius_m": 64.0,
		"roads": [],
		"truncated": false,
	}
	fixture.merge(extra, true)
	return fixture

## Missing, empty, wrong-typed, and malformed building payloads must build the
## road preview unchanged (zero boxes, untruncated) instead of failing.
func _assert_buildings_tolerance_probe() -> bool:
	var cases := [
		{"name": "missing", "extra": {}, "boxes": 0, "malformed": 0},
		{"name": "empty", "extra": {"buildings": []}, "boxes": 0, "malformed": 0},
		{
			"name": "wrong_type",
			"extra": {"buildings": "not a list", "buildings_truncated": "yes"},
			"boxes": 0,
			"malformed": 0,
		},
		{
			"name": "malformed",
			"extra": {
				"buildings":
				[
					1,
					"x",
					{},
					{"id": 7},
					{"id": 9, "center": [3.0, 4.0], "radius_m": "big"},
				],
			},
			"boxes": 1,
			"malformed": 4,
		},
	]
	var seen := {}
	for case in cases:
		if not _build(_view_fixture(case["extra"])):
			_fail("assert_buildings_tolerance_build_%s_%s" % [case["name"], _last_error])
			return false
		var expected_boxes := int(case["boxes"])
		if (
			_buildings.size() != expected_boxes
			or _building_count() != expected_boxes
			or _buildings_malformed != int(case["malformed"])
			or _buildings_truncated
		):
			_fail(
				"assert_buildings_tolerance_%s_parsed_%d_boxes_%d_malformed_%d_truncated_%s"
				% [
					case["name"],
					_buildings.size(),
					_building_count(),
					_buildings_malformed,
					str(_buildings_truncated),
				]
			)
			return false
		if expected_boxes > 0:
			var record: Dictionary = _buildings[0]
			if int(record.get("id", 0)) != 9:
				_fail("assert_buildings_tolerance_id_%d" % int(record.get("id", 0)))
				return false
			if not is_equal_approx(float(record.get("radius_m", 0.0)), DEFAULT_BUILDING_RADIUS_M):
				_fail("assert_buildings_tolerance_default_radius")
				return false
		seen["%s_boxes" % case["name"]] = _building_count()
		seen["%s_malformed" % case["name"]] = _buildings_malformed
		seen["%s_parsed" % case["name"]] = _buildings.size()
	print(
		"INTEREST_PREVIEW_BUILDINGS_TOLERANT missing=%d empty=%d wrong_type=%d malformed=%d parsed=%d"
		% [
			int(seen.get("missing_boxes", -1)),
			int(seen.get("empty_boxes", -1)),
			int(seen.get("wrong_type_boxes", -1)),
			int(seen.get("malformed_malformed", -1)),
			int(seen.get("malformed_parsed", -1)),
		]
	)
	return true

## A deterministic 5-building view (ids deliberately unsorted on the wire).
func _smoke_buildings_fixture() -> Dictionary:
	var buildings: Array = []
	for index in range(SMOKE_BUILDINGS):
		var angle := TAU * float(index) / float(SMOKE_BUILDINGS) + 0.2
		var radial := 12.0 + float(index) * 9.0
		var center := Vector2(cos(angle), sin(angle)) * radial
		var radius_m := 4.0 + float(index % 3) * 2.5
		buildings.append(
			{
				"id": 500 + (SMOKE_BUILDINGS - index) * 7,
				"center": [center.x, center.y],
				"radius_m": radius_m,
			}
		)
	return {
		"tick": 7,
		"center": [0.0, 0.0],
		"radius_m": 64.0,
		"roads": [{"id": 1, "start": [-8.0, 0.0], "end": [8.0, 0.0], "width_m": 6}],
		"truncated": false,
		"buildings": buildings,
		"buildings_truncated": false,
	}

## Build the synthetic view, check counts/order/label and every CPU-side
## transform mirror, then rebuild and require identical output.
func _assert_buildings_view_probe() -> bool:
	var fixture := _smoke_buildings_fixture()
	if not _build(fixture):
		_fail("assert_buildings_view_build_%s" % _last_error)
		return false
	if not _assert_buildings_view(fixture):
		return false
	var checksum := _building_checksum
	var transforms := _building_transforms.duplicate()

	if not _build(fixture):
		_fail("assert_buildings_rebuild_%s" % _last_error)
		return false
	if _building_checksum != checksum or not _transforms_match(_building_transforms, transforms):
		_fail("assert_buildings_rebuild_mismatch")
		return false
	_print_buildings_marker("INTEREST_PREVIEW_BUILDINGS_VIEW")
	return true

func _assert_buildings_view(fixture: Dictionary) -> bool:
	if _road_count() != 1:
		_fail("assert_buildings_view_roads_%d" % _road_count())
		return false
	if _buildings.size() != SMOKE_BUILDINGS or _building_count() != SMOKE_BUILDINGS:
		_fail("assert_buildings_view_count_%d_boxes_%d" % [_buildings.size(), _building_count()])
		return false
	if _buildings_truncated:
		_fail("assert_buildings_view_truncated")
		return false
	if _buildings_malformed != 0 or _buildings_raw_count != SMOKE_BUILDINGS:
		_fail("assert_buildings_view_raw_%d_malformed_%d" % [_buildings_raw_count, _buildings_malformed])
		return false
	if _building_transforms.size() != SMOKE_BUILDINGS:
		_fail("assert_buildings_view_transforms_%d" % _building_transforms.size())
		return false
	if _building_checksum.length() != 16 or not _is_hex16(_building_checksum):
		_fail("assert_buildings_view_checksum_%s" % _building_checksum)
		return false
	if _label == null:
		_fail("assert_buildings_view_label")
		return false
	if (
		not _label.text.contains("buildings=%d" % SMOKE_BUILDINGS)
		or not _label.text.contains("boxes=%d" % SMOKE_BUILDINGS)
		or not _label.text.contains("truncated=false")
	):
		_fail("assert_buildings_view_label_text")
		return false

	var expected := _expected_buildings(fixture)
	if expected.size() != SMOKE_BUILDINGS:
		_fail("assert_buildings_view_expected_%d" % expected.size())
		return false
	var previous_id := -9223372036854775807
	for index in range(_buildings.size()):
		var record: Dictionary = _buildings[index]
		var expected_record: Dictionary = expected[index]
		var id := int(record.get("id", 0))
		if id <= previous_id:
			_fail("assert_buildings_view_order_%d" % id)
			return false
		previous_id = id
		var center: Vector2 = record.get("center", Vector2.ZERO)
		var expected_center: Vector2 = expected_record.get("center", Vector2.ZERO)
		var radius_m := float(record.get("radius_m", 0.0))
		if (
			id != int(expected_record.get("id", 0))
			or not center.is_equal_approx(expected_center)
			or not is_equal_approx(radius_m, float(expected_record.get("radius_m", 0.0)))
		):
			_fail("assert_buildings_view_record_%d" % id)
			return false
		var height := clampf(
			radius_m * BUILDING_HEIGHT_RATIO, MIN_BUILDING_HEIGHT_M, MAX_BUILDING_HEIGHT_M
		)
		var transform: Transform3D = _building_transforms[index]
		if not transform.origin.is_equal_approx(Vector3(center.x, height * 0.5, center.y)):
			_fail("assert_buildings_view_position_%d" % id)
			return false
		if not transform.basis.get_scale().is_equal_approx(
			Vector3(radius_m * 2.0, height, radius_m * 2.0)
		):
			_fail("assert_buildings_view_scale_%d" % id)
			return false

	var counts := _render_node_counts()
	if int(counts["multimesh"]) != 2 or int(counts["mesh"]) != 3:
		_fail("assert_buildings_view_nodes_%d_%d" % [counts["multimesh"], counts["mesh"]])
		return false
	if _buildings_inside != SMOKE_BUILDINGS or _buildings_outside != 0:
		_fail("assert_buildings_view_classify_%d_%d" % [_buildings_inside, _buildings_outside])
		return false
	if _camera == null:
		_fail("assert_buildings_view_camera")
		return false
	return true

## Sorted mirror of the raw fixture records, independent of the render path.
func _expected_buildings(fixture: Dictionary) -> Array:
	var expected: Array = []
	var raw: Variant = fixture.get("buildings", null)
	if typeof(raw) != TYPE_ARRAY:
		return expected
	for entry in raw:
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
		expected.append(
			{
				"id": int(entry.get("id", 0)),
				"center": Vector2(float(center_raw[0]), float(center_raw[1])),
				"radius_m": radius_m,
			}
		)
	expected.sort_custom(
		func(a: Dictionary, b: Dictionary) -> bool:
			return int(a.get("id", 0)) < int(b.get("id", 0))
	)
	return expected

func _transforms_match(a: Array, b: Array) -> bool:
	if a.size() != b.size():
		return false
	for index in range(a.size()):
		var left: Transform3D = a[index]
		var right: Transform3D = b[index]
		if not left.origin.is_equal_approx(right.origin):
			return false
		if not left.basis.get_scale().is_equal_approx(right.basis.get_scale()):
			return false
	return true

## An 80-record payload: the client must clamp to 64 boxes and mark the view
## truncated even though the payload says `buildings_truncated: false`.
func _assert_buildings_cap_probe() -> bool:
	var buildings: Array = []
	for index in range(BUILDING_CAP_PROBE):
		var column := index % 8
		var row := index / 8
		buildings.append(
			{
				"id": index + 1,
				"center": [float(column) * 16.0 - 56.0, float(row) * 16.0 - 72.0],
				"radius_m": 6.0,
			}
		)
	var fixture := _view_fixture(
		{"radius_m": 2048.0, "buildings": buildings, "buildings_truncated": false}
	)
	if not _build(fixture):
		_fail("assert_buildings_cap_build_%s" % _last_error)
		return false
	print(
		"INTEREST_PREVIEW_BUILDINGS_CAP status=ok raw=%d buildings=%d boxes=%d truncated=%s checksum=%s"
		% [
			_buildings_raw_count,
			_buildings.size(),
			_building_count(),
			str(_buildings_truncated),
			_building_checksum,
		]
	)
	if (
		_buildings_raw_count != BUILDING_CAP_PROBE
		or _buildings.size() != MAX_BUILDINGS
		or _building_count() != MAX_BUILDINGS
	):
		_fail("assert_buildings_cap_counts")
		return false
	if not _buildings_truncated:
		_fail("assert_buildings_cap_truncated")
		return false
	if int(_buildings[0].get("id", 0)) != 1 or int(_buildings[-1].get("id", 0)) != MAX_BUILDINGS:
		_fail("assert_buildings_cap_order")
		return false
	if _building_checksum.length() != 16 or not _is_hex16(_building_checksum):
		_fail("assert_buildings_cap_checksum_%s" % _building_checksum)
		return false

	# An explicit server truncation flag must survive even when the client-side
	# list is under the cap (the server may have capped an earlier payload).
	var flagged := _view_fixture(
		{
			"buildings": [{"id": 1, "center": [0.0, 0.0], "radius_m": 4.0}],
			"buildings_truncated": true,
		}
	)
	if not _build(flagged):
		_fail("assert_buildings_flag_build_%s" % _last_error)
		return false
	print(
		"INTEREST_PREVIEW_BUILDINGS_FLAG status=ok buildings=%d boxes=%d truncated=%s"
		% [_buildings.size(), _building_count(), str(_buildings_truncated)]
	)
	if _buildings.size() != 1 or _building_count() != 1 or not _buildings_truncated:
		_fail("assert_buildings_flag")
		return false
	return true

func _fixture_bounds(fixture: Dictionary) -> Rect2:
	var roads: Array = InterestRender.parse_roads(fixture.get("roads", []))
	return _road_bounds(roads)

func _is_hex16(value: String) -> bool:
	if value.length() != 16:
		return false
	for index in range(16):
		var code := value.unicode_at(index)
		var is_digit := code >= 48 and code <= 57
		var is_lower := code >= 97 and code <= 102
		if not is_digit and not is_lower:
			return false
	return true

## Deterministic smoke fixture: 60 in-view roads arranged in five rings, four
## out-of-view roads beyond the circle, three malformed entries first, all
## inside the 64-road wire cap.
func _smoke_fixture() -> Dictionary:
	var roads: Array = []
	roads.append({"id": 9000, "start": [1.0], "end": [2.0, 3.0], "width_m": 6})
	roads.append("not a road")
	roads.append({"id": 9002, "start": [0.0, 0.0], "end": [1.0, 1.0], "width_m": -4})

	for index in range(SMOKE_INSIDE):
		var ring := index % 5
		var angle := TAU * float(index) / float(SMOKE_INSIDE)
		var radial := 8.0 + float(ring) * 7.0
		var length := 8.0 + float(index % 4) * 3.0
		var midpoint := SMOKE_CENTER + Vector2(cos(angle), sin(angle)) * radial
		var tangent := Vector2(cos(angle + PI * 0.5), sin(angle + PI * 0.5))
		var start := midpoint - tangent * (length * 0.5)
		var end := midpoint + tangent * (length * 0.5)
		roads.append(
			{
				"id": 1000 + index,
				"start": [start.x, start.y],
				"end": [end.x, end.y],
				"width_m": 4 + (index % 5) * 2,
			}
		)

	for index in range(SMOKE_OUTSIDE):
		var angle := TAU * float(index) / float(SMOKE_OUTSIDE) + 0.3
		var midpoint := (
			SMOKE_CENTER
			+ Vector2(cos(angle), sin(angle)) * (SMOKE_RADIUS_M + 24.0 + float(index) * 8.0)
		)
		var tangent := Vector2(cos(angle + PI * 0.5), sin(angle + PI * 0.5))
		var start := midpoint - tangent * 6.0
		var end := midpoint + tangent * 6.0
		roads.append(
			{
				"id": 2000 + index,
				"start": [start.x, start.y],
				"end": [end.x, end.y],
				"width_m": 6,
			}
		)

	return {
		"tick": SMOKE_TICK,
		"center": [SMOKE_CENTER.x, SMOKE_CENTER.y],
		"radius_m": SMOKE_RADIUS_M,
		"roads": roads,
		"truncated": false,
	}

## Small deterministic sample for the windowed/headless no-input run: a 5-road
## cross plus four outside roads, all inside the 64-road cap.
func _sample_fixture() -> Dictionary:
	var roads: Array = []
	var id := 1
	for step in range(-2, 3):
		var offset := float(step) * 16.0
		roads.append(
			{
				"id": id,
				"start": [-32.0, offset],
				"end": [32.0, offset],
				"width_m": 6,
			}
		)
		id += 1
		roads.append(
			{
				"id": id,
				"start": [offset, -32.0],
				"end": [offset, 32.0],
				"width_m": 4,
			}
		)
		id += 1
	for step in range(4):
		var angle := TAU * float(step) / 4.0 + 0.25
		var midpoint := Vector2(cos(angle), sin(angle)) * 84.0
		roads.append(
			{
				"id": id,
				"start": [midpoint.x - 5.0, midpoint.y],
				"end": [midpoint.x + 5.0, midpoint.y],
				"width_m": 6,
			}
		)
		id += 1
	return {
		"tick": 1,
		"center": [0.0, 0.0],
		"radius_m": 64.0,
		"roads": roads,
		"truncated": false,
		"buildings":
		[
			{"id": 1, "center": [0.0, 0.0], "radius_m": 7.0},
			{"id": 2, "center": [-24.0, 18.0], "radius_m": 5.0},
			{"id": 3, "center": [20.0, -26.0], "radius_m": 9.0},
			{"id": 4, "center": [32.0, 28.0], "radius_m": 6.0},
		],
		"buildings_truncated": false,
	}
