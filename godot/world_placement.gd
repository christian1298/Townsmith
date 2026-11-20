extends Node3D

## Streamed-world placement preview.
##
## Fly over the procedural terrain, aim the ghost 24 m ahead, validate the
## rotated footprint, place a saved blueprint, and save/load the placement
## layer. The bridge owns validation and persistence; this node owns the ghost,
## placed meshes, HUD, and input.

const TerrainStreamer = preload("res://terrain_streamer.gd")
const EditorCamera = preload("res://editor_camera.gd")
const MayorCamera = preload("res://mayor_camera.gd")

const SEED := 2026
const FIXTURE_X := 169.0
const FIXTURE_Z := 125.0
const DEFAULT_BLUEPRINT := "acceptance_house.tsbp"
const DEFAULT_SAVE := "world.json"

var core
var streamer
var camera: Camera3D
var mayor_camera
var architect_camera
var mode := "mayor"
var placed_root: Node3D
var road_root: Node3D
var road_nodes: Dictionary = {}
var road_material: StandardMaterial3D
var road_mode := false
var road_start := Vector2i(-999999, -999999)
var lot_root: Node3D
var lot_nodes: Dictionary = {}
var lot_materials: Dictionary = {}
var lot_mode := false
var lot_start := Vector2i(-999999, -999999)
var settlement_root: Node3D
var settlement_nodes: Dictionary = {}
var settlement_material: StandardMaterial3D
var industry_root: Node3D
var industry_nodes: Dictionary = {}
var industry_kinds: Dictionary = {}
var industry_time := 0.0
var utility_root: Node3D
var utility_nodes: Dictionary = {}
var utility_kinds: Dictionary = {}
var utility_time := 0.0
var lot_zone := "residential"
var selection_box: MeshInstance3D
var selected_id := -1

var ghost: Node3D
var ghost_mesh: MeshInstance3D
var ghost_foundation: MeshInstance3D
var ghost_material: StandardMaterial3D
var placed_material: StandardMaterial3D
var foundation_material: StandardMaterial3D
var construction_material: StandardMaterial3D

var hud: Label
var status_label: Label
var modal_label: Label

var blueprint_path := ""
var save_path := ""
var blueprint_mesh: ArrayMesh
var blueprint_info: Dictionary = {}
var blueprint_info_cache: Dictionary = {}
var mesh_cache: Dictionary = {}
var placed_nodes: Dictionary = {}
var construction_nodes: Dictionary = {}
var construction_tick_accum := 0.0
var first_click_guard := true

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
	_setup_selection()
	_setup_roads()
	_setup_lots()
	_setup_settlements()
	_setup_industry()
	_setup_utilities()
	if _ensure_blueprint():
		_refresh_preview(true)
	else:
		_set_modal("Blueprint failed: " + blueprint_path, true)
	_update_hud()

	if OS.get_cmdline_user_args().has("--scene-smoke"):
		_run_scene_smoke()
		return

	if DisplayServer.get_name() != "headless":
		Input.mouse_mode = Input.MOUSE_MODE_VISIBLE

func _process(delta: float) -> void:
	construction_tick_accum += delta
	var steps := 0
	while construction_tick_accum >= 0.25 and steps < 8:
		_advance_construction(0.25)
		construction_tick_accum -= 0.25
		steps += 1
	if construction_tick_accum >= 1.0:
		construction_tick_accum = fmod(construction_tick_accum, 0.25)
	industry_time += delta
	for node in industry_nodes.values():
		if is_instance_valid(node):
			var base_y := float(node.get_meta("base_y", 0.0))
			node.position.y = base_y + sin(industry_time * 2.0) * 0.15
			var spinner = node.get_meta("spinner", null)
			if spinner != null and is_instance_valid(spinner):
				spinner.rotation.y += delta * 2.4
				var worker = spinner.get_child(0) if spinner.get_child_count() > 0 else null
				if worker != null and is_instance_valid(worker):
					worker.position.y = 0.8 + absf(sin(industry_time * 3.0)) * 0.35
	utility_time += delta
	for node in utility_nodes.values():
		if is_instance_valid(node):
			var base_y := float(node.get_meta("base_y", 0.0))
			node.position.y = base_y + sin(utility_time * 1.6) * 0.08
	if mode == "mayor" and mayor_camera != null:
		var ground: float = core.world_height_at(
			SEED, mayor_camera.target.x, mayor_camera.target.z
		)
		if ground >= 0.0:
			mayor_camera.target.y = ground
		mayor_camera.refresh()
	if blueprint_mesh == null or not is_instance_valid(camera):
		return
	if mode == "architect":
		return
	var target: Vector3 = mayor_camera.target
	var cell := Vector2i(floori(target.x), floori(target.z))
	if cell == last_cell:
		return
	last_cell = cell
	target_x = float(cell.x)
	target_z = float(cell.y)
	_refresh_preview(false)

func _unhandled_input(event: InputEvent) -> void:
	if event is InputEventKey and event.pressed and not event.echo:
		if event.physical_keycode == KEY_M and mode == "architect":
			_return_to_mayor()
			return
		if event.physical_keycode == KEY_ESCAPE:
			if mode == "architect":
				_return_to_mayor()
			elif lot_mode:
				if lot_start.x != -999999:
					lot_start = Vector2i(-999999, -999999)
					_set_modal("Lot corner cleared", false)
				else:
					lot_mode = false
					_set_modal("", false)
				_update_hud()
			elif road_mode:
				road_mode = false
				road_start = Vector2i(-999999, -999999)
				_set_modal("", false)
				_update_hud()
			else:
				_set_modal("", false)
				Input.mouse_mode = Input.MOUSE_MODE_VISIBLE
				_update_hud()
			return
		if mode == "mayor":
			match event.physical_keycode:
				KEY_R:
					rotation_deg = (rotation_deg + 90) % 360
					_refresh_preview(true)
				KEY_X:
					_edit_target(1)
				KEY_Z:
					_edit_target(-1)
				KEY_C:
					_toggle_crew()
				KEY_I:
					_cycle_industry()
				KEY_U:
					_cycle_utility()
				KEY_V:
					core.construction_add_materials(10000)
					_update_hud()
				KEY_DELETE:
					_clear_placements()
				KEY_G:
					_toggle_road_mode()
				KEY_H:
					_toggle_lot_mode()
				KEY_N:
					if event.shift_pressed:
						_clear_settlements()
					else:
						_found_settlement()
				KEY_1:
					if lot_mode:
						_set_lot_zone("residential")
				KEY_2:
					if lot_mode:
						_set_lot_zone("commercial")
				KEY_3:
					if lot_mode:
						_set_lot_zone("industrial")
				KEY_BACKSPACE:
					if lot_mode:
						core.lot_clear()
						_rebuild_lots()
					else:
						core.road_clear()
						_rebuild_roads()
					_update_hud()
				KEY_ENTER, KEY_KP_ENTER:
					_enter_architect()
				KEY_F5:
					_save_world()
				KEY_F9:
					_load_world()
		else:
			match event.physical_keycode:
				KEY_F5:
					_save_world()
				KEY_F9:
					_load_world()
	elif event is InputEventMouseButton and event.pressed:
		if mode == "mayor" and event.button_index == MOUSE_BUTTON_RIGHT:
			_select_at_screen(event.position)
		elif mode == "mayor" and event.button_index == MOUSE_BUTTON_LEFT:
			if first_click_guard:
				first_click_guard = false
				return
			if lot_mode:
				_lot_click(event.position)
			elif road_mode:
				_road_click(event.position)
			else:
				_place_at_target()
		elif mode == "architect" and event.button_index == MOUSE_BUTTON_LEFT:
			if Input.mouse_mode != Input.MOUSE_MODE_CAPTURED:
				Input.mouse_mode = Input.MOUSE_MODE_CAPTURED

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
	streamer.core = core
	add_child(streamer)

	mayor_camera = MayorCamera.new()
	add_child(mayor_camera)
	mayor_camera.focus(Vector3(FIXTURE_X, 41.0, FIXTURE_Z), 90.0)
	mayor_camera.current = true
	camera = mayor_camera

	architect_camera = EditorCamera.new()
	architect_camera.position = Vector3(FIXTURE_X, 43.0, FIXTURE_Z + 12.0)
	architect_camera.far = 1500.0
	architect_camera.move_speed_mps = 8.0
	add_child(architect_camera)
	architect_camera.look_at(Vector3(FIXTURE_X, 42.0, FIXTURE_Z))
	architect_camera.sync_look_from_rotation()
	architect_camera.current = false
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
	construction_material = StandardMaterial3D.new()
	construction_material.transparency = BaseMaterial3D.TRANSPARENCY_ALPHA
	construction_material.albedo_color = Color(0.95, 0.62, 0.25, 0.55)
	construction_material.roughness = 1.0
	construction_material.cull_mode = BaseMaterial3D.CULL_DISABLED

func _setup_selection() -> void:
	selection_box = MeshInstance3D.new()
	selection_box.name = "Selection"
	selection_box.visible = false
	selection_box.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	var material := StandardMaterial3D.new()
	material.transparency = BaseMaterial3D.TRANSPARENCY_ALPHA
	material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	material.cull_mode = BaseMaterial3D.CULL_DISABLED
	material.albedo_color = Color(1.0, 0.82, 0.2, 0.22)
	selection_box.material_override = material
	add_child(selection_box)

func _setup_roads() -> void:
	road_root = Node3D.new()
	road_root.name = "Roads"
	add_child(road_root)
	road_material = StandardMaterial3D.new()
	road_material.albedo_color = Color(0.22, 0.23, 0.24)
	road_material.roughness = 1.0
	road_material.metallic = 0.0

