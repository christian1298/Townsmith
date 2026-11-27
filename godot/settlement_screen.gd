extends Control

## Standalone read-only production settlement screen shell (local bridge).
##
## Implements the first slice of `docs/85 - Connected Settlement Surface.md` §7:
## a cursor-paged list over `settlements_page(after_id, limit)` and a read-only
## detail pane with player names from `players_list()` and the D-039 preview
## from `tax_projection(rate)`. The header records the `Local` authority mode.
##
## Deliberately no mutations: no claim, rename, transfer, release, invite,
## set-role, remove-member, or set-tax controls, and no settlement founding.
## The smoke/fixture path seeds the in-memory bridge before the screen settles
## into read-only display, exactly like the developer district panel.
##
## Run:
##   godot --path godot res://settlement_screen.tscn
##   godot --path godot res://settlement_screen.tscn -- --empty
##   godot --headless --path godot res://settlement_screen.tscn -- --smoke
##
## Final smoke marker, printed twice with identical output:
##   SETTLEMENT_SCREEN status=ok pages=3 settlements=23 selected=2 checksum=<16 hex>

const WorldI18n = preload("res://world_i18n.gd")

const SUITE := "settlement_screen"
const DEFAULT_SEED := 2026
const PAGE_SIZE := 8
const MAX_TAX_RATE := 100

## Permission bit -> localized token. Mirrors `SettlementPermission` in
## `core_city::settlement`; only granted tokens are rendered.
const MEMBER_PERMISSION_KEYS := [
	[1, "base.ui.settlement_screen.permission.build"],
	[2, "base.ui.settlement_screen.permission.manage"],
	[4, "base.ui.settlement_screen.permission.tax"],
	[8, "base.ui.settlement_screen.permission.members"],
]

const FIXTURE_PLAYER_NAMES := {
	7: "Mayor",
	11: "Manager",
	12: "Member",
	13: "Builder",
}
const FIXTURE_PLAYER_MAX_ID := 13
const FIXTURE_UNOWNED_OWNER := "ai"
const FIXTURE_SETTLEMENT_COUNT := 23
## Claimed by an id with no registered display name, so the `#id` owner
## fallback is exercised through the real bridge path.
const FIXTURE_FALLBACK_OWNER_ID := 9002
## Invited to settlement 2 without a registered name: the `#id` member fallback.
const FIXTURE_FALLBACK_MEMBER_ID := 9001

const MUTED_COLOR := Color(0.66, 0.71, 0.78)
const TEXT_COLOR := Color(0.88, 0.91, 0.95)

var smoke_mode := false
var empty_mode := false
var unavailable_mode := false

var _i18n
var _core = null
var _seed := DEFAULT_SEED
var _locale_override := ""

# Page model. The cursor stack holds the exclusive `after_id` for each visited
# page; forward paging appends the response `next_after`, never an array index.
var _cursor_stack: Array[int] = [0]
var _page_index := 0
var _next_after := 0
var _rows: Array = []
var _matched := 0
var _has_more := false
var _checksum := ""
var _selected_index := 0
var _selected_id := 0
var _error_code := ""

var _player_names: Dictionary = {}
var _syncing_list := false

var _title_label: Label
var _authority_label: Label
var _page_label: Label
var _list: ItemList
var _more_label: Label
var _help_label: Label
var _detail_title: Label
var _detail_center: Label
var _detail_owner: Label
var _detail_tax: Label
var _detail_revision: Label
var _projection_label: Label
var _members_title: Label
var _members_box: VBoxContainer
var _detail_empty: Label
var _status_label: Label


func _ready() -> void:
	_parse_args()
	# The headless contract is pinned in English; `--locale de` previews the
	# localized UI and the smoke switches locales itself.
	var locale := _locale_override
	if locale.is_empty() and smoke_mode:
		locale = "en"
	_i18n = WorldI18n.new(locale)
	_build_ui()
	if smoke_mode:
		_run_smoke()
	elif empty_mode:
		_run_empty()
	elif unavailable_mode:
		_run_unavailable()
	else:
		_start_local_session()


