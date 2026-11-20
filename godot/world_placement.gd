extends Node3D

## Streamed-world placement preview.
##
## Fly over the procedural terrain, aim the ghost 24 m ahead, validate the
## rotated footprint, place a saved blueprint, and save/load the placement
## layer. The bridge owns validation and persistence; this node owns the ghost,
## placed meshes, HUD, and input.

const TerrainStreamer = preload("res://terrain_streamer.gd")
const EditorCamera = preload("res://editor_camera.gd")

const SEED := 2026
const TARGET_DISTANCE_M := 24.0
const FIXTURE_X := 169.0
const FIXTURE_Z := 125.0
const DEFAULT_BLUEPRINT := "acceptance_house.tsbp"
const DEFAULT_SAVE := "world.json"

var core
var streamer
var camera: Camera3D
var placed_root: Node3D

var ghost: Node3D
var ghost_mesh: MeshInstance3D
var ghost_foundation: MeshInstance3D
var ghost_material: StandardMaterial3D
var placed_material: StandardMaterial3D
var foundation_material: StandardMaterial3D

var hud: Label
var status_label: Label
var modal_label: Label

var blueprint_path := ""
var save_path := ""
var blueprint_mesh: ArrayMesh
var blueprint_info: Dictionary = {}
var mesh_cache: Dictionary = {}
var placed_nodes: Dictionary = {}

var rotation_deg := 0
var target_x := FIXTURE_X
var target_z := FIXTURE_Z
var last_cell := Vector2i(-999999, -999999)
var preview_validation: Dictionary = {}
var preview_valid := false
var preview_error := ""

func _ready() -> void:
	core = TownsmithCore.new()
	_resolve_paths()
	_setup_world()
	_setup_ui()
	_setup_ghost()
	if _ensure_blueprint():
		_refresh_preview(true)
	else:
		_set_modal("Blueprint failed: " + blueprint_path, true)
	_update_hud()

	if OS.get_cmdline_user_args().has("--scene-smoke"):
		_run_scene_smoke()
		return

	if DisplayServer.get_name() != "headless":
		Input.mouse_mode = Input.MOUSE_MODE_CAPTURED

func _process(_delta: float) -> void:
	if blueprint_mesh == null or not is_instance_valid(camera):
		return
	var forward := -camera.global_transform.basis.z
	forward.y = 0.0
	if forward.length_squared() < 0.0001:
		return
	var target := camera.global_position + forward.normalized() * TARGET_DISTANCE_M
	var cell := Vector2i(floori(target.x), floori(target.z))
	if cell == last_cell:
		return
	last_cell = cell
	target_x = float(cell.x)
	target_z = float(cell.y)
	_refresh_preview(false)

func _unhandled_input(event: InputEvent) -> void:
	if event is InputEventKey and event.pressed and not event.echo:
		match event.physical_keycode:
			KEY_R:
				rotation_deg = (rotation_deg + 90) % 360
				_refresh_preview(true)
			KEY_DELETE:
				_clear_placements()
			KEY_F5:
				_save_world()
			KEY_F9:
				_load_world()
			KEY_ESCAPE:
				_set_modal("", false)
				Input.mouse_mode = Input.MOUSE_MODE_VISIBLE
				_update_hud()
	elif event is InputEventMouseButton and event.pressed and event.button_index == MOUSE_BUTTON_LEFT:
		if Input.mouse_mode != Input.MOUSE_MODE_CAPTURED:
			Input.mouse_mode = Input.MOUSE_MODE_CAPTURED
			_set_modal("", false)
		else:
			_place_at_target()

func _resolve_paths() -> void:
	var user_dir := OS.get_user_data_dir().path_join("world_placement")
	if OS.get_cmdline_user_args().has("--scene-smoke"):
		user_dir = OS.get_user_data_dir().path_join("world_scene_smoke")
	DirAccess.make_dir_recursive_absolute(user_dir)
	if OS.get_cmdline_user_args().has("--scene-smoke"):
		# Keep the smoke deterministic across local reruns.
		for name in [DEFAULT_BLUEPRINT, DEFAULT_SAVE]:
			var stale := user_dir.path_join(name)
			if FileAccess.file_exists(stale):
				DirAccess.remove_absolute(stale)
	save_path = _arg_value("--world-save")
	if save_path == "":
		save_path = user_dir.path_join(DEFAULT_SAVE)
	else:
		save_path = _absolute_arg(save_path)
	var save_dir := save_path.get_base_dir()
	if save_dir != "":
		DirAccess.make_dir_recursive_absolute(save_dir)
	blueprint_path = _arg_value("--blueprint")
	if blueprint_path == "":
		blueprint_path = user_dir.path_join(DEFAULT_BLUEPRINT)
	else:
		blueprint_path = _absolute_arg(blueprint_path)

