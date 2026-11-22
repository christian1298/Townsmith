extends Control

## Standalone observer form for the Q-101 30-minute playtest.
##
## This scene is deliberately independent of the world scene: it only times the
## route from `docs/66 - Playtest Kit.md`, records the observer's fields, and
## exports a structured JSON report. It does not launch, automate, or score the
## playtest, and it reads no game state or extension APIs.
##
## Run:
##   godot --path godot res://playtest_capture.tscn
##   godot --path godot res://playtest_capture.tscn -- --output /tmp/q101.json
##
## Headless structural smoke (no display, no human input):
##   godot --headless --path godot res://playtest_capture.tscn -- --smoke \
##       --output /tmp/q101_capture.json
##
## Keyboard: F5 start/pause, F6 next stage, F7 export, F8 copy summary.
##
## Export policy: a partial report is always writable. An empty tester name or
## zero completed stages adds a validation warning and sets `status` to
## `partial`; it never blocks the write. Only I/O or schema failures fail.

const SUITE := "q101_playtest_capture"
const SCHEMA_VERSION := 1
const ROUTE_ID := "q101"
const ROUTE_TOTAL_MINUTES := 30
const DEFAULT_OUTPUT_PREFIX := "user://q101_playtest_"
const ISSUE_ROWS := 3

const STAGES := [
	{"id": "orient", "label": "Orient", "start_min": 0, "end_min": 3},
	{"id": "starter_build", "label": "Starter build", "start_min": 3, "end_min": 8},
	{"id": "house", "label": "House", "start_min": 8, "end_min": 13},
	{"id": "industry", "label": "Industry", "start_min": 13, "end_min": 19},
	{"id": "utility", "label": "Utility", "start_min": 19, "end_min": 22},
	{"id": "service", "label": "Service", "start_min": 22, "end_min": 25},
	{"id": "save_load", "label": "Save/load", "start_min": 25, "end_min": 29},
	{"id": "ratings", "label": "Ratings", "start_min": 29, "end_min": 30},
]

const EXPERIENCE_OPTIONS := [
	{"id": "", "label": "Not recorded"},
	{"id": "none", "label": "None"},
	{"id": "some", "label": "Some (played before)"},
	{"id": "team_member", "label": "Team member"},
]

const AREA_OPTIONS := [
	{"id": "", "label": "Not recorded"},
	{"id": "orient", "label": "Orient"},
	{"id": "starter_build", "label": "Starter build"},
	{"id": "house", "label": "House"},
	{"id": "industry", "label": "Industry"},
	{"id": "utility", "label": "Utility"},
	{"id": "service", "label": "Service"},
	{"id": "save_load", "label": "Save/load"},
	{"id": "ratings", "label": "Ratings"},
	{"id": "other", "label": "Other"},
]

const SEVERITY_OPTIONS := [
	{"id": "low", "label": "Low"},
	{"id": "medium", "label": "Medium"},
	{"id": "high", "label": "High"},
]

const REQUIRED_EXPORT_FIELDS := [
	"schema",
	"suite",
	"status",
	"captured_at",
	"route",
	"tester",
	"timer",
	"stages_completed",
	"stages_total",
	"stages",
	"issues",
	"notes",
	"validation",
]

var smoke_mode := false
var output_path := ""

var _started := false
var _running := false
var _elapsed_s := 0.0
var _current_stage := 0
var _stage_complete: Array[bool] = []
var _stage_completed_at: Array[float] = []

var _stage_checks: Array[CheckBox] = []
var _issue_area: Array[OptionButton] = []
var _issue_severity: Array[OptionButton] = []
var _issue_description: Array[LineEdit] = []
var _issue_repro: Array[CheckBox] = []

var _tester_name: LineEdit
var _experience_select: OptionButton
var _tester_build: LineEdit
var _tester_seed: LineEdit
var _tester_hardware: LineEdit
var _tester_display: LineEdit
var _tester_fps_notes: LineEdit
var _notes: TextEdit

var _timer_label: Label
var _timer_state_label: Label
var _start_button: Button
var _status_label: Label
var _validation_label: Label


func _ready() -> void:
	var args := OS.get_cmdline_user_args()
	smoke_mode = args.has("--smoke")
	output_path = _arg(args, "--output", "")

	for _spec in STAGES:
		_stage_complete.append(false)
		_stage_completed_at.append(-1.0)

	_build_ui()
	_refresh_timer()
	_refresh_validation()

	if smoke_mode:
		set_process(false)
		_run_smoke()
		return
	set_process(true)


func _process(delta: float) -> void:
	if smoke_mode or not _running:
		return
	_advance_clock(delta)
	_refresh_timer()