func _parse_args() -> void:
	var args := OS.get_cmdline_user_args()
	var index := 0
	while index < args.size():
		var arg: String = args[index]
		if arg == "--smoke":
			smoke_mode = true
		elif arg == "--empty":
			empty_mode = true
		elif arg == "--unavailable":
			unavailable_mode = true
		elif arg == "--seed" and index + 1 < args.size():
			index += 1
			var raw: String = args[index]
			if raw.is_valid_int() and int(raw) >= 0:
				_seed = int(raw)
		elif arg == "--locale" and index + 1 < args.size():
			index += 1
			_locale_override = args[index]
		index += 1


# --- UI --------------------------------------------------------------------

func _build_ui() -> void:
	set_anchors_preset(Control.PRESET_FULL_RECT)

	var margin := MarginContainer.new()
	margin.set_anchors_preset(Control.PRESET_FULL_RECT)
	margin.add_theme_constant_override("margin_left", 16)
	margin.add_theme_constant_override("margin_top", 12)
	margin.add_theme_constant_override("margin_right", 16)
	margin.add_theme_constant_override("margin_bottom", 12)
	add_child(margin)

	var root := VBoxContainer.new()
	root.add_theme_constant_override("separation", 8)
	margin.add_child(root)

	var header := HBoxContainer.new()
	header.add_theme_constant_override("separation", 12)
	_title_label = Label.new()
	_title_label.add_theme_font_size_override("font_size", 22)
	_title_label.add_theme_color_override("font_color", TEXT_COLOR)
	header.add_child(_title_label)
	var spacer := Control.new()
	spacer.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	header.add_child(spacer)
	_authority_label = Label.new()
	_authority_label.add_theme_color_override("font_color", MUTED_COLOR)
	_authority_label.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT
	header.add_child(_authority_label)
	root.add_child(header)
	root.add_child(HSeparator.new())

	var body := HBoxContainer.new()
	body.size_flags_vertical = Control.SIZE_EXPAND_FILL
	body.add_theme_constant_override("separation", 16)
	root.add_child(body)

	# List pane.
	var left := VBoxContainer.new()
	left.custom_minimum_size = Vector2(680, 0)
	left.size_flags_vertical = Control.SIZE_EXPAND_FILL
	left.add_theme_constant_override("separation", 6)
	body.add_child(left)

	_page_label = Label.new()
	_page_label.add_theme_color_override("font_color", MUTED_COLOR)
	left.add_child(_page_label)

	_list = ItemList.new()
	_list.size_flags_vertical = Control.SIZE_EXPAND_FILL
	_list.custom_minimum_size = Vector2(0, PAGE_SIZE * 30 + 20)
	_list.allow_reselect = true
	_list.item_selected.connect(_on_item_selected)
	left.add_child(_list)

	_more_label = Label.new()
	_more_label.add_theme_color_override("font_color", MUTED_COLOR)
	left.add_child(_more_label)

	_help_label = Label.new()
	_help_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	_help_label.add_theme_color_override("font_color", MUTED_COLOR)
	left.add_child(_help_label)

	# Detail pane.
	var right := VBoxContainer.new()
	right.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	right.size_flags_vertical = Control.SIZE_EXPAND_FILL
	right.add_theme_constant_override("separation", 6)
	body.add_child(right)

	_detail_title = _detail_label(right, 20)
	_detail_center = _detail_label(right)
	_detail_owner = _detail_label(right)
	_detail_tax = _detail_label(right)
	_detail_revision = _detail_label(right)
	_projection_label = _detail_label(right)
	_projection_label.add_theme_color_override("font_color", MUTED_COLOR)
	_members_title = _detail_label(right)
	_members_title.add_theme_color_override("font_color", MUTED_COLOR)

	var members_scroll := ScrollContainer.new()
	members_scroll.size_flags_vertical = Control.SIZE_EXPAND_FILL
	right.add_child(members_scroll)
	_members_box = VBoxContainer.new()
	_members_box.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	_members_box.add_theme_constant_override("separation", 2)
	members_scroll.add_child(_members_box)

	_detail_empty = Label.new()
	_detail_empty.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	_detail_empty.add_theme_color_override("font_color", MUTED_COLOR)
	right.add_child(_detail_empty)

	root.add_child(HSeparator.new())
	_status_label = Label.new()
	_status_label.add_theme_color_override("font_color", MUTED_COLOR)
	root.add_child(_status_label)


