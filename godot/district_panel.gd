extends Control

## Standalone developer overview for settlements and one district.
##
## This page is deliberately outside the world scene and the in-game dashboard:
## it owns one `TownsmithCore` instance and only reads bridge state
## (`settlements`, `district_summary`, and `district_policies_state`). It never
## creates, edits, removes, saves, or loads anything at runtime; the smoke and
## `--fixture` modes populate the in-memory layer through the existing
## `settlement_found` / `district_policy_create` bridge calls before the page
## settles into read-only display. See `docs/57 - Dashboard and Advisor.md` §4.3.
##
## Run:
##   godot --path godot res://district_panel.tscn
##   godot --path godot res://district_panel.tscn -- --fixture
##   godot --headless --path godot res://district_panel.tscn -- --smoke
##   godot --headless --path godot res://district_panel.tscn -- --empty
##
## Headless smoke: starts empty, rejects a blank settlement, founds two
## deterministic settlements, creates two overlapping policies, selects both
## settlements, checks the 256 m district summary and the smallest-id containing
## policy, exercises button/keyboard stepping, and checks the seed-mismatch
## unavailable state without logging a push_error. Final marker:
##   DISTRICT_PANEL status=ok settlements=2 selected=1 population=0 policy=1 checksum=<8 hex>
##
## `--empty` prints the empty contract:
##   DISTRICT_PANEL status=ok settlements=0 selected=none population=0 policy=none checksum=<8 hex>
##
## Strings are English-only developer text; this is a pre-dashboard tool and the
## shared `base.ui.policy.*` localization slice does not exist yet. Settlement
## names, owners, bridge error codes, and policy ids stay verbatim. Display
## numbers go through `WorldI18n.format_int`/`format_float` so the tool already
## follows the EN/DE number rules from `docs/16 - Localization.md`.

const WorldI18n = preload("res://world_i18n.gd")

const SUITE := "district_panel"
const DEFAULT_SEED := 2026
const DISTRICT_RADIUS_M := 256.0

# Mirrors `core_city::settlement`: radius 16..2,000 m.
const MIN_SETTLEMENT_RADIUS_M := 16
const MAX_SETTLEMENT_RADIUS_M := 2000
# Mirrors `core_city::policy`: radius 16..4,096 m.
const MIN_POLICY_RADIUS_M := 16.0
const MAX_POLICY_RADIUS_M := 4096.0

# Deterministic fixture. Settlement 1 and 2 are 1,024 m apart, so the 256 m
# district query cannot bleed into the other settlement. Both policies contain
# settlement 1's centre; ids 1 and 2 make "smallest containing id" observable.
# Settlement 2 sits outside both circles and must report no containing policy.
const FIXTURE_SETTLEMENTS := [
	{"name": "Smoke Alpha", "owner": "player", "x": 0, "z": 0, "radius": 512},
	{"name": "Smoke Beta", "owner": "ai", "x": 1024, "z": 0, "radius": 256},
]
const FIXTURE_POLICIES := [
	{"name": "Smoke Core", "x": 0.0, "z": 0.0, "radius": 300.0, "tax": 1.25, "funding": 0.5},
	{"name": "Smoke Outer", "x": 0.0, "z": 0.0, "radius": 400.0, "tax": 0.75, "funding": 1.5},
]

const OK_COLOR := Color(0.36, 0.86, 0.55)
const ERROR_COLOR := Color(0.96, 0.42, 0.38)
const MUTED_COLOR := Color(0.66, 0.71, 0.78)
const TEXT_COLOR := Color(0.88, 0.91, 0.95)

var smoke_mode := false
var fixture_mode := false
var empty_mode := false

var _i18n
var _core = null
var _seed := DEFAULT_SEED
var _settlements: Array = []
var _settlement_checksum := ""
var _selected_id := 0
var _district: Dictionary = {}
var _district_ok := false
var _policies: Array = []
var _policy_checksum := ""
var _policy_error := ""
var _syncing_list := false

var _seed_edit: LineEdit
var _state_label: Label
var _message_label: Label
var _list: ItemList
var _empty_label: Label
var _summary_title: Label
var _settlement_line: Label
var _membership_line: Label
var _district_line: Label
var _jobs_line: Label
var _needs_line: Label
var _build_line: Label
var _policy_title: Label
var _policy_line: Label
var _checksum_line: Label


func _ready() -> void:
	_parse_args()
	_i18n = WorldI18n.new()
	_build_ui()
	_reset_session(false)
	if smoke_mode:
		_run_smoke()
	elif empty_mode:
		_run_empty()
	elif fixture_mode:
		_load_fixture()


