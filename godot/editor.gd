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
##   godot --headless --path godot res://editor.tscn -- --locale-smoke
##
## See `docs/30 - Editor Shell.md` for the controls and rules, and
## `docs/38 - Editor Localization.md` for the EN/DE strings and the L toggle.

const EditorCamera = preload("res://editor_camera.gd")
const EditorI18n = preload("res://editor_i18n.gd")

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

const STRUCTURES := [
	"timber_frame",
	"load_bearing_stone",
	"load_bearing_brick",
	"reinforced_concrete",
	"steel_frame",
]
const MATERIALS := [
	{"id": 1, "name": "timber"},
	{"id": 2, "name": "stone"},
	{"id": 3, "name": "roof"},
	{"id": 4, "name": "glass"},
	{"id": 5, "name": "door"},
]
const PROPS := [
	{"kind": 0, "name": "bed"},
	{"kind": 1, "name": "stove"},
	{"kind": 2, "name": "toilet"},
	{"kind": 7, "name": "desk"},
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
const ROOM_SELECTED_COLOR := Color(1.0, 0.9, 0.35)
const FLOOR_BAND_M := 3.0
const ROOM_PANEL_WIDTH := 500.0
const MAX_HISTORY := 500

var _core
var _i18n
var _volume_origin := VOLUME_ORIGIN
var _volume_voxels := VOLUME_VOXELS
var _volume_bounds := VOLUME_BOUNDS_M
var _voxel_size_m := VOXEL_SIZE_M
var _volume_mesh: MeshInstance3D
var _camera
var _palette_label: Label
var _stats_label: Label
var _status_label: Label
var _language_label: Label
var _help_label: Label
var _save_button: Button
var _load_button: Button
var _active_material := 1
var _remesh_pending := false
var _remesh_due_msec := 0
var _blueprint_name: LineEdit
var _save_dialog: FileDialog
var _load_dialog: FileDialog
var _room_overlay: Node3D
var _room_overlay_on := false
var _room_panel: PanelContainer
var _room_panel_label: RichTextLabel
var _room_panel_plain := ""
var _room_stats_cache: Dictionary = {}
var _selected_room := -1
var _prop_root: Node3D
var _prop_nodes: Dictionary = {}
var _active_prop := -1
var _history: Array[Array] = []
var _redo: Array[Array] = []

func _ready() -> void:
	_i18n = EditorI18n.new()
	if _is_locale_smoke():
		_run_locale_smoke()
		return

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
	if _is_undo_smoke():
		_run_undo_smoke()
		return
	if _is_editor_smoke():
		_run_editor_smoke()
		return
	if _is_blueprint_smoke():
		_run_blueprint_smoke()
		return
	if _is_acceptance_smoke():
		_run_acceptance_smoke()
		return
	if _is_layout_smoke():
		_build_ui()
		_apply_locale(false)
		call_deferred("_run_layout_smoke")
		return

	_build_shell()
	_build_ui()
	_refresh_prop_nodes()
	_apply_locale(false)
	if DisplayServer.get_name() != "headless":
		Input.mouse_mode = Input.MOUSE_MODE_CAPTURED
		_status("base.editor.status.capture_mouse")

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
				_status("base.editor.status.click_to_capture")
			return
		if key == KEY_L and not event.ctrl_pressed and not event.alt_pressed:
			_toggle_locale()
			return
		if key == KEY_R:
			_toggle_room_overlay()
			return
		if key == KEY_H:
			_cycle_structure()
			return
		if event.keycode == KEY_BRACKETLEFT and not event.ctrl_pressed and not event.alt_pressed:
			_cycle_room(-1)
			return
		if event.keycode == KEY_BRACKETRIGHT and not event.ctrl_pressed and not event.alt_pressed:
			_cycle_room(1)
			return
		for index in PROPS.size():
			if key == KEY_6 + index:
				_select_prop(int(PROPS[index]["kind"]))
				return
		# Use keycode (layout-aware), not physical_keycode: on QWERTZ the
		# labelled Z/Y keys sit at the opposite physical positions.
		if event.ctrl_pressed and event.keycode == KEY_Z:
			_undo_last()
			return
		if event.ctrl_pressed and event.keycode == KEY_Y:
			_redo_last()
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
				_status("base.editor.status.capture_mouse")
			return
		if event.button_index == MOUSE_BUTTON_LEFT:
			if _active_prop >= 0:
				_remove_prop_under_crosshair()
			else:
				_remove_under_crosshair()
		elif event.button_index == MOUSE_BUTTON_RIGHT:
			if _active_prop >= 0:
				_place_prop_under_crosshair()
			else:
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
		_status("base.editor.status.no_voxel_in_reach")
		return
	if bool(hit.get("pad", false)):
		_status("base.editor.status.pad_not_editable")
		return
	var cell: Vector3i = hit["cell"]
	if not _set_voxel_with_history(cell, 0):
		_status("base.editor.status.remove_rejected", [cell])
		return
	_on_edit_applied(_i18n.t("base.editor.status.removed", [
		_material_name(int(hit.get("material", 0))), cell]))

func _place_under_crosshair() -> void:
	if _camera == null:
		return
	var hit := _raycast(_camera.global_position, -_camera.global_transform.basis.z)
	if not bool(hit.get("hit", false)):
		_status("base.editor.status.no_surface_in_reach")
		return
	if not bool(hit.get("has_place", false)):
		_status("base.editor.status.no_empty_cell")
		return
	var cell: Vector3i = hit["place"]
	if not _cell_in_bounds(cell):
		_status("base.editor.status.outside_volume")
		return
	if int(_core.editor_get_voxel(cell.x, cell.y, cell.z)) != 0:
		_status("base.editor.status.cell_occupied")
		return
	if not _set_voxel_with_history(cell, _active_material):
		_status("base.editor.status.place_rejected", [cell])
		return
	_on_edit_applied(_i18n.t("base.editor.status.placed", [
		_material_name(_active_material), cell]))

func _place_prop_under_crosshair() -> void:
	var hit := _raycast(_camera.global_position, -_camera.global_transform.basis.z)
	if not bool(hit.get("hit", false)) or not bool(hit.get("has_place", false)):
		_status("base.editor.status.no_prop_cell")
		return
	var cell: Vector3i = hit["place"]
	if not _core.editor_place_prop(_active_prop, cell.x, cell.y, cell.z, 0.0):
		_status("base.editor.status.prop_place_rejected", [cell])
		return
	_add_prop_node(_active_prop, cell)
	if _room_overlay_on:
		_rebuild_room_overlay()
	_status("base.editor.status.prop_placed", [_prop_name(_active_prop), cell])

func _remove_prop_under_crosshair() -> void:
	var hit := _raycast(_camera.global_position, -_camera.global_transform.basis.z)
	if not bool(hit.get("hit", false)) or not bool(hit.get("has_place", false)):
		_status("base.editor.status.no_prop_cell_in_reach")
		return
	var cell: Vector3i = hit["place"]
	if not _core.editor_remove_prop_at(cell.x, cell.y, cell.z):
		_status("base.editor.status.no_prop_at", [cell])
		return
	var key := _prop_key(cell)
	var node: Node = _prop_nodes.get(key)
	if node != null:
		node.queue_free()
		_prop_nodes.erase(key)
	if _room_overlay_on:
		_rebuild_room_overlay()
	_status("base.editor.status.prop_removed", [cell])

func _refresh_prop_nodes() -> void:
	if _prop_root == null:
		return
	for child in _prop_root.get_children():
		child.queue_free()
	_prop_nodes.clear()
	var result: Dictionary = _core.editor_props()
	if not bool(result.get("ok", false)):
		return
	var props: Array = result.get("props", [])
	for prop in props:
		var cell := Vector3i(int(prop.get("x", 0)), int(prop.get("y", 0)), int(prop.get("z", 0)))
		_add_prop_node(int(prop.get("kind", 8)), cell)

func _add_prop_node(kind: int, cell: Vector3i) -> void:
	if _prop_root == null:
		return
	var size := _prop_size(kind)
	var box := BoxMesh.new()
	box.size = size
	var instance := MeshInstance3D.new()
	instance.mesh = box
	instance.material_override = _flat_material(_prop_color(kind))
	instance.position = _volume_origin + Vector3(
		(cell.x + 0.5) * VOXEL_SIZE_M,
		cell.y * VOXEL_SIZE_M + size.y * 0.5,
		(cell.z + 0.5) * VOXEL_SIZE_M,
	)
	instance.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	var key := _prop_key(cell)
	var previous: Node = _prop_nodes.get(key)
	if previous != null:
		previous.queue_free()
	_prop_root.add_child(instance)
	_prop_nodes[key] = instance

func _prop_key(cell: Vector3i) -> String:
	return "%d:%d:%d" % [cell.x, cell.y, cell.z]

func _prop_size(kind: int) -> Vector3:
	match kind:
		0:
			return Vector3(1.0, 0.5, 1.9)
		1:
			return Vector3(0.9, 0.9, 0.7)
		2:
			return Vector3(0.5, 0.7, 0.7)
		7:
			return Vector3(1.2, 0.8, 0.7)
		_:
			return Vector3(0.6, 0.6, 0.6)

func _prop_color(kind: int) -> Color:
	match kind:
		0:
			return Color(0.45, 0.65, 1.0)
		1:
			return Color(0.9, 0.55, 0.25)
		2:
			return Color(0.3, 0.85, 0.85)
		7:
			return Color(0.75, 0.55, 1.0)
		_:
			return Color(0.7, 0.7, 0.7)

## Mark the mesh stale and push the rebuild deadline out.
##
## Repeated edits inside the 250 ms window keep sliding the deadline, so a
## short burst of clicks results in one remesh instead of one per click.
## Apply one voxel edit and record it for undo/redo.
func _set_voxel_with_history(cell: Vector3i, material: int) -> bool:
	var old := int(_core.editor_get_voxel(cell.x, cell.y, cell.z))
	if old < 0:
		return false
	if old == material:
		return true
	if not _core.editor_set_voxel(cell.x, cell.y, cell.z, material):
		return false
	_push_history({"x": cell.x, "y": cell.y, "z": cell.z, "old": old, "new": material})
	return true

func _push_history(change: Dictionary) -> void:
	_history.append([change])
	_redo.clear()
	if _history.size() > MAX_HISTORY:
		_history.pop_front()

func _undo_last() -> void:
	if _history.is_empty():
		_status("base.editor.status.nothing_to_undo")
		return
	var batch: Array = _history.pop_back()
	for change in batch:
		if not _core.editor_set_voxel(change["x"], change["y"], change["z"], change["old"]):
			_history.append(batch)
			_status("base.editor.status.undo_failed", [
				Vector3i(change["x"], change["y"], change["z"])])
			return
	_redo.append(batch)
	_on_edit_applied(_i18n.t("base.editor.status.undo"))

func _redo_last() -> void:
	if _redo.is_empty():
		_status("base.editor.status.nothing_to_redo")
		return
	var batch: Array = _redo.pop_back()
	for change in batch:
		if not _core.editor_set_voxel(change["x"], change["y"], change["z"], change["new"]):
			_redo.append(batch)
			_status("base.editor.status.redo_failed", [
				Vector3i(change["x"], change["y"], change["z"])])
			return
	_history.append(batch)
	# Undo/redo conserve the total history size, so the cap still holds.
	_on_edit_applied(_i18n.t("base.editor.status.redo"))

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

## Toggle the translucent room boxes/labels and the stats panel. Detection
## runs on demand. Selection is retained while the room view is hidden.
func _toggle_room_overlay() -> void:
	if _room_overlay != null:
		remove_child(_room_overlay)
		_room_overlay.queue_free()
		_room_overlay = null
		_room_overlay_on = false
		_set_room_panel_visible(false)
		_status("base.editor.status.room_view_off")
		return
	_room_overlay_on = true
	_rebuild_room_overlay()

func _rebuild_room_overlay() -> void:
	if _room_overlay != null:
		remove_child(_room_overlay)
		_room_overlay.queue_free()
	_room_overlay = Node3D.new()
	_room_overlay.name = "RoomOverlay"
	add_child(_room_overlay)

	var stats: Dictionary = _core.editor_room_stats()
	if not bool(stats.get("ok", false)):
		remove_child(_room_overlay)
		_room_overlay.queue_free()
		_room_overlay = null
		_room_overlay_on = false
		_set_room_panel_visible(false)
		_status("base.editor.status.room_overlay_failed", [
			str(stats.get("error", _i18n.t("base.editor.generic.unknown_error")))])
		return
	_room_stats_cache = stats
	var rooms: Array = stats.get("rooms", [])
	if rooms.is_empty():
		_selected_room = -1
	elif _selected_room < 0:
		_selected_room = 0
	else:
		_selected_room = clampi(_selected_room, 0, rooms.size() - 1)

	for index in rooms.size():
		var room: Dictionary = rooms[index]
		var bounds: PackedFloat32Array = room.get("bounds_m", PackedFloat32Array())
		var center: PackedFloat32Array = room.get("center_m", PackedFloat32Array())
		if bounds.size() < 4 or center.size() < 3:
			continue
		var kind := str(room.get("type", "room"))
		var selected := index == _selected_room
		var color := ROOM_SELECTED_COLOR if selected else _room_color(kind)
		var alpha := 0.45 if selected else 0.22
		var width := bounds[2] - bounds[0]
		var depth := bounds[3] - bounds[1]
		var box := BoxMesh.new()
		box.size = Vector3(width, ROOM_OVERLAY_HEIGHT_M, depth)
		var instance := MeshInstance3D.new()
		instance.mesh = box
		instance.material_override = _room_material(color, alpha)
		instance.position = _volume_origin + Vector3(center[0], center[1], center[2])
		instance.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
		_room_overlay.add_child(instance)

		var label := Label3D.new()
		label.text = _room_label_text(kind, str(room.get("id", "room_%d" % index)),
			float(room.get("area_m2", 0.0)), selected)
		label.position = instance.position + Vector3(0.0, ROOM_OVERLAY_HEIGHT_M * 0.5 + 0.4, 0.0)
		label.billboard = BaseMaterial3D.BILLBOARD_ENABLED
		label.no_depth_test = false
		label.font_size = 48
		label.pixel_size = 0.005
		label.outline_size = 8
		label.modulate = color
		_room_overlay.add_child(label)

	_set_room_panel_visible(true)
	_refresh_room_panel(stats)
	_status("base.editor.status.room_view_summary", [
		rooms.size(),
		int(stats.get("resident_capacity", 0)),
		int(stats.get("job_slots", 0)),
	])

func _room_material(color: Color, alpha: float = 0.22) -> StandardMaterial3D:
	var material := StandardMaterial3D.new()
	material.albedo_color = Color(color.r, color.g, color.b, alpha)
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

## Localized display name for a room type. Unknown types fall back to the raw
## internal type so a future detector addition still renders something useful.
func _room_type_label(kind: String) -> String:
	var key := "base.editor.room_type.%s" % kind
	return _i18n.t(key) if _i18n.has_key(key) else kind

## Localized display name for a building class. The core uses data ids such as
## `lower_middle`; unknown classes are shown raw.
func _room_class_label(value: String) -> String:
	var key := "base.editor.room_class.%s" % value
	return _i18n.t(key) if _i18n.has_key(key) else value

func _room_label_text(kind: String, room_id: String, area_m2: float, selected: bool) -> String:
	return _i18n.t("base.editor.rooms.label", [
		"> " if selected else "",
		_room_type_label(kind),
		room_id,
		_num(area_m2),
	])

## Build the read-only room/stats panel on the right side of the editor UI.
##
## The panel is populated from the same `editor_room_stats()` snapshot as the
## 3D overlay, so both views always agree. It starts hidden and is shown by
## the room view toggle (R).
func _build_room_panel(parent: Control) -> void:
	_room_panel = PanelContainer.new()
	_room_panel.name = "RoomStatsPanel"
	_room_panel.set_anchors_and_offsets_preset(Control.PRESET_TOP_RIGHT)
	_room_panel.offset_left = -ROOM_PANEL_WIDTH - 16.0
	_room_panel.offset_right = -16.0
	_room_panel.offset_top = 136.0
	var viewport_size := get_viewport().get_visible_rect().size
	_room_panel.offset_left = -minf(ROOM_PANEL_WIDTH + 16.0, viewport_size.x - 32.0)
	_room_panel.offset_right = -16.0
	_room_panel.offset_bottom = minf(496.0, viewport_size.y - 16.0)
	_room_panel.mouse_filter = Control.MOUSE_FILTER_IGNORE
	_room_panel.visible = false

	var style := StyleBoxFlat.new()
	style.bg_color = Color(0.04, 0.05, 0.07, 0.82)
	style.border_color = Color(1.0, 1.0, 1.0, 0.18)
	style.set_border_width_all(1)
	style.set_corner_radius_all(6)
	style.set_content_margin_all(10)
	_room_panel.add_theme_stylebox_override("panel", style)

	_room_panel_label = RichTextLabel.new()
	_room_panel_label.name = "RoomStatsText"
	_room_panel_label.bbcode_enabled = false
	_room_panel_label.fit_content = false
	_room_panel_label.scroll_active = true
	_room_panel_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	_room_panel_label.mouse_filter = Control.MOUSE_FILTER_IGNORE
	_room_panel_label.add_theme_font_size_override("normal_font_size", 13)
	_room_panel.add_child(_room_panel_label)
	parent.add_child(_room_panel)

func _set_room_panel_visible(is_visible: bool) -> void:
	if _room_panel != null:
		_room_panel.visible = is_visible

## Rebuild the panel text from a room-stats snapshot.
##
## Passing `stats` avoids a second detection pass when the caller (the overlay)
## already queried it. Selection is clamped to the new room count.
func _refresh_room_panel(stats: Dictionary = {}) -> void:
	if _room_panel_label == null:
		return
	if stats.is_empty():
		stats = _core.editor_room_stats()
	if not bool(stats.get("ok", false)):
		_selected_room = -1
		_room_stats_cache = {}
		var error := str(stats.get("error", _i18n.t("base.editor.generic.unknown")))
		_set_room_panel_text("%s\n%s" % [
			_i18n.t("base.editor.rooms.header"),
			_i18n.t("base.editor.rooms.unavailable", [error]),
		])
		_set_room_panel_visible(false)
		return
	_room_stats_cache = stats
	var rooms: Array = stats.get("rooms", [])
	if rooms.is_empty():
		_selected_room = -1
	elif _selected_room < 0:
		_selected_room = 0
	else:
		_selected_room = clampi(_selected_room, 0, rooms.size() - 1)

	var materials: Dictionary = stats.get("materials", {})
	var material_count := materials.size()
	var class_value := str(stats.get("class", _i18n.t("base.editor.generic.unknown")))
	var lines := PackedStringArray([
		_i18n.t("base.editor.rooms.header"),
		_i18n.t("base.editor.rooms.summary", [
			rooms.size(),
			int(stats.get("resident_capacity", 0)),
			int(stats.get("job_slots", 0)),
			_room_class_label(class_value),
		]),
		_i18n.t("base.editor.rooms.solid", [
			int(stats.get("solid_voxels", 0)),
			material_count,
		]),
		_i18n.t("base.editor.rooms.cost", [
			_num(float(stats.get("material_cost", 0.0)))]),
		"",
	])
	if rooms.is_empty():
		lines.append(_i18n.t("base.editor.rooms.none"))
	else:
		for index in rooms.size():
			lines.append(_format_room_line(rooms[index], index))
		lines.append("")
		lines.append(_format_room_detail(rooms[_selected_room], _selected_room))
	_set_room_panel_text("\n".join(lines))

func _set_room_panel_text(text: String) -> void:
	_room_panel_plain = text
	if _room_panel_label != null:
		_room_panel_label.text = text

func _format_room_line(room: Dictionary, index: int) -> String:
	var marker := ">" if index == _selected_room else " "
	return _i18n.t("base.editor.rooms.line", [
		marker,
		str(room.get("id", "room_%d" % index)),
		_room_type_label(str(room.get("type", _i18n.t("base.editor.generic.room")))),
		int(room.get("floor", 0)),
		_num(float(room.get("area_m2", 0.0))),
		int(room.get("beds", 0)),
		int(room.get("desks", 0)),
		int(room.get("doors", 0)),
		int(room.get("windows", 0)),
	])

## One selected-room detail block.
##
## Capacity contribution is the room's beds; job contribution is the room's
## desks, both supplied by the authoritative room detector.
func _format_room_detail(room: Dictionary, index: int) -> String:
	var floor_index := int(room.get("floor", 0))
	var lines := PackedStringArray([
		_i18n.t("base.editor.rooms.detail_selected", [
			str(room.get("id", "room_%d" % index)),
			_room_type_label(str(room.get("type", "room"))),
			floor_index,
		]),
		_i18n.t("base.editor.rooms.detail_metrics", [
			_num(float(room.get("area_m2", 0.0))),
			_num(float(room.get("volume_m3", 0.0))),
			int(room.get("beds", 0)),
			int(room.get("desks", 0)),
		]),
	])
	var bounds: PackedFloat32Array = room.get("bounds_m", PackedFloat32Array())
	if bounds.size() >= 4:
		lines.append(_i18n.t("base.editor.rooms.detail_bounds", [
			_num(bounds[0]), _num(bounds[2]),
			_num(bounds[1]), _num(bounds[3]),
			_num(floor_index * FLOOR_BAND_M),
			_num((floor_index + 1) * FLOOR_BAND_M),
		]))
	return "\n".join(lines)

## Cycle the selected room. `[` steps back, `]` steps forward.
func _cycle_room(step: int) -> void:
	if not _room_overlay_on:
		_status("base.editor.status.room_view_off_hint")
		return
	var rooms: Array = _room_stats_cache.get("rooms", [])
	if rooms.is_empty():
		_status("base.editor.status.no_rooms")
		return
	if _selected_room < 0:
		_selected_room = 0
	else:
		_selected_room = posmod(_selected_room + step, rooms.size())
	_rebuild_room_overlay()
	var refreshed_rooms: Array = _room_stats_cache.get("rooms", [])
	if refreshed_rooms.is_empty():
		_selected_room = -1
		_status("base.editor.status.no_rooms")
		return
	_selected_room = clampi(_selected_room, 0, refreshed_rooms.size() - 1)
	var selected: Dictionary = refreshed_rooms[_selected_room]
	_status("base.editor.status.room_selected", [
		str(selected.get("id", "room_%d" % _selected_room)),
		_room_type_label(str(selected.get("type", "room"))),
	])

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

	_prop_root = Node3D.new()
	_prop_root.name = "EditorProps"
	add_child(_prop_root)

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

	_language_label = _add_label(ui, Control.PRESET_TOP_LEFT)
	_language_label.offset_left = 16.0
	_language_label.offset_right = 420.0
	_language_label.offset_top = 168.0
	_language_label.offset_bottom = 196.0
	_language_label.add_theme_font_size_override("font_size", 14)

	_help_label = _add_label(ui, Control.PRESET_BOTTOM_LEFT)
	_help_label.offset_left = 16.0
	_help_label.offset_right = 720.0
	_help_label.offset_top = -96.0
	_help_label.offset_bottom = -16.0
	_help_label.add_theme_font_size_override("font_size", 14)

	_build_room_panel(ui)

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
	_blueprint_name.custom_minimum_size = Vector2(220.0, 0.0)
	blueprint_bar.add_child(_blueprint_name)

	_save_button = Button.new()
	_save_button.pressed.connect(_open_save_dialog)
	blueprint_bar.add_child(_save_button)

	_load_button = Button.new()
	_load_button.pressed.connect(_open_load_dialog)
	blueprint_bar.add_child(_load_button)

	var directory_error := DirAccess.make_dir_recursive_absolute(
		ProjectSettings.globalize_path(BLUEPRINT_DIR))
	if directory_error != OK:
		push_warning("Could not create blueprint directory %s: %s" % [
			BLUEPRINT_DIR, error_string(directory_error)])

	_save_dialog = FileDialog.new()
	_save_dialog.file_mode = FileDialog.FILE_MODE_SAVE_FILE
	_save_dialog.access = FileDialog.ACCESS_USERDATA
	_save_dialog.current_dir = BLUEPRINT_DIR
	_save_dialog.use_native_dialog = false
	_save_dialog.file_selected.connect(_on_save_file_selected)
	_save_dialog.canceled.connect(_restore_mouse_after_dialog)
	add_child(_save_dialog)

	_load_dialog = FileDialog.new()
	_load_dialog.file_mode = FileDialog.FILE_MODE_OPEN_FILE
	_load_dialog.access = FileDialog.ACCESS_USERDATA
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
	_active_prop = -1
	_update_palette_label()
	var material_name := _material_name(id)
	print("Editor material: %d %s" % [id, material_name])
	_status("base.editor.status.material_selected", [id, material_name])

func _select_prop(kind: int) -> void:
	_active_prop = kind
	_update_palette_label()
	var name := _prop_name(kind)
	print("Editor prop: %d %s" % [kind, name])
	_status("base.editor.status.prop_selected", [name])

func _cycle_structure() -> void:
	var state: Dictionary = _core.editor_structure_state()
	var current := str(state.get("system", "timber_frame"))
	var index := STRUCTURES.find(current)
	var next_index := (index + 1) % STRUCTURES.size()
	var selected: Dictionary = _core.editor_set_structure(STRUCTURES[next_index])
	if not bool(selected.get("ok", false)):
		_status("base.editor.generic.unknown_error", [str(selected.get("error", "structure"))])
		return
	_update_palette_label()
	_refresh_stats_label()
	print("Editor structure: %s" % str(selected.get("system_name", "?")))

func _update_palette_label() -> void:
	if _palette_label == null:
		return
	var lines := PackedStringArray([_i18n.t("base.editor.palette.materials")])
	for entry in MATERIALS:
		var id := int(entry["id"])
		var marker := ">" if id == _active_material and _active_prop < 0 else " "
		lines.append("%s %d  %s" % [marker, id, _material_name(id)])
	var structure: Dictionary = _core.editor_structure_state()
	if bool(structure.get("ok", false)):
		var limit := int(structure.get("effective_max_stories", 0))
		lines.append("H  structure: %s (%d/%d stories)" % [
			str(structure.get("system_name", "?")),
			int(structure.get("stories", 0)),
			limit,
		])
	lines.append(_i18n.t("base.editor.palette.props"))
	for index in PROPS.size():
		var entry: Dictionary = PROPS[index]
		var kind := int(entry["kind"])
		var marker := ">" if kind == _active_prop else " "
		lines.append("%s %d  %s" % [marker, 6 + index, _prop_name(kind)])
	_palette_label.text = "\n".join(lines)

func _refresh_stats_label(build_ms: float = -1.0) -> void:
	if _stats_label == null:
		return
	var stats: Dictionary = _core.editor_volume_stats()
	var text := _format_stats(stats)
	if build_ms >= 0.0:
		text += "\n" + _i18n.t("base.editor.stats.build", [
			_num(build_ms), int(stats.get("mesh_builds", 0))])
	elif _remesh_pending:
		text += "\n" + _i18n.t("base.editor.stats.remesh_pending")
	_stats_label.text = text

func _format_stats(stats: Dictionary) -> String:
	return _i18n.t("base.editor.stats.solid_quads", [
		int(stats.get("solid_voxels", 0)),
		int(stats.get("last_mesh_quads", 0)),
		_num(float(stats.get("mesh_ms", 0.0))),
	]) + "\n" + _i18n.t("base.editor.stats.chunks_volume", [
		int(stats.get("allocated_chunks", 0)),
		_num(_volume_bounds.x), _num(_volume_bounds.y), _num(_volume_bounds.z),
	]) + "\n" + _i18n.t("base.editor.stats.cost", [
		_num(float(stats.get("material_cost", 0.0)))])

func _set_status(text: String) -> void:
	if _status_label != null:
		_status_label.text = text

func _status(key: String, params = []) -> void:
	_set_status(_i18n.t(key, params))

func _num(value: float) -> String:
	var text := "%.1f" % value
	return text.replace(".", ",") if _i18n.locale() == "de" else text

func _apply_locale(update_status: bool = true) -> void:
	if get_window() != null:
		get_window().title = _i18n.t("base.editor.window.title")
	if _language_label != null:
		_language_label.text = _i18n.t("base.editor.hud.language", [_i18n.locale_name()])
	if _help_label != null:
		_help_label.text = "\n".join(PackedStringArray([
			_i18n.t("base.editor.help.move"),
			_i18n.t("base.editor.help.edit"),
			_i18n.t("base.editor.help.tools"),
		]))
	if _blueprint_name != null:
		_blueprint_name.placeholder_text = _i18n.t("base.editor.blueprint.name_placeholder")
		_blueprint_name.tooltip_text = _i18n.t("base.editor.blueprint.name_tooltip")
	if _save_button != null:
		_save_button.text = _i18n.t("base.editor.blueprint.save")
		_save_button.tooltip_text = _i18n.t("base.editor.blueprint.save_tooltip")
	if _load_button != null:
		_load_button.text = _i18n.t("base.editor.blueprint.load")
		_load_button.tooltip_text = _i18n.t("base.editor.blueprint.load_tooltip")
	if _save_dialog != null:
		_save_dialog.title = _i18n.t("base.editor.blueprint.save_title")
		_save_dialog.filters = PackedStringArray([
			_i18n.t("base.editor.blueprint.filter", [BLUEPRINT_EXT])])
	if _load_dialog != null:
		_load_dialog.title = _i18n.t("base.editor.blueprint.load_title")
		_load_dialog.filters = PackedStringArray([
			_i18n.t("base.editor.blueprint.filter", [BLUEPRINT_EXT])])
	_update_palette_label()
	_refresh_stats_label()
	if _room_overlay_on:
		_rebuild_room_overlay()
	elif _room_panel != null and _room_panel.visible:
		_refresh_room_panel()
	if update_status:
		_status("base.editor.status.language", [_i18n.locale_name()])

func _toggle_locale() -> void:
	_i18n.toggle()
	_apply_locale()

func _material_name(id: int) -> String:
	for entry in MATERIALS:
		if int(entry["id"]) == id:
			var key := "base.editor.material.%s" % str(entry["name"])
			return _i18n.t(key) if _i18n.has_key(key) else str(entry["name"])
	return _i18n.t("base.editor.generic.unknown")

func _prop_name(kind: int) -> String:
	for entry in PROPS:
		if int(entry["kind"]) == kind:
			var key := "base.editor.prop.%s" % str(entry["name"])
			return _i18n.t(key) if _i18n.has_key(key) else str(entry["name"])
	return _i18n.t("base.editor.generic.prop")

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
		_status("base.editor.status.blueprint_name_empty")
		return false
	var id := "local:%s" % _slug(name)
	var result: Dictionary = _core.editor_save_blueprint(
		ProjectSettings.globalize_path(path), id, name)
	if not bool(result.get("ok", false)):
		var error := str(result.get("error", _i18n.t("base.editor.generic.unknown_error")))
		_status("base.editor.status.save_failed", [error])
		push_error("Blueprint save failed: %s" % error)
		return false
	_status("base.editor.status.saved", [name, _short_hash(str(result.get("hash", "")))])
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
		var error := str(result.get("error", _i18n.t("base.editor.generic.unknown_error")))
		_status("base.editor.status.load_failed", [error])
		push_error("Blueprint load failed: %s" % error)
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
	_history.clear()
	_redo.clear()
	_selected_room = -1
	_room_stats_cache = {}
	_remesh_pending = false
	_rebuild_volume_mesh()
	_refresh_prop_nodes()
	_update_palette_label()
	_status("base.editor.status.loaded", [
		str(result.get("name", _i18n.t("base.editor.generic.blueprint"))),
		_short_hash(str(result.get("hash", "")))])
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

## Headless localization contract.
##
## Checks EN/DE key parity, placeholder parity, required content/room keys,
## toggle behavior, and representative rendered strings. Runs before the core
## is created so it can test the string table alone.
func _run_locale_smoke() -> void:
	var status := "ok"
	var en := EditorI18n.new("en")
	var de := EditorI18n.new("de")
	var en_keys := en.keys("en")
	var de_keys := de.keys("de")
	if en_keys.size() != de_keys.size():
		status = "key_count"
	elif not en.missing_keys("en", "de").is_empty():
		status = "missing_de"
	elif not de.missing_keys("de", "en").is_empty():
		status = "missing_en"

	var required := [
		"base.editor.status.saved",
		"base.editor.status.loaded",
		"base.editor.rooms.line",
		"base.editor.rooms.detail_bounds",
		"base.editor.material.timber",
		"base.editor.prop.bed",
		"base.editor.room_type.bedroom",
		"base.editor.room_type.kitchen",
		"base.editor.room_type.bathroom",
		"base.editor.room_type.office",
		"base.editor.room_type.living_dining",
		"base.editor.room_type.empty",
	]
	for key in required:
		if status == "ok" and not en.has_key(key, "en"):
			status = "en:" + key
		elif status == "ok" and not de.has_key(key, "de"):
			status = "de:" + key

	if status == "ok":
		for key in en_keys:
			if EditorI18n.placeholders(en.raw(key, "en")) != EditorI18n.placeholders(de.raw(key, "de")):
				status = "placeholders:" + key
				break

	if status == "ok" and en.t("base.editor.missing") != "!base.editor.missing!":
		status = "missing_marker"
	if status == "ok" and de.t("base.editor.room_type.bedroom") != "Schlafzimmer":
		status = "german_room"

	var toggle := EditorI18n.new("en")
	toggle.toggle()
	if status == "ok" and toggle.locale() != "de":
		status = "toggle_de"
	toggle.toggle()
	if status == "ok" and toggle.locale() != "en":
		status = "toggle_en"

	if status == "ok":
		var samples := PackedStringArray([
			en.t("base.editor.status.removed", ["timber", "(1, 2, 3)"]),
			en.t("base.editor.status.saved", ["River House", "blake3:abc"]),
			en.t("base.editor.rooms.line", ["", "room_0", "bedroom", 0, "20.2", 1, 0, 4, 8]),
			en.t("base.editor.rooms.detail_bounds", ["0.1", "3.9", "0.1", "3.9", "0.0", "3.0"]),
			de.t("base.editor.status.removed", ["Holz", "(1, 2, 3)"]),
			de.t("base.editor.status.saved", ["Haus", "blake3:abc"]),
			de.t("base.editor.rooms.line", ["", "room_0", "Schlafzimmer", 0, "20,2", 1, 0, 4, 8]),
			de.t("base.editor.rooms.detail_bounds", ["0,1", "3,9", "0,1", "3,9", "0,0", "3,0"]),
		])
		for sample in samples:
			if sample.contains("!") or sample.contains("{"):
				status = "render"
				break

	if status == "ok":
		print("LOCALE_SMOKE en=ok de=ok keys=%d status=ok" % en_keys.size())
	else:
		print("LOCALE_SMOKE en=fail de=fail keys=0 status=%s" % status)
	get_tree().quit(0 if status == "ok" else 1)

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
	elif not _core.editor_place_prop(0, 1, 1, 1, 0.0):
		status = "prop_failed"

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
		elif int(saved.get("props", -1)) != 1:
			status = "save_props"

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
		elif int(loaded.get("props", -1)) != 1:
			status = "load_props"
		elif int(_core.editor_props().get("props", []).size()) != 1:
			status = "load_prop_state"
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
	print("BLUEPRINT_SMOKE solid=%d props=%d hash=%s status=%s" % [
		int(loaded.get("solid_voxels", 0)),
		int(loaded.get("props", 0)),
		str(loaded.get("hash", "")),
		status,
	])
	get_tree().quit(0 if status == "ok" else 1)

## End-to-end vertical-slice acceptance check.
##
## Builds the shared two-story acceptance house, asserts room/cost stats, saves
## and reloads the package, places a mesh copy in the scene, and replays 500
## deterministic random edits through undo/redo with content-hash checks.
func _run_acceptance_smoke() -> void:
	var status := "ok"
	var rooms_status := "not_run"
	var copy_status := "not_run"
	var undo_status := "not_run"
	var room_count := 0
	var bedrooms := 0
	var kitchens := 0
	var bathrooms := 0
	var capacity := 0
	var jobs := 0
	var cost := 0.0
	var solid := 0
	var baseline_hash := ""

	var save_path := ProjectSettings.globalize_path("user://slice_acceptance.tsbp")
	var edited_path := ProjectSettings.globalize_path("user://slice_acceptance_edited.tsbp")
	var undo_path := ProjectSettings.globalize_path("user://slice_acceptance_undo.tsbp")
	var redo_path := ProjectSettings.globalize_path("user://slice_acceptance_redo.tsbp")
	for path in [save_path, edited_path, undo_path, redo_path]:
		if FileAccess.file_exists(path):
			DirAccess.remove_absolute(path)

	if not _core.editor_build_acceptance_house():
		status = "build_failed"

	var stats: Dictionary = {}
	if status == "ok":
		stats = _core.editor_room_stats()
		var rooms: Array = stats.get("rooms", [])
		room_count = rooms.size()
		var types := {}
		for room in rooms:
			var kind := str(room.get("type", ""))
			types[kind] = int(types.get(kind, 0)) + 1
		bedrooms = int(types.get("bedroom", 0))
		kitchens = int(types.get("kitchen", 0))
		bathrooms = int(types.get("bathroom", 0))
		capacity = int(stats.get("resident_capacity", 0))
		jobs = int(stats.get("job_slots", 0))
		cost = float(stats.get("material_cost", 0.0))
		solid = int(stats.get("solid_voxels", 0))
		if not bool(stats.get("ok", false)):
			status = "stats_failed"
		elif room_count != 4 or bedrooms != 2 or kitchens != 1 or bathrooms != 1:
			status = "room_types"
		elif capacity != 2 or jobs != 2:
			status = "capacity"
		elif absf(cost - 38028.8) > 0.1:
			status = "cost"
		elif solid != 64224:
			status = "solid"
		else:
			rooms_status = "ok"

	var saved: Dictionary = {}
	if status == "ok":
		saved = _core.editor_save_blueprint(
			save_path, "local:slice_acceptance", "Slice Acceptance")
		if not bool(saved.get("ok", false)):
			status = "save_failed"
		elif int(saved.get("rooms", -1)) != 4 or int(saved.get("props", -1)) != 8:
			status = "save_metadata"
		elif absf(float(saved.get("material_cost", 0.0)) - cost) > 0.1:
			status = "save_cost"
		else:
			baseline_hash = str(saved.get("hash", ""))
			if not baseline_hash.begins_with("blake3:"):
				status = "save_hash"

	if status == "ok" and not _core.editor_clear():
		status = "clear_failed"

	var loaded: Dictionary = {}
	if status == "ok":
		loaded = _core.editor_load_blueprint(save_path)
		if not bool(loaded.get("ok", false)):
			status = "load_failed"
		elif str(loaded.get("hash", "")) != baseline_hash:
			status = "load_hash"
		elif int(loaded.get("rooms", -1)) != 4 or int(loaded.get("props", -1)) != 8:
			status = "load_metadata"
		elif int(loaded.get("solid_voxels", -1)) != 64224:
			status = "load_solid"
		elif absf(float(loaded.get("material_cost", 0.0)) - cost) > 0.1:
			status = "load_cost"

	if status == "ok":
		var editor_mesh: ArrayMesh = _core.build_editor_volume_mesh()
		var copy_mesh: ArrayMesh = _core.build_blueprint_mesh(save_path)
		if editor_mesh.get_surface_count() != 1:
			status = "mesh_surface"
		elif copy_mesh.get_surface_count() != 1:
			status = "copy_surface"
		elif not copy_mesh.get_aabb().size.is_equal_approx(editor_mesh.get_aabb().size):
			status = "copy_aabb"
		else:
			var editor_arrays := editor_mesh.surface_get_arrays(0)
			var copy_arrays := copy_mesh.surface_get_arrays(0)
			if editor_arrays[Mesh.ARRAY_VERTEX] != copy_arrays[Mesh.ARRAY_VERTEX]:
				status = "copy_vertices"
			elif editor_arrays[Mesh.ARRAY_INDEX] != copy_arrays[Mesh.ARRAY_INDEX]:
				status = "copy_indices"
			else:
				var copy := MeshInstance3D.new()
				copy.name = "BlueprintCopy"
				copy.mesh = copy_mesh
				copy.position = Vector3(20.0, 0.0, 0.0)
				copy.material_override = _flat_material(VOXEL_COLOR)
				add_child(copy)
				if get_node_or_null("BlueprintCopy") == null:
					status = "copy_node"
				else:
					copy_status = "ok"

	if status == "ok":
		_history.clear()
		_redo.clear()
		var rng := RandomNumberGenerator.new()
		rng.seed = 20261120
		var attempts := 0
		while _history.size() < 500 and attempts < 20000:
			attempts += 1
			var cell := Vector3i(
				rng.randi_range(0, _volume_voxels.x - 1),
				rng.randi_range(0, _volume_voxels.y - 1),
				rng.randi_range(0, _volume_voxels.z - 1))
			_set_voxel_with_history(cell, rng.randi_range(0, 5))
		if _history.size() != 500:
			status = "undo_setup"
		else:
			var edited: Dictionary = _core.editor_save_blueprint(
				edited_path, "local:slice_acceptance_edited", "Slice Acceptance Edited")
			if not bool(edited.get("ok", false)):
				status = "undo_save"
			else:
				var edited_hash := str(edited.get("hash", ""))
				if edited_hash == baseline_hash:
					status = "undo_noop"
				else:
					for _index in 500:
						_undo_last()
					if not _history.is_empty() or _redo.size() != 500:
						status = "undo_replay"
					else:
						var undone: Dictionary = _core.editor_save_blueprint(
							undo_path, "local:slice_acceptance", "Slice Acceptance")
						if not bool(undone.get("ok", false)) or str(undone.get("hash", "")) != baseline_hash:
							status = "undo_hash"
						else:
							for _index in 500:
								_redo_last()
							if _redo.size() != 0 or _history.size() != 500:
								status = "redo_replay"
							else:
								var redone: Dictionary = _core.editor_save_blueprint(
									redo_path, "local:slice_acceptance_edited", "Slice Acceptance Edited")
								if not bool(redone.get("ok", false)) or str(redone.get("hash", "")) != edited_hash:
									status = "redo_hash"
								else:
									undo_status = "ok"

	for path in [save_path, edited_path, undo_path, redo_path]:
		if FileAccess.file_exists(path):
			DirAccess.remove_absolute(path)

	print("SLICE_ACCEPTANCE rooms=%d bedrooms=%d kitchen=%d bathroom=%d capacity=%d jobs=%d cost=%.2f solid=%d hash=%s copy=%s rooms_check=%s undo=%s status=%s" % [
		room_count, bedrooms, kitchens, bathrooms, capacity, jobs, cost, solid,
		baseline_hash, copy_status, rooms_status, undo_status, status,
	])
	get_tree().quit(0 if status == "ok" else 1)

## German layout budget check.
##
## Builds the real editor UI headlessly and asserts that every fixed-width
## label and button's minimum text size fits its rect in EN and DE. It cannot
## replace a visual pass, but it catches the common text-expansion regressions.
func _run_layout_smoke() -> void:
	await get_tree().process_frame
	var status := "ok"
	var locale_status := {"en": "ok", "de": "ok"}
	var checks := 0
	var stats := {
		"ok": true,
		"rooms": [{
			"id": "room_0", "type": "bedroom", "floor": 1,
			"area_m2": 20.2, "volume_m3": 60.5, "beds": 1, "desks": 1,
			"doors": 4, "windows": 8,
			"bounds_m": PackedFloat32Array([0.0, 0.0, 4.0, 4.0]),
			"center_m": PackedFloat32Array([2.0, 4.5, 2.0]),
		}],
		"resident_capacity": 2,
		"job_slots": 2,
		"class": "upper",
		"solid_voxels": 64224,
		"material_cost": 38028.8,
		"materials": {"material:1": 35296, "material:2": 9600},
	}
	for locale in ["en", "de"]:
		_i18n.set_locale(locale)
		_apply_locale(false)
		_status("base.editor.status.saved", [
			"River House", "blake3:0123456789abcdef"])
		_stats_label.text = _format_stats(stats)
		_refresh_room_panel(stats)
		await get_tree().process_frame
		var controls := {
			"palette": _palette_label,
			"stats": _stats_label,
			"status": _status_label,
			"language": _language_label,
			"help": _help_label,
			"blueprint_name": _blueprint_name,
			"save": _save_button,
			"load": _load_button,
		}
		for name in controls:
			var control: Control = controls[name]
			checks += 1
			if control.get_combined_minimum_size().x > control.size.x + 0.5:
				status = "%s:%s" % [locale, name]
				locale_status[locale] = name
				break
		if status != "ok":
			break
	print("LAYOUT_SMOKE en=%s de=%s checks=%d status=%s" % [
		locale_status["en"], locale_status["de"], checks, status])
	get_tree().quit(0 if status == "ok" else 1)

func _is_locale_smoke() -> bool:
	return OS.get_cmdline_user_args().has("--locale-smoke")

func _is_editor_smoke() -> bool:
	return OS.get_cmdline_user_args().has("--editor-smoke")

func _is_room_overlay_smoke() -> bool:
	return OS.get_cmdline_user_args().has("--room-overlay-smoke")

func _is_undo_smoke() -> bool:
	return OS.get_cmdline_user_args().has("--undo-smoke")

func _is_blueprint_smoke() -> bool:
	return OS.get_cmdline_user_args().has("--blueprint-smoke")

func _is_acceptance_smoke() -> bool:
	return OS.get_cmdline_user_args().has("--acceptance-smoke")

func _is_layout_smoke() -> bool:
	return OS.get_cmdline_user_args().has("--layout-smoke")

## Headless contract check for undo/redo.
func _run_undo_smoke() -> void:
	var cell := Vector3i(1, 1, 1)
	var undo_status := "ok"
	var redo_status := "ok"
	var status := "ok"
	if not _set_voxel_with_history(cell, 1):
		status = "set_failed"
	elif int(_core.editor_get_voxel(cell.x, cell.y, cell.z)) != 1:
		status = "set_read"
	else:
		_undo_last()
		if int(_core.editor_get_voxel(cell.x, cell.y, cell.z)) != 0:
			undo_status = "undo_failed"
			status = "undo_failed"
		else:
			_redo_last()
			if int(_core.editor_get_voxel(cell.x, cell.y, cell.z)) != 1:
				redo_status = "redo_failed"
				status = "redo_failed"
	if status == "ok":
		var remove_cell := Vector3i(2, 1, 1)
		if not _core.editor_set_voxel(remove_cell.x, remove_cell.y, remove_cell.z, 2):
			status = "remove_setup"
		elif not _set_voxel_with_history(remove_cell, 0):
			status = "remove_failed"
		else:
			_undo_last()
			if int(_core.editor_get_voxel(remove_cell.x, remove_cell.y, remove_cell.z)) != 2:
				undo_status = "undo_remove_failed"
				status = "undo_remove_failed"
			else:
				_redo_last()
				if int(_core.editor_get_voxel(remove_cell.x, remove_cell.y, remove_cell.z)) != 0:
					redo_status = "redo_remove_failed"
					status = "redo_remove_failed"
	print("UNDO_SMOKE undo=%s redo=%s status=%s" % [undo_status, redo_status, status])
	get_tree().quit(0 if status == "ok" else 1)

## Headless contract check for the room overlay and stats panel.
##
## Builds a sealed room, toggles the room view, and asserts the detector,
## overlay node tree, panel text, and selected room. Uses the same volume
## created in `_ready`.
func _run_room_overlay_smoke() -> void:
	var status := "ok"
	var panel_status := "not_run"
	var stats: Dictionary = {}
	var rooms: Array = []
	_build_room_smoke_ui()
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
	elif not _core.editor_place_prop(0, 10, 1, 10, 0.0):
		status = "bed_failed"
	else:
		_toggle_room_overlay()
		stats = _core.editor_room_stats()
		rooms = stats.get("rooms", [])
		if not bool(stats.get("ok", false)):
			status = "stats_failed"
		elif rooms.size() != 1:
			status = "room_count"
		elif str(rooms[0].get("type", "")) != "bedroom":
			status = "room_type"
		elif int(stats.get("resident_capacity", 0)) != 1:
			status = "capacity"
		elif _room_overlay == null or _room_overlay.get_child_count() < 1:
			status = "overlay_nodes"
		elif _room_panel == null or not _room_panel.visible:
			status = "panel_hidden"
		elif not _room_panel_plain.contains("room_0 · bedroom"):
			status = "panel_bedroom"
		elif not _room_panel_plain.contains("capacity 1"):
			status = "panel_capacity"
		elif _selected_room < 0 or _selected_room >= rooms.size():
			status = "panel_selected_range"
		elif str(rooms[_selected_room].get("type", "")) != "bedroom":
			status = "panel_selected"
		else:
			panel_status = "ok"
			print("ROOM_OVERLAY_SMOKE rooms=%d nodes=%d type=%s capacity=%d status=ok" % [
				rooms.size(), _room_overlay.get_child_count(),
				str(rooms[0].get("type", "")), int(stats.get("resident_capacity", 0))])

	var selected := "none"
	if _selected_room >= 0 and _selected_room < rooms.size():
		selected = str(rooms[_selected_room].get("type", "none"))
	if status != "ok" and panel_status == "not_run":
		panel_status = status
	print("ROOM_PANEL_SMOKE rooms=%d selected=%s capacity=%d status=%s" % [
		rooms.size(),
		selected,
		int(stats.get("resident_capacity", 0)),
		panel_status,
	])
	if status != "ok":
		print("ROOM_OVERLAY_SMOKE rooms=0 nodes=0 type=none capacity=0 status=%s" % status)
		get_tree().quit(1)
		return
	get_tree().quit(0)

## Minimal UI host so the headless smoke exercises the real panel builder.
func _build_room_smoke_ui() -> void:
	var layer := CanvasLayer.new()
	layer.name = "RoomSmokeUI"
	add_child(layer)
	var ui := Control.new()
	ui.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	ui.mouse_filter = Control.MOUSE_FILTER_IGNORE
	layer.add_child(ui)
	_build_room_panel(ui)

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

	var structure: Dictionary = _core.editor_structure_state()
	if status == "ok" and (
		not bool(structure.get("ok", false))
		or not bool(structure.get("valid", false))
		or str(structure.get("system", "")) != "timber_frame"
	):
		status = "structure_mismatch"

	print("EDITOR_SMOKE solid=%d quads=%d surfaces=%d status=%s" % [solid, quads, surfaces, status])
	get_tree().quit(0 if status == "ok" else 1)
