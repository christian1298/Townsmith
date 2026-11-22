extends Control

## Standalone developer editor for the district-policy registry.
##
## This page is deliberately outside the world scene and the in-game dashboard:
## it owns one `TownsmithCore` instance and only calls the data-only bridge CRUD
## (`district_policy_create/update/remove` and `district_policies_state`). No
## simulation, household, treasury, advisor, or world-save code is involved, and
## the registry is not part of the save envelope yet. See
## `docs/57 - Dashboard and Advisor.md` §4.2.
##
## Run:
##   godot --path godot res://policy_editor.tscn
##   godot --headless --path godot res://policy_editor.tscn -- --smoke
##
## Headless smoke: creates two deterministic policies, lists the registry,
## updates one, rejects an out-of-range radius, removes both by id, and requires
## a clean empty state with a repeatable checksum. Final marker:
##   POLICY_EDITOR status=ok policies=0 checksum=<16 hex>
##
## Strings are English-only developer text: this is a pre-dashboard tool, and
## the shared `base.ui.policy.*` localization slice does not exist yet. Policy
## names, ids, allowlist entries, and bridge error codes are data and stay
## verbatim. All numbers in the list and status line go through
## `WorldI18n.format_int`/`format_float` so the tool already follows the EN/DE
## number rules from `docs/16 - Localization.md`.
##
## The bridge keeps one optional registry per `TownsmithCore`. "New session"
## drops the instance (and therefore the registry); it does not save or load.

const WorldI18n = preload("res://world_i18n.gd")

const SUITE := "policy_editor"
const DEFAULT_SEED := 2026
const DEFAULT_RADIUS_M := 64.0
const DEFAULT_TAX := 1.0
const DEFAULT_FUNDING := 1.0
const DEFAULT_NAME_PREFIX := "District"

# Mirrors `crates/core_city/src/policy.rs`. The SpinBoxes clamp the form, but
# the bridge stays authoritative and its error codes are what the tool shows.
const MIN_RADIUS_M := 16.0
const MAX_RADIUS_M := 4096.0
const MIN_TAX := 0.25
const MAX_TAX := 4.0
const MAX_FUNDING := 2.0
const MAX_WORLD_COORD_M := 1000000.0

# Deterministic, overlap-free defaults around the origin. Index 0 is the
# origin; the rest sit on a 160 m square ring, so radius-64 defaults never
# overlap and repeated creates are reproducible.
const DEFAULT_CENTERS := [
	Vector2(0.0, 0.0),
	Vector2(160.0, 0.0),
	Vector2(0.0, 160.0),
	Vector2(-160.0, 0.0),
	Vector2(0.0, -160.0),
	Vector2(160.0, 160.0),
	Vector2(-160.0, -160.0),
	Vector2(160.0, -160.0),
	Vector2(-160.0, 160.0),
]

const OK_COLOR := Color(0.36, 0.86, 0.55)
const ERROR_COLOR := Color(0.96, 0.42, 0.38)
const MUTED_COLOR := Color(0.66, 0.71, 0.78)
const TEXT_COLOR := Color(0.88, 0.91, 0.95)

var smoke_mode := false

var _i18n
var _core = null
var _seed := DEFAULT_SEED
var _policies: Array = []
var _checksum := ""
var _selected_id := 0

var _seed_edit: LineEdit
var _state_label: Label
var _message_label: Label
var _list: ItemList
var _form_title: Label

var _name_edit: LineEdit
var _center_x: SpinBox
var _center_z: SpinBox
var _radius: SpinBox
var _tax: SpinBox
var _funding: SpinBox
var _construction: CheckBox
var _allowlist_edit: LineEdit
var _remove_id: SpinBox


func _ready() -> void:
	var args := OS.get_cmdline_user_args()
	smoke_mode = args.has("--smoke")
	_i18n = WorldI18n.new()
	_build_ui()
	_reset_session(false)
	if smoke_mode:
		_run_smoke()


# --- Bridge ----------------------------------------------------------------

func _reset_session(reset_seed: bool) -> void:
	_core = TownsmithCore.new()
	_policies = []
	_checksum = ""
	_selected_id = 0
	if reset_seed:
		_seed = DEFAULT_SEED
	_refresh()


