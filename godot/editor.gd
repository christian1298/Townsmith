extends Node3D

## First-person voxel editor shell for the Townsmith vertical slice.
##
## This is the first playable editor: a default 5 x 3 x 5 chunk volume on a
## neutral pad, a fly camera, crosshair raycast edits, a five-material
## palette, a debounced whole-volume remesh, and `.tsbp` save/load.
##
## Run it with:
##   godot --path godot res://editor.tscn
##
## Headless smoke:
##   godot --headless --path godot res://editor.tscn -- --editor-smoke
##   godot --headless --path godot res://editor.tscn -- --blueprint-smoke
##
## See `docs/30 - Editor Shell.md` for the controls and rules.

const EditorCamera = preload("res://editor_camera.gd")

const VOXEL_SIZE_M := 0.1
const VOLUME_CHUNKS := Vector3i(5, 3, 5)
const VOLUME_VOXELS := Vector3i(160, 96, 160)
const VOLUME_BOUNDS_M := Vector3(16.0, 9.6, 16.0)
## World position of the volume's minimum corner. Centered so the default
## volume spans -8..8 m in X/Z and 0..9.6 m in Y; the pad top is the Y=0 plane.
const VOLUME_ORIGIN := Vector3(-8.0, 0.0, -8.0)

const RAY_STEP_M := VOXEL_SIZE_M * 0.5
const MAX_REACH_M := 20.0
const REMESH_DEBOUNCE_MS := 250

const PAD_SIZE_M := Vector2(16.0, 16.0)
const PAD_HEIGHT_M := 2.0
const PAD_COLOR := Color(0.34, 0.36, 0.33)
const SURROUND_COLOR := Color(0.20, 0.22, 0.20)
const VOXEL_COLOR := Color(0.78, 0.73, 0.62)

const MATERIALS := [
	{"id": 1, "name": "timber"},
	{"id": 2, "name": "stone"},
	{"id": 3, "name": "roof"},
	{"id": 4, "name": "glass"},
	{"id": 5, "name": "door"},
]
const SMOKE_FLOOR_VOXELS := 16
const SMOKE_SET_CELL := Vector3i(4, 1, 4)
const SMOKE_FLOOR_SOLID := 256
const SMOKE_RAY_LOCAL := Vector3(0.83, 2.03, 0.83)
const SMOKE_RAY_CELL := Vector3i(8, 0, 8)
const SMOKE_RAY_PLACE := Vector3i(8, 1, 8)
const DEFAULT_BLUEPRINT_NAME := "River House"
const BLUEPRINT_DIR := "user://blueprints"
const BLUEPRINT_EXT := "tsbp"
const ROOM_OVERLAY_HEIGHT_M := 2.8

var _core
var _volume_origin := VOLUME_ORIGIN
var _volume_voxels := VOLUME_VOXELS
var _volume_bounds := VOLUME_BOUNDS_M
var _voxel_size_m := VOXEL_SIZE_M
var _volume_mesh: MeshInstance3D
var _camera
var _palette_label: Label
var _stats_label: Label
var _status_label: Label
var _active_material := 1
var _remesh_pending := false
var _remesh_due_msec := 0
var _blueprint_name: LineEdit
var _save_dialog: FileDialog
var _load_dialog: FileDialog
var _room_overlay: Node3D
var _room_overlay_on := false

func _ready() -> void:
	_core = TownsmithCore.new()
	print("Townsmith core version: ", _core.version())

	if not _core.create_editor_volume(VOLUME_CHUNKS.x, VOLUME_CHUNKS.y, VOLUME_CHUNKS.z):
		push_error("Editor: failed to create the default editor volume")
		if _is_editor_smoke():
			print("EDITOR_SMOKE solid=0 quads=0 surfaces=0 status=create_failed")
			get_tree().quit(1)
		elif _is_blueprint_smoke():
			print("BLUEPRINT_SMOKE solid=0 hash= status=create_failed")
			get_tree().quit(1)
		return

	_volume_origin = VOLUME_ORIGIN
	_volume_voxels = VOLUME_VOXELS
	_volume_bounds = VOLUME_BOUNDS_M
	var size: Dictionary = _core.editor_volume_size_voxels()
	var voxels: PackedInt32Array = size.get("voxels", PackedInt32Array())
	if voxels.size() == 3:
		_volume_voxels = Vector3i(voxels[0], voxels[1], voxels[2])
	var bounds: PackedFloat32Array = _core.editor_volume_bounds()
	if bounds.size() == 3:
		_volume_bounds = Vector3(bounds[0], bounds[1], bounds[2])
	var voxel_size := float(size.get("voxel_size_m", VOXEL_SIZE_M))
	if voxel_size > 0.0:
		_voxel_size_m = voxel_size

	if _is_room_overlay_smoke():
		_run_room_overlay_smoke()
		return
	if _is_editor_smoke():
		_run_editor_smoke()
		return
	if _is_blueprint_smoke():
		_run_blueprint_smoke()
		return

	_build_shell()
	_build_ui()
	_update_palette_label()
	_refresh_stats_label()
	if DisplayServer.get_name() != "headless":
		Input.mouse_mode = Input.MOUSE_MODE_CAPTURED
		_set_status("Esc releases the mouse")