func _setup_lots() -> void:
	lot_root = Node3D.new()
	lot_root.name = "Lots"
	add_child(lot_root)
	var colors := {
		"residential": Color(0.25, 0.75, 0.35, 0.3),
		"commercial": Color(0.25, 0.5, 0.95, 0.3),
		"industrial": Color(0.95, 0.55, 0.2, 0.3),
	}
	for zone in colors:
		var material := StandardMaterial3D.new()
		material.transparency = BaseMaterial3D.TRANSPARENCY_ALPHA
		material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
		material.cull_mode = BaseMaterial3D.CULL_DISABLED
		material.albedo_color = colors[zone]
		lot_materials[zone] = material

func _toggle_lot_mode() -> void:
	lot_mode = not lot_mode
	lot_start = Vector2i(-999999, -999999)
	if lot_mode:
		road_mode = false
		road_start = Vector2i(-999999, -999999)
		_set_modal("Lot mode: click two corners · 1/2/3 zone · Esc cancels", false)
	else:
		_set_modal("", false)
	_update_hud()

func _set_lot_zone(zone: String) -> void:
	if not lot_materials.has(zone):
		return
	lot_zone = zone
	_set_modal("Lot zone: " + zone, false)
	_update_hud()

func _lot_click(screen: Vector2) -> bool:
	var point := _road_ground_point(screen)
	var cell := Vector2i(roundi(point.x / 4.0) * 4, roundi(point.y / 4.0) * 4)
	if lot_start.x == -999999:
		lot_start = cell
		_set_modal("Lot corner (%d, %d) · click the opposite corner" % [cell.x, cell.y], false)
		return true
	var min_x := mini(lot_start.x, cell.x)
	var min_z := mini(lot_start.y, cell.y)
	var max_x := maxi(lot_start.x, cell.x)
	var max_z := maxi(lot_start.y, cell.y)
	if max_x - min_x < 4 or max_z - min_z < 4:
		_set_modal("Lot needs at least 4 × 4 m", true)
		return false
	var ok := _add_lot(min_x, min_z, max_x, max_z, lot_zone)
	if ok:
		lot_start = Vector2i(-999999, -999999)
	return ok

func _add_lot(min_x: int, min_z: int, max_x: int, max_z: int, zone: String) -> bool:
	var result: Dictionary = core.lot_add(
		SEED, min_x, min_z, max_x, max_z, zone
	)
	if not bool(result.get("ok", false)):
		_set_modal("Lot rejected: " + str(result.get("error_code", "unknown")), true)
		return false
	_add_lot_node(result.get("lot", {}))
	_update_hud()
	return true

func _rebuild_lots() -> void:
	_clear_lot_nodes()
	var state: Dictionary = core.lots()
	for lot in state.get("lots", []):
		_add_lot_node(lot)

func _clear_lot_nodes() -> void:
	for node in lot_nodes.values():
		if is_instance_valid(node):
			node.free()
	lot_nodes.clear()

func _add_lot_node(lot: Dictionary) -> void:
	var id := int(lot.get("id", -1))
	if id < 0 or lot_nodes.has(id):
		return
	var bounds: PackedInt32Array = lot.get("bounds", PackedInt32Array())
	var center: PackedFloat32Array = lot.get("center", PackedFloat32Array())
	if bounds.size() != 4 or center.size() != 2:
		return
	var ground: float = core.world_height_at(SEED, center[0], center[1])
	if ground < 0.0:
		return
	var box := BoxMesh.new()
	box.size = Vector3(
		float(bounds[2] - bounds[0]), 0.1, float(bounds[3] - bounds[1])
	)
	var node := MeshInstance3D.new()
	node.mesh = box
	node.material_override = lot_materials.get(str(lot.get("zone", "residential")))
	node.position = Vector3(center[0], ground + 0.06, center[1])
	node.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	lot_root.add_child(node)
	lot_nodes[id] = node

func _lot_node_matches(lot: Dictionary) -> bool:
	var id := int(lot.get("id", -1))
	var node: MeshInstance3D = lot_nodes.get(id)
	if not is_instance_valid(node) or not (node.mesh is BoxMesh):
		return false
	var box := node.mesh as BoxMesh
	var bounds: PackedInt32Array = lot.get("bounds", PackedInt32Array())
	var center: PackedFloat32Array = lot.get("center", PackedFloat32Array())
	if bounds.size() != 4 or center.size() != 2:
		return false
	var zone := str(lot.get("zone", "residential"))
	return (
		absf(box.size.x - float(bounds[2] - bounds[0])) < 0.001
		and absf(box.size.z - float(bounds[3] - bounds[1])) < 0.001
		and absf(node.position.x - center[0]) < 0.001
		and absf(node.position.z - center[1]) < 0.001
		and node.material_override == lot_materials.get(zone)
	)

func _setup_settlements() -> void:
	settlement_root = Node3D.new()
	settlement_root.name = "Settlements"
	add_child(settlement_root)
	settlement_material = StandardMaterial3D.new()
	settlement_material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	settlement_material.cull_mode = BaseMaterial3D.CULL_DISABLED
	settlement_material.albedo_color = Color(0.95, 0.78, 0.2, 0.95)

func _setup_industry() -> void:
	industry_root = Node3D.new()
	industry_root.name = "Industry"
	add_child(industry_root)

func _industry_color(kind: String) -> Color:
	match kind:
		"farm":
			return Color(0.85, 0.75, 0.2)
		"hunt":
			return Color(0.7, 0.35, 0.2)
		_:
			return Color(0.45, 0.3, 0.15)

func _refresh_industry_visuals() -> void:
	if industry_root == null:
		return
	for node in industry_nodes.values():
		if is_instance_valid(node):
			node.free()
	industry_nodes.clear()
	industry_kinds.clear()
	if core == null:
		return
	var state: Dictionary = core.industry_state()
	for site in state.get("sites", []):
		var id := int(site.get("placement_id", -1))
		if id < 0:
			continue
		var kind := str(site.get("kind", ""))
		industry_kinds[id] = kind
		var pivot := Node3D.new()
		pivot.name = "Industry%d" % id
		var base_y := float(site.get("center_y", 40.0))
		pivot.position = Vector3(
			float(site.get("center_x", 0.0)),
			base_y,
			float(site.get("center_z", 0.0))
		)
		pivot.set_meta("base_y", base_y)
		var mesh_instance := MeshInstance3D.new()
		var box := BoxMesh.new()
		box.size = Vector3(0.8, 3.0, 0.8)
		mesh_instance.mesh = box
		mesh_instance.position.y = 1.7
		var material := StandardMaterial3D.new()
		material.albedo_color = _industry_color(kind)
		material.emission_enabled = true
		material.emission = _industry_color(kind) * 0.35
		mesh_instance.material_override = material
		pivot.add_child(mesh_instance)
		var spinner := Node3D.new()
		spinner.name = "WorkerLoop"
		var worker := MeshInstance3D.new()
		var worker_box := BoxMesh.new()
		worker_box.size = Vector3(0.45, 0.9, 0.45)
		worker.mesh = worker_box
		worker.position = Vector3(2.0, 0.8, 0.0)
		var worker_material := StandardMaterial3D.new()
		worker_material.albedo_color = Color(0.9, 0.9, 0.85)
		worker.material_override = worker_material
		spinner.add_child(worker)
		pivot.add_child(spinner)
		pivot.set_meta("spinner", spinner)
		var label := Label3D.new()
		label.text = kind.capitalize()
		label.font_size = 40
		label.billboard = BaseMaterial3D.BILLBOARD_ENABLED
		label.position.y = 3.9
		label.modulate = _industry_color(kind)
		pivot.add_child(label)
		industry_root.add_child(pivot)
		industry_nodes[id] = pivot

func _cycle_industry() -> void:
	if selected_id < 0:
		_set_modal("Select a building first (RMB)", true)
		return
	var selected := _selected_placement()
	if int(selected.get("job_slots", 0)) <= 0:
		_set_modal("This building has no job slots", true)
		return
	var current := str(industry_kinds.get(selected_id, ""))
	var next := ""
	match current:
		"":
			next = "lumberjack"
		"lumberjack":
			next = "farm"
		"farm":
			next = "hunt"
		"hunt":
			next = ""
	if next == "":
		core.industry_remove_site(selected_id)
		_set_modal("Industry removed", false)
	else:
		var result: Dictionary = core.industry_add_site(SEED, selected_id, next)
		if not bool(result.get("ok", false)):
			_set_modal(
				"Industry failed: " + str(result.get("error_code", result.get("error", ""))),
				true
			)
			return
		_set_modal("Industry: " + next, false)
	_refresh_industry_visuals()
	_update_hud()

func _setup_utilities() -> void:
	utility_root = Node3D.new()
	utility_root.name = "Utilities"
	add_child(utility_root)

func _utility_color(kind: String) -> Color:
	if kind == "power":
		return Color(0.95, 0.8, 0.25)
	return Color(0.25, 0.65, 0.95)