func _refresh() -> void:
	if _core == null:
		_policies = []
		_checksum = ""
		_rebuild_list()
		return
	var result: Dictionary = _core.district_policies_state(_seed)
	if not bool(result.get("ok", false)):
		_policies = []
		_checksum = ""
		_rebuild_list()
		_apply_bridge_result("List", result)
		return
	var policies_value = result.get("policies", [])
	_policies = policies_value if policies_value is Array else []
	_checksum = str(result.get("checksum", ""))
	if _find_policy(_selected_id).is_empty():
		var had_selection := _selected_id != 0
		_selected_id = 0
		if had_selection and _form_title != null:
			_form_title.text = "No policy selected"
	_rebuild_list()


func _create_from_form() -> Dictionary:
	if _core == null:
		_set_message("Create failed: the bridge is not loaded.", true)
		return {"ok": false, "error_code": "no_core", "error": "bridge not loaded"}
	var request := _form_request()
	var result: Dictionary = _core.district_policy_create(
		_seed,
		str(request["name"]),
		float(request["center_x"]),
		float(request["center_z"]),
		float(request["radius_m"]),
		float(request["tax_multiplier"]),
		float(request["service_funding"]),
		bool(request["construction_allowed"]),
		request["allowlist"]
	)
	if _apply_bridge_result("Create", result):
		var policy: Dictionary = result.get("policy", {})
		_selected_id = int(policy.get("id", 0))
		_refresh()
	return result


func _update_selected() -> Dictionary:
	if _core == null:
		_set_message("Save failed: the bridge is not loaded.", true)
		return {"ok": false, "error_code": "no_core", "error": "bridge not loaded"}
	if _selected_id == 0:
		_set_message("Select a policy in the list before saving changes.", true)
		return {"ok": false, "error_code": "no_selection", "error": "no policy selected"}
	var request := _form_request()
	var result: Dictionary = _core.district_policy_update(
		_seed,
		_selected_id,
		str(request["name"]),
		float(request["center_x"]),
		float(request["center_z"]),
		float(request["radius_m"]),
		float(request["tax_multiplier"]),
		float(request["service_funding"]),
		bool(request["construction_allowed"]),
		request["allowlist"]
	)
	_apply_bridge_result("Save", result)
	_refresh()
	return result


func _remove_by_id(id: int) -> Dictionary:
	if _core == null:
		_set_message("Remove failed: the bridge is not loaded.", true)
		return {"ok": false, "error_code": "no_core", "error": "bridge not loaded"}
	var result: Dictionary = _core.district_policy_remove(_seed, id)
	if _apply_bridge_result("Remove #%s" % _i18n.format_int(id), result):
		if _selected_id == id:
			_selected_id = 0
			if _form_title != null:
				_form_title.text = "No policy selected"
	_refresh()
	return result


func _apply_bridge_result(context: String, result: Dictionary) -> bool:
	if bool(result.get("ok", false)):
		_set_message("%s: ok" % context, false)
		return true
	var code := str(result.get("error_code", "error"))
	var message := str(result.get("error", ""))
	_set_message("%s failed: %s (%s)" % [context, code, message], true)
	return false


func _form_request() -> Dictionary:
	return {
		"name": _name_edit.text,
		"center_x": _center_x.value,
		"center_z": _center_z.value,
		"radius_m": _radius.value,
		"tax_multiplier": _tax.value,
		"service_funding": _funding.value,
		"construction_allowed": _construction.button_pressed,
		"allowlist": _allowlist_from_text(_allowlist_edit.text),
	}


func _allowlist_from_text(text: String) -> PackedStringArray:
	var entries := PackedStringArray()
	for part in text.split(","):
		var trimmed := part.strip_edges()
		if not trimmed.is_empty():
			entries.append(trimmed)
	return entries


# --- Editor state ----------------------------------------------------------

func _find_policy(id: int) -> Dictionary:
	for policy in _policies:
		if int(policy.get("id", 0)) == id:
			return policy
	return {}


func _center_of(policy: Dictionary) -> Vector2:
	var center = policy.get("center", [])
	if center is PackedFloat32Array and center.size() >= 2:
		return Vector2(center[0], center[1])
	if center is Array and center.size() >= 2:
		return Vector2(float(center[0]), float(center[1]))
	return Vector2.ZERO


func _circles_overlap(first: Dictionary, second: Dictionary) -> bool:
	var offset := _center_of(first) - _center_of(second)
	var radii := float(first.get("radius_m", 0.0)) + float(second.get("radius_m", 0.0))
	return offset.length_squared() <= radii * radii