func _arg_value(name: String) -> String:
	var args := OS.get_cmdline_user_args()
	for index in range(args.size()):
		if args[index] == name and index + 1 < args.size():
			return args[index + 1]
		var prefix := name + "="
		if args[index].begins_with(prefix):
			return args[index].substr(prefix.length())
	return ""

func _absolute_arg(value: String) -> String:
	if value.begins_with("res://") or value.begins_with("user://"):
		return ProjectSettings.globalize_path(value)
	return value

func _setup_world() -> void:
	streamer = TerrainStreamer.new()
	streamer.seed = SEED
	streamer.view_radius = 1
	streamer.keep_radius = 2
	streamer.builds_per_frame = 2
	add_child(streamer)

	camera = EditorCamera.new()
	camera.position = Vector3(FIXTURE_X, 62.0, FIXTURE_Z + 26.0)
	camera.far = 1500.0
	camera.move_speed_mps = 24.0
	add_child(camera)
	camera.look_at(Vector3(FIXTURE_X, 41.0, FIXTURE_Z))
	camera.sync_look_from_rotation()
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

	placed_root = Node3D.new()
	placed_root.name = "Placed"
	add_child(placed_root)

func _setup_ui() -> void:
	var layer := CanvasLayer.new()
	add_child(layer)
	var panel := PanelContainer.new()
	panel.position = Vector2(16.0, 16.0)
	panel.custom_minimum_size = Vector2(620.0, 0.0)
	layer.add_child(panel)
	var box := VBoxContainer.new()
	box.add_theme_constant_override("separation", 4)
	panel.add_child(box)
	hud = Label.new()
	hud.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	box.add_child(hud)
	status_label = Label.new()
	status_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	box.add_child(status_label)
	modal_label = Label.new()
	modal_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	modal_label.add_theme_color_override("font_color", Color(1.0, 0.55, 0.35))
	modal_label.visible = false
	box.add_child(modal_label)

func _setup_ghost() -> void:
	ghost = Node3D.new()
	ghost.name = "PlacementGhost"
	ghost.visible = false
	add_child(ghost)

	ghost_material = StandardMaterial3D.new()
	ghost_material.transparency = BaseMaterial3D.TRANSPARENCY_ALPHA
	ghost_material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	ghost_material.cull_mode = BaseMaterial3D.CULL_DISABLED
	ghost_material.albedo_color = Color(0.2, 0.9, 0.4, 0.35)

	ghost_mesh = MeshInstance3D.new()
	ghost_mesh.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	ghost_mesh.material_override = ghost_material
	ghost.add_child(ghost_mesh)

	ghost_foundation = MeshInstance3D.new()
	ghost_foundation.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	ghost_foundation.visible = false
	var ghost_foundation_material := StandardMaterial3D.new()
	ghost_foundation_material.transparency = BaseMaterial3D.TRANSPARENCY_ALPHA
	ghost_foundation_material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	ghost_foundation_material.albedo_color = Color(0.3, 0.55, 0.3, 0.25)
	ghost_foundation.material_override = ghost_foundation_material
	ghost.add_child(ghost_foundation)

	placed_material = StandardMaterial3D.new()
	placed_material.albedo_color = Color(0.72, 0.62, 0.48)
	placed_material.roughness = 1.0
	placed_material.metallic = 0.0
	foundation_material = StandardMaterial3D.new()
	foundation_material.albedo_color = Color(0.45, 0.45, 0.42)
	foundation_material.roughness = 1.0
	foundation_material.metallic = 0.0