func _detail_label(parent: Node, font_size: int = 16) -> Label:
	var label := Label.new()
	label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	label.add_theme_color_override("font_color", TEXT_COLOR)
	if font_size != 16:
		label.add_theme_font_size_override("font_size", font_size)
	parent.add_child(label)
	return label


# --- Session / bridge reads ------------------------------------------------

func _start_local_session() -> void:
	_core = TownsmithCore.new()
	_refresh_player_names()
	_load_page(0)


func _run_empty() -> void:
	_core = TownsmithCore.new()
	_refresh_player_names()
	_load_page(0)
	if _status_label.text != _i18n.t("base.ui.settlement_screen.empty"):
		print("SETTLEMENT_SCREEN status=failed reason=empty-state")
		get_tree().quit(1)
		return
	print("SETTLEMENT_SCREEN status=empty settlements=0")
	get_tree().quit(0)


func _run_unavailable() -> void:
	_core = null
	_show_unavailable("no_core")
	if not _status_label.text.contains("no_core") or _status_label.text.contains("!base.ui."):
		print("SETTLEMENT_SCREEN status=failed reason=unavailable-state")
		get_tree().quit(1)
		return
	print("SETTLEMENT_SCREEN status=unavailable code=no_core")
	get_tree().quit(0)


func _refresh_player_names() -> void:
	_player_names = {}
	if _core == null:
		return
	var result: Dictionary = _core.players_list()
	if not bool(result.get("ok", false)):
		return
	var players = result.get("players", [])
	if not (players is Array):
		return
	for record in players:
		if not (record is Dictionary):
			continue
		var id := int(record.get("id", 0))
		var name := str(record.get("name", ""))
		if id > 0 and not name.is_empty():
			_player_names[id] = name


func _load_page(index: int) -> void:
	if _core == null:
		_show_unavailable("no_core")
		return
	if index < 0 or index >= _cursor_stack.size():
		return
	var after := int(_cursor_stack[index])
	var response: Dictionary = _core.settlements_page(after, PAGE_SIZE)
	if not bool(response.get("ok", false)):
		_show_unavailable(str(response.get("error_code", "error")))
		return
	var rows = response.get("settlements", [])
	_rows = rows if rows is Array else []
	_matched = int(response.get("matched", 0))
	_has_more = bool(response.get("has_more", false))
	_next_after = int(response.get("next_after", 0))
	_checksum = str(response.get("checksum", ""))
	_page_index = index
	_error_code = ""
	# Keep one forward cursor per page. A refresh that returns a different
	# `next_after` discards the stale deeper pages.
	if _has_more:
		if index + 1 < _cursor_stack.size():
			if _cursor_stack[index + 1] != _next_after:
				_cursor_stack.resize(index + 1)
				_cursor_stack.append(_next_after)
		else:
			_cursor_stack.append(_next_after)
	elif index + 1 < _cursor_stack.size():
		_cursor_stack.resize(index + 1)

	var keep_id := _selected_id
	_selected_index = 0
	var found := _index_of(keep_id)
	if found >= 0:
		_selected_index = found
	if _rows.is_empty():
		_selected_id = 0
	else:
		_selected_id = int((_rows[_selected_index] as Dictionary).get("id", 0))
	_render_all()


func _page_down() -> void:
	if _has_more:
		_load_page(_page_index + 1)