func _input(event: InputEvent) -> void:
	if smoke_mode:
		return
	if not event is InputEventKey:
		return
	var key_event := event as InputEventKey
	if not key_event.pressed or key_event.echo:
		return
	var code := key_event.keycode
	if code == 0:
		code = key_event.physical_keycode
	match code:
		KEY_F5:
			_on_start_pause()
		KEY_F6:
			_advance_stage()
		KEY_F7:
			_on_export_pressed()
		KEY_F8:
			_on_copy_pressed()
		_:
			return
	get_viewport().set_input_as_handled()


func _arg(args: PackedStringArray, name: String, fallback: String) -> String:
	var index := args.find(name)
	if index >= 0 and index + 1 < args.size():
		return args[index + 1]
	return fallback


func _advance_clock(delta: float) -> void:
	if _running:
		_elapsed_s += delta


func _on_start_pause() -> void:
	if _running:
		_running = false
	elif _started:
		_running = true
	else:
		_started = true
		_running = true
	_refresh_timer()


func _on_reset_timer() -> void:
	_started = false
	_running = false
	_elapsed_s = 0.0
	_refresh_timer()


func _refresh_timer() -> void:
	if _timer_label != null:
		_timer_label.text = _format_time(_elapsed_s)
	if _timer_state_label != null:
		if _running:
			_timer_state_label.text = "Running · %d/%d stages complete" % [
				_completed_count(),
				STAGES.size(),
			]
			_timer_state_label.add_theme_color_override("font_color", Color(0.42, 0.78, 0.96))
		elif _started:
			_timer_state_label.text = "Paused · %d/%d stages complete" % [
				_completed_count(),
				STAGES.size(),
			]
			_timer_state_label.add_theme_color_override("font_color", Color(0.96, 0.75, 0.30))
		else:
			_timer_state_label.text = "Not started · tick a stage when the tester finishes it"
			_timer_state_label.add_theme_color_override("font_color", Color(0.66, 0.71, 0.78))
	if _start_button != null:
		if _running:
			_start_button.text = "Pause (F5)"
		elif _started:
			_start_button.text = "Resume (F5)"
		else:
			_start_button.text = "Start (F5)"


func _format_time(seconds: float) -> String:
	var total := maxi(int(seconds), 0)
	return "%02d:%02d" % [total / 60, total % 60]


func _completed_count() -> int:
	var count := 0
	for complete in _stage_complete:
		if complete:
			count += 1
	return count


func _on_stage_toggled(pressed: bool, index: int) -> void:
	if index < 0 or index >= _stage_complete.size():
		return
	_stage_complete[index] = pressed
	_stage_completed_at[index] = _elapsed_s if pressed else -1.0
	_refresh_timer()
	_refresh_validation()


func _advance_stage() -> void:
	if _current_stage >= STAGES.size():
		return
	var check := _stage_checks[_current_stage]
	if not check.button_pressed:
		check.button_pressed = true
	_current_stage = mini(_current_stage + 1, STAGES.size())
	_refresh_timer()
	_refresh_validation()


func _set_stage_complete(index: int, complete: bool) -> void:
	if index < 0 or index >= _stage_checks.size():
		return
	_stage_checks[index].button_pressed = complete
	if _stage_complete[index] != complete:
		_on_stage_toggled(complete, index)


func _on_field_changed(_text: String) -> void:
	_refresh_validation()


func _on_option_changed(_index: int) -> void:
	_refresh_validation()


func _on_notes_changed() -> void:
	_refresh_validation()


func _selected_id(select: OptionButton) -> String:
	if select == null or select.selected < 0:
		return ""
	return str(select.get_item_metadata(select.selected))


func _select_by_metadata(select: OptionButton, id: String) -> void:
	for index in range(select.item_count):
		if str(select.get_item_metadata(index)) == id:
			select.select(index)
			return


func _issue_values(row: int) -> Dictionary:
	return {
		"row": row + 1,
		"area": _selected_id(_issue_area[row]),
		"severity": _selected_id(_issue_severity[row]),
		"description": _issue_description[row].text.strip_edges(),
		"reproducible": _issue_repro[row].button_pressed,
	}


func _collect_issues() -> Array:
	var issues: Array = []
	for row in range(ISSUE_ROWS):
		var issue := _issue_values(row)
		if str(issue["description"]).is_empty():
			continue
		issues.append(issue)
	return issues


func _warnings_for(tester_name: String, completed: int) -> Array[String]:
	var warnings: Array[String] = []
	if tester_name.strip_edges().is_empty():
		warnings.append("tester_name_missing")
	if completed == 0:
		warnings.append("no_completed_stage")
	return warnings


func _join_warnings(warnings: Array[String]) -> String:
	var parts := PackedStringArray()
	for warning in warnings:
		parts.append(warning)
	return ", ".join(parts)


func _refresh_validation() -> void:
	if _validation_label == null or _tester_name == null:
		return
	var warnings := _warnings_for(_tester_name.text, _completed_count())
	if warnings.is_empty():
		_validation_label.text = "Report complete: tester name and at least one completed stage."
		_validation_label.add_theme_color_override("font_color", Color(0.36, 0.86, 0.55))
	else:
		_validation_label.text = "Partial report: %s. Export still writes the report." % _join_warnings(warnings)
		_validation_label.add_theme_color_override("font_color", Color(0.96, 0.75, 0.30))