func _process(_delta: float) -> void:
	if not _remesh_pending:
		return
	if Time.get_ticks_msec() < _remesh_due_msec:
		return
	_rebuild_volume_mesh()

func _unhandled_input(event: InputEvent) -> void:
	if event is InputEventKey and event.pressed and not event.echo:
		var key := _event_key(event)
		if key == KEY_ESCAPE:
			if Input.mouse_mode == Input.MOUSE_MODE_CAPTURED:
				Input.mouse_mode = Input.MOUSE_MODE_VISIBLE
				_set_status("Click to capture the mouse")
			return
		if key == KEY_R:
			_toggle_room_overlay()
			return
		for index in MATERIALS.size():
			if key == KEY_1 + index:
				_select_material(int(MATERIALS[index]["id"]))
				return
		if key == KEY_F5:
			_open_save_dialog()
			return
		if key == KEY_F9:
			_open_load_dialog()
			return

	if event is InputEventMouseButton and event.pressed:
		if Input.mouse_mode != Input.MOUSE_MODE_CAPTURED:
			if event.button_index == MOUSE_BUTTON_LEFT:
				Input.mouse_mode = Input.MOUSE_MODE_CAPTURED
				_set_status("Esc releases the mouse")
			return
		if event.button_index == MOUSE_BUTTON_LEFT:
			_remove_under_crosshair()
		elif event.button_index == MOUSE_BUTTON_RIGHT:
			_place_under_crosshair()

## Cast from the crosshair to the first solid voxel.
##
## The ray is stepped at half a voxel (0.05 m) up to 20 m, and each sample is
## converted to a volume-local cell before `editor_get_voxel` is queried. The
## last empty in-volume cell before the first solid cell is the placement
## candidate, so placement lands against the face the ray entered. The pad is
## not voxel data, so its top face is a fallback hit that resolves to the Y=0
## voxel layer; that is how the first voxel gets placed on an empty volume.
func _raycast(origin: Vector3, direction: Vector3) -> Dictionary:
	var dir := direction.normalized()
	var distance := 0.0
	var previous := Vector3i(-1, -1, -1)
	var has_previous := false
	while distance <= MAX_REACH_M:
		var cell := _world_to_cell(origin + dir * distance)
		if _cell_in_bounds(cell):
			var material := int(_core.editor_get_voxel(cell.x, cell.y, cell.z))
			if material > 0:
				return {
					"hit": true,
					"pad": false,
					"cell": cell,
					"material": material,
					"place": previous,
					"has_place": has_previous,
				}
			previous = cell
			has_previous = true
		distance += RAY_STEP_M

	if dir.y >= -0.0001:
		return {"hit": false}
	var pad_distance := (_volume_origin.y - origin.y) / dir.y
	if pad_distance < 0.0 or pad_distance > MAX_REACH_M:
		return {"hit": false}
	var pad_cell := _world_to_cell(origin + dir * pad_distance)
	pad_cell.y = 0
	if not _cell_in_bounds(pad_cell):
		return {"hit": false}
	return {
		"hit": true,
		"pad": true,
		"cell": pad_cell,
		"material": 0,
		"place": pad_cell,
		"has_place": true,
	}

func _remove_under_crosshair() -> void:
	if _camera == null:
		return
	var hit := _raycast(_camera.global_position, -_camera.global_transform.basis.z)
	if not bool(hit.get("hit", false)):
		_set_status("No voxel in reach")
		return
	if bool(hit.get("pad", false)):
		_set_status("The pad is not editable; place a voxel first")
		return
	var cell: Vector3i = hit["cell"]
	if not _core.editor_set_voxel(cell.x, cell.y, cell.z, 0):
		_set_status("Remove rejected at %s" % cell)
		return
	_on_edit_applied("Removed %s at %s" % [_material_name(int(hit.get("material", 0))), cell])

func _place_under_crosshair() -> void:
	if _camera == null:
		return
	var hit := _raycast(_camera.global_position, -_camera.global_transform.basis.z)
	if not bool(hit.get("hit", false)):
		_set_status("No surface in reach")
		return
	if not bool(hit.get("has_place", false)):
		_set_status("No empty cell against the hit face")
		return
	var cell: Vector3i = hit["place"]
	if not _cell_in_bounds(cell):
		_set_status("Placement outside the volume")
		return
	if int(_core.editor_get_voxel(cell.x, cell.y, cell.z)) != 0:
		_set_status("Placement cell is occupied")
		return
	if not _core.editor_set_voxel(cell.x, cell.y, cell.z, _active_material):
		_set_status("Place rejected at %s" % cell)
		return
	_on_edit_applied("Placed %s at %s" % [_material_name(_active_material), cell])

