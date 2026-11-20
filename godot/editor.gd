extends Node3D

## First-person voxel editor shell for the Townsmith vertical slice.
##
## This is the first playable editor: a default 5 x 3 x 5 chunk volume on a
## neutral pad, a fly camera, crosshair raycast edits, a five-material
## palette, and a debounced whole-volume remesh. Edits live only in this
## session; blueprint save/load is the next slice step.
##
## Run it with:
##   godot --path godot res://editor.tscn
##
## Headless smoke:
##   godot --headless --path godot res://editor.tscn -- --editor-smoke
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

func _ready() -> void:
	_core = TownsmithCore.new()
	print("Townsmith core version: ", _core.version())

	if not _core.create_editor_volume(VOLUME_CHUNKS.x, VOLUME_CHUNKS.y, VOLUME_CHUNKS.z):
		push_error("Editor: failed to create the default editor volume")
		if _is_editor_smoke():
			print("EDITOR_SMOKE solid=0 quads=0 surfaces=0 status=create_failed")
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

	if _is_editor_smoke():
		_run_editor_smoke()
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
		for index in MATERIALS.size():
			if key == KEY_1 + index:
				_select_material(int(MATERIALS[index]["id"]))
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
		"Mesh rebuilds 250 ms after the last edit",
	]))

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

func _is_editor_smoke() -> bool:
	return OS.get_cmdline_user_args().has("--editor-smoke")

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