func _refresh_utility_visuals() -> void:
	if utility_root == null:
		return
	for node in utility_nodes.values():
		if is_instance_valid(node):
			node.free()
	utility_nodes.clear()
	utility_kinds.clear()
	if core == null:
		return
	var state: Dictionary = core.utilities_state()
	for site in state.get("sites", []):
		var id := int(site.get("placement_id", -1))
		if id < 0:
			continue
		var kind := str(site.get("kind", ""))
		if kind != "water" and kind != "power":
			continue
		if not utility_kinds.has(id):
			utility_kinds[id] = []
		utility_kinds[id].append(kind)
		var pivot := Node3D.new()
		pivot.name = "%s%d" % [kind.capitalize(), id]
		var base_y := float(site.get("center_y", 40.0))
		var offset := -1.4 if kind == "water" else 1.4
		pivot.position = Vector3(
			float(site.get("center_x", 0.0)) + offset,
			base_y,
			float(site.get("center_z", 0.0))
		)
		pivot.set_meta("base_y", base_y)
		var color := _utility_color(kind)
		var base := MeshInstance3D.new()
		var base_box := BoxMesh.new()
		base_box.size = Vector3(2.4, 0.22, 2.4)
		base.mesh = base_box
		base.position.y = 0.11
		var base_material := StandardMaterial3D.new()
		base_material.albedo_color = color
		base_material.emission_enabled = true
		base_material.emission = color * 0.25
		base.material_override = base_material
		pivot.add_child(base)
		var pipe := MeshInstance3D.new()
		var pipe_box := BoxMesh.new()
		pipe_box.size = Vector3(0.42, 2.6, 0.42)
		pipe.mesh = pipe_box
		pipe.position.y = 1.4
		var pipe_material := StandardMaterial3D.new()
		pipe_material.albedo_color = color
		pipe.material_override = pipe_material
		pivot.add_child(pipe)
		var label := Label3D.new()
		label.text = kind.capitalize()
		label.font_size = 36
		label.billboard = BaseMaterial3D.BILLBOARD_ENABLED
		label.position.y = 3.2
		label.modulate = color
		pivot.add_child(label)
		utility_root.add_child(pivot)
		utility_nodes["%s:%d" % [kind, id]] = pivot

func _cycle_utility() -> void:
	if selected_id < 0:
		_set_modal("Select a building first (RMB)", true)
		return
	var selected := _selected_placement()
	if int(selected.get("job_slots", 0)) <= 0:
		_set_modal("This building has no job slots", true)
		return
	var kinds: Array = utility_kinds.get(selected_id, [])
	var has_water := kinds.has("water")
	var has_power := kinds.has("power")
	if not has_water and not has_power:
		var result: Dictionary = core.utility_add_site(SEED, selected_id, "water")
		if not bool(result.get("ok", false)):
			_set_modal("Utility failed: " + str(result.get("error_code", "")), true)
			return
		_set_modal("Utility: water", false)
	elif has_water and not has_power:
		var result: Dictionary = core.utility_add_site(SEED, selected_id, "power")
		if not bool(result.get("ok", false)):
			_set_modal("Utility failed: " + str(result.get("error_code", "")), true)
			return
		_set_modal("Utility: water + power", false)
	elif has_power and not has_water:
		var result: Dictionary = core.utility_add_site(SEED, selected_id, "water")
		if not bool(result.get("ok", false)):
			_set_modal("Utility failed: " + str(result.get("error_code", "")), true)
			return
		_set_modal("Utility: water + power", false)
	else:
		core.utility_remove_site(SEED, selected_id, "water")
		core.utility_remove_site(SEED, selected_id, "power")
		_set_modal("Utility removed", false)
	_refresh_utility_visuals()
	_update_hud()

func _found_settlement() -> bool:
	var state: Dictionary = core.settlements()
	var used: Dictionary = {}
	for settlement in state.get("settlements", []):
		used[str(settlement.get("name", "")).to_lower()] = true
	var index: int = core.settlement_count() + 1
	var name := "Settlement %d" % index
	while used.has(name.to_lower()):
		index += 1
		name = "Settlement %d" % index
	var result: Dictionary = core.settlement_found(
		SEED,
		name,
		"player",
		roundi(target_x),
		roundi(target_z),
		64
	)
	if not bool(result.get("ok", false)):
		_set_modal("Settlement rejected: " + str(result.get("error_code", "unknown")), true)
		_update_hud()
		return false
	_add_settlement_node(result.get("settlement", {}))
	_set_modal("Founded %s" % str(result.get("settlement", {}).get("name", "settlement")), false)
	_update_hud()
	return true

func _clear_settlements() -> void:
	core.settlement_clear()
	_clear_settlement_nodes()
	_set_modal("Settlements cleared", false)
	_update_hud()

func _rebuild_settlements() -> void:
	_clear_settlement_nodes()
	var state: Dictionary = core.settlements()
	for settlement in state.get("settlements", []):
		_add_settlement_node(settlement)

func _clear_settlement_nodes() -> void:
	for node in settlement_nodes.values():
		if is_instance_valid(node):
			node.free()
	settlement_nodes.clear()

func _add_settlement_node(settlement: Dictionary) -> void:
	var id := int(settlement.get("id", -1))
	if id < 0 or settlement_nodes.has(id):
		return
	var center: PackedInt32Array = settlement.get("center", PackedInt32Array())
	if center.size() != 2:
		return
	var ground: float = core.world_height_at(SEED, center[0], center[1])
	if ground < 0.0:
		return
	var radius := float(settlement.get("radius_m", 0))
	if radius < 1.0:
		return
	var root := Node3D.new()
	root.position = Vector3(center[0], ground, center[1])
	root.set_meta("radius_m", radius)
	root.set_meta("owner", str(settlement.get("owner", "player")))

	var ring_mesh := ImmediateMesh.new()
	ring_mesh.surface_begin(Mesh.PRIMITIVE_LINES, settlement_material)
	var segments := 64
	for index in range(segments):
		var a0 := TAU * float(index) / float(segments)
		var a1 := TAU * float(index + 1) / float(segments)
		var p0 := Vector3(cos(a0) * radius, 0.0, sin(a0) * radius)
		var p1 := Vector3(cos(a1) * radius, 0.0, sin(a1) * radius)
		var h0: float = core.world_height_at(SEED, root.position.x + p0.x, root.position.z + p0.z)
		var h1: float = core.world_height_at(SEED, root.position.x + p1.x, root.position.z + p1.z)
		p0.y = (h0 - ground) + 0.5 if h0 >= 0.0 else 0.5
		p1.y = (h1 - ground) + 0.5 if h1 >= 0.0 else 0.5
		ring_mesh.surface_add_vertex(p0)
		ring_mesh.surface_add_vertex(p1)
	ring_mesh.surface_end()
	var ring := MeshInstance3D.new()
	ring.mesh = ring_mesh
	ring.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	root.add_child(ring)

	var label := Label3D.new()
	label.text = "%s\n%s" % [
		str(settlement.get("name", "Settlement")),
		str(settlement.get("owner", "player")),
	]
	label.position = Vector3(0.0, 5.0, 0.0)
	label.font_size = 64
	label.pixel_size = 0.02
	label.billboard = BaseMaterial3D.BILLBOARD_ENABLED
	label.no_depth_test = true
	label.modulate = Color(1.0, 0.9, 0.5)
	root.add_child(label)

	settlement_root.add_child(root)
	settlement_nodes[id] = root