## Pairwise overlap query for the list: id -> [other ids], ascending.
## The bridge does not reject overlaps; this is a display-only local helper.
func _overlap_map() -> Dictionary:
	var overlaps := {}
	for policy in _policies:
		overlaps[int(policy.get("id", 0))] = []
	for first_index in range(_policies.size()):
		for second_index in range(first_index + 1, _policies.size()):
			var first: Dictionary = _policies[first_index]
			var second: Dictionary = _policies[second_index]
			if not _circles_overlap(first, second):
				continue
			var first_id := int(first.get("id", 0))
			var second_id := int(second.get("id", 0))
			var first_list: Array = overlaps.get(first_id, [])
			first_list.append(second_id)
			overlaps[first_id] = first_list
			var second_list: Array = overlaps.get(second_id, [])
			second_list.append(first_id)
			overlaps[second_id] = second_list
	return overlaps


func _count_overlapping() -> int:
	var overlaps := _overlap_map()
	var count := 0
	for id in overlaps:
		if not (overlaps[id] as Array).is_empty():
			count += 1
	return count


func _row_text(policy: Dictionary) -> String:
	var center := _center_of(policy)
	var allowlist = policy.get("blueprint_allowlist", [])
	var allowlist_count := 0
	if allowlist is Array or allowlist is PackedStringArray:
		allowlist_count = allowlist.size()
	return "#%s  %s  ·  center (%s, %s)  ·  r %s m  ·  tax ×%s  ·  funding ×%s  ·  construction %s  ·  allowlist %s" % [
		_i18n.format_int(int(policy.get("id", 0))),
		str(policy.get("name", "")),
		_i18n.format_float(center.x, 0),
		_i18n.format_float(center.y, 0),
		_i18n.format_float(float(policy.get("radius_m", 0.0)), 1),
		_i18n.format_float(float(policy.get("tax_multiplier", 0.0)), 2),
		_i18n.format_float(float(policy.get("service_funding", 0.0)), 2),
		"on" if bool(policy.get("construction_allowed", false)) else "off",
		_i18n.format_int(allowlist_count),
	]


func _rebuild_list() -> void:
	if _list == null:
		return
	_list.clear()
	var overlaps := _overlap_map()
	for policy in _policies:
		var id := int(policy.get("id", 0))
		var suffix := ""
		var other_ids: Array = overlaps.get(id, [])
		if not other_ids.is_empty():
			var parts := PackedStringArray()
			for other_id in other_ids:
				parts.append("#%s" % _i18n.format_int(int(other_id)))
			suffix = "  ·  overlaps " + ", ".join(parts)
		_list.add_item(_row_text(policy) + suffix)
		_list.set_item_metadata(_list.item_count - 1, id)
		if id == _selected_id:
			_list.select(_list.item_count - 1)
	_refresh_state_label()


func _refresh_state_label() -> void:
	if _state_label == null:
		return
	var text := "Seed %s · %s policies · checksum %s" % [
		_i18n.format_int(_seed),
		_i18n.format_int(_policies.size()),
		_checksum if not _checksum.is_empty() else "—",
	]
	var overlapping := _count_overlapping()
	if overlapping > 0:
		text += " · %s overlapping" % _i18n.format_int(overlapping)
	_state_label.text = text


func _set_message(text: String, is_error: bool) -> void:
	if _message_label == null:
		return
	_message_label.text = text
	_message_label.add_theme_color_override("font_color", ERROR_COLOR if is_error else OK_COLOR)


func _select_policy(id: int) -> void:
	var policy := _find_policy(id)
	if policy.is_empty():
		_selected_id = 0
		_form_title.text = "No policy selected"
		return
	_selected_id = id
	_load_form(policy)
	_form_title.text = "Editing #%s" % _i18n.format_int(id)
	for index in range(_list.item_count):
		if int(_list.get_item_metadata(index)) == id:
			_list.select(index)
			break


func _load_form(policy: Dictionary) -> void:
	var center := _center_of(policy)
	_name_edit.text = str(policy.get("name", ""))
	_center_x.set_value_no_signal(center.x)
	_center_z.set_value_no_signal(center.y)
	_radius.set_value_no_signal(float(policy.get("radius_m", DEFAULT_RADIUS_M)))
	_tax.set_value_no_signal(float(policy.get("tax_multiplier", DEFAULT_TAX)))
	_funding.set_value_no_signal(float(policy.get("service_funding", DEFAULT_FUNDING)))
	_construction.button_pressed = bool(policy.get("construction_allowed", true))
	var entries := PackedStringArray()
	var allowlist = policy.get("blueprint_allowlist", [])
	for entry in allowlist:
		entries.append(str(entry))
	_allowlist_edit.text = ", ".join(entries)