## Mark the mesh stale and push the rebuild deadline out.
##
## Repeated edits inside the 250 ms window keep sliding the deadline, so a
## short burst of clicks results in one remesh instead of one per click.
func _on_edit_applied(message: String) -> void:
	_remesh_pending = true
	_remesh_due_msec = Time.get_ticks_msec() + REMESH_DEBOUNCE_MS
	_refresh_stats_label()
	_set_status(message)

func _rebuild_volume_mesh() -> void:
	_remesh_pending = false
	if _volume_mesh == null:
		return
	var started_usec := Time.get_ticks_usec()
	var mesh: ArrayMesh = _core.build_editor_volume_mesh()
	_volume_mesh.mesh = mesh
	var wall_ms := float(Time.get_ticks_usec() - started_usec) / 1000.0
	_refresh_stats_label(wall_ms)
	if _room_overlay_on:
		_rebuild_room_overlay()

## Toggle the translucent room boxes/labels. Detection runs on demand.
func _toggle_room_overlay() -> void:
	if _room_overlay != null:
		_room_overlay.queue_free()
		_room_overlay = null
		_room_overlay_on = false
		_set_status("Room overlay off")
		return
	_room_overlay_on = true
	_rebuild_room_overlay()

func _rebuild_room_overlay() -> void:
	if _room_overlay != null:
		_room_overlay.queue_free()
	_room_overlay = Node3D.new()
	_room_overlay.name = "RoomOverlay"
	add_child(_room_overlay)

	var stats: Dictionary = _core.editor_room_stats()
	if not bool(stats.get("ok", false)):
		_set_status("Room overlay failed: %s" % str(stats.get("error", "unknown error")))
		return
	var rooms: Array = stats.get("rooms", [])
	for room in rooms:
		var bounds: PackedFloat32Array = room.get("bounds_m", PackedFloat32Array())
		var center: PackedFloat32Array = room.get("center_m", PackedFloat32Array())
		if bounds.size() < 4 or center.size() < 3:
			continue
		var kind := str(room.get("type", "room"))
		var color := _room_color(kind)
		var width := bounds[2] - bounds[0]
		var depth := bounds[3] - bounds[1]
		var box := BoxMesh.new()
		box.size = Vector3(width, ROOM_OVERLAY_HEIGHT_M, depth)
		var instance := MeshInstance3D.new()
		instance.mesh = box
		instance.material_override = _room_material(color)
		instance.position = _volume_origin + Vector3(center[0], center[1], center[2])
		instance.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
		_room_overlay.add_child(instance)

		var label := Label3D.new()
		label.text = "%s\n%.1f m²" % [kind, float(room.get("area_m2", 0.0))]
		label.position = instance.position + Vector3(0.0, ROOM_OVERLAY_HEIGHT_M * 0.5 + 0.4, 0.0)
		label.billboard = BaseMaterial3D.BILLBOARD_ENABLED
		label.no_depth_test = true
		label.font_size = 48
		label.pixel_size = 0.005
		label.outline_size = 8
		label.modulate = color
		_room_overlay.add_child(label)

	_set_status("Room overlay: %d room(s), capacity %d, jobs %d" % [
		rooms.size(),
		int(stats.get("resident_capacity", 0)),
		int(stats.get("job_slots", 0)),
	])

func _room_material(color: Color) -> StandardMaterial3D:
	var material := StandardMaterial3D.new()
	material.albedo_color = Color(color.r, color.g, color.b, 0.22)
	material.transparency = BaseMaterial3D.TRANSPARENCY_ALPHA
	material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	material.cull_mode = BaseMaterial3D.CULL_DISABLED
	return material

func _room_color(kind: String) -> Color:
	match kind:
		"bedroom":
			return Color(0.35, 0.65, 1.0)
		"kitchen":
			return Color(1.0, 0.72, 0.25)
		"bathroom":
			return Color(0.25, 0.9, 0.9)
		"office":
			return Color(0.75, 0.55, 1.0)
		"living_dining":
			return Color(0.45, 0.9, 0.45)
		_:
			return Color(0.8, 0.8, 0.8)

