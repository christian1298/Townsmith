extends Node

## Non-invasive session recorder for the Q-101 human playtest.
##
## This node is a child of the world scene root, so it can observe input before
## the root consumes it in `_unhandled_input` and read the root's coarse state
## without the root knowing it exists. It is inert unless the scene is launched
## with `--playtest` (human session) or `--playtest-smoke` (headless smoke).
##
## The recording is local only: timestamps, key/mouse actions, and state
## snapshots. It does not read names, clipboard contents, or network data.

const SESSION_PATH := "user://playtest_session.json"
const SNAPSHOT_SECONDS := 10.0
const SMOKE_SECONDS := 3.0
const SMOKE_SNAPSHOT_SECONDS := 0.5

var _active := false
var _smoke := false
var _finished := false
var _started_at := ""
var _started_ticks := 0
var _last_snapshot_ticks := 0
var _session_status := "ok"
var _out_path := ""
var _events: Array = []
var _snapshots: Array = []


func _ready() -> void:
	var args := OS.get_cmdline_user_args()
	_smoke = args.has("--playtest-smoke")
	_active = _smoke or args.has("--playtest")
	if not _active:
		set_process(false)
		set_process_input(false)
		return

	_started_ticks = Time.get_ticks_msec()
	_last_snapshot_ticks = _started_ticks
	_started_at = _utc_now()
	_out_path = _arg_value("--playtest-out")
	if _out_path != "":
		_out_path = _absolute_path(_out_path)

	# A fresh session should never be masked by an old file after a failed run.
	var session_file := ProjectSettings.globalize_path(SESSION_PATH)
	if FileAccess.file_exists(session_file):
		DirAccess.remove_absolute(session_file)

	print("PLAYTEST_PROBE_START smoke=%d duration_s=%.1f" % [1 if _smoke else 0, SMOKE_SECONDS if _smoke else 0.0])
	# The world root's `_ready` may run after this child; defer the first
	# snapshot until the bridge and HUD exist.
	call_deferred("_capture_snapshot")


func _process(_delta: float) -> void:
	if not _active or _finished:
		return
	var now := Time.get_ticks_msec()
	var interval := SMOKE_SNAPSHOT_SECONDS if _smoke else SNAPSHOT_SECONDS
	if float(now - _last_snapshot_ticks) / 1000.0 >= interval:
		_last_snapshot_ticks = now
		_capture_snapshot()
	if _smoke and float(now - _started_ticks) / 1000.0 >= SMOKE_SECONDS:
		_finish()
		get_tree().quit(0 if _session_status == "ok" else 1)


func _input(event: InputEvent) -> void:
	if not _active or _finished:
		return
	if event is InputEventKey:
		if event.echo:
			return
		var code := int(event.physical_keycode)
		if code == 0:
			code = int(event.keycode)
		var action := _action_for_key(code)
		if code == KEY_N and event.shift_pressed:
			action = "clear_settlements"
		_events.append({
			"t_s": _elapsed(),
			"type": "key",
			"key": OS.get_keycode_string(code),
			"pressed": event.pressed,
			"shift": event.shift_pressed,
			"action": action,
		})
	elif event is InputEventMouseButton:
		_events.append({
			"t_s": _elapsed(),
			"type": "mouse",
			"key": _mouse_label(event.button_index),
			"pressed": event.pressed,
			"action": _action_for_mouse(event.button_index),
		})


func _notification(what: int) -> void:
	if what == NOTIFICATION_WM_CLOSE_REQUEST and _active and not _finished:
		_finish()


func _exit_tree() -> void:
	if _active and not _finished:
		_finish()