func _build_payload() -> Dictionary:
	var completed := _completed_count()
	var issues := _collect_issues()
	var warnings := _warnings_for(_tester_name.text, completed)
	var complete := warnings.is_empty()
	var stages: Array = []
	for index in range(STAGES.size()):
		var spec: Dictionary = STAGES[index]
		stages.append({
			"index": index + 1,
			"id": str(spec["id"]),
			"label": str(spec["label"]),
			"target_start_min": int(spec["start_min"]),
			"target_end_min": int(spec["end_min"]),
			"complete": _stage_complete[index],
			"completed_at_s": snappedf(_stage_completed_at[index], 0.001) if _stage_complete[index] else null,
		})
	return {
		"schema": SCHEMA_VERSION,
		"suite": SUITE,
		"generated_by": "playtest_capture",
		"status": "ok" if complete else "partial",
		# The only wall-clock value in the payload. The smoke compares two runs
		# after removing this field; everything else must be deterministic.
		"captured_at": _utc_now(),
		"smoke": smoke_mode,
		"engine_version": str(Engine.get_version_info().get("string", "")),
		"route": {
			"id": ROUTE_ID,
			"title": "Q-101 30-minute vertical-slice route",
			"source": "docs/66 - Playtest Kit.md",
			"total_minutes": ROUTE_TOTAL_MINUTES,
			"stages_total": STAGES.size(),
		},
		"tester": {
			"name": _tester_name.text.strip_edges(),
			"experience": _selected_id(_experience_select),
			"build": _tester_build.text.strip_edges(),
			"seed": _tester_seed.text.strip_edges(),
			"hardware": _tester_hardware.text.strip_edges(),
			"display": _tester_display.text.strip_edges(),
			"fps_notes": _tester_fps_notes.text.strip_edges(),
		},
		"timer": {
			"started": _started,
			"running": _running,
			"elapsed_s": snappedf(_elapsed_s, 0.001),
			"route_minutes": ROUTE_TOTAL_MINUTES,
		},
		"stages_completed": completed,
		"stages_total": STAGES.size(),
		"stages": stages,
		"issues": issues,
		"notes": _notes.text.strip_edges(),
		"validation": {
			"complete": complete,
			"warnings": warnings,
			"policy": "partial reports are written; an empty tester name or zero completed stages warns only",
		},
		"limits": "Observer tool only. It times and records the Q-101 route; it does not automate play, score usability, or replace human findings.",
	}


func _utc_now() -> String:
	return Time.get_datetime_string_from_system(true) + "Z"


func _timestamped_output_path() -> String:
	var stamp := Time.get_datetime_string_from_system(true)
	stamp = stamp.replace("-", "").replace(":", "")
	return DEFAULT_OUTPUT_PREFIX + stamp + "Z.json"


func _absolute(path: String) -> String:
	if path.begins_with("user://") or path.begins_with("res://"):
		return ProjectSettings.globalize_path(path)
	return path


func _write_json(path: String, data: Dictionary) -> bool:
	var absolute := _absolute(path)
	var directory_error := DirAccess.make_dir_recursive_absolute(absolute.get_base_dir())
	if directory_error != OK and directory_error != ERR_ALREADY_EXISTS:
		push_error("Could not create playtest capture directory: " + absolute.get_base_dir())
		return false
	var file := FileAccess.open(absolute, FileAccess.WRITE)
	if file == null:
		push_error("Could not write playtest capture JSON: %s (%s)" % [
			absolute,
			error_string(FileAccess.get_open_error()),
		])
		return false
	file.store_string(JSON.stringify(data, "  "))
	file.close()
	return true