func _build_shell() -> void:
	var world_environment := WorldEnvironment.new()
	var environment := Environment.new()
	environment.background_mode = Environment.BG_COLOR
	environment.background_color = Color(0.55, 0.72, 0.9)
	environment.ambient_light_source = Environment.AMBIENT_SOURCE_COLOR
	environment.ambient_light_color = Color(0.78, 0.82, 0.9)
	environment.ambient_light_energy = 0.6
	world_environment.environment = environment
	add_child(world_environment)

	var light := DirectionalLight3D.new()
	light.rotation_degrees = Vector3(-55.0, -35.0, 0.0)
	light.light_energy = 1.2
	add_child(light)

	# A cheap neutral surround and the 2 m pad the volume sits on. Neither is
	# voxel data, so neither costs mesh time or chunk memory.
	var surround := MeshInstance3D.new()
	surround.name = "Surround"
	var plane := PlaneMesh.new()
	plane.size = Vector2(256.0, 256.0)
	surround.mesh = plane
	surround.material_override = _flat_material(SURROUND_COLOR)
	surround.position = Vector3(0.0, _volume_origin.y - PAD_HEIGHT_M - 0.02, 0.0)
	add_child(surround)

	var pad := MeshInstance3D.new()
	pad.name = "BuildPad"
	var box := BoxMesh.new()
	box.size = Vector3(PAD_SIZE_M.x, PAD_HEIGHT_M, PAD_SIZE_M.y)
	pad.mesh = box
	pad.material_override = _flat_material(PAD_COLOR)
	pad.position = Vector3(0.0, _volume_origin.y - PAD_HEIGHT_M * 0.5, 0.0)
	add_child(pad)

	_volume_mesh = MeshInstance3D.new()
	_volume_mesh.name = "EditorVolume"
	_volume_mesh.position = _volume_origin
	_volume_mesh.material_override = _flat_material(VOXEL_COLOR)
	add_child(_volume_mesh)

	_camera = EditorCamera.new()
	_camera.name = "FlyCamera"
	_camera.far = 256.0
	add_child(_camera)
	_camera.position = _volume_origin + Vector3(8.0, 2.4, 13.4)
	_camera.look_at(_volume_origin + Vector3(8.0, 0.6, 6.0))
	_camera.sync_look_from_rotation()
	_camera.current = true

func _build_ui() -> void:
	var layer := CanvasLayer.new()
	layer.name = "EditorUI"
	add_child(layer)

	var ui := Control.new()
	ui.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	ui.mouse_filter = Control.MOUSE_FILTER_IGNORE
	layer.add_child(ui)

	var crosshair := Control.new()
	crosshair.set_anchors_and_offsets_preset(Control.PRESET_CENTER)
	crosshair.mouse_filter = Control.MOUSE_FILTER_IGNORE
	ui.add_child(crosshair)
	_add_bar(crosshair, Vector2(-10.0, -2.0), Vector2(20.0, 4.0), Color(0.0, 0.0, 0.0, 0.5))
	_add_bar(crosshair, Vector2(-2.0, -10.0), Vector2(4.0, 20.0), Color(0.0, 0.0, 0.0, 0.5))
	_add_bar(crosshair, Vector2(-9.0, -1.0), Vector2(18.0, 2.0), Color(1.0, 1.0, 1.0, 0.9))
	_add_bar(crosshair, Vector2(-1.0, -9.0), Vector2(2.0, 18.0), Color(1.0, 1.0, 1.0, 0.9))

	_palette_label = _add_label(ui, Control.PRESET_TOP_LEFT)
	_palette_label.offset_left = 16.0
	_palette_label.offset_top = 16.0
	_palette_label.offset_right = 280.0
	_palette_label.offset_bottom = 160.0

	_stats_label = _add_label(ui, Control.PRESET_TOP_RIGHT)
	_stats_label.offset_left = -380.0
	_stats_label.offset_right = -16.0
	_stats_label.offset_top = 16.0
	_stats_label.offset_bottom = 120.0
	_stats_label.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT

	_status_label = _add_label(ui, Control.PRESET_CENTER_BOTTOM)
	_status_label.offset_left = -260.0
	_status_label.offset_right = 260.0
	_status_label.offset_top = -40.0
	_status_label.offset_bottom = -16.0
	_status_label.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER

	var help := _add_label(ui, Control.PRESET_BOTTOM_LEFT)
	help.offset_left = 16.0
	help.offset_right = 620.0
	help.offset_top = -96.0
	help.offset_bottom = -16.0
	help.text = "\n".join(PackedStringArray([
		"WASD move   Space/Ctrl up/down   Shift boost   Esc release mouse",
		"Left click remove   Right click place   Keys 1-5 select material",
		"R room overlay   F5 save blueprint   F9 load blueprint   Mesh rebuilds 250 ms after edit",
	]))

	var blueprint_bar := HBoxContainer.new()
	blueprint_bar.set_anchors_and_offsets_preset(Control.PRESET_CENTER_TOP)
	blueprint_bar.offset_left = -300.0
	blueprint_bar.offset_right = 300.0
	blueprint_bar.offset_top = 16.0
	blueprint_bar.offset_bottom = 52.0
	blueprint_bar.add_theme_constant_override("separation", 8)
	ui.add_child(blueprint_bar)

	_blueprint_name = LineEdit.new()
	_blueprint_name.text = DEFAULT_BLUEPRINT_NAME
	_blueprint_name.placeholder_text = "Blueprint name"
	_blueprint_name.custom_minimum_size = Vector2(220.0, 0.0)
	_blueprint_name.tooltip_text = "Name used for the saved .tsbp package"
	blueprint_bar.add_child(_blueprint_name)

	var save_button := Button.new()
	save_button.text = "Save"
	save_button.tooltip_text = "Save the current volume (F5)"
	save_button.pressed.connect(_open_save_dialog)
	blueprint_bar.add_child(save_button)

	var load_button := Button.new()
	load_button.text = "Load"
	load_button.tooltip_text = "Load a .tsbp package (F9)"
	load_button.pressed.connect(_open_load_dialog)
	blueprint_bar.add_child(load_button)

	var directory_error := DirAccess.make_dir_recursive_absolute(
		ProjectSettings.globalize_path(BLUEPRINT_DIR))
	if directory_error != OK:
		push_warning("Could not create blueprint directory %s: %s" % [
			BLUEPRINT_DIR, error_string(directory_error)])

	_save_dialog = FileDialog.new()
	_save_dialog.title = "Save Blueprint"
	_save_dialog.file_mode = FileDialog.FILE_MODE_SAVE_FILE
	_save_dialog.access = FileDialog.ACCESS_USERDATA
	_save_dialog.filters = PackedStringArray(["*.%s ; Townsmith Blueprint" % BLUEPRINT_EXT])
	_save_dialog.current_dir = BLUEPRINT_DIR
	_save_dialog.use_native_dialog = false
	_save_dialog.file_selected.connect(_on_save_file_selected)
	_save_dialog.canceled.connect(_restore_mouse_after_dialog)
	add_child(_save_dialog)

	_load_dialog = FileDialog.new()
	_load_dialog.title = "Load Blueprint"
	_load_dialog.file_mode = FileDialog.FILE_MODE_OPEN_FILE
	_load_dialog.access = FileDialog.ACCESS_USERDATA
	_load_dialog.filters = PackedStringArray(["*.%s ; Townsmith Blueprint" % BLUEPRINT_EXT])
	_load_dialog.current_dir = BLUEPRINT_DIR
	_load_dialog.use_native_dialog = false
	_load_dialog.file_selected.connect(_on_load_file_selected)
	_load_dialog.canceled.connect(_restore_mouse_after_dialog)
	add_child(_load_dialog)