func _page_up() -> void:
	if _page_index > 0:
		_load_page(_page_index - 1)


func _page_count() -> int:
	if _matched <= 0:
		return 1
	return ceili(float(_matched) / float(PAGE_SIZE))


func _index_of(id: int) -> int:
	for index in _rows.size():
		if int((_rows[index] as Dictionary).get("id", 0)) == id:
			return index
	return -1


func _select_index(index: int) -> void:
	if _rows.is_empty() or index < 0 or index >= _rows.size():
		return
	_selected_index = index
	_selected_id = int((_rows[index] as Dictionary).get("id", 0))
	if not _syncing_list:
		_syncing_list = true
		_list.select(index)
		_syncing_list = false
	_render_detail()


func _select_page_number(number: int) -> void:
	if number < 1 or number > PAGE_SIZE:
		return
	_select_index(number - 1)


func _on_item_selected(index: int) -> void:
	if _syncing_list:
		return
	_select_index(index)


func _unhandled_key_input(event: InputEvent) -> void:
	if not (event is InputEventKey):
		return
	var key := event as InputEventKey
	if not key.pressed or key.echo:
		return
	match key.keycode:
		KEY_PAGEUP:
			_page_up()
		KEY_PAGEDOWN:
			_page_down()
		KEY_UP:
			_select_index(_selected_index - 1)
		KEY_DOWN:
			_select_index(_selected_index + 1)
		KEY_1, KEY_2, KEY_3, KEY_4, KEY_5, KEY_6, KEY_7, KEY_8:
			_select_page_number(key.keycode - KEY_0)
		_:
			return
	get_viewport().set_input_as_handled()


# --- Rendering -------------------------------------------------------------

func _render_all() -> void:
	_title_label.text = _i18n.t("base.ui.settlement_screen.title")
	_authority_label.text = _i18n.t(
		"base.ui.settlement_screen.authority",
		{"mode": _i18n.t("base.ui.settlement_screen.authority_local")}
	)
	_help_label.text = _i18n.t("base.ui.settlement_screen.help")
	_render_list()
	_render_detail()


func _render_list() -> void:
	_syncing_list = true
	_list.clear()
	for row in _rows:
		_list.add_item(_row_text(row as Dictionary))
	if not _rows.is_empty():
		_list.select(_selected_index)
	_syncing_list = false

	_page_label.text = _i18n.t(
		"base.ui.settlement_screen.page",
		{
			"page": _i18n.format_int(_page_index + 1),
			"pages": _i18n.format_int(_page_count()),
			"count": _i18n.format_int(_rows.size()),
			"total": _i18n.format_int(_matched),
		}
	)
	var remaining := maxi(0, _matched - mini(_matched, (_page_index + 1) * PAGE_SIZE))
	if remaining > 0:
		_more_label.text = _i18n.t(
			"base.ui.settlement_screen.footer_more",
			{"count": _i18n.format_int(remaining)}
		)
	else:
		_more_label.text = _i18n.t("base.ui.settlement_screen.footer_end")