func _validate_export_file(path: String, payload: Dictionary) -> String:
	var text := FileAccess.get_file_as_string(_absolute(path))
	if text.is_empty():
		return "empty"
	var parsed: Variant = JSON.parse_string(text)
	if typeof(parsed) != TYPE_DICTIONARY:
		return "parse"
	var data: Dictionary = parsed
	for field in REQUIRED_EXPORT_FIELDS:
		if not data.has(field):
			return "missing_" + str(field)
	var captured := str(data.get("captured_at", ""))
	if captured.length() < 20 or not captured.contains("T") or not captured.ends_with("Z"):
		return "captured_at"
	if str(data.get("suite", "")) != SUITE:
		return "suite"
	if int(data.get("schema", -1)) != SCHEMA_VERSION:
		return "schema"
	if str(data.get("status", "")) != str(payload.get("status", "")):
		return "status"
	if int(data.get("stages_total", -1)) != STAGES.size():
		return "stages_total"
	var stages_value: Variant = data.get("stages", [])
	if typeof(stages_value) != TYPE_ARRAY:
		return "stages_type"
	var stages: Array = stages_value
	if stages.size() != STAGES.size():
		return "stages_count"
	for index in range(STAGES.size()):
		var stage_value: Variant = stages[index]
		if typeof(stage_value) != TYPE_DICTIONARY:
			return "stage_type_%d" % index
		var stage: Dictionary = stage_value
		if str(stage.get("id", "")) != str((STAGES[index] as Dictionary)["id"]):
			return "stage_id_%d" % index
		if bool(stage.get("complete", false)) != _stage_complete[index]:
			return "stage_complete_%d" % index
	if int(data.get("stages_completed", -1)) != _completed_count():
		return "stages_completed"
	var issues_value: Variant = data.get("issues", [])
	if typeof(issues_value) != TYPE_ARRAY:
		return "issues_type"
	if (issues_value as Array).size() != _collect_issues().size():
		return "issues_count"
	var timer_value: Variant = data.get("timer", {})
	if typeof(timer_value) != TYPE_DICTIONARY:
		return "timer_type"
	var timer: Dictionary = timer_value
	if absf(float(timer.get("elapsed_s", -1.0)) - _elapsed_s) > 0.001:
		return "timer_elapsed"
	var validation_value: Variant = data.get("validation", {})
	if typeof(validation_value) != TYPE_DICTIONARY:
		return "validation_type"
	return ""


func _core_payload(payload: Dictionary) -> Dictionary:
	var core: Dictionary = payload.duplicate(true)
	core.erase("captured_at")
	return core


func _export_to(path: String) -> Dictionary:
	var target := path
	if target.is_empty():
		target = _timestamped_output_path()
	var payload := _build_payload()
	var problem := ""
	if _write_json(target, payload):
		problem = _validate_export_file(target, payload)
		if not problem.is_empty():
			push_error("Playtest capture export failed validation: " + problem)
	else:
		problem = "write"
	var exported := problem.is_empty()
	if _status_label != null:
		if exported:
			_status_label.text = "Exported %s (%s)" % [_absolute(target), str(payload.get("status", ""))]
			_status_label.add_theme_color_override("font_color", Color(0.36, 0.86, 0.55))
		else:
			_status_label.text = "Export failed: %s" % problem
			_status_label.add_theme_color_override("font_color", Color(0.96, 0.42, 0.38))
	return {
		"ok": exported,
		"path": _absolute(target),
		"payload": payload,
		"problem": problem,
	}


func _summary_text() -> String:
	var payload := _build_payload()
	var tester: Dictionary = payload["tester"]
	var timer: Dictionary = payload["timer"]
	var issues_value: Variant = payload["issues"]
	var issues: Array = issues_value if typeof(issues_value) == TYPE_ARRAY else []
	var validation: Dictionary = payload["validation"]
	var name := str(tester.get("name", ""))
	if name.is_empty():
		name = "not recorded"
	var lines: Array[String] = []
	lines.append("Q-101 playtest capture")
	lines.append("Tester: %s (%s)" % [name, str(tester.get("experience", ""))])
	lines.append("Build: %s  Seed: %s" % [
		str(tester.get("build", "")),
		str(tester.get("seed", "")),
	])
	lines.append("Elapsed: %s  Stages: %d/%d" % [
		_format_time(float(timer.get("elapsed_s", 0.0))),
		int(payload["stages_completed"]),
		int(payload["stages_total"]),
	])
	lines.append("Issues:")
	if issues.is_empty():
		lines.append("  - none recorded")
	for issue_value in issues:
		var issue: Dictionary = issue_value
		lines.append("  %d. [%s] [%s] %s (reproducible: %s)" % [
			int(issue.get("row", 0)),
			str(issue.get("severity", "")),
			str(issue.get("area", "")),
			str(issue.get("description", "")),
			"yes" if bool(issue.get("reproducible", false)) else "no",
		])
	var notes := str(payload.get("notes", ""))
	lines.append("Notes: " + ("-" if notes.is_empty() else notes))
	if not bool(validation.get("complete", false)):
		var warnings_value: Variant = validation.get("warnings", [])
		var warnings: Array = warnings_value if typeof(warnings_value) == TYPE_ARRAY else []
		var parts := PackedStringArray()
		for warning in warnings:
			parts.append(str(warning))
		lines.append("Warnings: " + ", ".join(parts))
	return "\n".join(lines)


func _on_export_pressed() -> void:
	var result := _export_to(output_path)
	_print_marker(
		"ok" if bool(result["ok"]) else "failed",
		str(result["path"]),
		result["payload"] as Dictionary,
		str(result["problem"])
	)


func _on_copy_pressed() -> void:
	var text := _summary_text()
	if DisplayServer.get_name() == "headless":
		if _status_label != null:
			_status_label.text = "Summary ready (%d chars); clipboard is disabled headless." % text.length()
		return
	DisplayServer.clipboard_set(text)
	if _status_label != null:
		_status_label.text = "Summary copied (%d chars)." % text.length()