func _settlement_node_matches(settlement: Dictionary) -> bool:
	var id := int(settlement.get("id", -1))
	var root: Node3D = settlement_nodes.get(id) as Node3D
	if not is_instance_valid(root) or root.get_child_count() < 2:
		return false
	var center: PackedInt32Array = settlement.get("center", PackedInt32Array())
	if center.size() != 2:
		return false
	var ring := root.get_child(0) as MeshInstance3D
	var label := root.get_child(1) as Label3D
	if ring == null or not (ring.mesh is ImmediateMesh) or label == null:
		return false
	var radius := float(settlement.get("radius_m", -1))
	var owner := str(settlement.get("owner", ""))
	var arrays: Array = ring.mesh.surface_get_arrays(0)
	var vertices: PackedVector3Array = arrays[Mesh.ARRAY_VERTEX]
	return (
		ring.mesh.get_surface_count() == 1
		and vertices.size() == 128
		and label.text.begins_with(str(settlement.get("name", "")))
		and label.text.contains(owner)
		and absf(float(root.get_meta("radius_m", -1)) - radius) < 0.001
		and str(root.get_meta("owner", "")) == owner
		and absf(root.position.x - center[0]) < 0.001
		and absf(root.position.z - center[1]) < 0.001
	)

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
	blueprint_info_cache[blueprint_path] = info
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
	var blueprint_name := str(blueprint_info.get("name", "none"))
	var rooms := int(blueprint_info.get("rooms", 0))
	var capacity := int(blueprint_info.get("resident_capacity", 0))
	var selected := _selected_placement()
	var selected_line := "Selected: none"
	if not selected.is_empty():
		var selected_info := _info_for_path(str(selected.get("blueprint_path", "")))
		var footprint: PackedFloat32Array = selected.get("footprint", PackedFloat32Array())
		var footprint_text := "? × ? m"
		if footprint.size() == 2:
			footprint_text = "%.0f × %.0f m" % [footprint[0], footprint[1]]
		selected_line = "Selected #%d: %s  ·  %d rooms  ·  capacity %d  ·  %d jobs  ·  %s" % [
			selected_id,
			str(selected.get("name", "?")),
			int(selected_info.get("rooms", 0)),
			int(selected.get("resident_capacity", 0)),
			int(selected.get("job_slots", 0)),
			footprint_text,
		]
		var center: PackedFloat32Array = selected.get("center", PackedFloat32Array())
		if center.size() == 3:
			var owner_info: Dictionary = core.settlement_at(roundi(center[0]), roundi(center[2]))
			var owner_name := "wilderness"
			if bool(owner_info.get("found", false)):
				var settlement: Dictionary = owner_info.get("settlement", {})
				owner_name = str(settlement.get("name", owner_name))
			selected_line += "  ·  owner " + owner_name
		var industry_kind := str(industry_kinds.get(selected_id, ""))
		if industry_kind != "":
			selected_line += "  ·  industry " + industry_kind
		var utility_kinds_here: Array = utility_kinds.get(selected_id, [])
		if utility_kinds_here.size() > 0:
			selected_line += "  ·  utility " + ", ".join(utility_kinds_here)
	if mode == "architect":
		hud.text = "Townsmith — Architect View (read-only)  (seed %d)\nBlueprint: %s\nFirst-person view at the selected door · M / Esc returns to Mayor" % [
			SEED, blueprint_name,
		]
	else:
		hud.text = "Townsmith — Mayor Mode  (seed %d)\nBlueprint: %s  ·  %d rooms  ·  capacity %d\nWASD pan · Q/E rotate · wheel zoom · R rotate · X raise · Z lower · N found settlement\nLMB place · RMB select · Enter: enter selected building\nC crew on/off · I industry · U utility · V +10k materials · F5 save · F9 load" % [
			SEED, blueprint_name, rooms, capacity,
		]
	var state := "VALID" if preview_valid else "INVALID: " + preview_error
	var construction: Dictionary = core.construction_summary()
	var city: Dictionary = core.city_summary()
	var economy: Dictionary = core.economy_state()
	var treasury_text := "—"
	if bool(economy.get("open", false)):
		var net := float(economy.get("daily_net", 0.0))
		var net_text := ("+" if net >= 0.0 else "") + "%.0f" % net
		treasury_text = "%.0f balance · %s/day · tax %.0f/%.0f" % [
			float(economy.get("balance", 0.0)),
			net_text,
			float(economy.get("tax_per_resident_per_day", 0.0)),
			float(economy.get("business_tax_per_job_per_day", 0.0)),
		]
	var industry: Dictionary = core.industry_state()
	var industry_text := "—"
	if bool(industry.get("open", false)):
		industry_text = "wood %.0f · food %.0f · hides %.0f · supply %.0f%%" % [
			float(industry.get("wood", 0.0)),
			float(industry.get("food", 0.0)),
			float(industry.get("hides", 0.0)),
			float(industry.get("food_supply", 0.0)) * 100.0,
		]
	var utilities: Dictionary = core.utilities_state()
	var utilities_text := "—"
	if bool(utilities.get("open", false)):
		utilities_text = "water %.0f%% (%d sites) · power %.0f%% (%d sites)" % [
			float(utilities.get("water_supply", 0.0)) * 100.0,
			int(utilities.get("water_sites", 0)),
			float(utilities.get("power_supply", 0.0)) * 100.0,
			int(utilities.get("power_sites", 0)),
		]
	var happiness_text := "—"
	var needs_text := "—"
	if int(city.get("population", 0)) > 0:
		happiness_text = "%.0f%%" % (float(city.get("happiness", 0.0)) * 100.0)
		needs_text = "food %.0f%% · water %.0f%% · rest %.0f%%" % [
			float(city.get("food", 0.0)) * 100.0,
			float(city.get("water", 0.0)) * 100.0,
			float(city.get("rest", 0.0)) * 100.0,
		]
	status_label.text = "%s\nTarget (%d, %d)  rotation %d°  %s\nPlacements: %d   Roads: %d   Lots: %d   Settlements: %d   Terrain edits: %d\nCity: population %d / housing %d (%d vacant) · jobs %d (%d employed) · happiness %s\nCommute %.0f m · wages %.0f/day\nNeeds: %s\nTreasury: %s\nIndustry: %s\nUtilities: %s\nConstruction: %d active / %d stalled   Crew: %d   Materials: %d L   Save: %s" % [
		selected_line,
		int(target_x),
		int(target_z),
		rotation_deg,
		state,
		core.world_placement_count(),
		core.road_count(),
		core.lot_count(),
		core.settlement_count(),
		core.terrain_delta_count(),
		int(city.get("population", 0)),
		int(city.get("housing_capacity", 0)),
		int(city.get("vacancies", 0)),
		int(city.get("jobs", 0)),
		int(city.get("employed", 0)),
		happiness_text,
		float(city.get("average_commute_m", 0.0)),
		float(city.get("daily_wages", 0.0)),
		needs_text,
		treasury_text,
		industry_text,
		utilities_text,
		int(construction.get("active", 0)),
		int(construction.get("stalled", 0)),
		int(construction.get("crew_capacity", 0)),
		int(construction.get("materials_stock", 0)),
		save_path,
	]

func _info_for_path(path: String) -> Dictionary:
	if path == "":
		return {}
	if blueprint_info_cache.has(path):
		return blueprint_info_cache[path]
	var info: Dictionary = core.world_blueprint_info(path)
	blueprint_info_cache[path] = info
	return info

func _selected_placement() -> Dictionary:
	if selected_id < 0:
		return {}
	var state: Dictionary = core.world_placements()
	for placement in state.get("placements", []):
		if int(placement.get("id", -1)) == selected_id:
			return placement
	return {}

func _select_at_screen(screen: Vector2) -> bool:
	var id := _pick_placement_at(screen)
	if id < 0:
		_clear_selection()
		return false
	return _select_placement(id)

func _select_placement(id: int) -> bool:
	selected_id = id
	var placement := _selected_placement()
	if placement.is_empty():
		_clear_selection()
		return false
	var center: PackedFloat32Array = placement.get("center", PackedFloat32Array())
	var footprint: PackedFloat32Array = placement.get("footprint", PackedFloat32Array())
	var bounds: PackedFloat32Array = placement.get("bounds", PackedFloat32Array())
	if center.size() != 3 or footprint.size() != 2 or bounds.size() != 3:
		_clear_selection()
		return false
	var box := BoxMesh.new()
	box.size = Vector3(footprint[0] + 0.4, bounds[1] + 0.4, footprint[1] + 0.4)
	selection_box.mesh = box
	selection_box.position = Vector3(
		center[0], center[1] + bounds[1] * 0.5, center[2]
	)
	selection_box.visible = true
	_update_hud()
	return true

func _clear_selection() -> void:
	selected_id = -1
	if selection_box != null:
		selection_box.visible = false
	_update_hud()

func _pick_placement_at(screen: Vector2) -> int:
	if camera == null:
		return -1
	var origin := camera.project_ray_origin(screen)
	var direction := camera.project_ray_normal(screen).normalized()
	var state: Dictionary = core.world_placements()
	var best := INF
	var best_id := -1
	for placement in state.get("placements", []):
		var center: PackedFloat32Array = placement.get("center", PackedFloat32Array())
		var footprint: PackedFloat32Array = placement.get("footprint", PackedFloat32Array())
		var bounds: PackedFloat32Array = placement.get("bounds", PackedFloat32Array())
		if center.size() != 3 or footprint.size() != 2 or bounds.size() != 3:
			continue
		var min_point := Vector3(
			center[0] - footprint[0] * 0.5,
			center[1],
			center[2] - footprint[1] * 0.5
		)
		var max_point := Vector3(
			center[0] + footprint[0] * 0.5,
			center[1] + bounds[1],
			center[2] + footprint[1] * 0.5
		)
		var hit := _ray_aabb(origin, direction, min_point, max_point)
		if hit >= 0.0 and hit < best:
			best = hit
			best_id = int(placement.get("id", -1))
	return best_id

func _ray_aabb(
	origin: Vector3, direction: Vector3, min_point: Vector3, max_point: Vector3
) -> float:
	var t_min := -INF
	var t_max := INF
	for axis in range(3):
		if absf(direction[axis]) < 0.000001:
			if origin[axis] < min_point[axis] or origin[axis] > max_point[axis]:
				return -1.0
			continue
		var inverse := 1.0 / direction[axis]
		var t1 := (min_point[axis] - origin[axis]) * inverse
		var t2 := (max_point[axis] - origin[axis]) * inverse
		if t1 > t2:
			var swap := t1
			t1 = t2
			t2 = swap
		t_min = maxf(t_min, t1)
		t_max = minf(t_max, t2)
		if t_min > t_max:
			return -1.0
	if t_min >= 0.0:
		return t_min
	return t_max if t_max >= 0.0 else -1.0

func _door_position(placement: Dictionary) -> Vector3:
	var center: PackedFloat32Array = placement.get("center", PackedFloat32Array())
	var footprint: PackedFloat32Array = placement.get("footprint", PackedFloat32Array())
	if center.size() != 3 or footprint.size() != 2:
		return Vector3.ZERO
	var bounds: PackedFloat32Array = placement.get("bounds", PackedFloat32Array())
	if bounds.size() != 3:
		return Vector3.ZERO
	var rotation := deg_to_rad(float(placement.get("rotation_deg", 0)))
	var front := Vector3(0.0, 0.0, 1.0).rotated(Vector3.UP, rotation)
	var distance := bounds[2] * 0.5 + 2.0
	return Vector3(center[0], center[1], center[2]) + front * distance

func _enter_architect() -> bool:
	if mode != "mayor":
		return false
	var placement := _selected_placement()
	if placement.is_empty():
		_set_modal("Select a building first (right-click)", true)
		return false
	var door := _door_position(placement)
	var center: PackedFloat32Array = placement.get("center", PackedFloat32Array())
	var ground: float = core.world_height_at(SEED, door.x, door.z)
	var base := center[1] if center.size() == 3 else 0.0
	if ground < 0.0:
		ground = base
	ground = maxf(ground, base)
	architect_camera.global_position = Vector3(door.x, ground + 1.7, door.z)
	if center.size() == 3:
		architect_camera.look_at(
			Vector3(center[0], center[1] + 1.5, center[2]), Vector3.UP
		)
	architect_camera.sync_look_from_rotation()
	architect_camera.current = true
	mayor_camera.current = false
	camera = architect_camera
	mode = "architect"
	streamer.bind_camera(architect_camera)
	ghost.visible = false
	selection_box.visible = false
	if DisplayServer.get_name() != "headless":
		Input.mouse_mode = Input.MOUSE_MODE_CAPTURED
	_set_modal("Read-only Architect view at the door · M returns to Mayor · voxel editing uses the workshop editor", false)
	_update_hud()
	return true