func _parse_args() -> void:
	var args := OS.get_cmdline_user_args()
	var index := 0
	while index < args.size():
		var arg: String = args[index]
		if arg == "--smoke":
			smoke_mode = true
		elif arg == "--fixture":
			fixture_mode = true
		elif arg == "--empty":
			empty_mode = true
		elif arg == "--seed" and index + 1 < args.size():
			index += 1
			var raw: String = args[index]
			if raw.is_valid_int() and int(raw) >= 0:
				_seed = int(raw)
		index += 1


# --- Bridge ----------------------------------------------------------------

func _reset_session(reset_seed: bool) -> void:
	_core = TownsmithCore.new()
	_settlements = []
	_settlement_checksum = ""
	_selected_id = 0
	_district = {}
	_district_ok = false
	_policies = []
	_policy_checksum = ""
	_policy_error = ""
	if reset_seed:
		_seed = DEFAULT_SEED
	_refresh()


func _refresh() -> void:
	if _core == null:
		_show_unavailable("no_core", "the bridge is not loaded")
		return
	var state: Dictionary = _core.settlements()
	if not bool(state.get("ok", false)):
		_show_unavailable(str(state.get("error_code", "error")), str(state.get("error", "")))
		return
	var rows = state.get("settlements", [])
	_settlements = rows if rows is Array else []
	_settlement_checksum = str(state.get("checksum", ""))
	_rebuild_list()
	if _settlements.is_empty():
		_selected_id = 0
		_district = {}
		_district_ok = false
		_policies = []
		_policy_checksum = ""
		_policy_error = ""
		_show_empty()
		return
	if _find_settlement(_selected_id).is_empty():
		_selected_id = int((_settlements[0] as Dictionary).get("id", 0))
	_select_settlement(_selected_id)


func _select_settlement(id: int) -> void:
	if _find_settlement(id).is_empty():
		_selected_id = 0
		_show_empty()
		return
	_selected_id = id
	_sync_list_selection()
	_load_district()
	_load_policies()
	_render_selection()


func _load_district() -> void:
	_district = {}
	_district_ok = false
	var settlement := _find_settlement(_selected_id)
	if settlement.is_empty() or _core == null:
		return
	var center := _settlement_center(settlement)
	var result: Dictionary = _core.district_summary(
		_seed, center.x, center.y, DISTRICT_RADIUS_M
	)
	if bool(result.get("ok", false)):
		_district = result
		_district_ok = true


func _load_policies() -> void:
	_policies = []
	_policy_checksum = ""
	_policy_error = ""
	if _core == null:
		return
	var result: Dictionary = _core.district_policies_state(_seed)
	if not bool(result.get("ok", false)):
		_policy_error = str(result.get("error_code", "error"))
		return
	var rows = result.get("policies", [])
	_policies = rows if rows is Array else []
	_policy_checksum = str(result.get("checksum", ""))


## Smallest-id containing policy, matching `DistrictPolicyStore::policy_at`.
## The bridge has no `policy_at` method yet, so the panel reads the state array.
func _containing_policy(point: Vector2) -> Dictionary:
	var best: Dictionary = {}
	for policy in _policies:
		var center := _policy_center(policy)
		var radius := float(policy.get("radius_m", 0.0))
		if point.distance_squared_to(center) > radius * radius:
			continue
		if best.is_empty() or int(policy.get("id", 0)) < int(best.get("id", 0)):
			best = policy
	return best


# --- State helpers ---------------------------------------------------------

func _find_settlement(id: int) -> Dictionary:
	for settlement in _settlements:
		if int(settlement.get("id", 0)) == id:
			return settlement
	return {}


func _settlement_center(settlement: Dictionary) -> Vector2:
	return _xy_of(settlement.get("center", []))


func _membership_summary(settlement: Dictionary) -> String:
	var revision := int(settlement.get("membership_revision", 0))
	var tax := " · tax %s%%" % _i18n.format_int(int(settlement.get("tax_rate", 0)))
	if not bool(settlement.get("owned", false)):
		return "Unowned · revision %s%s" % [_i18n.format_int(revision), tax]
	var owner_id := int(settlement.get("owner_id", 0))
	var rows = settlement.get("members", [])
	var counts := {}
	if rows is Array:
		for member in rows:
			if member is Dictionary:
				var role := str(member.get("role", "member"))
				counts[role] = int(counts.get(role, 0)) + 1
	var parts: Array[String] = []
	for role in counts.keys():
		parts.append("%s %s" % [role, _i18n.format_int(int(counts[role]))])
	parts.sort()
	var suffix := "" if parts.is_empty() else " · " + " · ".join(parts)
	return "Owner #%s · %s members · revision %s%s%s" % [
		_i18n.format_int(owner_id),
		_i18n.format_int((rows as Array).size() if rows is Array else 0),
		_i18n.format_int(revision),
		tax,
		suffix,
	]