func _set_form(
	name: String,
	center_x: float,
	center_z: float,
	radius_m: float,
	tax: float,
	funding: float,
	construction: bool,
	allowlist: String
) -> void:
	_name_edit.text = name
	_center_x.set_value_no_signal(center_x)
	_center_z.set_value_no_signal(center_z)
	_radius.set_value_no_signal(radius_m)
	_tax.set_value_no_signal(tax)
	_funding.set_value_no_signal(funding)
	_construction.button_pressed = construction
	_allowlist_edit.text = allowlist


func _default_center(index: int) -> Vector2:
	return DEFAULT_CENTERS[index % DEFAULT_CENTERS.size()]


# --- UI callbacks ----------------------------------------------------------

func _on_refresh_pressed() -> void:
	_refresh()
	if _message_label != null and _message_label.text.is_empty():
		_set_message("List refreshed.", false)


func _on_new_session_pressed() -> void:
	_reset_session(false)
	_set_message("New empty registry session for seed %s." % _i18n.format_int(_seed), false)


func _on_apply_seed_pressed() -> void:
	var text := _seed_edit.text.strip_edges()
	if not text.is_valid_int() or int(text) < 0:
		_set_message("Seed must be a non-negative integer; the current seed is unchanged.", true)
		return
	_seed = int(text)
	_reset_session(false)
	_set_message("New empty registry session for seed %s." % _i18n.format_int(_seed), false)


func _on_create_default_pressed() -> void:
	var index := _policies.size()
	var center := _default_center(index)
	_set_form(
		"%s %s" % [DEFAULT_NAME_PREFIX, _i18n.format_int(index + 1)],
		center.x,
		center.y,
		DEFAULT_RADIUS_M,
		DEFAULT_TAX,
		DEFAULT_FUNDING,
		true,
		""
	)
	_create_from_form()


func _on_create_pressed() -> void:
	_create_from_form()


func _on_save_pressed() -> void:
	_update_selected()


func _on_remove_pressed() -> void:
	_remove_by_id(int(_remove_id.value))


func _on_list_selected(index: int) -> void:
	_select_policy(int(_list.get_item_metadata(index)))


# --- UI construction -------------------------------------------------------

func _build_ui() -> void:
	set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)

	var background := ColorRect.new()
	background.color = Color(0.055, 0.065, 0.08)
	background.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	background.mouse_filter = Control.MOUSE_FILTER_IGNORE
	add_child(background)

	var margin := MarginContainer.new()
	margin.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	margin.add_theme_constant_override("margin_left", 28)
	margin.add_theme_constant_override("margin_top", 22)
	margin.add_theme_constant_override("margin_right", 28)
	margin.add_theme_constant_override("margin_bottom", 20)
	add_child(margin)

	var root := VBoxContainer.new()
	root.add_theme_constant_override("separation", 10)
	margin.add_child(root)

	var title := Label.new()
	title.text = "District policy editor"
	title.add_theme_font_size_override("font_size", 30)
	root.add_child(title)

	var subtitle := Label.new()
	subtitle.text = (
		"Developer tool over the data-only bridge registry (docs/57 §4.2). "
		+ "Nothing in the game reads these policies yet and they are not saved. "
		+ "Edit the in-memory registry for one seed; overlap is reported, not rejected."
	)
	subtitle.add_theme_color_override("font_color", MUTED_COLOR)
	subtitle.add_theme_font_size_override("font_size", 14)
	subtitle.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	root.add_child(subtitle)

	root.add_child(_build_toolbar())

	var columns := HBoxContainer.new()
	columns.add_theme_constant_override("separation", 16)
	columns.size_flags_vertical = Control.SIZE_EXPAND_FILL
	root.add_child(columns)
	columns.add_child(_build_list_panel())
	columns.add_child(_build_form_panel())

	var footer := HBoxContainer.new()
	footer.add_theme_constant_override("separation", 12)
	root.add_child(footer)

	var hint := Label.new()
	hint.text = "Bridge errors are shown by code and never abort the tool."
	hint.add_theme_color_override("font_color", MUTED_COLOR)
	hint.add_theme_font_size_override("font_size", 13)
	footer.add_child(hint)

	_message_label = Label.new()
	_message_label.text = "Ready."
	_message_label.add_theme_font_size_override("font_size", 13)
	_message_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	_message_label.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	footer.add_child(_message_label)