func _return_to_mayor() -> bool:
	if mode != "architect":
		return false
	mode = "mayor"
	camera = mayor_camera
	mayor_camera.current = true
	architect_camera.current = false
	streamer.bind_camera(mayor_camera)
	ghost.visible = true
	selection_box.visible = selected_id >= 0
	Input.mouse_mode = Input.MOUSE_MODE_VISIBLE
	_set_modal("", false)
	_refresh_preview(true)
	return true

func _edit_target(delta: int) -> bool:
	var current: float = core.world_height_at(SEED, target_x, target_z)
	if current < 0.0:
		_set_modal("No terrain at the target", true)
		return false
	var height := clampi(roundi(current) + delta, 0, 255)
	var result: Dictionary = core.terrain_edit_height(
		SEED, int(target_x), int(target_z), height
	)
	if not bool(result.get("ok", false)):
		_set_modal("Terrain edit failed: " + str(result.get("error_code", "unknown")), true)
		_update_hud()
		return false
	var chunk := Vector2i(
		int(result.get("chunk_x", 0)), int(result.get("chunk_z", 0))
	)
	var local_x := int(result.get("local_x", 0))
	var local_z := int(result.get("local_z", 0))
	for coord in _touched_chunks(chunk, local_x, local_z):
		streamer.invalidate_chunk(coord)
	_set_modal("Terrain %d m at (%d, %d)" % [height, int(target_x), int(target_z)], false)
	_refresh_preview(true)
	return true

func _touched_chunks(chunk: Vector2i, local_x: int, local_z: int) -> Array[Vector2i]:
	var touched: Array[Vector2i] = [chunk]
	if local_x == 0:
		touched.append(chunk + Vector2i(-1, 0))
	elif local_x == 255:
		touched.append(chunk + Vector2i(1, 0))
	if local_z == 0:
		touched.append(chunk + Vector2i(0, -1))
	elif local_z == 255:
		touched.append(chunk + Vector2i(0, 1))
	return touched

func _toggle_road_mode() -> void:
	road_mode = not road_mode
	road_start = Vector2i(-999999, -999999)
	if road_mode:
		lot_mode = false
		lot_start = Vector2i(-999999, -999999)
		_set_modal("Road mode: click start, then end · 8 m wide · Esc cancels", false)
	else:
		_set_modal("", false)
	_update_hud()

func _road_click(screen: Vector2) -> bool:
	var point := _road_ground_point(screen)
	var cell := Vector2i(roundi(point.x), roundi(point.y))
	if road_start.x == -999999:
		road_start = cell
		_set_modal("Road start (%d, %d) · click the end" % [cell.x, cell.y], false)
		return true
	var end := cell
	if absi(end.x - road_start.x) >= absi(end.y - road_start.y):
		end.y = road_start.y
	else:
		end.x = road_start.x
	if end == road_start:
		_set_modal("Road needs a non-zero length", true)
		return false
	var ok := _add_road(road_start.x, road_start.y, end.x, end.y)
	if ok:
		_set_modal("Road %d m at (%d, %d)..(%d, %d)" % [
			absi(end.x - road_start.x) + absi(end.y - road_start.y),
			road_start.x, road_start.y, end.x, end.y
		], false)
		road_start = Vector2i(-999999, -999999)
	return ok

func _add_road(x1: int, z1: int, x2: int, z2: int) -> bool:
	var result: Dictionary = core.road_add(SEED, x1, z1, x2, z2, 8)
	if not bool(result.get("ok", false)):
		_set_modal("Road rejected: " + str(result.get("error_code", "unknown")), true)
		return false
	_rebuild_roads()
	_update_hud()
	return true

func _rebuild_roads() -> void:
	for node in road_nodes.values():
		if is_instance_valid(node):
			node.free()
	road_nodes.clear()
	var state: Dictionary = core.roads()
	for road in state.get("roads", []):
		_add_road_node(road)

func _add_road_node(road: Dictionary) -> void:
	var id := int(road.get("id", -1))
	if id < 0 or road_nodes.has(id):
		return
	var center: PackedFloat32Array = road.get("center", PackedFloat32Array())
	var start: PackedInt32Array = road.get("start", PackedInt32Array())
	var end: PackedInt32Array = road.get("end", PackedInt32Array())
	var width := float(road.get("width_m", 8))
	var length := float(road.get("length_m", 0))
	if center.size() != 2 or start.size() != 2 or end.size() != 2 or length <= 0.0:
		return
	var ground: float = core.world_height_at(SEED, center[0], center[1])
	if ground < 0.0:
		return
	var box := BoxMesh.new()
	if bool(road.get("runs_along_x", false)):
		box.size = Vector3(length + 0.2, 0.2, width)
	else:
		box.size = Vector3(width, 0.2, length + 0.2)
	var node := MeshInstance3D.new()
	node.mesh = box
	node.material_override = road_material
	node.position = Vector3(center[0], ground + 0.12, center[1])
	node.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	road_root.add_child(node)
	road_nodes[id] = node

func _road_ground_point(screen: Vector2) -> Vector2:
	var origin := camera.project_ray_origin(screen)
	var direction := camera.project_ray_normal(screen).normalized()
	for distance in range(0, 600, 2):
		var point := origin + direction * float(distance)
		var height: float = core.world_height_at(SEED, point.x, point.z)
		if height >= 0.0 and point.y <= height + 0.5:
			return Vector2(point.x, point.z)
	return Vector2(mayor_camera.target.x, mayor_camera.target.z)

func _road_node_matches(road: Dictionary) -> bool:
	var id := int(road.get("id", -1))
	var node: MeshInstance3D = road_nodes.get(id)
	if not is_instance_valid(node) or not (node.mesh is BoxMesh):
		return false
	var box := node.mesh as BoxMesh
	var width := float(road.get("width_m", 8))
	var length := float(road.get("length_m", 0))
	if bool(road.get("runs_along_x", false)):
		return absf(box.size.x - (length + 0.2)) < 0.001 and absf(box.size.z - width) < 0.001
	return absf(box.size.x - width) < 0.001 and absf(box.size.z - (length + 0.2)) < 0.001

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
	_add_placement_visual(result.get("placement", {}))
	_set_modal("Construction site created", false)
	_refresh_preview(true)
	return true

func _add_placement_visual(placement: Dictionary, site_override: Dictionary = {}) -> void:
	var site := site_override
	if site.is_empty():
		site = _construction_site_for(int(placement.get("id", -1)))
	if site.is_empty() or bool(site.get("complete", false)):
		_add_placement_node(placement)
	else:
		_add_construction_node(placement, site)

func _construction_index() -> Dictionary:
	var index: Dictionary = {}
	var state: Dictionary = core.construction_sites()
	for site in state.get("sites", []):
		index[int(site.get("id", -1))] = site
	return index

func _construction_site_for(id: int) -> Dictionary:
	if id < 0:
		return {}
	var state: Dictionary = core.construction_sites()
	for site in state.get("sites", []):
		if int(site.get("id", -1)) == id:
			return site
	return {}

func _create_placement_pivot(
	placement: Dictionary, material: StandardMaterial3D, y_scale: float
) -> Node3D:
	var id := int(placement.get("id", -1))
	if id < 0 or placed_nodes.has(id):
		return null
	var path := str(placement.get("blueprint_path", blueprint_path))
	var mesh: ArrayMesh = mesh_cache.get(path)
	if mesh == null:
		mesh = core.build_blueprint_mesh(path)
		if mesh.get_surface_count() == 0:
			return null
		mesh_cache[path] = mesh
	var bounds: PackedFloat32Array = placement.get("bounds", PackedFloat32Array())
	var center: PackedFloat32Array = placement.get("center", PackedFloat32Array())
	if bounds.size() != 3 or center.size() != 3:
		return null
	var pivot := Node3D.new()
	pivot.position = Vector3(center[0], center[1], center[2])
	pivot.rotation_degrees = Vector3(0.0, float(placement.get("rotation_deg", 0)), 0.0)
	var mesh_node := MeshInstance3D.new()
	mesh_node.mesh = mesh
	mesh_node.position = Vector3(-bounds[0] * 0.5, 0.0, -bounds[2] * 0.5)
	mesh_node.scale.y = maxf(y_scale, 0.01)
	mesh_node.material_override = material
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
	return pivot

func _add_placement_node(placement: Dictionary) -> void:
	var pivot := _create_placement_pivot(placement, placed_material, 1.0)
	if pivot == null:
		return
	placed_root.add_child(pivot)
	placed_nodes[int(placement.get("id", -1))] = pivot

func _add_construction_node(placement: Dictionary, site: Dictionary) -> void:
	var id := int(placement.get("id", -1))
	var progress := clampf(float(site.get("progress", 0.0)), 0.02, 0.98)
	var pivot := _create_placement_pivot(placement, construction_material, progress)
	if pivot == null:
		return
	var bounds: PackedFloat32Array = placement.get("bounds", PackedFloat32Array())
	var label := Label3D.new()
	label.billboard = BaseMaterial3D.BILLBOARD_ENABLED
	label.no_depth_test = true
	label.font_size = 28
	label.position = Vector3(0.0, (bounds[1] if bounds.size() == 3 else 6.0) + 1.2, 0.0)
	label.text = _construction_label(site)
	pivot.add_child(label)
	placed_root.add_child(pivot)
	placed_nodes[id] = pivot
	construction_nodes[id] = {
		"pivot": pivot,
		"mesh": pivot.get_child(0),
		"label": label,
		"placement": placement,
	}

func _construction_label(site: Dictionary) -> String:
	var phase := str(site.get("phase_label", site.get("phase", "Construction")))
	var percent := roundi(float(site.get("progress", 0.0)) * 100.0)
	var stall := str(site.get("stall_reason", ""))
	if stall != "":
		return "%s %d%% · stalled: %s" % [phase, percent, stall]
	return "%s %d%%" % [phase, percent]