func _add_bar(parent: Control, position: Vector2, size: Vector2, color: Color) -> void:
	var bar := ColorRect.new()
	bar.position = position
	bar.size = size
	bar.color = color
	bar.mouse_filter = Control.MOUSE_FILTER_IGNORE
	parent.add_child(bar)

func _add_label(parent: Control, preset: Control.LayoutPreset) -> Label:
	var label := Label.new()
	label.set_anchors_and_offsets_preset(preset)
	label.mouse_filter = Control.MOUSE_FILTER_IGNORE
	label.add_theme_font_size_override("font_size", 16)
	label.add_theme_color_override("font_shadow_color", Color(0.0, 0.0, 0.0, 0.85))
	label.add_theme_constant_override("shadow_offset_x", 1)
	label.add_theme_constant_override("shadow_offset_y", 1)
	parent.add_child(label)
	return label

func _flat_material(color: Color) -> StandardMaterial3D:
	var material := StandardMaterial3D.new()
	material.albedo_color = color
	material.roughness = 1.0
	material.metallic = 0.0
	material.transparency = BaseMaterial3D.TRANSPARENCY_DISABLED
	return material

func _select_material(id: int) -> void:
	_active_material = id
	_update_palette_label()
	var material_name := _material_name(id)
	print("Editor material: %d %s" % [id, material_name])
	_set_status("Material %d: %s" % [id, material_name])

func _update_palette_label() -> void:
	if _palette_label == null:
		return
	var lines := PackedStringArray(["Materials  (keys 1-5)"])
	for entry in MATERIALS:
		var id := int(entry["id"])
		var marker := ">" if id == _active_material else " "
		lines.append("%s %d  %s" % [marker, id, str(entry["name"])])
	_palette_label.text = "\n".join(lines)

func _refresh_stats_label(build_ms: float = -1.0) -> void:
	if _stats_label == null:
		return
	var stats: Dictionary = _core.editor_volume_stats()
	var text := _format_stats(stats)
	if build_ms >= 0.0:
		text += "\nbuild=%.1f ms  builds=%d" % [build_ms, int(stats.get("mesh_builds", 0))]
	elif _remesh_pending:
		text += "\nremesh pending..."
	_stats_label.text = text