func _policy_center(policy: Dictionary) -> Vector2:
	return _xy_of(policy.get("center", []))


func _xy_of(value) -> Vector2:
	if value is PackedInt32Array and value.size() >= 2:
		return Vector2(float(value[0]), float(value[1]))
	if value is PackedFloat32Array and value.size() >= 2:
		return Vector2(value[0], value[1])
	if value is Array and value.size() >= 2:
		return Vector2(float(value[0]), float(value[1]))
	return Vector2.ZERO


func _panel_checksum(selected_id: int, district_checksum: String, policy_id: int) -> String:
	var canonical := "%d|%s|%d|%s|%d" % [
		_seed,
		_settlement_checksum if not _settlement_checksum.is_empty() else "0000000000000000",
		selected_id,
		district_checksum if not district_checksum.is_empty() else "0000000000000000",
		policy_id,
	]
	return "%08x" % _fnv1a32(canonical)


func _fnv1a32(text: String) -> int:
	# Presentation-only digest; never enters save, network, or simulation state.
	var digest := 2166136261
	for byte in text.to_utf8_buffer():
		digest = ((digest ^ int(byte)) * 16777619) & 0xFFFFFFFF
	return digest


func _district_checksum() -> String:
	return str(_district.get("checksum", "0000000000000000")) if _district_ok else "0000000000000000"


func _contained_policy_id() -> int:
	if not _district_ok:
		return 0
	var settlement := _find_settlement(_selected_id)
	if settlement.is_empty():
		return 0
	return int(_containing_policy(_settlement_center(settlement)).get("id", 0))


# --- Rendering -------------------------------------------------------------

func _show_empty() -> void:
	_rebuild_list()
	if _empty_label != null:
		_empty_label.visible = true
		_empty_label.text = (
			"No settlements in this session.\n"
			+ "The page is read-only. Run with --fixture to inspect the deterministic "
			+ "two-settlement fixture, or --smoke for the headless contract."
		)
	if _summary_title != null:
		_summary_title.text = "No settlement selected"
	if _settlement_line != null:
		_settlement_line.text = "—"
		_membership_line.text = "—"
		_district_line.text = "—"
		_jobs_line.text = "—"
		_needs_line.text = "—"
		_build_line.text = "—"
		_policy_title.text = "Containing policy"
		_policy_line.text = "—"
		_checksum_line.text = "settlements %s · district — · policies —" % _settlement_checksum
	_set_message("Empty settlement layer: %s settlements." % _i18n.format_int(0), false)
	_refresh_state_label()


func _show_unavailable(code: String, message: String) -> void:
	_district = {}
	_district_ok = false
	_settlements = []
	_settlement_checksum = ""
	_selected_id = 0
	_policies = []
	_policy_checksum = ""
	_policy_error = ""
	_rebuild_list()
	if _empty_label != null:
		_empty_label.visible = true
		_empty_label.text = "Settlement layer unavailable."
	if _summary_title != null:
		_summary_title.text = "District unavailable"
	if _settlement_line != null:
		_settlement_line.text = (
			"Cannot read settlements: %s (%s)" % [code, message]
			if not message.is_empty()
			else "Cannot read settlements: %s" % code
		)
		_membership_line.text = "—"
		_district_line.text = "District unavailable — the panel does not retry."
		_jobs_line.text = "—"
		_needs_line.text = "—"
		_build_line.text = "—"
		_policy_title.text = "Containing policy"
		_policy_line.text = "—"
		_checksum_line.text = "settlements — · district — · policies —"
	_set_message("Unavailable: %s" % code, false)
	_refresh_state_label()


func _render_selection() -> void:
	var settlement := _find_settlement(_selected_id)
	if settlement.is_empty():
		_show_empty()
		return
	if _empty_label != null:
		_empty_label.visible = false
	var center := _settlement_center(settlement)
	var settlement_name := str(settlement.get("name", ""))
	var owner := str(settlement.get("owner", ""))
	if _summary_title != null:
		_summary_title.text = "#%s %s" % [_i18n.format_int(_selected_id), settlement_name]
	if _settlement_line != null:
		_settlement_line.text = "#%s %s · owner %s · centre (%s, %s) · radius %s m · tax %s%%" % [
			_i18n.format_int(_selected_id),
			settlement_name,
			owner,
			_i18n.format_float(center.x, 0),
			_i18n.format_float(center.y, 0),
			_i18n.format_int(int(settlement.get("radius_m", 0))),
			_i18n.format_int(int(settlement.get("tax_rate", 0))),
		]
		if _membership_line != null:
			_membership_line.text = _membership_summary(settlement)
		_render_district()
		_render_policy(center)
		_checksum_line.text = "settlements %s · district %s · policies %s" % [
			_settlement_checksum if not _settlement_checksum.is_empty() else "—",
			_district_checksum(),
			_policy_checksum if not _policy_checksum.is_empty() else "—",
		]
	_set_message(
		"Selected #%s %s · 256 m district." % [_i18n.format_int(_selected_id), settlement_name],
		false
	)
	_refresh_state_label()