func _print_marker(status: String, path: String, payload: Dictionary, problem: String = "") -> void:
	var stages_total := int(payload.get("stages_total", STAGES.size()))
	var completed := int(payload.get("stages_completed", _completed_count()))
	var issues_value: Variant = payload.get("issues", [])
	var issue_count := (issues_value as Array).size() if typeof(issues_value) == TYPE_ARRAY else 0
	var report := str(payload.get("status", "unknown"))
	var line := "PLAYTEST_CAPTURE status=%s stages=%d completed=%d issues=%d report=%s export=%s" % [
		status,
		stages_total,
		completed,
		issue_count,
		report,
		path,
	]
	if not problem.is_empty():
		line += " problem=" + problem
	print(line)


# --- UI -------------------------------------------------------------------

func _build_ui() -> void:
	set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)

	var background := ColorRect.new()
	background.color = Color(0.055, 0.065, 0.08)
	background.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	background.mouse_filter = Control.MOUSE_FILTER_IGNORE
	add_child(background)

	var margin := MarginContainer.new()
	margin.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	margin.add_theme_constant_override("margin_left", 34)
	margin.add_theme_constant_override("margin_top", 26)
	margin.add_theme_constant_override("margin_right", 34)
	margin.add_theme_constant_override("margin_bottom", 24)
	add_child(margin)

	var root := VBoxContainer.new()
	root.add_theme_constant_override("separation", 12)
	margin.add_child(root)

	var title := Label.new()
	title.text = "Q-101 playtest capture"
	title.add_theme_font_size_override("font_size", 32)
	root.add_child(title)

	var subtitle := Label.new()
	subtitle.text = (
		"Observer tool for the 30-minute route in docs/66 §2. "
		+ "F5 start/pause · F6 next stage · F7 export · F8 copy summary. "
		+ "This form does not play the game or score the tester."
	)
	subtitle.add_theme_color_override("font_color", Color(0.70, 0.75, 0.82))
	subtitle.add_theme_font_size_override("font_size", 15)
	subtitle.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	root.add_child(subtitle)

	var columns := HBoxContainer.new()
	columns.add_theme_constant_override("separation", 18)
	columns.size_flags_vertical = Control.SIZE_EXPAND_FILL
	root.add_child(columns)
	columns.add_child(_build_route_panel())
	columns.add_child(_build_capture_panel())

	var footer := HBoxContainer.new()
	footer.add_theme_constant_override("separation", 12)
	root.add_child(footer)

	var export_button := Button.new()
	export_button.text = "Export JSON (F7)"
	export_button.pressed.connect(_on_export_pressed)
	footer.add_child(export_button)

	var copy_button := Button.new()
	copy_button.text = "Copy summary (F8)"
	copy_button.pressed.connect(_on_copy_pressed)
	footer.add_child(copy_button)

	_status_label = Label.new()
	_status_label.text = "Ready. Export writes %s<timestamp>.json" % DEFAULT_OUTPUT_PREFIX
	_status_label.add_theme_color_override("font_color", Color(0.70, 0.75, 0.82))
	_status_label.add_theme_font_size_override("font_size", 14)
	_status_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	_status_label.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	footer.add_child(_status_label)