func _build_toolbar() -> Control:
	var panel := PanelContainer.new()
	panel.add_theme_stylebox_override("panel", _panel_style())

	var row := HBoxContainer.new()
	row.add_theme_constant_override("separation", 8)
	panel.add_child(row)

	var seed_label := Label.new()
	seed_label.text = "Seed"
	row.add_child(seed_label)

	_seed_edit = LineEdit.new()
	_seed_edit.text = str(DEFAULT_SEED)
	_seed_edit.custom_minimum_size = Vector2(110.0, 0.0)
	row.add_child(_seed_edit)

	var apply_seed := Button.new()
	apply_seed.text = "Apply seed (new session)"
	apply_seed.pressed.connect(_on_apply_seed_pressed)
	row.add_child(apply_seed)

	var refresh_button := Button.new()
	refresh_button.text = "Refresh"
	refresh_button.pressed.connect(_on_refresh_pressed)
	row.add_child(refresh_button)

	var new_session := Button.new()
	new_session.text = "New session"
	new_session.pressed.connect(_on_new_session_pressed)
	row.add_child(new_session)

	_state_label = Label.new()
	_state_label.add_theme_color_override("font_color", TEXT_COLOR)
	_state_label.add_theme_font_size_override("font_size", 13)
	_state_label.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	_state_label.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT
	row.add_child(_state_label)
	return panel


func _build_list_panel() -> Control:
	var panel := PanelContainer.new()
	panel.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	panel.size_flags_vertical = Control.SIZE_EXPAND_FILL
	panel.custom_minimum_size = Vector2(560.0, 0.0)
	panel.add_theme_stylebox_override("panel", _panel_style())

	var box := VBoxContainer.new()
	box.add_theme_constant_override("separation", 8)
	panel.add_child(box)

	var heading := Label.new()
	heading.text = "Policies (ascending id)"
	heading.add_theme_font_size_override("font_size", 18)
	box.add_child(heading)

	var hint := Label.new()
	hint.text = "Select a row to load it into the form. Ids are assigned by the registry."
	hint.add_theme_color_override("font_color", MUTED_COLOR)
	hint.add_theme_font_size_override("font_size", 12)
	hint.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	box.add_child(hint)

	_list = ItemList.new()
	_list.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	_list.size_flags_vertical = Control.SIZE_EXPAND_FILL
	_list.item_selected.connect(_on_list_selected)
	box.add_child(_list)

	var remove_row := HBoxContainer.new()
	remove_row.add_theme_constant_override("separation", 8)
	box.add_child(remove_row)

	var remove_label := Label.new()
	remove_label.text = "Remove id"
	remove_row.add_child(remove_label)

	_remove_id = SpinBox.new()
	_remove_id.min_value = 1.0
	_remove_id.max_value = 1000000.0
	_remove_id.step = 1.0
	_remove_id.value = 1.0
	_remove_id.custom_minimum_size = Vector2(110.0, 0.0)
	remove_row.add_child(_remove_id)

	var remove_button := Button.new()
	remove_button.text = "Remove"
	remove_button.pressed.connect(_on_remove_pressed)
	remove_row.add_child(remove_button)
	return panel