func _render_district() -> void:
	if not _district_ok:
		_district_line.text = "District summary unavailable (%s)." % (
			"no settlement" if _district.is_empty() else "bridge error"
		)
		_jobs_line.text = "—"
		_needs_line.text = "—"
		_build_line.text = "—"
		return
	var population := int(_district.get("population", 0))
	var happiness := "—"
	var needs := "—"
	if population > 0:
		happiness = _i18n.format_float(float(_district.get("happiness", 0.0)) * 100.0, 0) + "%"
		needs = "food %s%% · water %s%% · rest %s%%" % [
			_i18n.format_float(float(_district.get("food", 0.0)) * 100.0, 0),
			_i18n.format_float(float(_district.get("water", 0.0)) * 100.0, 0),
			_i18n.format_float(float(_district.get("rest", 0.0)) * 100.0, 0),
		]
	_district_line.text = "Population %s · housing %s (%s vacant) · households %s" % [
		_i18n.format_int(population),
		_i18n.format_int(int(_district.get("housing_capacity", 0))),
		_i18n.format_int(int(_district.get("vacancies", 0))),
		_i18n.format_int(int(_district.get("households", 0))),
	]
	_jobs_line.text = "Jobs %s · %s employed · %s unemployed" % [
		_i18n.format_int(int(_district.get("jobs", 0))),
		_i18n.format_int(int(_district.get("employed", 0))),
		_i18n.format_int(int(_district.get("unemployed", 0))),
	]
	_needs_line.text = "Happiness %s · %s · commute %s m" % [
		happiness,
		needs,
		_i18n.format_float(float(_district.get("average_commute_m", 0.0)), 0),
	]
	_build_line.text = "Completed buildings %s · construction sites %s · radius %s m" % [
		_i18n.format_int(int(_district.get("completed_buildings", 0))),
		_i18n.format_int(int(_district.get("construction_sites", 0))),
		_i18n.format_float(float(_district.get("radius_m", DISTRICT_RADIUS_M)), 0),
	]


func _render_policy(settlement_center: Vector2) -> void:
	if not _policy_error.is_empty():
		_policy_line.text = "Policies unavailable (%s)." % _policy_error
		return
	var policy := _containing_policy(settlement_center)
	if policy.is_empty():
		_policy_line.text = "No containing policy."
		return
	var center := _policy_center(policy)
	_policy_line.text = "#%s %s · centre (%s, %s) · r %s m · tax ×%s · funding ×%s · construction %s" % [
		_i18n.format_int(int(policy.get("id", 0))),
		str(policy.get("name", "")),
		_i18n.format_float(center.x, 0),
		_i18n.format_float(center.y, 0),
		_i18n.format_float(float(policy.get("radius_m", 0.0)), 1),
		_i18n.format_float(float(policy.get("tax_multiplier", 0.0)), 2),
		_i18n.format_float(float(policy.get("service_funding", 0.0)), 2),
		"on" if bool(policy.get("construction_allowed", false)) else "off",
	]


func _rebuild_list() -> void:
	if _list == null:
		return
	_syncing_list = true
	_list.clear()
	for settlement in _settlements:
		var id := int(settlement.get("id", 0))
		var center := _settlement_center(settlement)
		_list.add_item("#%s  %s  ·  %s  ·  centre (%s, %s)  ·  r %s m  ·  tax %s%%" % [
			_i18n.format_int(id),
			str(settlement.get("name", "")),
			str(settlement.get("owner", "")),
			_i18n.format_float(center.x, 0),
			_i18n.format_float(center.y, 0),
			_i18n.format_int(int(settlement.get("radius_m", 0))),
			_i18n.format_int(int(settlement.get("tax_rate", 0))),
		])
		_list.set_item_metadata(_list.item_count - 1, id)
		if id == _selected_id:
			_list.select(_list.item_count - 1)
	_syncing_list = false
	_refresh_state_label()


func _sync_list_selection() -> void:
	if _list == null:
		return
	_syncing_list = true
	for index in range(_list.item_count):
		if int(_list.get_item_metadata(index)) == _selected_id:
			_list.select(index)
			_list.ensure_current_is_visible()
			break
	_syncing_list = false