func _format_stats(stats: Dictionary) -> String:
	return "solid=%d  quads=%d  mesh=%.1f ms\nchunks=%d  volume=%.1f x %.1f x %.1f m" % [
		int(stats.get("solid_voxels", 0)),
		int(stats.get("last_mesh_quads", 0)),
		float(stats.get("mesh_ms", 0.0)),
		int(stats.get("allocated_chunks", 0)),
		_volume_bounds.x,
		_volume_bounds.y,
		_volume_bounds.z,
	]

func _set_status(text: String) -> void:
	if _status_label != null:
		_status_label.text = text

func _material_name(id: int) -> String:
	for entry in MATERIALS:
		if int(entry["id"]) == id:
			return str(entry["name"])
	return "unknown"

func _world_to_cell(world: Vector3) -> Vector3i:
	var local := (world - _volume_origin) / _voxel_size_m
	return Vector3i(floori(local.x), floori(local.y), floori(local.z))

func _cell_in_bounds(cell: Vector3i) -> bool:
	return cell.x >= 0 and cell.y >= 0 and cell.z >= 0 \
		and cell.x < _volume_voxels.x \
		and cell.y < _volume_voxels.y \
		and cell.z < _volume_voxels.z

func _event_key(event: InputEventKey) -> int:
	return event.physical_keycode if event.physical_keycode != KEY_NONE else event.keycode

func _open_save_dialog() -> void:
	if _save_dialog == null:
		return
	if DisplayServer.get_name() != "headless":
		Input.mouse_mode = Input.MOUSE_MODE_VISIBLE
	_save_dialog.current_file = "%s.%s" % [_slug(_blueprint_name.text), BLUEPRINT_EXT]
	_save_dialog.popup_centered_ratio(0.6)

func _open_load_dialog() -> void:
	if _load_dialog == null:
		return
	if DisplayServer.get_name() != "headless":
		Input.mouse_mode = Input.MOUSE_MODE_VISIBLE
	_load_dialog.popup_centered_ratio(0.6)

func _on_save_file_selected(path: String) -> void:
	_save_blueprint(path)
	_restore_mouse_after_dialog()

func _on_load_file_selected(path: String) -> void:
	_load_blueprint(path)
	_restore_mouse_after_dialog()

func _restore_mouse_after_dialog() -> void:
	if _blueprint_name != null:
		_blueprint_name.release_focus()
	if DisplayServer.get_name() != "headless":
		Input.mouse_mode = Input.MOUSE_MODE_CAPTURED

func _save_blueprint(path: String) -> bool:
	var name := _blueprint_name.text.strip_edges()
	if name.is_empty():
		_set_status("Blueprint name is empty")
		return false
	var id := "local:%s" % _slug(name)
	var result: Dictionary = _core.editor_save_blueprint(
		ProjectSettings.globalize_path(path), id, name)
	if not bool(result.get("ok", false)):
		_set_status("Save failed: %s" % str(result.get("error", "unknown error")))
		push_error("Blueprint save failed: %s" % str(result.get("error", "unknown error")))
		return false
	_set_status("Saved %s  hash=%s" % [name, _short_hash(str(result.get("hash", "")))])
	print("BLUEPRINT_SAVED path=%s hash=%s solid=%d" % [
		str(result.get("path", path)),
		str(result.get("hash", "")),
		int(result.get("solid_voxels", 0)),
	])
	_refresh_stats_label()
	return true

func _load_blueprint(path: String) -> bool:
	var result: Dictionary = _core.editor_load_blueprint(ProjectSettings.globalize_path(path))
	if not bool(result.get("ok", false)):
		_set_status("Load failed: %s" % str(result.get("error", "unknown error")))
		push_error("Blueprint load failed: %s" % str(result.get("error", "unknown error")))
		return false
	var bounds: PackedFloat32Array = result.get("bounds", PackedFloat32Array())
	if bounds.size() == 3:
		_volume_bounds = Vector3(bounds[0], bounds[1], bounds[2])
	var size: Dictionary = _core.editor_volume_size_voxels()
	var voxels: PackedInt32Array = size.get("voxels", PackedInt32Array())
	if voxels.size() == 3:
		_volume_voxels = Vector3i(voxels[0], voxels[1], voxels[2])
	_voxel_size_m = float(size.get("voxel_size_m", _voxel_size_m))
	_blueprint_name.text = str(result.get("name", _blueprint_name.text))
	_remesh_pending = false
	_rebuild_volume_mesh()
	_update_palette_label()
	_set_status("Loaded %s  hash=%s" % [
		str(result.get("name", "blueprint")), _short_hash(str(result.get("hash", "")))])
	print("BLUEPRINT_LOADED path=%s hash=%s solid=%d rooms=%d" % [
		str(result.get("path", path)),
		str(result.get("hash", "")),
		int(result.get("solid_voxels", 0)),
		int(result.get("rooms", 0)),
	])
	return true