func _capture_snapshot() -> void:
	if not _active or _finished:
		return
	var parent := get_parent()
	if parent == null or not is_instance_valid(parent):
		return

	var core = _field(parent, "core", null)
	var placements := 0
	var construction_sites := 0
	var construction_active := 0
	var construction_completed := 0
	var construction_stalled := 0
	var population := 0
	var completed_buildings := 0

	if core != null:
		if core.has_method("world_placement_count"):
			placements = int(core.world_placement_count())
		if core.has_method("construction_summary"):
			var construction: Dictionary = core.construction_summary()
			construction_sites = int(construction.get("sites", 0))
			construction_active = int(construction.get("active", 0))
			construction_completed = int(construction.get("complete", 0))
			construction_stalled = int(construction.get("stalled", 0))
		if core.has_method("city_summary"):
			var city: Dictionary = core.city_summary()
			population = int(city.get("population", 0))
			completed_buildings = int(city.get("completed_buildings", 0))

	# Fallback to the scene's node maps if the bridge is older or missing a
	# method. `world_placement_count` is authoritative when present.
	if core == null or not core.has_method("world_placement_count"):
		if "placed_nodes" in parent:
			placements = int(parent.placed_nodes.size())
	if core == null or not core.has_method("construction_summary"):
		if "construction_nodes" in parent:
			construction_sites = int(parent.construction_nodes.size())

	_snapshots.append({
		"t_s": _elapsed(),
		"mode": str(_field(parent, "mode", "")),
		"selected_id": int(_field(parent, "selected_id", -1)),
		"placements": placements,
		"construction_sites": construction_sites,
		"construction_active": construction_active,
		"construction_completed": construction_completed,
		"construction_stalled": construction_stalled,
		"population": population,
		"completed_buildings": completed_buildings,
		"dashboard_visible": bool(_field(parent, "dashboard_visible", false)),
		"materialization_visible": bool(_field(parent, "materialization_visible", false)),
	})
	_last_snapshot_ticks = Time.get_ticks_msec()


func _finish() -> void:
	if _finished:
		return
	_capture_snapshot()
	_finished = true

	var data := {
		"schema": 1,
		"status": "ok",
		"started_at": _started_at,
		"ended_at": _utc_now(),
		"duration_s": _elapsed(),
		"godot_version": str(Engine.get_version_info().get("string", "unknown")),
		"seed": _world_seed(),
		"window_size": _window_size(),
		"smoke": _smoke,
		"events": _events,
		"snapshots": _snapshots,
	}

	var write_ok := _write_session(data)
	var readback_ok := false
	if write_ok:
		readback_ok = _validate_session(_read_session())
	if not write_ok or not readback_ok:
		_session_status = "failed"
		data["status"] = "failed"
		if write_ok:
			_write_session(data)

	var json_path := ProjectSettings.globalize_path(SESSION_PATH)
	if _out_path != "":
		json_path = _out_path
	print("PLAYTEST_PROBE status=%s events=%d duration_s=%.2f snapshots=%d json=\"%s\"" % [
		_session_status,
		_events.size(),
		_elapsed(),
		_snapshots.size(),
		json_path,
	])


func _write_session(data: Dictionary) -> bool:
	var text := JSON.stringify(data, "\t", false)
	if not _write_text(ProjectSettings.globalize_path(SESSION_PATH), text):
		return false
	if _out_path != "":
		return _write_text(_out_path, text)
	return true


func _write_text(path: String, text: String) -> bool:
	var dir := path.get_base_dir()
	if dir != "":
		DirAccess.make_dir_recursive_absolute(dir)
	var file := FileAccess.open(path, FileAccess.WRITE)
	if file == null:
		return false
	file.store_string(text)
	file.close()
	return true


func _read_session():
	var path := _out_path if _out_path != "" else ProjectSettings.globalize_path(SESSION_PATH)
	if not FileAccess.file_exists(path):
		return null
	return JSON.parse_string(FileAccess.get_file_as_string(path))


func _validate_session(parsed) -> bool:
	if not (parsed is Dictionary):
		return false
	var data: Dictionary = parsed
	for key in ["status", "events", "snapshots"]:
		if not data.has(key):
			return false
	if not (data["events"] is Array) or not (data["snapshots"] is Array):
		return false
	if str(data.get("status", "")) != "ok":
		return false
	if _smoke and (data["snapshots"] as Array).is_empty():
		return false
	return true


func _field(object: Object, property: String, fallback):
	if object != null and property in object:
		return object.get(property)
	return fallback


func _world_seed() -> int:
	var parent := get_parent()
	if parent == null:
		return 0
	var script = parent.get_script()
	if script != null:
		var constants: Dictionary = script.get_script_constant_map()
		if constants.has("SEED"):
			return int(constants["SEED"])
	return 0