func _ensure_blueprint() -> bool:
	if not FileAccess.file_exists(blueprint_path):
		if not core.create_editor_volume(0, 0, 0):
			return false
		if not core.editor_build_acceptance_house():
			return false
		var saved: Dictionary = core.editor_save_blueprint(
			blueprint_path,
			"local:acceptance_house",
			"Acceptance House"
		)
		if not bool(saved.get("ok", false)):
			return false

	var info: Dictionary = core.world_blueprint_info(blueprint_path)
	if not bool(info.get("ok", false)):
		_set_modal("Blueprint rejected: " + str(info.get("error_code", "unknown")), true)
		return false
	blueprint_info = info
	var mesh: ArrayMesh = core.build_blueprint_mesh(blueprint_path)
	if mesh.get_surface_count() == 0:
		_set_modal("Blueprint mesh is empty", true)
		return false
	blueprint_mesh = mesh
	mesh_cache[blueprint_path] = mesh
	var bounds: PackedFloat32Array = info.get("bounds", PackedFloat32Array())
	if bounds.size() != 3:
		_set_modal("Blueprint bounds are invalid", true)
		return false
	ghost_mesh.mesh = mesh
	ghost_mesh.position = Vector3(-bounds[0] * 0.5, 0.0, -bounds[2] * 0.5)
	ghost.visible = true
	return true

func _refresh_preview(force: bool) -> void:
	if blueprint_mesh == null:
		return
	if force:
		last_cell = Vector2i(floori(target_x), floori(target_z))
	preview_validation = core.world_validate_placement(
		SEED, blueprint_path, target_x, target_z, rotation_deg
	)
	preview_valid = bool(preview_validation.get("ok", false))
	preview_error = str(preview_validation.get(
		"error_code", preview_validation.get("error", "")
	))

	var ground: float = core.world_height_at(SEED, target_x, target_z)
	if ground < 0.0:
		ground = camera.global_position.y - 8.0
	var y := ground
	var foundation := 0.0
	var footprint := Vector2(1.0, 1.0)
	var local_bounds: PackedFloat32Array = blueprint_info.get("bounds", PackedFloat32Array())
	if local_bounds.size() == 3:
		# The ghost pivot is rotated, so the foundation box must use the
		# unrotated blueprint footprint; `footprint_m` is already world-axis.
		footprint = Vector2(local_bounds[0], local_bounds[2])
	if preview_valid:
		var center: PackedFloat32Array = preview_validation.get("center", PackedFloat32Array())
		if center.size() == 3:
			y = center[1]
		foundation = float(preview_validation.get("foundation_m", 0.0))

	ghost.position = Vector3(target_x, y, target_z)
	ghost.rotation_degrees = Vector3(0.0, float(rotation_deg), 0.0)
	ghost_foundation.visible = preview_valid and foundation > 0.001
	if ghost_foundation.visible:
		var box := BoxMesh.new()
		box.size = Vector3(footprint.x, foundation, footprint.y)
		ghost_foundation.mesh = box
		ghost_foundation.position = Vector3(0.0, -foundation * 0.5, 0.0)
	_update_ghost_color()
	_update_hud()

func _update_ghost_color() -> void:
	if ghost_material == null:
		return
	ghost_material.albedo_color = (
		Color(0.2, 0.9, 0.4, 0.35) if preview_valid else Color(0.9, 0.2, 0.15, 0.35)
	)

func _update_hud() -> void:
	if hud == null:
		return
	var name := str(blueprint_info.get("name", "none"))
	var rooms := int(blueprint_info.get("rooms", 0))
	var capacity := int(blueprint_info.get("resident_capacity", 0))
	hud.text = "Townsmith — World Placement  (seed %d)\nBlueprint: %s  ·  %d rooms  ·  capacity %d\nWASD fly · Shift boost · Space/Ctrl up/down · R rotate · LMB place · Del clear · F5 save · F9 load · Esc release mouse" % [
		SEED, name, rooms, capacity,
	]
	var state := "VALID" if preview_valid else "INVALID: " + preview_error
	status_label.text = "Target (%d, %d)  rotation %d°  %s\nPlacements: %d   Save: %s" % [
		int(target_x),
		int(target_z),
		rotation_deg,
		state,
		core.world_placement_count(),
		save_path,
	]