func _short_hash(hash: String) -> String:
	return hash.substr(0, 19) + "..." if hash.length() > 22 else hash

func _slug(text: String) -> String:
	var normalized := text.strip_edges().to_lower()
	normalized = normalized.replace("ä", "ae").replace("ö", "oe").replace("ü", "ue").replace("ß", "ss")
	var cleaned := ""
	for character in normalized:
		if (character >= "a" and character <= "z") or (character >= "0" and character <= "9"):
			cleaned += character
		elif character == " " or character == "-" or character == "_":
			cleaned += "_"
	while cleaned.contains("__"):
		cleaned = cleaned.replace("__", "_")
	cleaned = cleaned.trim_prefix("_").trim_suffix("_")
	return cleaned if not cleaned.is_empty() else "blueprint"

## Headless blueprint save/load contract.
##
## Builds a small floor, saves a `.tsbp`, reloads it into a fresh editor
## state, verifies the hash/solid count/bounds, and removes the temp package.
func _run_blueprint_smoke() -> void:
	var status := "ok"
	var path := "user://blueprint_smoke.tsbp"
	var global_path := ProjectSettings.globalize_path(path)
	if FileAccess.file_exists(path):
		DirAccess.remove_absolute(global_path)

	if not _core.editor_fill_box(0, 0, 0, SMOKE_FLOOR_VOXELS, 1, SMOKE_FLOOR_VOXELS, 2):
		status = "fill_failed"
	elif not _core.editor_set_voxel(SMOKE_SET_CELL.x, SMOKE_SET_CELL.y, SMOKE_SET_CELL.z, 1):
		status = "set_failed"

	var saved: Dictionary = {}
	var loaded: Dictionary = {}
	if status == "ok":
		saved = _core.editor_save_blueprint(global_path, "local:blueprint_smoke", "Blueprint Smoke")
		if not bool(saved.get("ok", false)):
			status = "save_failed"
		elif int(saved.get("solid_voxels", -1)) != SMOKE_FLOOR_SOLID + 1:
			status = "save_solid"
		elif not str(saved.get("hash", "")).begins_with("blake3:"):
			status = "save_hash"

	if status == "ok":
		if not _core.editor_clear():
			status = "clear_failed"
		elif int(_core.editor_volume_stats().get("solid_voxels", -1)) != 0:
			status = "clear_not_empty"

	if status == "ok":
		loaded = _core.editor_load_blueprint(global_path)
		if not bool(loaded.get("ok", false)):
			status = "load_failed"
		elif int(loaded.get("solid_voxels", -1)) != SMOKE_FLOOR_SOLID + 1:
			status = "load_solid"
		elif str(loaded.get("hash", "")) != str(saved.get("hash", "")):
			status = "load_hash"
		elif int(loaded.get("rooms", -1)) != 0:
			status = "load_rooms"
		var bounds: PackedFloat32Array = loaded.get("bounds", PackedFloat32Array())
		if status == "ok" and bounds.size() != 3:
			status = "load_bounds"
		elif status == "ok":
			for value in bounds:
				if not is_finite(value):
					status = "load_bounds_value"
					break
		if status == "ok" and not str(loaded.get("hash", "")).begins_with("blake3:"):
			status = "load_hash_prefix"

	if status == "ok":
		var mesh: ArrayMesh = _core.build_editor_volume_mesh()
		var stats: Dictionary = _core.editor_volume_stats()
		if mesh.get_surface_count() != 1:
			status = "load_surface"
		elif int(stats.get("solid_voxels", -1)) != SMOKE_FLOOR_SOLID + 1:
			status = "load_stats"

	DirAccess.remove_absolute(global_path)
	print("BLUEPRINT_SMOKE solid=%d hash=%s status=%s" % [
		int(loaded.get("solid_voxels", 0)),
		str(loaded.get("hash", "")),
		status,
	])
	get_tree().quit(0 if status == "ok" else 1)

func _is_editor_smoke() -> bool:
	return OS.get_cmdline_user_args().has("--editor-smoke")

func _is_room_overlay_smoke() -> bool:
	return OS.get_cmdline_user_args().has("--room-overlay-smoke")

func _is_blueprint_smoke() -> bool:
	return OS.get_cmdline_user_args().has("--blueprint-smoke")