func _refresh_state_label() -> void:
	if _state_label == null:
		return
	var selected_text: String = (
		"none" if _selected_id == 0 else "#" + _i18n.format_int(_selected_id)
	)
	var policy_id := _contained_policy_id()
	var policy_text: String = "none" if policy_id == 0 else "#" + _i18n.format_int(policy_id)
	_state_label.text = "Seed %s · %s settlements · selected %s · policy %s · checksum %s" % [
		_i18n.format_int(_seed),
		_i18n.format_int(_settlements.size()),
		selected_text,
		policy_text,
		_panel_checksum(_selected_id, _district_checksum(), policy_id),
	]


func _set_message(text: String, is_error: bool) -> void:
	if _message_label == null:
		return
	_message_label.text = text
	_message_label.add_theme_color_override("font_color", ERROR_COLOR if is_error else OK_COLOR)


func _step_selection(step: int) -> void:
	if _settlements.is_empty():
		return
	var ids: Array[int] = []
	for settlement in _settlements:
		ids.append(int(settlement.get("id", 0)))
	ids.sort()
	var index := ids.find(_selected_id)
	if index < 0:
		index = 0
	else:
		index = posmod(index + step, ids.size())
	_select_settlement(ids[index])


# --- UI --------------------------------------------------------------------

func _build_ui() -> void:
	set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	var background := ColorRect.new()
	background.color = Color(0.06, 0.07, 0.09)
	background.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	add_child(background)

	var margin := MarginContainer.new()
	margin.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	margin.add_theme_constant_override("margin_left", 18)
	margin.add_theme_constant_override("margin_top", 16)
	margin.add_theme_constant_override("margin_right", 18)
	margin.add_theme_constant_override("margin_bottom", 16)
	add_child(margin)

	var root := VBoxContainer.new()
	root.add_theme_constant_override("separation", 10)
	margin.add_child(root)

	var title := Label.new()
	title.text = "District Overview"
	title.add_theme_font_size_override("font_size", 24)
	title.add_theme_color_override("font_color", TEXT_COLOR)
	root.add_child(title)

	var subtitle := Label.new()
	subtitle.text = (
		"Developer-only read-only panel: one settlement per row, its 256 m "
		+ "district summary, and the smallest-id containing policy. Nothing here "
		+ "is saved, loaded, or applied to simulation."
	)
	subtitle.add_theme_color_override("font_color", MUTED_COLOR)
	subtitle.add_theme_font_size_override("font_size", 13)
	subtitle.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	root.add_child(subtitle)

	root.add_child(_build_toolbar())

	var columns := HBoxContainer.new()
	columns.add_theme_constant_override("separation", 16)
	columns.size_flags_vertical = Control.SIZE_EXPAND_FILL
	root.add_child(columns)
	columns.add_child(_build_list_panel())
	columns.add_child(_build_summary_panel())

	var footer := HBoxContainer.new()
	footer.add_theme_constant_override("separation", 12)
	root.add_child(footer)

	var hint := Label.new()
	hint.text = "Empty and unavailable states are displayed, never logged as errors."
	hint.add_theme_color_override("font_color", MUTED_COLOR)
	hint.add_theme_font_size_override("font_size", 12)
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

	var previous := Button.new()
	previous.text = "Previous"
	previous.pressed.connect(_on_previous_pressed)
	row.add_child(previous)

	var next := Button.new()
	next.text = "Next"
	next.pressed.connect(_on_next_pressed)
	row.add_child(next)

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
	heading.text = "Settlements (ascending id)"
	heading.add_theme_font_size_override("font_size", 18)
	box.add_child(heading)

	var hint := Label.new()
	hint.text = (
		"Select a row, or focus the list and use Up/Down. Previous/Next step the "
		+ "bounds in id order."
	)
	hint.add_theme_color_override("font_color", MUTED_COLOR)
	hint.add_theme_font_size_override("font_size", 12)
	hint.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	box.add_child(hint)

	_list = ItemList.new()
	_list.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	_list.size_flags_vertical = Control.SIZE_EXPAND_FILL
	_list.item_selected.connect(_on_list_selected)
	box.add_child(_list)

	_empty_label = Label.new()
	_empty_label.text = "No settlements in this session."
	_empty_label.add_theme_color_override("font_color", MUTED_COLOR)
	_empty_label.add_theme_font_size_override("font_size", 13)
	_empty_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	box.add_child(_empty_label)
	return panel