func _build_route_panel() -> Control:
	var scroll := ScrollContainer.new()
	scroll.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	scroll.size_flags_vertical = Control.SIZE_EXPAND_FILL
	scroll.horizontal_scroll_mode = ScrollContainer.SCROLL_MODE_DISABLED
	scroll.custom_minimum_size = Vector2(520.0, 0.0)

	var panel := PanelContainer.new()
	panel.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	panel.add_theme_stylebox_override("panel", _panel_style())
	scroll.add_child(panel)

	var box := VBoxContainer.new()
	box.add_theme_constant_override("separation", 10)
	panel.add_child(box)

	var timer_row := HBoxContainer.new()
	timer_row.add_theme_constant_override("separation", 14)
	box.add_child(timer_row)

	_timer_label = Label.new()
	_timer_label.text = "00:00"
	_timer_label.add_theme_font_size_override("font_size", 46)
	timer_row.add_child(_timer_label)

	var timer_detail := VBoxContainer.new()
	timer_detail.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	timer_detail.add_theme_constant_override("separation", 2)
	timer_row.add_child(timer_detail)

	var timer_caption := Label.new()
	timer_caption.text = "Elapsed route time (target 30:00)"
	timer_caption.add_theme_color_override("font_color", Color(0.66, 0.71, 0.78))
	timer_caption.add_theme_font_size_override("font_size", 13)
	timer_detail.add_child(timer_caption)

	_timer_state_label = Label.new()
	_timer_state_label.add_theme_font_size_override("font_size", 14)
	timer_detail.add_child(_timer_state_label)

	var controls := HBoxContainer.new()
	controls.add_theme_constant_override("separation", 8)
	box.add_child(controls)

	_start_button = Button.new()
	_start_button.text = "Start (F5)"
	_start_button.pressed.connect(_on_start_pause)
	controls.add_child(_start_button)

	var reset_button := Button.new()
	reset_button.text = "Reset timer"
	reset_button.pressed.connect(_on_reset_timer)
	controls.add_child(reset_button)

	var next_button := Button.new()
	next_button.text = "Next stage (F6)"
	next_button.pressed.connect(_advance_stage)
	controls.add_child(next_button)

	var separator := HSeparator.new()
	box.add_child(separator)

	var route_title := Label.new()
	route_title.text = "Route stages"
	route_title.add_theme_font_size_override("font_size", 20)
	box.add_child(route_title)

	var route_hint := Label.new()
	route_hint.text = "Tick a segment when the tester finishes it. Ticks record the current elapsed time."
	route_hint.add_theme_color_override("font_color", Color(0.66, 0.71, 0.78))
	route_hint.add_theme_font_size_override("font_size", 13)
	route_hint.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	box.add_child(route_hint)

	for index in range(STAGES.size()):
		var spec: Dictionary = STAGES[index]
		var check := CheckBox.new()
		check.text = "%d. %s  ·  %d–%d min" % [
			index + 1,
			str(spec["label"]),
			int(spec["start_min"]),
			int(spec["end_min"]),
		]
		check.add_theme_font_size_override("font_size", 16)
		check.toggled.connect(_on_stage_toggled.bind(index))
		box.add_child(check)
		_stage_checks.append(check)

	var notes_hint := Label.new()
	notes_hint.text = (
		"Tip: log the stuck moment even when the tester recovers. "
		+ "Reset clears the timer only; it does not clear stage ticks or fields."
	)
	notes_hint.add_theme_color_override("font_color", Color(0.55, 0.60, 0.68))
	notes_hint.add_theme_font_size_override("font_size", 12)
	notes_hint.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	box.add_child(notes_hint)
	return scroll


func _build_capture_panel() -> Control:
	var scroll := ScrollContainer.new()
	scroll.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	scroll.size_flags_vertical = Control.SIZE_EXPAND_FILL
	scroll.horizontal_scroll_mode = ScrollContainer.SCROLL_MODE_DISABLED

	var panel := PanelContainer.new()
	panel.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	panel.add_theme_stylebox_override("panel", _panel_style())
	scroll.add_child(panel)

	var box := VBoxContainer.new()
	box.add_theme_constant_override("separation", 10)
	panel.add_child(box)

	var meta_title := Label.new()
	meta_title.text = "Tester and build"
	meta_title.add_theme_font_size_override("font_size", 20)
	box.add_child(meta_title)

	var grid := GridContainer.new()
	grid.columns = 2
	grid.add_theme_constant_override("h_separation", 12)
	grid.add_theme_constant_override("v_separation", 6)
	box.add_child(grid)

	_tester_name = _add_field(grid, "Tester name / ID", "required for a complete report")
	var experience_label := Label.new()
	experience_label.text = "Prior experience"
	grid.add_child(experience_label)
	_experience_select = OptionButton.new()
	_populate_options(_experience_select, EXPERIENCE_OPTIONS)
	_experience_select.item_selected.connect(_on_option_changed)
	grid.add_child(_experience_select)
	_tester_build = _add_field(grid, "Build commit", "git rev-parse --short HEAD")
	_tester_seed = _add_field(grid, "World seed", "2026")
	_tester_hardware = _add_field(grid, "Hardware", "CPU / GPU / RAM")
	_tester_display = _add_field(grid, "Display", "resolution / window mode / vsync")
	_tester_fps_notes = _add_field(grid, "FPS notes", "hitches, stalls, placement count")

	var separator := HSeparator.new()
	box.add_child(separator)

	var issues_title := Label.new()
	issues_title.text = "Top issues"
	issues_title.add_theme_font_size_override("font_size", 20)
	box.add_child(issues_title)

	var issues_hint := Label.new()
	issues_hint.text = (
		"Three rows, ranked by severity and frequency. "
		+ "A row with an empty description is ignored on export; high severity is valid."
	)
	issues_hint.add_theme_color_override("font_color", Color(0.66, 0.71, 0.78))
	issues_hint.add_theme_font_size_override("font_size", 13)
	issues_hint.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	box.add_child(issues_hint)

	for row in range(ISSUE_ROWS):
		box.add_child(_build_issue_row(row))

	var notes_title := Label.new()
	notes_title.text = "Free notes"
	notes_title.add_theme_font_size_override("font_size", 20)
	box.add_child(notes_title)

	_notes = TextEdit.new()
	_notes.custom_minimum_size = Vector2(0.0, 120.0)
	_notes.placeholder_text = "Quotes in the tester's own words, time lost, UI overlap, modal text."
	_notes.wrap_mode = TextEdit.LINE_WRAPPING_BOUNDARY
	_notes.text_changed.connect(_on_notes_changed)
	box.add_child(_notes)

	var separator_two := HSeparator.new()
	box.add_child(separator_two)

	_validation_label = Label.new()
	_validation_label.add_theme_font_size_override("font_size", 14)
	_validation_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	box.add_child(_validation_label)
	return scroll