func _place_at_target() -> bool:
	if not preview_valid or blueprint_mesh == null:
		_set_modal("Cannot place: " + preview_error, true)
		return false
	var result: Dictionary = core.world_place_blueprint(
		SEED, blueprint_path, target_x, target_z, rotation_deg
	)
	if not bool(result.get("ok", false)):
		preview_error = str(result.get("error_code", result.get("error", "place_failed")))
		_set_modal("Placement failed: " + preview_error, true)
		_update_hud()
		return false
	_add_placement_node(result.get("placement", {}))
	_set_modal("", false)
	_refresh_preview(true)
	return true

func _add_placement_node(placement: Dictionary) -> void:
	var id := int(placement.get("id", -1))
	if id < 0 or placed_nodes.has(id):
		return
	var path := str(placement.get("blueprint_path", blueprint_path))
	var mesh: ArrayMesh = mesh_cache.get(path)
	if mesh == null:
		mesh = core.build_blueprint_mesh(path)
		if mesh.get_surface_count() == 0:
			return
		mesh_cache[path] = mesh
	var bounds: PackedFloat32Array = placement.get("bounds", PackedFloat32Array())
	var center: PackedFloat32Array = placement.get("center", PackedFloat32Array())
	var footprint: PackedFloat32Array = placement.get("footprint", PackedFloat32Array())
	if bounds.size() != 3 or center.size() != 3 or footprint.size() != 2:
		return
	var pivot := Node3D.new()
	pivot.position = Vector3(center[0], center[1], center[2])
	pivot.rotation_degrees = Vector3(0.0, float(placement.get("rotation_deg", 0)), 0.0)
	var mesh_node := MeshInstance3D.new()
	mesh_node.mesh = mesh
	mesh_node.position = Vector3(-bounds[0] * 0.5, 0.0, -bounds[2] * 0.5)
	mesh_node.material_override = placed_material
	pivot.add_child(mesh_node)
	var foundation := float(placement.get("foundation_m", 0.0))
	if foundation > 0.001:
		var box := BoxMesh.new()
		box.size = Vector3(bounds[0], foundation, bounds[2])
		var foundation_node := MeshInstance3D.new()
		foundation_node.mesh = box
		foundation_node.position = Vector3(0.0, -foundation * 0.5, 0.0)
		foundation_node.material_override = foundation_material
		pivot.add_child(foundation_node)
	placed_root.add_child(pivot)
	placed_nodes[id] = pivot

func _clear_placed_nodes() -> void:
	for node in placed_nodes.values():
		if is_instance_valid(node):
			node.free()
	placed_nodes.clear()

func _clear_placements() -> void:
	_clear_placed_nodes()
	core.world_clear_placements()
	_set_modal("", false)
	_refresh_preview(true)
	_update_hud()

func _save_world() -> bool:
	var result: Dictionary = core.world_save(save_path)
	var ok := bool(result.get("ok", false))
	if ok:
		_set_modal("Saved %d placement(s) to %s" % [core.world_placement_count(), save_path], false)
	else:
		_set_modal("Save failed: " + str(result.get("error_code", result.get("error", ""))), true)
	_update_hud()
	return ok

func _load_world() -> bool:
	var result: Dictionary = core.world_load(save_path, SEED)
	if not bool(result.get("ok", false)):
		_set_modal("Load failed: " + str(result.get("error_code", result.get("error", ""))), true)
		_update_hud()
		return false
	_clear_placed_nodes()
	var state: Dictionary = core.world_placements()
	for placement in state.get("placements", []):
		_add_placement_node(placement)
	if placed_nodes.size() != int(state.get("count", -1)):
		_set_modal(
			"Loaded %d placement(s) but rebuilt %d" % [
				core.world_placement_count(), placed_nodes.size()
			],
			true
		)
		_update_hud()
		return false
	_set_modal("Loaded %d placement(s) from %s" % [core.world_placement_count(), save_path], false)
	_refresh_preview(true)
	return true

func _set_modal(message: String, is_error: bool) -> void:
	if modal_label == null:
		return
	modal_label.text = message
	modal_label.visible = message != ""
	modal_label.add_theme_color_override(
		"font_color", Color(1.0, 0.55, 0.35) if is_error else Color(0.75, 1.0, 0.75)
	)