func _build_form_panel() -> Control:
	var scroll := ScrollContainer.new()
	scroll.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	scroll.size_flags_vertical = Control.SIZE_EXPAND_FILL
	scroll.horizontal_scroll_mode = ScrollContainer.SCROLL_MODE_DISABLED

	var panel := PanelContainer.new()
	panel.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	panel.add_theme_stylebox_override("panel", _panel_style())
	scroll.add_child(panel)

	var box := VBoxContainer.new()
	box.add_theme_constant_override("separation", 8)
	panel.add_child(box)

	_form_title = Label.new()
	_form_title.text = "New policy"
	_form_title.add_theme_font_size_override("font_size", 18)
	box.add_child(_form_title)

	var grid := GridContainer.new()
	grid.columns = 2
	grid.add_theme_constant_override("h_separation", 10)
	grid.add_theme_constant_override("v_separation", 6)
	box.add_child(grid)

	var name_label := Label.new()
	name_label.text = "Name"
	grid.add_child(name_label)
	_name_edit = LineEdit.new()
	_name_edit.placeholder_text = "1–64 trimmed characters"
	_name_edit.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	grid.add_child(_name_edit)

	_center_x = _add_spin(grid, "Center X (m)", -MAX_WORLD_COORD_M, MAX_WORLD_COORD_M, 1.0, 0.0)
	_center_z = _add_spin(grid, "Center Z (m)", -MAX_WORLD_COORD_M, MAX_WORLD_COORD_M, 1.0, 0.0)
	_radius = _add_spin(grid, "Radius (m)", MIN_RADIUS_M, MAX_RADIUS_M, 1.0, DEFAULT_RADIUS_M)
	_tax = _add_spin(grid, "Tax multiplier", MIN_TAX, MAX_TAX, 0.05, DEFAULT_TAX)
	_funding = _add_spin(grid, "Service funding", 0.0, MAX_FUNDING, 0.05, DEFAULT_FUNDING)

	var construction_label := Label.new()
	construction_label.text = "Construction"
	grid.add_child(construction_label)
	_construction = CheckBox.new()
	_construction.text = "Allowed inside the circle"
	_construction.button_pressed = true
	grid.add_child(_construction)

	var allowlist_label := Label.new()
	allowlist_label.text = "Allowlist"
	grid.add_child(allowlist_label)
	_allowlist_edit = LineEdit.new()
	_allowlist_edit.placeholder_text = "base:workshop, base:house"
	_allowlist_edit.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	grid.add_child(_allowlist_edit)

	var bounds := Label.new()
	bounds.text = (
		"Bridge bounds: radius 16–4,096 m · tax 0.25–4.0 · funding 0–2.0 · "
		+ "name 1–64 chars · allowlist ≤256 namespaced ids. "
		+ "The form accepts out-of-range numbers so the bridge can reject them by code."
	)
	bounds.add_theme_color_override("font_color", MUTED_COLOR)
	bounds.add_theme_font_size_override("font_size", 12)
	bounds.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	box.add_child(bounds)

	var actions := HBoxContainer.new()
	actions.add_theme_constant_override("separation", 8)
	box.add_child(actions)

	var create_default := Button.new()
	create_default.text = "Create default"
	create_default.tooltip_text = "Deterministic radius-64 policy on a 160 m ring around the origin."
	create_default.pressed.connect(_on_create_default_pressed)
	actions.add_child(create_default)

	var create_button := Button.new()
	create_button.text = "Create from fields"
	create_button.pressed.connect(_on_create_pressed)
	actions.add_child(create_button)

	var save_button := Button.new()
	save_button.text = "Save changes"
	save_button.pressed.connect(_on_save_pressed)
	actions.add_child(save_button)

	var hint := Label.new()
	hint.text = (
		"Create assigns the next id. Save replaces every mutable field of the "
		+ "selected id. Overlapping circles are allowed; the list flags them."
	)
	hint.add_theme_color_override("font_color", MUTED_COLOR)
	hint.add_theme_font_size_override("font_size", 12)
	hint.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	box.add_child(hint)
	return scroll


func _add_spin(
	grid: GridContainer,
	label_text: String,
	min_value: float,
	max_value: float,
	step: float,
	value: float
) -> SpinBox:
	var label := Label.new()
	label.text = label_text
	grid.add_child(label)
	var spin := SpinBox.new()
	spin.min_value = min_value
	spin.max_value = max_value
	spin.step = step
	spin.value = value
	# Out-of-range entry is deliberately allowed: this is a bridge-validation
	# harness, and the core must reject the value with a stable error code
	# instead of the widget silently repairing it.
	spin.allow_lesser = true
	spin.allow_greater = true
	spin.custom_minimum_size = Vector2(150.0, 0.0)
	grid.add_child(spin)
	return spin


func _panel_style() -> StyleBoxFlat:
	var style := StyleBoxFlat.new()
	style.bg_color = Color(0.09, 0.105, 0.125)
	style.border_color = Color(0.20, 0.24, 0.30)
	style.set_border_width_all(1)
	style.set_corner_radius_all(8)
	style.content_margin_left = 14.0
	style.content_margin_top = 12.0
	style.content_margin_right = 14.0
	style.content_margin_bottom = 12.0
	return style