func _refresh_construction_visuals() -> void:
	if construction_nodes.is_empty():
		return
	var state: Dictionary = core.construction_sites()
	var sites: Dictionary = {}
	for site in state.get("sites", []):
		sites[int(site.get("id", -1))] = site
	for id in construction_nodes.keys().duplicate():
		var entry: Dictionary = construction_nodes[id]
		var site: Dictionary = sites.get(id, {})
		if site.is_empty() or bool(site.get("complete", false)):
			var pivot: Node3D = entry.get("pivot")
			if is_instance_valid(pivot):
				pivot.free()
			placed_nodes.erase(id)
			construction_nodes.erase(id)
			_add_placement_node(entry.get("placement", {}))
			continue
		var mesh: MeshInstance3D = entry.get("mesh")
		var label: Label3D = entry.get("label")
		if is_instance_valid(mesh):
			mesh.scale.y = clampf(float(site.get("progress", 0.0)), 0.02, 0.98)
		if is_instance_valid(label):
			label.text = _construction_label(site)

func _advance_construction(seconds: float) -> Dictionary:
	if seconds <= 0.0:
		return {}
	var report: Dictionary = core.construction_tick(seconds)
	core.utilities_refresh(SEED)
	core.industry_tick(SEED, seconds)
	core.population_tick(SEED, seconds)
	core.economy_tick(SEED, seconds)
	_refresh_construction_visuals()
	_update_hud()
	return report

func _toggle_crew() -> void:
	var summary: Dictionary = core.construction_summary()
	var capacity := int(summary.get("crew_capacity", 4))
	var next := 0 if capacity > 0 else 4
	if core.construction_set_crew(next):
		_set_modal("Construction crew: %d" % next, false)
	_update_hud()

func _clear_placed_nodes() -> void:
	for node in placed_nodes.values():
		if is_instance_valid(node):
			node.free()
	placed_nodes.clear()
	construction_nodes.clear()

func _clear_placements() -> void:
	_clear_selection()
	_clear_placed_nodes()
	core.world_clear_placements()
	_refresh_industry_visuals()
	_refresh_utility_visuals()
	_set_modal("", false)
	_refresh_preview(true)
	_update_hud()