func _window_size() -> Dictionary:
	var size := DisplayServer.window_get_size()
	if size.x <= 0 or size.y <= 0:
		size = Vector2i(
			int(ProjectSettings.get_setting("display/window/size/viewport_width", 0)),
			int(ProjectSettings.get_setting("display/window/size/viewport_height", 0))
		)
	return {"width": size.x, "height": size.y}


func _elapsed() -> float:
	return snappedf(float(Time.get_ticks_msec() - _started_ticks) / 1000.0, 0.001)


func _utc_now() -> String:
	var text := Time.get_datetime_string_from_unix_time(
		int(Time.get_unix_time_from_system()), false
	)
	return text if text.ends_with("Z") else text + "Z"


func _arg_value(name: String) -> String:
	for index in range(OS.get_cmdline_user_args().size()):
		var arg: String = OS.get_cmdline_user_args()[index]
		if arg == name and index + 1 < OS.get_cmdline_user_args().size():
			return OS.get_cmdline_user_args()[index + 1]
		var prefix := name + "="
		if arg.begins_with(prefix):
			return arg.substr(prefix.length())
	return ""


func _absolute_path(value: String) -> String:
	if value.begins_with("res://") or value.begins_with("user://"):
		return ProjectSettings.globalize_path(value)
	return value


func _action_for_key(code: int) -> String:
	match code:
		KEY_W:
			return "pan_forward"
		KEY_A:
			return "pan_left"
		KEY_S:
			return "pan_back"
		KEY_D:
			return "pan_right"
		KEY_Q:
			return "rotate_left"
		KEY_E:
			return "rotate_right"
		KEY_R:
			return "rotate_placement"
		KEY_X:
			return "raise_target"
		KEY_Z:
			return "lower_target"
		KEY_N:
			return "found_settlement"
		KEY_B:
			return "cycle_starter"
		KEY_I:
			return "cycle_industry"
		KEY_U:
			return "cycle_utility"
		KEY_K:
			return "cycle_service"
		KEY_C:
			return "toggle_crew"
		KEY_T:
			return "cycle_tax"
		KEY_L:
			return "borrow"
		KEY_Y:
			return "research_next"
		KEY_V:
			return "add_materials"
		KEY_F1:
			return "toggle_dashboard"
		KEY_F5:
			return "save_world"
		KEY_F9:
			return "load_world"
		KEY_G:
			return "road_mode"
		KEY_H:
			return "lot_mode"
		KEY_ENTER, KEY_KP_ENTER:
			return "enter_architect"
		KEY_ESCAPE:
			return "escape"
		KEY_DELETE:
			return "clear_placements"
		KEY_M:
			return "return_to_mayor"
		KEY_F2:
			return "toggle_citizen_panel"
		KEY_F3:
			return "toggle_follow"
		KEY_F4:
			return "toggle_materialization"
		KEY_BACKSPACE:
			return "clear_roads_or_lots"
		KEY_1:
			return "lot_residential"
		KEY_2:
			return "lot_commercial"
		KEY_3:
			return "lot_industrial"
		_:
			return ""


func _action_for_mouse(button: int) -> String:
	match button:
		MOUSE_BUTTON_LEFT:
			return "place"
		MOUSE_BUTTON_RIGHT:
			return "select"
		MOUSE_BUTTON_WHEEL_UP:
			return "zoom_in"
		MOUSE_BUTTON_WHEEL_DOWN:
			return "zoom_out"
		MOUSE_BUTTON_WHEEL_LEFT:
			return "zoom_left"
		MOUSE_BUTTON_WHEEL_RIGHT:
			return "zoom_right"
		_:
			return ""


func _mouse_label(button: int) -> String:
	match button:
		MOUSE_BUTTON_LEFT:
			return "LMB"
		MOUSE_BUTTON_RIGHT:
			return "RMB"
		MOUSE_BUTTON_MIDDLE:
			return "MMB"
		MOUSE_BUTTON_WHEEL_UP:
			return "WheelUp"
		MOUSE_BUTTON_WHEEL_DOWN:
			return "WheelDown"
		MOUSE_BUTTON_WHEEL_LEFT:
			return "WheelLeft"
		MOUSE_BUTTON_WHEEL_RIGHT:
			return "WheelRight"
		_:
			return "Mouse%d" % button