func _render_detail() -> void:
	_clear_members()
	if _rows.is_empty() or _selected_index >= _rows.size():
		_detail_title.text = ""
		_detail_center.text = ""
		_detail_owner.text = ""
		_detail_tax.text = ""
		_detail_revision.text = ""
		_projection_label.text = ""
		_members_title.text = ""
		_detail_empty.text = _i18n.t("base.ui.settlement_screen.detail_empty")
		_detail_empty.visible = true
		_status_label.text = _i18n.t("base.ui.settlement_screen.empty") if _error_code.is_empty() else ""
		return
	_detail_empty.visible = false

	var settlement: Dictionary = _rows[_selected_index]
	var id := int(settlement.get("id", 0))
	var name := str(settlement.get("name", ""))
	var center := _center(settlement)
	var tax := clampi(int(settlement.get("tax_rate", 0)), 0, MAX_TAX_RATE)

	_detail_title.text = _i18n.t(
		"base.ui.settlement_screen.detail_title",
		{"name": name, "id": str(id)}
	)
	_detail_center.text = _i18n.t(
		"base.ui.settlement_screen.detail_center",
		{
			"x": _i18n.format_int(center[0]),
			"z": _i18n.format_int(center[1]),
			"radius": _i18n.format_int(int(settlement.get("radius_m", 0))),
		}
	)
	_detail_owner.text = _i18n.t(
		"base.ui.settlement_screen.owner_label",
		{"owner": _owner_text(settlement)}
	)
	_detail_tax.text = _i18n.t(
		"base.ui.settlement_screen.detail_tax",
		{"tax": _i18n.format_int(tax)}
	)
	_detail_revision.text = _i18n.t(
		"base.ui.settlement_screen.detail_revision",
		{"revision": _i18n.format_int(int(settlement.get("membership_revision", 0)))}
	)
	_projection_label.text = _projection_text(tax)

	var members = settlement.get("members", [])
	var member_rows: Array = members if members is Array else []
	_members_title.text = _i18n.t(
		"base.ui.settlement_screen.detail_members",
		{"count": _i18n.format_int(_member_display_count(settlement, member_rows))}
	)
	var rendered := 0
	for member in member_rows:
		if not (member is Dictionary):
			continue
		var row: Dictionary = member
		if _is_owner_row(settlement, row):
			continue
		var label := Label.new()
		label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
		label.text = _member_text(row)
		_members_box.add_child(label)
		rendered += 1
	if rendered == 0:
		var empty := Label.new()
		empty.text = _i18n.t("base.ui.settlement_screen.detail_no_members")
		empty.add_theme_color_override("font_color", MUTED_COLOR)
		_members_box.add_child(empty)
	_status_label.text = ""


func _clear_members() -> void:
	for child in _members_box.get_children():
		_members_box.remove_child(child)
		child.free()


func _row_text(settlement: Dictionary) -> String:
	var members = settlement.get("members", [])
	var member_count := (members as Array).size() if members is Array else 0
	return _i18n.t(
		"base.ui.settlement_screen.row",
		{
			"name": str(settlement.get("name", "")),
			"id": str(int(settlement.get("id", 0))),
			"owner": _owner_text(settlement),
			"tax": _i18n.format_int(int(settlement.get("tax_rate", 0))),
			"revision": _i18n.format_int(int(settlement.get("membership_revision", 0))),
			"members": _i18n.format_int(member_count),
		}
	)


func _owner_text(settlement: Dictionary) -> String:
	if not bool(settlement.get("owned", false)):
		return _i18n.t("base.ui.settlement_screen.owner_unowned")
	var owner_id = settlement.get("owner_id")
	if owner_id == null:
		var legacy := str(settlement.get("owner", ""))
		if legacy.is_empty():
			return _i18n.t("base.ui.settlement_screen.owner_unowned")
		return legacy
	return _person_text(int(owner_id))


func _person_text(id: int) -> String:
	var name := str(_player_names.get(id, ""))
	if name.is_empty():
		return _i18n.t("base.ui.settlement_screen.person_unknown", {"id": str(id)})
	return _i18n.t("base.ui.settlement_screen.person", {"name": name, "id": str(id)})


func _member_text(member: Dictionary) -> String:
	var role := str(member.get("role", ""))
	return _i18n.t(
		"base.ui.settlement_screen.member",
		{
			"person": _person_text(int(member.get("player", 0))),
			"role": _role_text(role),
			"permissions": _permission_text(int(member.get("permissions", 0))),
		}
	)


func _role_text(role: String) -> String:
	if role.is_empty():
		return role
	var key := "base.ui.settlement_screen.role.%s" % role
	if _i18n.has_key(key, "en") or _i18n.has_key(key, "de"):
		return _i18n.t(key)
	return role