# --- Headless smoke --------------------------------------------------------

func _run_smoke() -> void:
	var seed := DEFAULT_SEED
	_seed = seed
	_reset_session(false)

	var status := "ok"
	var created_checksum := ""
	var updated_checksum := ""
	var final_checksum := ""
	var overlap_touching := false
	var overlap_apart := false
	var rejection_count := 0

	if _policies.size() != 0:
		status = "initial_count"
	if _checksum != "0000000000000000":
		status = "initial_checksum"
	if _list.item_count != 0:
		status = "initial_list"

	# Invalid create: a blank name is rejected by the bridge and the failure is
	# rendered in the message line (error colour + code), with no registry left
	# behind and the zero checksum unchanged.
	_set_form("", 0.0, 0.0, DEFAULT_RADIUS_M, DEFAULT_TAX, DEFAULT_FUNDING, true, "")
	var invalid_create := _create_from_form()
	if status == "ok" and bool(invalid_create.get("ok", true)):
		status = "create_invalid_accepted"
	if status == "ok" and str(invalid_create.get("error_code", "")) != "empty_name":
		status = "create_invalid_code"
	if status == "ok" and _policies.size() != 0:
		status = "create_invalid_count"
	if status == "ok" and _checksum != "0000000000000000":
		status = "create_invalid_checksum"
	if status == "ok" and not _message_label.text.contains("empty_name"):
		status = "create_invalid_message"
	if status == "ok" and _message_label.get_theme_color("font_color") != ERROR_COLOR:
		status = "create_invalid_color"

	# Create two policies through the same form/button path the UI uses. The
	# circles touch (distance 96 m, radii 64 + 32 m), which counts as overlap.
	_set_form("Smoke Riverside", 0.0, 0.0, 64.0, 1.25, 0.75, true, "base:workshop")
	var first_result := _create_from_form()
	var first_id := int((first_result.get("policy", {}) as Dictionary).get("id", 0))
	if status == "ok" and (not bool(first_result.get("ok", false)) or first_id != 1):
		status = "create_first"

	_set_form("Smoke Harbour", 96.0, 0.0, 32.0, 2.0, 1.5, false, "")
	var second_result := _create_from_form()
	var second_id := int((second_result.get("policy", {}) as Dictionary).get("id", 0))
	if status == "ok" and (not bool(second_result.get("ok", false)) or second_id != 2):
		status = "create_second"

	_refresh()
	if status == "ok" and (_policies.size() != 2 or _list.item_count != 2):
		status = "list_count"
	if status == "ok":
		var first_row := _list.get_item_text(0)
		var second_row := _list.get_item_text(1)
		if not first_row.contains("Smoke Riverside") or not second_row.contains("Smoke Harbour"):
			status = "list_rows"
		elif int(_list.get_item_metadata(0)) != first_id or int(_list.get_item_metadata(1)) != second_id:
			status = "list_ids"
		elif not first_row.contains("#1") or not second_row.contains("#2"):
			status = "list_row_ids"
	created_checksum = _checksum
	if status == "ok" and (created_checksum.is_empty() or created_checksum == "0000000000000000"):
		status = "list_checksum"

	var overlaps := _overlap_map()
	overlap_touching = (
		(overlaps.get(first_id, []) as Array).has(second_id)
		and (overlaps.get(second_id, []) as Array).has(first_id)
	)
	overlap_apart = _circles_overlap(
		{"center": [0.0, 0.0], "radius_m": 16.0},
		{"center": [1000.0, 0.0], "radius_m": 16.0}
	)
	if status == "ok" and not overlap_touching:
		status = "overlap_touching"
	if status == "ok" and overlap_apart:
		status = "overlap_apart"

	# Update the first policy: id stays, name/center/radius/tax/funding/
	# construction/allowlist all change.
	_select_policy(first_id)
	_set_form(
		"Smoke Riverside North",
		0.0,
		64.0,
		128.0,
		0.5,
		2.0,
		false,
		"base:workshop, base:house"
	)
	var update_result := _update_selected()
	if status == "ok" and not bool(update_result.get("ok", false)):
		status = "update_failed"
	var updated := _find_policy(first_id)
	if status == "ok" and updated.is_empty():
		status = "update_missing"
	if status == "ok":
		updated_checksum = _checksum
		var allowlist = updated.get("blueprint_allowlist", [])
		if updated_checksum == created_checksum:
			status = "update_checksum"
		elif int(updated.get("id", 0)) != first_id:
			status = "update_id"
		elif str(updated.get("name", "")) != "Smoke Riverside North":
			status = "update_name"
		elif absf(float(updated.get("radius_m", 0.0)) - 128.0) > 0.001:
			status = "update_radius"
		elif absf(float(updated.get("tax_multiplier", 0.0)) - 0.5) > 0.001:
			status = "update_tax"
		elif absf(float(updated.get("service_funding", 0.0)) - 2.0) > 0.001:
			status = "update_funding"
		elif bool(updated.get("construction_allowed", true)):
			status = "update_construction"
		elif allowlist.size() != 2 or not allowlist.has("base:workshop") or not allowlist.has("base:house"):
			status = "update_allowlist"

	# Invalid radius: the bridge returns invalid_radius and must not mutate the
	# registry, the checksum, or the stored record.
	_select_policy(first_id)
	_set_form("Smoke Riverside North", 0.0, 64.0, 8.0, 0.5, 2.0, false, "base:workshop")
	var before_reject := _checksum
	var reject_result := _update_selected()
	_refresh()
	var reject_code := str(reject_result.get("error_code", ""))
	if status == "ok" and bool(reject_result.get("ok", true)):
		status = "reject_accepted"
	if status == "ok" and reject_code != "invalid_radius":
		status = "reject_code"
	if status == "ok" and _checksum != before_reject:
		status = "reject_checksum"
	rejection_count = _policies.size()
	if status == "ok" and rejection_count != 2:
		status = "reject_count"
	if status == "ok" and absf(float(_find_policy(first_id).get("radius_m", 0.0)) - 128.0) > 0.001:
		status = "reject_radius"
	if status == "ok" and not _message_label.text.contains("invalid_radius"):
		status = "reject_message"
	if status == "ok" and _message_label.get_theme_color("font_color") != ERROR_COLOR:
		status = "reject_message_color"

	# A missing id after removal must report not_found without touching state.
	var remove_first := _remove_by_id(first_id)
	if status == "ok" and (not bool(remove_first.get("ok", false)) or int(remove_first.get("count", -1)) != 1):
		status = "remove_first"
	var remove_second := _remove_by_id(second_id)
	if status == "ok" and (not bool(remove_second.get("ok", false)) or int(remove_second.get("count", -1)) != 0):
		status = "remove_second"
	if status == "ok" and not _message_label.text.begins_with("Remove #"):
		status = "remove_message"
	if status == "ok" and _message_label.get_theme_color("font_color") != OK_COLOR:
		status = "remove_message_color"

	_refresh()
	final_checksum = _checksum
	if status == "ok" and (_policies.size() != 0 or _list.item_count != 0):
		status = "final_count"
	if status == "ok" and _selected_id != 0:
		status = "final_selection"
	if status == "ok" and _form_title.text != "No policy selected":
		status = "final_form_title"
	if status == "ok" and final_checksum == created_checksum:
		status = "final_checksum_same"
	if status == "ok":
		var state_again: Dictionary = _core.district_policies_state(seed)
		if str(state_again.get("checksum", "")) != final_checksum:
			status = "final_checksum_unstable"
	var missing_remove: Dictionary = _core.district_policy_remove(seed, first_id)
	if status == "ok" and bool(missing_remove.get("ok", true)):
		status = "missing_remove_accepted"
	if status == "ok" and str(missing_remove.get("error_code", "")) != "not_found":
		status = "missing_remove_code"

	print("POLICY_EDITOR_STATE empty=0000000000000000 created=%s updated=%s final=%s" % [
		created_checksum,
		updated_checksum,
		final_checksum,
	])
	print("POLICY_EDITOR_REJECT code=%s count=%d" % [
		reject_code if not reject_code.is_empty() else "none",
		rejection_count,
	])
	print("POLICY_EDITOR_OVERLAP touching=%s apart=%s" % [
		"yes" if overlap_touching else "no",
		"yes" if overlap_apart else "no",
	])
	var marker := "POLICY_EDITOR status=%s policies=%d checksum=%s" % [
		"ok" if status == "ok" else "failed",
		_policies.size(),
		final_checksum,
	]
	if status != "ok":
		marker += " reason=" + status
	print(marker)
	if status != "ok":
		push_error("Policy editor smoke failed: " + status)
	get_tree().quit(0 if status == "ok" else 1)