func _build_summary_panel() -> Control:
	var scroll := ScrollContainer.new()
	scroll.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	scroll.size_flags_vertical = Control.SIZE_EXPAND_FILL
	scroll.horizontal_scroll_mode = ScrollContainer.SCROLL_MODE_DISABLED

	var panel := PanelContainer.new()
	panel.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	panel.add_theme_stylebox_override("panel", _panel_style())
	scroll.add_child(panel)

	var box := VBoxContainer.new()
	box.add_theme_constant_override("separation", 6)
	panel.add_child(box)

	_summary_title = Label.new()
	_summary_title.text = "No settlement selected"
	_summary_title.add_theme_font_size_override("font_size", 18)
	_summary_title.add_theme_color_override("font_color", TEXT_COLOR)
	box.add_child(_summary_title)

	_settlement_line = _add_summary_label(box, "—", TEXT_COLOR)
	_add_summary_heading(box, "Membership (read-only)")
	_membership_line = _add_summary_label(box, "—", TEXT_COLOR)
	_add_summary_heading(box, "District (256 m around the settlement centre)")
	_district_line = _add_summary_label(box, "—", TEXT_COLOR)
	_jobs_line = _add_summary_label(box, "—", TEXT_COLOR)
	_needs_line = _add_summary_label(box, "—", TEXT_COLOR)
	_build_line = _add_summary_label(box, "—", TEXT_COLOR)

	_policy_title = _add_summary_heading(box, "Containing policy")
	_policy_line = _add_summary_label(box, "—", TEXT_COLOR)
	_policy_title.text = "Containing policy"

	_add_summary_heading(box, "Bridge checksums")
	_checksum_line = _add_summary_label(box, "—", MUTED_COLOR)

	var note := Label.new()
	note.text = (
		"Containment is a squared-radius test over district_policies_state; the "
		+ "smallest containing id wins, matching DistrictPolicyStore::policy_at. "
		+ "No policy value is applied."
	)
	note.add_theme_color_override("font_color", MUTED_COLOR)
	note.add_theme_font_size_override("font_size", 12)
	note.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	box.add_child(note)
	return scroll


func _add_summary_heading(box: VBoxContainer, text: String) -> Label:
	var label := Label.new()
	label.text = text
	label.add_theme_font_size_override("font_size", 14)
	label.add_theme_color_override("font_color", MUTED_COLOR)
	box.add_child(label)
	return label


func _add_summary_label(box: VBoxContainer, text: String, color: Color) -> Label:
	var label := Label.new()
	label.text = text
	label.add_theme_font_size_override("font_size", 14)
	label.add_theme_color_override("font_color", color)
	label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	box.add_child(label)
	return label


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


func _on_apply_seed_pressed() -> void:
	var raw := _seed_edit.text.strip_edges()
	if not raw.is_valid_int() or int(raw) < 0:
		_set_message("Seed must be a non-negative integer; session unchanged.", true)
		return
	_seed = int(raw)
	_reset_session(false)
	_set_message("New session for seed %s." % _i18n.format_int(_seed), false)


func _on_refresh_pressed() -> void:
	_refresh()


func _on_previous_pressed() -> void:
	_step_selection(-1)


func _on_next_pressed() -> void:
	_step_selection(1)


func _on_list_selected(index: int) -> void:
	if _syncing_list or index < 0 or index >= _list.item_count:
		return
	_select_settlement(int(_list.get_item_metadata(index)))


# --- Fixture ---------------------------------------------------------------

## Populate the in-memory bridge for display. This is developer fixture setup,
## not a panel action: the buttons and list never mutate the bridge.
func _load_fixture() -> bool:
	if _core == null:
		_show_unavailable("no_core", "the bridge is not loaded")
		return false
	for entry in FIXTURE_SETTLEMENTS:
		var result: Dictionary = _core.settlement_found(
			_seed,
			str(entry["name"]),
			str(entry["owner"]),
			int(entry["x"]),
			int(entry["z"]),
			int(entry["radius"])
		)
		if not bool(result.get("ok", false)):
			_set_message(
				"Fixture settlement failed: %s" % str(result.get("error_code", "error")),
				true
			)
			return false
	for entry in FIXTURE_POLICIES:
		var result: Dictionary = _core.district_policy_create(
			_seed,
			str(entry["name"]),
			float(entry["x"]),
			float(entry["z"]),
			float(entry["radius"]),
			float(entry["tax"]),
			float(entry["funding"]),
			true,
			PackedStringArray()
		)
		if not bool(result.get("ok", false)):
			_set_message(
				"Fixture policy failed: %s" % str(result.get("error_code", "error")),
				true
			)
			return false
	_refresh()
	_set_message("Deterministic fixture loaded (2 settlements, 2 policies).", false)
	return true


# --- Headless smoke --------------------------------------------------------