func _build_issue_row(row: int) -> PanelContainer:
	var panel := PanelContainer.new()
	var style := StyleBoxFlat.new()
	style.bg_color = Color(0.075, 0.088, 0.108)
	style.border_color = Color(0.17, 0.20, 0.25)
	style.set_border_width_all(1)
	style.set_corner_radius_all(6)
	style.content_margin_left = 10.0
	style.content_margin_top = 8.0
	style.content_margin_right = 10.0
	style.content_margin_bottom = 8.0
	panel.add_theme_stylebox_override("panel", style)

	var box := VBoxContainer.new()
	box.add_theme_constant_override("separation", 6)
	panel.add_child(box)

	var top := HBoxContainer.new()
	top.add_theme_constant_override("separation", 8)
	box.add_child(top)

	var rank := Label.new()
	rank.text = "#%d" % (row + 1)
	rank.add_theme_font_size_override("font_size", 16)
	top.add_child(rank)

	var area := OptionButton.new()
	_populate_options(area, AREA_OPTIONS)
	area.item_selected.connect(_on_option_changed)
	area.custom_minimum_size = Vector2(150.0, 0.0)
	top.add_child(area)
	_issue_area.append(area)

	var severity := OptionButton.new()
	_populate_options(severity, SEVERITY_OPTIONS)
	severity.select(SEVERITY_OPTIONS.size() - 1)
	severity.item_selected.connect(_on_option_changed)
	severity.custom_minimum_size = Vector2(96.0, 0.0)
	top.add_child(severity)
	_issue_severity.append(severity)

	var reproducible := CheckBox.new()
	reproducible.text = "Reproducible"
	reproducible.add_theme_font_size_override("font_size", 13)
	reproducible.toggled.connect(_on_repro_toggled)
	top.add_child(reproducible)
	_issue_repro.append(reproducible)

	var description := LineEdit.new()
	description.placeholder_text = "One-line description (required for this row to export)"
	description.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	description.text_changed.connect(_on_field_changed)
	box.add_child(description)
	_issue_description.append(description)
	return panel


func _on_repro_toggled(_pressed: bool) -> void:
	_refresh_validation()


func _add_field(grid: GridContainer, label_text: String, placeholder: String) -> LineEdit:
	var label := Label.new()
	label.text = label_text
	grid.add_child(label)
	var edit := LineEdit.new()
	edit.placeholder_text = placeholder
	edit.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	edit.text_changed.connect(_on_field_changed)
	grid.add_child(edit)
	return edit


func _populate_options(select: OptionButton, options: Array) -> void:
	select.clear()
	for option_value in options:
		var option: Dictionary = option_value
		select.add_item(str(option["label"]))
		select.set_item_metadata(select.item_count - 1, str(option["id"]))
	select.select(0)


func _panel_style() -> StyleBoxFlat:
	var style := StyleBoxFlat.new()
	style.bg_color = Color(0.09, 0.105, 0.125)
	style.border_color = Color(0.20, 0.24, 0.30)
	style.set_border_width_all(1)
	style.set_corner_radius_all(8)
	style.content_margin_left = 16.0
	style.content_margin_top = 14.0
	style.content_margin_right = 16.0
	style.content_margin_bottom = 14.0
	return style


# --- Headless smoke -------------------------------------------------------