func _permission_text(bits: int) -> String:
	var tokens: Array[String] = []
	for entry in MEMBER_PERMISSION_KEYS:
		if (bits & int(entry[0])) != 0:
			tokens.append(_i18n.t(str(entry[1])))
	if tokens.is_empty():
		return _i18n.t("base.ui.settlement_screen.permissions_none")
	return ", ".join(tokens)


func _is_owner_row(settlement: Dictionary, member: Dictionary) -> bool:
	var owner_id = settlement.get("owner_id")
	return owner_id != null and int(member.get("player", 0)) == int(owner_id)


func _member_display_count(settlement: Dictionary, members: Array) -> int:
	var count := members.size()
	if _selected_has_owner_in_members(settlement, members):
		count -= 1
	return maxi(0, count)


func _selected_has_owner_in_members(settlement: Dictionary, members: Array) -> bool:
	for member in members:
		if member is Dictionary and _is_owner_row(settlement, member):
			return true
	return false


func _center(settlement: Dictionary) -> Array:
	var value = settlement.get("center", PackedInt32Array())
	if value is PackedInt32Array:
		if value.size() >= 2:
			return [value[0], value[1]]
		return [0, 0]
	if value is Array and value.size() >= 2:
		return [int(value[0]), int(value[1])]
	return [0, 0]


## D-039 preview. Stateless bridge helper; never mutates the store.
func _projection_text(rate: int) -> String:
	if _core == null:
		return _i18n.t(
			"base.ui.settlement_screen.projection_unavailable",
			{"code": "no_core"}
		)
	var result: Dictionary = _core.tax_projection(rate)
	if not bool(result.get("ok", false)):
		return _i18n.t(
			"base.ui.settlement_screen.projection_unavailable",
			{"code": str(result.get("error_code", "error"))}
		)
	return _i18n.t(
		"base.ui.settlement_screen.projection",
		{"multiplier": _i18n.format_float(float(result.get("combined_multiplier", 1.0)), 2)}
	)


func _show_unavailable(code: String) -> void:
	_error_code = code
	_rows = []
	_matched = 0
	_selected_index = 0
	_selected_id = 0
	_list.clear()
	_page_label.text = ""
	_more_label.text = ""
	_help_label.text = ""
	_detail_title.text = ""
	_detail_center.text = ""
	_detail_owner.text = ""
	_detail_tax.text = ""
	_detail_revision.text = ""
	_projection_label.text = ""
	_members_title.text = ""
	_clear_members()
	_detail_empty.text = ""
	_detail_empty.visible = false
	_status_label.text = _i18n.t(
		"base.ui.settlement_screen.unavailable",
		{"code": code}
	)


# --- Smoke -----------------------------------------------------------------