func _run_empty() -> void:
	var status := "ok"
	if _core == null:
		status = "no_core"
	var state: Dictionary = _core.settlements() if _core != null else {}
	if status == "ok" and (not bool(state.get("ok", false)) or int(state.get("count", -1)) != 0):
		status = "empty_count"
	if status == "ok" and _settlements.size() != 0:
		status = "empty_list"
	if status == "ok" and _selected_id != 0:
		status = "empty_selection"
	if status == "ok" and _district_ok:
		status = "empty_district"
	_print_marker(status, 0, 0, 0, 0)
	get_tree().quit(0 if status == "ok" else 1)


func _run_smoke() -> void:
	var seed := DEFAULT_SEED
	_seed = seed
	_reset_session(false)

	var status := "ok"
	var rejection_code := "none"
	var selected_id := 0
	var contained_id := 0
	var unavailable_code := "none"
	var fixture_ok := false

	# 1. Initial empty state: settlement layer closed, no list rows, no district
	# query, and the empty label visible. No store may be created by a read.
	if _core == null:
		status = "no_core"
	if status == "ok":
		var initial: Dictionary = _core.settlements()
		if not bool(initial.get("ok", true)):
			status = "initial_state"
		elif bool(initial.get("open", true)) or int(initial.get("count", -1)) != 0:
			status = "initial_open"
		elif _settlements.size() != 0 or _list.item_count != 0:
			status = "initial_list"
		elif _selected_id != 0 or _district_ok:
			status = "initial_selection"
		elif _empty_label == null or not _empty_label.visible:
			status = "initial_empty_label"
		elif not _empty_label.text.contains("No settlements"):
			status = "initial_empty_text"

	# 2. A rejected first settlement leaves no store behind. The panel shows the
	# bridge code in the footer instead of logging an error.
	if status == "ok":
		var rejected: Dictionary = _core.settlement_found(seed, "", "player", 0, 0, 64)
		rejection_code = str(rejected.get("error_code", "none"))
		if bool(rejected.get("ok", true)):
			status = "reject_accepted"
		elif int(rejected.get("count", 0)) != 0:
			status = "reject_count"
		elif bool((_core.settlements() as Dictionary).get("open", true)):
			status = "reject_opened"
		elif _settlements.size() != 0:
			status = "reject_list"

	# 3. Found two settlements and two overlapping policies through the bridge.
	if status == "ok":
		fixture_ok = _load_fixture()
		if not fixture_ok:
			status = "fixture"
	if status == "ok" and (_settlements.size() != 2 or _list.item_count != 2):
		status = "fixture_list"
	if status == "ok" and (_policies.size() != 2 or _policy_checksum == ""):
		status = "fixture_policies"
	if status == "ok":
		var first_row := _list.get_item_text(0)
		var second_row := _list.get_item_text(1)
		if not first_row.contains("Smoke Alpha") or not first_row.contains("player"):
			status = "fixture_row_first"
		elif not second_row.contains("Smoke Beta") or not second_row.contains("ai"):
			status = "fixture_row_second"
		elif not first_row.contains("#1") or not second_row.contains("#2"):
			status = "fixture_row_ids"
		elif not first_row.contains("tax 0%") or not second_row.contains("tax 0%"):
			status = "fixture_row_tax"
		elif int(_list.get_item_metadata(0)) != 1 or int(_list.get_item_metadata(1)) != 2:
			status = "fixture_metadata"
		elif _selected_id != 1:
			status = "fixture_default_selection"

	# 4. Settlement 1: the 256 m district is readable, empty of buildings, and
	# the smallest containing policy is id 1 (both fixture policies contain it).
	if status == "ok":
		_select_settlement(1)
		selected_id = _selected_id
		contained_id = _contained_policy_id()
		if selected_id != 1 or contained_id != 1:
			status = "select_first"
		elif not _district_ok:
			status = "select_first_district"
		elif absf(float(_district.get("radius_m", -1.0)) - DISTRICT_RADIUS_M) > 0.001:
			status = "select_first_radius"
		elif int(_district.get("population", -1)) != 0:
			status = "select_first_population"
		elif str(_district.get("checksum", "")).length() != 16:
			status = "select_first_district_checksum"
		elif not _settlement_line.text.contains("Smoke Alpha"):
			status = "select_first_title"
		elif not _membership_line.text.contains("Unowned"):
			status = "select_first_membership"
		elif not _settlement_line.text.contains("tax 0%"):
			status = "select_first_tax"
		elif not _membership_line.text.contains("tax 0%"):
			status = "select_first_membership_tax"
		elif not _district_line.text.contains("Population 0"):
			status = "select_first_row"
		elif not _policy_line.text.contains("#1") or not _policy_line.text.contains("Smoke Core"):
			status = "select_first_policy_row"

	# 5. Settlement 2 is 1,024 m away and outside both policy circles: the
	# summary still succeeds and containment reports no policy.
	if status == "ok":
		_select_settlement(2)
		selected_id = _selected_id
		contained_id = _contained_policy_id()
		if selected_id != 2 or contained_id != 0:
			status = "select_second"
		elif not _district_ok:
			status = "select_second_district"
		elif not _policy_line.text.contains("No containing policy"):
			status = "select_second_policy_row"
		elif not _summary_title.text.contains("Smoke Beta"):
			status = "select_second_title"
		elif not _settlement_line.text.contains("tax 0%"):
			status = "select_second_tax"

	# 6. Keyboard/button stepping wraps in id order.
	if status == "ok":
		_select_settlement(1)
		_on_next_pressed()
		if _selected_id != 2:
			status = "step_next"
		else:
			_on_next_pressed()
			if _selected_id != 1:
				status = "step_wrap"
			else:
				_on_previous_pressed()
				if _selected_id != 2:
					status = "step_previous"
		if status == "ok":
			_select_settlement(1)

	# 7. Unavailable state: a foreign seed is a seed_mismatch for both the
	# district and the policy registry. The panel must render it, not push_error
	# or abort, and the next refresh must recover.
	if status == "ok":
		_seed = seed + 1
		_refresh()
		if _district_ok:
			status = "unavailable_district_ok"
		elif _selected_id != 1 or _settlements.size() != 2:
			status = "unavailable_selection"
		elif not _policy_error.is_empty() and _policy_error != "seed_mismatch":
			status = "unavailable_policy_code"
		elif not _message_label.text.contains("Selected"):
			status = "unavailable_message"
		unavailable_code = _policy_error if not _policy_error.is_empty() else "none"
		_seed = seed
		_refresh()
		if status == "ok" and (not _district_ok or _contained_policy_id() != 1):
			status = "unavailable_recovery"

	# 8. Repeated reads are stable: the panel digest and the two bridge
	# checksums must not drift across refreshes.
	if status == "ok":
		_select_settlement(1)
		var first_digest := _panel_checksum(
			_selected_id, _district_checksum(), _contained_policy_id()
		)
		var first_settlements := _settlement_checksum
		var first_policies := _policy_checksum
		_refresh()
		var second_digest := _panel_checksum(
			_selected_id, _district_checksum(), _contained_policy_id()
		)
		if first_digest != second_digest:
			status = "digest_unstable"
		elif first_settlements != _settlement_checksum:
			status = "settlement_checksum_unstable"
		elif first_policies != _policy_checksum:
			status = "policy_checksum_unstable"

	selected_id = _selected_id
	contained_id = _contained_policy_id()
	var population := int(_district.get("population", 0)) if _district_ok else 0
	print("DISTRICT_PANEL_STATE seed=%d settlements=%d policies=%d selected=%d centre=(%s,%s) district=%s policy=%s reject=%s unavailable=%s" % [
		seed,
		_settlements.size(),
		_policies.size(),
		selected_id,
		_i18n.format_float(_settlement_center(_find_settlement(selected_id)).x, 0),
		_i18n.format_float(_settlement_center(_find_settlement(selected_id)).y, 0),
		_district_checksum(),
		contained_id,
		rejection_code,
		unavailable_code,
	])
	var alpha_tax := int(_find_settlement(1).get("tax_rate", 0))
	var beta_tax := int(_find_settlement(2).get("tax_rate", 0))
	var selected_tax := int(_find_settlement(selected_id).get("tax_rate", 0)) if selected_id != 0 else 0
	print("DISTRICT_TAX alpha=%d%% beta=%d%% selected=%d%%" % [
		alpha_tax,
		beta_tax,
		selected_tax,
	])
	_print_marker(status, _settlements.size(), selected_id, population, contained_id)
	get_tree().quit(0 if status == "ok" else 1)


func _print_marker(
	status: String,
	settlement_count: int,
	selected_id: int,
	population: int,
	policy_id: int
) -> void:
	var marker := "DISTRICT_PANEL status=%s settlements=%d selected=%s population=%d policy=%s checksum=%s" % [
		"ok" if status == "ok" else "failed",
		settlement_count,
		"none" if selected_id == 0 else _i18n.format_int(selected_id),
		population,
		"none" if policy_id == 0 else _i18n.format_int(policy_id),
		_panel_checksum(selected_id, _district_checksum(), policy_id),
	]
	if status != "ok":
		marker += " reason=" + status
	print(marker)
	if status != "ok":
		printerr("District panel smoke failed: " + status)