func _save_world() -> bool:
	var result: Dictionary = core.world_save(save_path)
	var ok := bool(result.get("ok", false))
	if ok:
		_set_modal("Saved %d placement(s), %d edit(s), %d site(s), %d road(s), %d lot(s), %d settlement(s) to %s" % [
			core.world_placement_count(), core.terrain_delta_count(),
			int(result.get("construction_sites", 0)), core.road_count(),
			core.lot_count(), core.settlement_count(), save_path
		], false)
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
	_clear_selection()
	_clear_placed_nodes()
	_rebuild_roads()
	_rebuild_lots()
	_rebuild_settlements()
	_refresh_industry_visuals()
	_refresh_utility_visuals()
	streamer.invalidate_all()
	var state: Dictionary = core.world_placements()
	var construction_index := _construction_index()
	for placement in state.get("placements", []):
		_add_placement_visual(
			placement, construction_index.get(int(placement.get("id", -1)), {})
		)
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
	var edit_ok := false
	var place_ok := false
	var road_ok := false
	var lot_ok := false
	var settlement_ok := false
	var select_ok := false
	var enter_ok := false
	var return_ok := false
	var build_ok := false
	var build_done_ok := false
	var occupancy_before_ok := false
	var occupancy_after_ok := false
	var industry_ok := false
	var utility_ok := false
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

	if status == "ok":
		var bad_road: Dictionary = core.road_add(SEED, 120, 128, 136, 128, 8)
		var empty_after_reject := not bool(core.world_placements().get("open", true))
		var road_added := _add_road(180, 125, 200, 125)
		var roads_state: Dictionary = core.roads()
		var road_list: Array = roads_state.get("roads", [])
		road_ok = not bool(bad_road.get("ok", true)) and empty_after_reject
		road_ok = road_ok and road_added and core.road_count() == 1
		road_ok = road_ok and road_nodes.size() == 1
		road_ok = road_ok and road_list.size() == 1 and _road_node_matches(road_list[0])
		road_ok = road_ok and bool(core.world_placements().get("open", false))
		road_ok = road_ok and core.world_placement_count() == 0
		if not road_ok:
			status = "road_failed:" + str(bad_road.get("error_code", ""))

		var bad_lot: Dictionary = core.lot_add(SEED, 120, 128, 136, 140, "residential")
		var lot_added := _add_lot(184, 128, 198, 140, "residential")
		var lots_state: Dictionary = core.lots()
		var lot_list: Array = lots_state.get("lots", [])
		lot_ok = not bool(bad_lot.get("ok", true)) and str(bad_lot.get("error_code", "")) == "no_road_frontage"
		lot_ok = lot_ok and lot_added and core.lot_count() == 1 and lot_nodes.size() == 1
		lot_ok = lot_ok and lot_list.size() == 1 and _lot_node_matches(lot_list[0])
		if not lot_ok:
			status = "lot_failed:" + str(bad_lot.get("error_code", ""))

		var settlement_found := _found_settlement()
		var settlement_state: Dictionary = core.settlements()
		var settlement_list: Array = settlement_state.get("settlements", [])
		var owner_state: Dictionary = core.settlement_at(int(FIXTURE_X), int(FIXTURE_Z))
		settlement_ok = settlement_found and core.settlement_count() == 1
		settlement_ok = settlement_ok and settlement_list.size() == 1
		settlement_ok = settlement_ok and _settlement_node_matches(settlement_list[0])
		settlement_ok = settlement_ok and bool(owner_state.get("found", false))
		var owner_settlement: Dictionary = owner_state.get("settlement", {})
		settlement_ok = settlement_ok and str(owner_settlement.get("owner", "")) == "player"
		if not settlement_ok:
			status = "settlement_failed"

	edit_ok = _edit_target(1) and core.terrain_delta_count() == 1 and absf(core.world_height_at(SEED, FIXTURE_X, FIXTURE_Z) - 42.0) < 0.001 and streamer.dirty_count() > 0
	var border_touched := _touched_chunks(Vector2i(0, 0), 0, 255)
	edit_ok = edit_ok and border_touched.size() == 3 and border_touched.has(Vector2i(-1, 0)) and border_touched.has(Vector2i(0, 1))
	# A chunk that is still on its first build must also be invalidated by load.
	streamer.in_flight[9999] = {"coord": Vector2i(1, 1), "step": 1, "generation": 0}
	streamer.invalidate_all()
	edit_ok = edit_ok and streamer.dirty.has(Vector2i(1, 1))
	streamer.in_flight.erase(9999)
	if not edit_ok and status == "ok":
		status = "edit_failed:" + preview_error

	place_ok = _place_at_target() and core.world_placement_count() == 1 and placed_root.get_child_count() == 1
	if place_ok:
		var node := placed_root.get_child(0)
		place_ok = absf(node.rotation_degrees.y - 90.0) < 0.001 and _foundation_matches_bounds(node)
	if not place_ok and status == "ok":
		status = "place_failed:" + preview_error

	if status == "ok":
		var site := _construction_site_for(1)
		build_ok = not site.is_empty() and core.construction_summary().sites == 1
		build_ok = build_ok and construction_nodes.has(1) and int(site.get("crew_required", 0)) >= 1
		var before_progress := float(site.get("progress", 0.0))
		_advance_construction(2.0)
		var after_site := _construction_site_for(1)
		build_ok = build_ok and float(after_site.get("progress", 0.0)) > before_progress
		var city_before: Dictionary = core.city_summary()
		occupancy_before_ok = int(city_before.get("population", -1)) == 0
		occupancy_before_ok = occupancy_before_ok and int(city_before.get("housing_capacity", -1)) == 0
		occupancy_before_ok = occupancy_before_ok and int(city_before.get("jobs", -1)) == 0
		occupancy_before_ok = occupancy_before_ok and int(city_before.get("completed_buildings", -1)) == 0
		occupancy_before_ok = occupancy_before_ok and int(city_before.get("construction_sites", -1)) == 1
		if not build_ok:
			status = "construction_failed"

	if status == "ok":
		mayor_camera.focus(Vector3(FIXTURE_X, 42.0, FIXTURE_Z), 60.0)
		mayor_camera.current = true
		architect_camera.current = false
		camera = mayor_camera
		var screen := camera.unproject_position(Vector3(FIXTURE_X, 44.0, FIXTURE_Z))
		var miss_ok := not _select_at_screen(screen + Vector2(2000.0, 0.0))
		select_ok = miss_ok and _select_at_screen(screen) and selected_id >= 0 and selection_box.visible
		if select_ok:
			var selected := _selected_placement()
			var selected_info := _info_for_path(str(selected.get("blueprint_path", "")))
			select_ok = int(selected_info.get("rooms", -1)) == 4
		if select_ok:
			var mayor_target: Vector3 = mayor_camera.target
			enter_ok = _enter_architect() and mode == "architect" and camera == architect_camera
			if enter_ok:
				var selected := _selected_placement()
				var center: PackedFloat32Array = selected.get("center", PackedFloat32Array())
				enter_ok = architect_camera.current and streamer.camera == architect_camera
				enter_ok = enter_ok and not ghost.visible and not selection_box.visible
				if center.size() == 3:
					enter_ok = enter_ok and architect_camera.global_position.y >= center[1] + 1.7 - 0.001
					var forward: Vector3 = -architect_camera.global_transform.basis.z
					var to_center: Vector3 = Vector3(center[0], center[1] + 1.5, center[2]) - architect_camera.global_position
					enter_ok = enter_ok and forward.dot(to_center.normalized()) > 0.9
				return_ok = _return_to_mayor() and mode == "mayor" and camera == mayor_camera
				return_ok = return_ok and mayor_camera.current and streamer.camera == mayor_camera
				return_ok = return_ok and ghost.visible and selection_box.visible
				return_ok = return_ok and mayor_camera.target.distance_to(mayor_target) < 0.001
		if not select_ok:
			status = "select_failed"
		elif not enter_ok:
			status = "enter_failed"
		elif not return_ok:
			status = "return_failed"

	var saved_site_progress := 0.0
	if status == "ok":
		saved_site_progress = float(_construction_site_for(1).get("progress", 0.0))
		save_ok = _save_world()
		if not save_ok:
			status = "save_failed"

	var road_checksum := ""
	var lot_checksum := ""
	var settlement_checksum := ""
	if status == "ok":
		var edit_checksum := str(core.terrain_delta_checksum())
		road_checksum = str(core.road_checksum())
		lot_checksum = str(core.lot_checksum())
		settlement_checksum = str(core.settlement_checksum())
		_clear_placements()
		clear_ok = core.world_placement_count() == 0 and placed_root.get_child_count() == 0
		clear_ok = clear_ok and core.construction_summary().sites == 0
		if not clear_ok:
			status = "clear_failed"
		else:
			core.terrain_clear_deltas()
			core.road_clear()
			_rebuild_roads()
			core.lot_clear()
			_rebuild_lots()
			_clear_settlements()
			if core.terrain_delta_count() != 0 or core.road_count() != 0 or road_nodes.size() != 0:
				status = "delta_clear_failed"
			if core.lot_count() != 0 or lot_nodes.size() != 0:
				status = "lot_clear_failed"
			if core.settlement_count() != 0 or settlement_nodes.size() != 0:
				status = "settlement_clear_failed"
			load_ok = _load_world() and core.world_placement_count() == 1 and placed_root.get_child_count() == 1
			if load_ok:
				var node := placed_root.get_child(0)
				load_ok = absf(node.rotation_degrees.y - 90.0) < 0.001 and _foundation_matches_bounds(node)
				load_ok = load_ok and core.terrain_delta_count() == 1
				load_ok = load_ok and absf(core.world_height_at(SEED, FIXTURE_X, FIXTURE_Z) - 42.0) < 0.001
				load_ok = load_ok and str(core.terrain_delta_checksum()) == edit_checksum
				load_ok = load_ok and core.terrain_deltas_seed() == SEED
				load_ok = load_ok and core.road_count() == 1
				load_ok = load_ok and str(core.road_checksum()) == road_checksum
				load_ok = load_ok and road_nodes.size() == 1
				load_ok = load_ok and core.lot_count() == 1
				load_ok = load_ok and str(core.lot_checksum()) == lot_checksum
				load_ok = load_ok and lot_nodes.size() == 1
				load_ok = load_ok and core.settlement_count() == 1
				load_ok = load_ok and str(core.settlement_checksum()) == settlement_checksum
				load_ok = load_ok and settlement_nodes.size() == 1
				if core.road_count() == 1:
					var roads_state: Dictionary = core.roads()
					var road_list: Array = roads_state.get("roads", [])
					load_ok = load_ok and road_list.size() == 1 and _road_node_matches(road_list[0])
				if core.lot_count() == 1:
					var lots_state: Dictionary = core.lots()
					var lot_list: Array = lots_state.get("lots", [])
					load_ok = load_ok and lot_list.size() == 1 and _lot_node_matches(lot_list[0])
				if core.settlement_count() == 1:
					var settlements_state: Dictionary = core.settlements()
					var settlement_list: Array = settlements_state.get("settlements", [])
					load_ok = load_ok and settlement_list.size() == 1 and _settlement_node_matches(settlement_list[0])
					var owner_after: Dictionary = core.settlement_at(int(FIXTURE_X), int(FIXTURE_Z))
					var owner_record: Dictionary = owner_after.get("settlement", {})
					load_ok = load_ok and bool(owner_after.get("found", false))
					load_ok = load_ok and str(owner_record.get("owner", "")) == "player"
				load_ok = load_ok and construction_nodes.has(1) and not placed_nodes.is_empty()
				var loaded_site := _construction_site_for(1)
				load_ok = load_ok and absf(float(loaded_site.get("progress", -1.0)) - saved_site_progress) < 0.001
				var loaded_entry: Dictionary = construction_nodes.get(1, {})
				var loaded_mesh: MeshInstance3D = loaded_entry.get("mesh")
				var loaded_label: Label3D = loaded_entry.get("label")
				load_ok = load_ok and is_instance_valid(loaded_mesh)
				load_ok = load_ok and absf(loaded_mesh.scale.y - saved_site_progress) < 0.01
				load_ok = load_ok and is_instance_valid(loaded_label) and loaded_label.text.contains("%")
			if load_ok:
				_advance_construction(1000.0)
				var done_site := _construction_site_for(1)
				build_done_ok = bool(done_site.get("complete", false)) and construction_nodes.is_empty()
				build_done_ok = build_done_ok and placed_nodes.has(1) and placed_root.get_child_count() == 1
				build_done_ok = build_done_ok and _foundation_matches_bounds(placed_root.get_child(0))
				var finished_mesh := placed_root.get_child(0).get_child(0) as MeshInstance3D
				build_done_ok = build_done_ok and finished_mesh != null and finished_mesh.material_override == placed_material
				build_done_ok = build_done_ok and absf(finished_mesh.scale.y - 1.0) < 0.001
				var pacing_ok: bool = core.population_set_rate(1.0)
				core.population_clear()
				_advance_construction(0.25)
				pacing_ok = pacing_ok and int(core.city_summary().get("population", -1)) == 0
				_advance_construction(2.0)
				pacing_ok = pacing_ok and int(core.city_summary().get("population", -1)) == 2
				var city_after: Dictionary = core.city_summary()
				occupancy_after_ok = pacing_ok
				occupancy_after_ok = occupancy_after_ok and int(city_after.get("population", -1)) == 2
				occupancy_after_ok = occupancy_after_ok and int(city_after.get("housing_capacity", -1)) == 2
				occupancy_after_ok = occupancy_after_ok and int(city_after.get("jobs", -1)) == 2
				occupancy_after_ok = occupancy_after_ok and int(city_after.get("employed", -1)) == 2
				occupancy_after_ok = occupancy_after_ok and int(city_after.get("completed_buildings", -1)) == 1
				occupancy_after_ok = occupancy_after_ok and int(city_after.get("construction_sites", -1)) == 0
				occupancy_after_ok = occupancy_after_ok and int(city_after.get("vacancies", -1)) == 0
				occupancy_after_ok = occupancy_after_ok and float(city_after.get("happiness", 0.0)) > 0.5
				occupancy_after_ok = occupancy_after_ok and absf(float(city_after.get("average_commute_m", -1.0))) < 0.001
				occupancy_after_ok = occupancy_after_ok and absf(float(city_after.get("daily_wages", 0.0)) - 40.0) < 0.01
				occupancy_after_ok = occupancy_after_ok and float(city_after.get("food", 0.0)) > 0.0
				occupancy_after_ok = occupancy_after_ok and float(city_after.get("water", 0.0)) > 0.0
				occupancy_after_ok = occupancy_after_ok and float(city_after.get("rest", 0.0)) > 0.0
				core.population_tick(SEED, 1000.0)
				var utility_before: Dictionary = core.city_summary()
				var before_water := float(utility_before.get("water", 0.0))
				var before_rest := float(utility_before.get("rest", 0.0))
				var water_added: Dictionary = core.utility_add_site(SEED, 1, "water")
				var power_added: Dictionary = core.utility_add_site(SEED, 1, "power")
				_refresh_utility_visuals()
				var utility_report: Dictionary = core.utilities_refresh(SEED)
				utility_ok = bool(water_added.get("ok", false)) and bool(power_added.get("ok", false))
				utility_ok = utility_ok and bool(utility_report.get("open", false))
				utility_ok = utility_ok and int(utility_report.get("water_sites", 0)) == 1
				utility_ok = utility_ok and int(utility_report.get("power_sites", 0)) == 1
				utility_ok = utility_ok and float(utility_report.get("water_supply", 0.0)) > 0.99
				utility_ok = utility_ok and float(utility_report.get("power_supply", 0.0)) > 0.99
				utility_ok = utility_ok and utility_nodes.size() == 2
				utility_ok = utility_ok and before_water < 0.8
				core.population_tick(SEED, 300.0)
				var utility_after: Dictionary = core.city_summary()
				var after_water := float(utility_after.get("water", 0.0))
				var after_rest := float(utility_after.get("rest", 0.0))
				utility_ok = utility_ok and after_water > before_water
				utility_ok = utility_ok and after_rest >= before_rest
				industry_ok = true
				for kind in ["lumberjack", "farm", "hunt"]:
					var added: Dictionary = core.industry_add_site(SEED, 1, kind)
					industry_ok = industry_ok and bool(added.get("ok", false))
					_refresh_industry_visuals()
					industry_ok = industry_ok and industry_nodes.size() == 1
					industry_ok = industry_ok and str(industry_kinds.get(1, "")) == kind
					var kind_tick: Dictionary = core.industry_tick(SEED, 300.0)
					industry_ok = industry_ok and int(kind_tick.get("workers", 0)) == 2
					match kind:
						"lumberjack":
							industry_ok = industry_ok and float(kind_tick.get("stock_wood", 0.0)) > 0.0
						"farm":
							industry_ok = industry_ok and float(kind_tick.get("stock_food", 0.0)) > 0.0
						"hunt":
							industry_ok = industry_ok and float(kind_tick.get("stock_food", 0.0)) > 0.0
							industry_ok = industry_ok and float(kind_tick.get("stock_hides", 0.0)) > 0.0
				var industry_added: Dictionary = core.industry_add_site(SEED, 1, "farm")
				industry_ok = industry_ok and bool(industry_added.get("ok", false))
				_refresh_industry_visuals()
				var beacon = industry_nodes.get(1, null)
				industry_ok = industry_ok and beacon != null
				industry_ok = industry_ok and beacon.has_meta("spinner")
				var industry_tick: Dictionary = core.industry_tick(SEED, 300.0)
				industry_ok = industry_ok and bool(industry_tick.get("open", false))
				industry_ok = industry_ok and int(industry_tick.get("workers", 0)) == 2
				industry_ok = industry_ok and float(industry_tick.get("food", 0.0)) > 0.0
				industry_ok = industry_ok and float(industry_tick.get("stock_food", 0.0)) > 0.0
				industry_ok = industry_ok and float(industry_tick.get("food_supply", 0.0)) > 0.99
				core.population_tick(SEED, 300.0)
				industry_ok = industry_ok and float(core.city_summary().get("food", 0.0)) > 0.9
				var industry_after: Dictionary = core.industry_state()
				industry_ok = industry_ok and int(industry_after.get("site_count", 0)) == 1
				industry_ok = industry_ok and float(industry_after.get("food", 0.0)) > 0.0
				occupancy_after_ok = occupancy_after_ok and industry_ok
				var economy_after: Dictionary = core.economy_state()
				occupancy_after_ok = occupancy_after_ok and bool(economy_after.get("open", false))
				occupancy_after_ok = occupancy_after_ok and float(economy_after.get("balance", 100000.0)) < 100000.0
				occupancy_after_ok = occupancy_after_ok and float(economy_after.get("daily_net", 0.0)) < 0.0
				occupancy_after_ok = occupancy_after_ok and not bool(economy_after.get("bankrupt", true))
				var economy_balance := float(economy_after.get("balance", 0.0))
				var economy_net := float(economy_after.get("daily_net", 0.0))
				if occupancy_after_ok:
					var population_checksum: String = str(core.population_checksum())
					var economy_checksum: String = str(core.economy_checksum())
					var industry_checksum: String = str(core.industry_checksum())
					var industry_stock := float(core.industry_state().get("food", 0.0))
					var utility_checksum: String = str(core.utilities_checksum())
					var utility_state_before: Dictionary = core.utilities_state()
					var utility_water := float(utility_state_before.get("water_supply", 0.0))
					var utility_power := float(utility_state_before.get("power_supply", 0.0))
					var populated := _save_world()
					core.population_clear()
					core.economy_clear()
					core.industry_clear()
					core.utilities_clear()
					var reloaded := _load_world()
					var city_persisted: Dictionary = core.city_summary()
					var households: Dictionary = core.population_state()
					occupancy_after_ok = populated and reloaded
					occupancy_after_ok = occupancy_after_ok and int(city_persisted.get("population", -1)) == 2
					occupancy_after_ok = occupancy_after_ok and int(city_persisted.get("vacancies", -1)) == 0
					occupancy_after_ok = occupancy_after_ok and absf(float(city_persisted.get("daily_wages", 0.0)) - 40.0) < 0.01
					occupancy_after_ok = occupancy_after_ok and absf(float(city_persisted.get("average_commute_m", -1.0))) < 0.001
					var household_list: Array = households.get("households", [])
					occupancy_after_ok = occupancy_after_ok and household_list.size() == 1
					if household_list.size() == 1:
						var saved_household: Dictionary = household_list[0]
						occupancy_after_ok = occupancy_after_ok and absf(float(saved_household.get("commute_m", -1.0))) < 0.001
						occupancy_after_ok = occupancy_after_ok and absf(float(saved_household.get("income_per_day", 0.0)) - 40.0) < 0.01
					occupancy_after_ok = occupancy_after_ok and str(core.population_checksum()) == population_checksum
					occupancy_after_ok = occupancy_after_ok and str(core.economy_checksum()) == economy_checksum
					var economy_persisted: Dictionary = core.economy_state()
					occupancy_after_ok = occupancy_after_ok and bool(economy_persisted.get("open", false))
					occupancy_after_ok = occupancy_after_ok and absf(float(economy_persisted.get("balance", 0.0)) - economy_balance) < 0.01
					occupancy_after_ok = occupancy_after_ok and absf(float(economy_persisted.get("daily_net", 0.0)) - economy_net) < 0.01
					occupancy_after_ok = occupancy_after_ok and not bool(economy_persisted.get("bankrupt", true))
					var industry_persisted: Dictionary = core.industry_state()
					occupancy_after_ok = occupancy_after_ok and bool(industry_persisted.get("open", false))
					occupancy_after_ok = occupancy_after_ok and int(industry_persisted.get("site_count", -1)) == 1
					occupancy_after_ok = occupancy_after_ok and absf(float(industry_persisted.get("food", 0.0)) - industry_stock) < 0.01
					occupancy_after_ok = occupancy_after_ok and str(core.industry_checksum()) == industry_checksum
					occupancy_after_ok = occupancy_after_ok and industry_nodes.size() == 1
					var utility_persisted: Dictionary = core.utilities_state()
					var utility_persisted_ok := bool(utility_persisted.get("open", false))
					utility_persisted_ok = utility_persisted_ok and int(utility_persisted.get("site_count", 0)) == 2
					utility_persisted_ok = utility_persisted_ok and int(utility_persisted.get("water_sites", 0)) == 1
					utility_persisted_ok = utility_persisted_ok and int(utility_persisted.get("power_sites", 0)) == 1
					utility_persisted_ok = utility_persisted_ok and absf(float(utility_persisted.get("water_supply", 0.0)) - utility_water) < 0.01
					utility_persisted_ok = utility_persisted_ok and absf(float(utility_persisted.get("power_supply", 0.0)) - utility_power) < 0.01
					utility_persisted_ok = utility_persisted_ok and str(core.utilities_checksum()) == utility_checksum
					utility_persisted_ok = utility_persisted_ok and str(utility_persisted.get("checksum", "")) == str(core.utilities_checksum())
					utility_persisted_ok = utility_persisted_ok and utility_nodes.size() == 2
					utility_ok = utility_ok and utility_persisted_ok
				if not build_done_ok:
					status = "construction_complete_failed"
				elif not (occupancy_before_ok and occupancy_after_ok):
					status = "occupancy_failed"
				elif not industry_ok:
					status = "industry_failed"
				elif not utility_ok:
					status = "utility_failed"
			if not load_ok:
				status = "load_failed"

	if status == "ok":
		# A failed load must leave the active store, deltas, and nodes alone.
		var before_failed_load := placed_root.get_child_count()
		var before_failed_edits: int = core.terrain_delta_count()
		var before_failed_checksum: String = str(core.terrain_delta_checksum())
		var before_failed_lots: int = core.lot_count()
		var before_failed_lot_checksum: String = str(core.lot_checksum())
		var before_failed_settlements: int = core.settlement_count()
		var before_failed_settlement_checksum: String = str(core.settlement_checksum())
		var before_failed_economy: String = str(core.economy_checksum())
		var before_failed_industry: String = str(core.industry_checksum())
		var before_failed_utility: String = str(core.utilities_checksum())
		var blueprint_backup := blueprint_path + ".bak"
		if DirAccess.rename_absolute(blueprint_path, blueprint_backup) != OK:
			status = "load_guard_backup_failed"
		else:
			var failed_load := _load_world()
			if failed_load or core.world_placement_count() != 1 or placed_root.get_child_count() != before_failed_load:
				status = "load_failure_changed_scene"
			elif core.terrain_delta_count() != before_failed_edits or str(core.terrain_delta_checksum()) != before_failed_checksum:
				status = "load_failure_changed_deltas"
			elif str(core.economy_checksum()) != before_failed_economy:
				status = "load_failure_changed_economy"
			elif str(core.industry_checksum()) != before_failed_industry:
				status = "load_failure_changed_industry"
			elif str(core.utilities_checksum()) != before_failed_utility:
				status = "load_failure_changed_utility"
			elif core.lot_count() != before_failed_lots or str(core.lot_checksum()) != before_failed_lot_checksum:
				status = "load_failure_changed_lots"
			elif core.settlement_count() != before_failed_settlements or str(core.settlement_checksum()) != before_failed_settlement_checksum:
				status = "load_failure_changed_settlements"
			if DirAccess.rename_absolute(blueprint_backup, blueprint_path) != OK:
				status = "load_guard_restore_failed"

	if status == "ok" and rooms != 4:
		status = "rooms_mismatch"

	print("WORLD_SCENE rooms=%d preview=%s water=%s rotate=%s edits=%s place=%s roads=%s lots=%s settlements=%s build=%s done=%s occupancy=%s industry=%s utilities=%s select=%s enter=%s return=%s save=%s load=%s guard=%s status=%s" % [
		rooms,
		"ok" if preview_ok else "failed",
		"ok" if water_ok else "failed",
		"ok" if rotation_ok else "failed",
		"ok" if edit_ok else "failed",
		"ok" if place_ok else "failed",
		"ok" if road_ok else "failed",
		"ok" if lot_ok else "failed",
		"ok" if settlement_ok else "failed",
		"ok" if build_ok else "failed",
		"ok" if build_done_ok else "failed",
		"ok" if occupancy_before_ok and occupancy_after_ok else "failed",
		"ok" if industry_ok else "failed",
		"ok" if utility_ok else "failed",
		"ok" if select_ok else "failed",
		"ok" if enter_ok else "failed",
		"ok" if return_ok else "failed",
		"ok" if save_ok else "failed",
		"ok" if load_ok else "failed",
		"ok" if status == "ok" else "failed",
		status,
	])
	get_tree().quit(0 if status == "ok" else 1)