## Headless contract check for the room overlay.
##
## Builds a sealed room, toggles the overlay, and asserts the detector and
## overlay node tree. Uses the same volume created in `_ready`.
func _run_room_overlay_smoke() -> void:
	var status := "ok"
	if not _core.editor_fill_box(0, 0, 0, 40, 1, 40, 2):
		status = "floor_failed"
	elif not _core.editor_fill_box(0, 30, 0, 40, 31, 40, 3):
		status = "ceiling_failed"
	elif not _core.editor_fill_box(0, 1, 0, 1, 30, 40, 1):
		status = "west_failed"
	elif not _core.editor_fill_box(39, 1, 0, 40, 30, 40, 1):
		status = "east_failed"
	elif not _core.editor_fill_box(1, 1, 0, 39, 30, 1, 1):
		status = "north_failed"
	elif not _core.editor_fill_box(1, 1, 39, 39, 30, 40, 1):
		status = "south_failed"
	elif not _core.editor_set_voxel(0, 15, 20, 5):
		status = "door_failed"
	elif not _core.editor_set_voxel(0, 15, 10, 4):
		status = "window_failed"
	else:
		_toggle_room_overlay()
		var stats: Dictionary = _core.editor_room_stats()
		var rooms: Array = stats.get("rooms", [])
		if not bool(stats.get("ok", false)):
			status = "stats_failed"
		elif rooms.size() != 1:
			status = "room_count"
		elif _room_overlay == null or _room_overlay.get_child_count() < 1:
			status = "overlay_nodes"
		else:
			print("ROOM_OVERLAY_SMOKE rooms=%d nodes=%d status=ok" % [
				rooms.size(), _room_overlay.get_child_count()])
	if status != "ok":
		print("ROOM_OVERLAY_SMOKE rooms=0 nodes=0 status=%s" % status)
		get_tree().quit(1)
		return
	get_tree().quit(0)

## Headless contract check for CI.
##
## Creates the default volume in `_ready`, fills a 16 x 16 floor, writes and
## reads one voxel, exercises the raycast, builds the mesh, and prints one
## marker line. Any failure exits non-zero.
func _run_editor_smoke() -> void:
	var status := "ok"
	if not _core.editor_fill_box(0, 0, 0, SMOKE_FLOOR_VOXELS, 1, SMOKE_FLOOR_VOXELS, 2):
		status = "fill_failed"
	elif not _core.editor_set_voxel(SMOKE_SET_CELL.x, SMOKE_SET_CELL.y, SMOKE_SET_CELL.z, 1):
		status = "set_failed"
	elif _core.editor_get_voxel(SMOKE_SET_CELL.x, SMOKE_SET_CELL.y, SMOKE_SET_CELL.z) != 1:
		status = "get_mismatch"
	elif _core.editor_get_voxel(0, 1, 0) != 0:
		status = "empty_mismatch"
	elif _core.editor_get_voxel(-1, 0, 0) != -1:
		status = "oob_read_allowed"

	var size: Dictionary = _core.editor_volume_size_voxels()
	var voxels: PackedInt32Array = size.get("voxels", PackedInt32Array())
	if status == "ok" and (not bool(size.get("created", false)) or voxels != PackedInt32Array([160, 96, 160])):
		status = "size_mismatch"

	var mesh: ArrayMesh = _core.build_editor_volume_mesh()
	var stats: Dictionary = _core.editor_volume_stats()
	var solid := int(stats.get("solid_voxels", -1))
	var quads := int(stats.get("last_mesh_quads", -1))
	var vertices := int(stats.get("last_mesh_vertices", -1))
	var surfaces := mesh.get_surface_count()
	if status == "ok" and solid != SMOKE_FLOOR_SOLID + 1:
		status = "solid_mismatch"
	elif status == "ok" and surfaces != 1:
		status = "surface_mismatch"
	elif status == "ok" and (quads < 1 or vertices != quads * 4):
		status = "mesh_mismatch"

	var hit := _raycast(_volume_origin + SMOKE_RAY_LOCAL, Vector3.DOWN)
	var hit_cell: Vector3i = hit.get("cell", Vector3i(-1, -1, -1))
	var place_cell: Vector3i = hit.get("place", Vector3i(-1, -1, -1))
	if status == "ok" and not bool(hit.get("hit", false)):
		status = "raycast_miss"
	elif status == "ok" and hit_cell != SMOKE_RAY_CELL:
		status = "raycast_cell"
	elif status == "ok" and place_cell != SMOKE_RAY_PLACE:
		status = "raycast_place"

	var pad_hit := _raycast(_volume_origin + Vector3(4.0, 2.0, 4.0), Vector3.DOWN)
	if status == "ok" and not bool(pad_hit.get("hit", false)):
		status = "pad_miss"
	elif status == "ok" and not bool(pad_hit.get("pad", false)):
		status = "pad_not_used"
	elif status == "ok" and pad_hit.get("cell") != Vector3i(40, 0, 40):
		status = "pad_cell"
	elif status == "ok" and pad_hit.get("place") != Vector3i(40, 0, 40):
		status = "pad_place"

	print("EDITOR_SMOKE solid=%d quads=%d surfaces=%d status=%s" % [solid, quads, surfaces, status])
	get_tree().quit(0 if status == "ok" else 1)