func _run_smoke() -> void:
	var failures: Array[String] = []
	_core = TownsmithCore.new()
	_seed = DEFAULT_SEED
	# Open a world layer first: the bridge player registry needs an open-store
	# seed before it will register names.
	_seed_fixture_settlements()
	_register_fixture_players()
	_refresh_player_names()
	_cursor_stack = [0]
	_page_index = 0
	_selected_id = 0
	_load_page(0)

	# Paging: 23 settlements at 8 per cursor page -> 8 / 8 / 7.
	_expect(_rows.size() == 8, "page1 size 8, got %d" % _rows.size(), failures)
	_expect(_page_count() == 3, "page count 3, got %d" % _page_count(), failures)
	_expect(_matched == 23, "matched 23, got %d" % _matched, failures)
	_expect(_has_more, "page1 has_more", failures)
	_page_down()
	_expect(_page_index == 1 and _rows.size() == 8, "page2 8 rows", failures)
	_page_down()
	_expect(_page_index == 2 and _rows.size() == 7, "page3 7 rows", failures)
	_expect(not _has_more, "page3 terminal", failures)
	_expect(int((_rows[0] as Dictionary).get("id", 0)) == 17, "page3 starts at 17", failures)
	_expect(int((_rows[6] as Dictionary).get("id", 0)) == 23, "page3 ends at 23", failures)

	# Page-local number selection: 7 on page 3 picks id 23; 8 is out of range.
	_select_page_number(7)
	_expect(_selected_id == 23, "number 7 selects id 23, got %d" % _selected_id, failures)
	_expect(_detail_title.text.contains("#23"), "detail shows #23", failures)
	_select_page_number(8)
	_expect(_selected_id == 23, "number 8 is a no-op on a 7-row page", failures)

	# Page-local selection resets to the first row of a fresh page.
	_page_up()
	_expect(_page_index == 1 and _selected_id == 9, "page up selects 9", failures)
	_page_up()
	_select_page_number(1)
	_expect(_page_index == 0 and _selected_id == 1, "page 1 row 1 is id 1", failures)

	# Owned detail: owner name/id, manager/builder/member roles, permission
	# tokens, and the unregistered member `#id` fallback.
	_select_page_number(2)
	_expect(_selected_id == 2, "row 2 selects settlement 2", failures)
	var en_detail := _detail_text()
	_expect(en_detail.contains("Mayor #7"), "owner name/id: %s" % en_detail, failures)
	_expect(en_detail.contains("Manager #11"), "manager name/id", failures)
	_expect(en_detail.contains("Builder #13"), "builder name/id", failures)
	_expect(en_detail.contains("Member #12"), "member name/id", failures)
	_expect(en_detail.contains("#9001"), "unregistered member id fallback", failures)
	_expect(en_detail.contains("build, manage, tax, members"), "permission tokens", failures)
	_expect(en_detail.contains("no permissions"), "no-permission token", failures)
	_expect(_projection_label.text.contains("x1.05"), "settlement 2 tax 5 projection", failures)

	# Owner fallback through a real claimed record with no registry name.
	_select_page_number(3)
	_expect(_selected_id == 3, "row 3 selects settlement 3", failures)
	_expect(_detail_owner.text.contains("#9002"), "unregistered owner fallback: %s" % _detail_owner.text, failures)

	# Unowned record on the last page.
	_page_down()
	_page_down()
	_select_page_number(7)
	_expect(_selected_id == 23, "unowned row is id 23", failures)
	_expect(_detail_owner.text.contains(_i18n.t("base.ui.settlement_screen.owner_unowned")), "unowned detail", failures)

	# D-039 preview values: 0%, 5%, 100%, invalid.
	var p0 := _projection_text(0)
	var p5 := _projection_text(5)
	var p100 := _projection_text(100)
	var pbad := _projection_text(MAX_TAX_RATE + 1)
	_expect(p0.contains("x1.00") and p0.contains("preview, D-039 proposed"), "projection 0: %s" % p0, failures)
	_expect(p5.contains("x1.05"), "projection 5: %s" % p5, failures)
	_expect(p100.contains("x2.00"), "projection 100: %s" % p100, failures)
	_expect(pbad.contains("tax.projection.rate_out_of_range"), "projection invalid: %s" % pbad, failures)

	# EN/DE key coverage with no `!base.ui.` leaks and localized role tokens.
	_load_page(0)
	_select_page_number(2)
	_expect(not _all_text().contains("!base.ui."), "EN key leak", failures)
	_switch_locale("de")
	var de_detail := _detail_text()
	_expect(not _all_text().contains("!base.ui."), "DE key leak", failures)
	_expect(_title_label.text == "Siedlungen", "DE title", failures)
	_expect(de_detail.contains("Eigentümer: Mayor #7"), "DE owner: %s" % de_detail, failures)
	_expect(de_detail.contains("Manager #11 · Verwalter"), "DE manager role", failures)
	_expect(de_detail.contains("bauen, verwalten, Steuern, Mitglieder"), "DE permissions", failures)
	_expect(de_detail.contains("Prognostizierter lokaler Multiplikator: x1,05"), "DE projection", failures)
	_switch_locale("en")
	_select_page_number(2)

	print("SETTLEMENT_SCREEN_PAGES sizes=8/8/7 selected_last=23")
	print("SETTLEMENT_SCREEN_NAMES owner=Mayor#7 manager=Manager#11 builder=Builder#13 member=Member#12 fallback=#9001 owner_fallback=#9002")
	print("SETTLEMENT_SCREEN_PROJECTION tax0=x1.00 tax5=x1.05 tax100=x2.00 invalid=tax.projection.rate_out_of_range")
	print("SETTLEMENT_SCREEN_LOCALE en=ok de=ok")

	if failures.is_empty():
		print(
			"SETTLEMENT_SCREEN status=ok pages=%d settlements=%d selected=%d checksum=%s"
			% [_page_count(), _matched, _selected_id, _checksum]
		)
		get_tree().quit(0)
	else:
		print("SETTLEMENT_SCREEN status=failed reason=%s" % failures[0])
		for failure in failures:
			print("SETTLEMENT_SCREEN_FAIL %s" % failure)
		get_tree().quit(1)