func _run_smoke() -> void:
	var status := "ok"
	var stage_ids: Array[String] = []
	for spec_value in STAGES:
		var spec: Dictionary = spec_value
		stage_ids.append(str(spec["id"]))

	if _stage_checks.size() != STAGES.size():
		_fail_smoke("stage_controls")
		return
	if stage_ids != [
		"orient",
		"starter_build",
		"house",
		"industry",
		"utility",
		"service",
		"save_load",
		"ratings",
	]:
		_fail_smoke("stage_ids")
		return
	if _started or _running or _elapsed_s != 0.0:
		_fail_smoke("timer_initial")
		return

	# Partial-report policy: empty tester name and no completed stage warn, but
	# the payload is still valid and writable.
	var partial := _build_payload()
	var partial_warnings: Array = (partial["validation"] as Dictionary)["warnings"]
	if str(partial.get("status", "")) != "partial":
		status = "partial_status"
	elif not partial_warnings.has("tester_name_missing"):
		status = "partial_name"
	elif not partial_warnings.has("no_completed_stage"):
		status = "partial_stage"

	# Timer transitions: start, advance, pause (frozen), resume, reset.
	if status == "ok":
		_on_start_pause()
		if not _running or not _started:
			status = "timer_start"
	if status == "ok":
		_advance_clock(90.0)
		if absf(_elapsed_s - 90.0) > 0.001:
			status = "timer_advance"
	if status == "ok":
		_on_start_pause()
		if _running:
			status = "timer_pause_state"
	if status == "ok":
		_advance_clock(10.0)
		if absf(_elapsed_s - 90.0) > 0.001:
			status = "timer_pause_elapsed"
	if status == "ok":
		_on_start_pause()
		_advance_clock(30.0)
		if absf(_elapsed_s - 120.0) > 0.001:
			status = "timer_resume"
	if status == "ok":
		_on_reset_timer()
		if _running or _started or _elapsed_s != 0.0:
			status = "timer_reset"

	# Deterministic fixture for the exported report.
	_tester_name.text = "Smoke Tester"
	_select_by_metadata(_experience_select, "some")
	_tester_build.text = "smoke-build-822c509"
	_tester_seed.text = "2026"
	_tester_hardware.text = "headless"
	_tester_display.text = "headless 1280x720"
	_tester_fps_notes.text = "No hitches observed in the smoke run."
	_notes.text = "Deterministic smoke capture; no human findings."

	if status == "ok":
		_on_start_pause()
		_advance_clock(10.0)
		_advance_stage()
		if not _stage_complete[0]:
			status = "stage_advance_0"
	if status == "ok":
		_advance_clock(20.0)
		_advance_stage()
		if not _stage_complete[1]:
			status = "stage_advance_1"
	if status == "ok":
		_advance_clock(90.0)
		for index in range(2, STAGES.size()):
			_set_stage_complete(index, true)
		if _completed_count() != STAGES.size():
			status = "stage_complete_count"

	# Issue validation: the blank row must be ignored; high severity is kept;
	# medium is kept; reproducible yes/no round-trips as a bool.
	_set_issue(0, "starter_build", "high", "", true)
	_set_issue(1, "starter_build", "high", "Placement ghost showed green over an occupied cell.", true)
	_set_issue(2, "save_load", "medium", "Load modal did not say what was restored.", false)
	var issues := _collect_issues()
	if status == "ok" and issues.size() != 2:
		status = "issue_count"
	if status == "ok" and int((issues[0] as Dictionary).get("row", 0)) != 2:
		status = "issue_empty_not_ignored"
	if status == "ok" and str((issues[0] as Dictionary).get("severity", "")) != "high":
		status = "issue_high"
	if status == "ok" and not bool((issues[0] as Dictionary).get("reproducible", false)):
		status = "issue_repro_yes"
	if status == "ok" and str((issues[1] as Dictionary).get("severity", "")) != "medium":
		status = "issue_medium"
	if status == "ok" and bool((issues[1] as Dictionary).get("reproducible", true)):
		status = "issue_repro_no"

	if status == "ok":
		_on_start_pause()
		if _running:
			status = "timer_final_pause"

	var result := _export_to(output_path)
	if status == "ok" and not bool(result["ok"]):
		status = "export_" + str(result["problem"])
	var exported_payload: Dictionary = result["payload"] as Dictionary

	if status == "ok":
		var summary := _summary_text()
		for needle in ["Q-101 playtest capture", "Smoke Tester", "Stages: 8/8", "[high]", "[medium]"]:
			if not summary.contains(needle):
				status = "summary_missing_" + needle
				break
	if status == "ok":
		var core_a := _core_payload(exported_payload)
		var core_b := _core_payload(_build_payload())
		if JSON.stringify(core_a) != JSON.stringify(core_b):
			status = "determinism"
	if status == "ok" and int(exported_payload.get("stages_completed", 0)) != STAGES.size():
		status = "export_completed"

	print("PLAYTEST_CAPTURE_STAGES " + ",".join(PackedStringArray(stage_ids)))
	print("PLAYTEST_CAPTURE_TIMER start=ok pause=ok resume=ok reset=ok elapsed_s=%.3f" % _elapsed_s)
	print("PLAYTEST_CAPTURE_ISSUES empty=ignored high=accepted count=%d" % issues.size())
	_print_marker("ok" if status == "ok" else "failed", str(result["path"]), exported_payload, "" if status == "ok" else status)
	if status != "ok":
		push_error("Playtest capture smoke failed: " + status)
	get_tree().quit(0 if status == "ok" else 1)


func _set_issue(row: int, area: String, severity: String, description: String, reproducible: bool) -> void:
	_select_by_metadata(_issue_area[row], area)
	_select_by_metadata(_issue_severity[row], severity)
	_issue_description[row].text = description
	_issue_repro[row].button_pressed = reproducible


func _fail_smoke(reason: String) -> void:
	print("PLAYTEST_CAPTURE status=failed stages=%d completed=%d issues=%d report=unknown export= reason=%s" % [
		_stage_checks.size(),
		_completed_count(),
		_collect_issues().size() if _issue_description.size() == ISSUE_ROWS else 0,
		reason,
	])
	push_error("Playtest capture smoke failed: " + reason)
	get_tree().quit(1)