func _foundation_matches_bounds(node: Node) -> bool:
	if node.get_child_count() < 2:
		return false
	var foundation_node := node.get_child(1) as MeshInstance3D
	if foundation_node == null or not (foundation_node.mesh is BoxMesh):
		return false
	var box := foundation_node.mesh as BoxMesh
	var bounds: PackedFloat32Array = blueprint_info.get("bounds", PackedFloat32Array())
	if bounds.size() != 3:
		return false
	return absf(box.size.x - bounds[0]) < 0.001 and absf(box.size.z - bounds[2]) < 0.001

func _run_scene_smoke() -> void:
	var status := "ok"
	var preview_ok := false
	var water_ok := false
	var rotation_ok := false
	var place_ok := false
	var save_ok := false
	var clear_ok := false
	var load_ok := false
	var rooms := int(blueprint_info.get("rooms", -1))

	target_x = FIXTURE_X
	target_z = FIXTURE_Z
	rotation_deg = 0
	_refresh_preview(true)
	preview_ok = preview_valid and ghost_material.albedo_color.g > ghost_material.albedo_color.r and absf(ghost.position.y - 41.0) < 0.001
	if not preview_ok:
		status = "preview_failed:" + preview_error

	target_x = 128.0
	target_z = 128.0
	_refresh_preview(true)
	water_ok = not preview_valid and preview_error == "water" and ghost_material.albedo_color.r > ghost_material.albedo_color.g
	if not water_ok and status == "ok":
		status = "water_failed:" + preview_error

	rotation_deg = 90
	target_x = FIXTURE_X
	target_z = FIXTURE_Z
	_refresh_preview(true)
	rotation_ok = preview_valid and absf(ghost.rotation_degrees.y - 90.0) < 0.001
	if not rotation_ok and status == "ok":
		status = "rotation_failed:" + preview_error

	place_ok = _place_at_target() and core.world_placement_count() == 1 and placed_root.get_child_count() == 1
	if place_ok:
		var node := placed_root.get_child(0)
		place_ok = absf(node.rotation_degrees.y - 90.0) < 0.001 and _foundation_matches_bounds(node)
	if not place_ok and status == "ok":
		status = "place_failed:" + preview_error

	if status == "ok":
		save_ok = _save_world()
		if not save_ok:
			status = "save_failed"

	if status == "ok":
		_clear_placements()
		clear_ok = core.world_placement_count() == 0 and placed_root.get_child_count() == 0
		if not clear_ok:
			status = "clear_failed"
		else:
			load_ok = _load_world() and core.world_placement_count() == 1 and placed_root.get_child_count() == 1
			if load_ok:
				var node := placed_root.get_child(0)
				load_ok = absf(node.rotation_degrees.y - 90.0) < 0.001 and _foundation_matches_bounds(node)
			if not load_ok:
				status = "load_failed"

	if status == "ok":
		# A failed load must leave the active store and placed nodes alone.
		var before_failed_load := placed_root.get_child_count()
		var blueprint_backup := blueprint_path + ".bak"
		if DirAccess.rename_absolute(blueprint_path, blueprint_backup) != OK:
			status = "load_guard_backup_failed"
		else:
			var failed_load := _load_world()
			if failed_load or core.world_placement_count() != 1 or placed_root.get_child_count() != before_failed_load:
				status = "load_failure_changed_scene"
			if DirAccess.rename_absolute(blueprint_backup, blueprint_path) != OK:
				status = "load_guard_restore_failed"

	if status == "ok" and rooms != 4:
		status = "rooms_mismatch"

	print("WORLD_SCENE rooms=%d preview=%s water=%s rotate=%s place=%s save=%s load=%s guard=%s status=%s" % [
		rooms,
		"ok" if preview_ok else "failed",
		"ok" if water_ok else "failed",
		"ok" if rotation_ok else "failed",
		"ok" if place_ok else "failed",
		"ok" if save_ok else "failed",
		"ok" if load_ok else "failed",
		"ok" if status == "ok" else "failed",
		status,
	])
	get_tree().quit(0 if status == "ok" else 1)