func _register_fixture_players() -> void:
	for id in range(1, FIXTURE_PLAYER_MAX_ID + 1):
		var name := str(FIXTURE_PLAYER_NAMES.get(id, "Player %d" % id))
		_core.players_register(name)


func _seed_fixture_settlements() -> void:
	for i in range(1, FIXTURE_SETTLEMENT_COUNT + 1):
		var owner := "player" if i < FIXTURE_SETTLEMENT_COUNT else FIXTURE_UNOWNED_OWNER
		var found: Dictionary = _core.settlement_found(
			_seed,
			"Smoke %02d" % i,
			owner,
			(i - 1) * 4096,
			0,
			256 + i * 8
		)
		if not bool(found.get("ok", false)):
			continue
		var id := int((found.get("settlement", {}) as Dictionary).get("id", 0))
		if i >= FIXTURE_SETTLEMENT_COUNT:
			continue
		var actor := FIXTURE_FALLBACK_OWNER_ID if i == 3 else 7
		var claimed: Dictionary = _core.settlement_claim(id, actor, 0)
		if not bool(claimed.get("ok", false)):
			continue
		var revision := int((claimed.get("settlement", {}) as Dictionary).get("membership_revision", 0))
		if i == 1:
			_core.settlement_set_tax(id, 0)
		elif i == 2:
			_core.settlement_set_tax(id, 5)
		elif i == 3:
			_core.settlement_set_tax(id, MAX_TAX_RATE)
		else:
			_core.settlement_set_tax(id, (i * 7) % (MAX_TAX_RATE + 1))
		if i == 2:
			var invites := [
				[11, "manager"],
				[13, "builder"],
				[12, "member"],
				[FIXTURE_FALLBACK_MEMBER_ID, "member"],
			]
			for invite in invites:
				var updated: Dictionary = _core.settlement_invite(
					id, int(invite[0]), str(invite[1]), revision
				)
				revision = int((updated.get("settlement", {}) as Dictionary).get("membership_revision", revision))


func _switch_locale(locale: String) -> void:
	_i18n.set_locale(locale)
	_render_all()


func _detail_text() -> String:
	var parts: Array[String] = [
		_detail_title.text,
		_detail_center.text,
		_detail_owner.text,
		_detail_tax.text,
		_detail_revision.text,
		_projection_label.text,
		_members_title.text,
	]
	for child in _members_box.get_children():
		if child is Label:
			parts.append((child as Label).text)
	return "\n".join(parts)


func _all_text() -> String:
	var parts: Array[String] = [
		_title_label.text,
		_authority_label.text,
		_page_label.text,
		_more_label.text,
		_help_label.text,
		_detail_empty.text,
		_status_label.text,
		_detail_text(),
	]
	for index in _list.item_count:
		parts.append(_list.get_item_text(index))
	return "\n".join(parts)


func _expect(condition: bool, label: String, failures: Array) -> void:
	if not condition:
		failures.append(label)
