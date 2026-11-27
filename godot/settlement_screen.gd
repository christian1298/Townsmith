extends Control

## Emitted when an embedded host should close the overlay (Esc in shared mode).
signal close_requested

## Standalone production settlement screen shell (local bridge mutations).
##
## Implements `docs/85 - Connected Settlement Surface.md` §7 over the local
## bridge: a cursor-paged list over `settlements_page(after_id, limit)`, the
## detail pane with `players_list()` names and the D-039 `tax_projection(rate)`
## preview, plus local claim, rename, tax, invite, role, remove, transfer, and
## release actions. The header records the `Local` authority mode.
##
## Every mutation re-reads the selected record/page first and sends the fresh
## `membership_revision`; a rejection never changes a row, the selected member,
## or the page. Rename and the destructive actions use an explicit confirm or
## two-press flow. Settlement founding stays outside this screen.
##
## Run:
##   godot --path godot res://settlement_screen.tscn
##   godot --path godot res://settlement_screen.tscn -- --empty
##   godot --headless --path godot res://settlement_screen.tscn -- --smoke
##   godot --path godot res://settlement_screen.tscn -- --connected --host H --port P --name NAME
##   godot --headless --path godot res://settlement_screen.tscn -- --smoke --connected --port P
##
## Embedded mode (`world_placement.gd`, `F10`): the host calls
## `configure_shared(core, seed)` before adding this scene, so the screen reads,
## mutates, and projects tax over the live world core instead of creating a
## second `TownsmithCore`. The screen never frees the injected core, emits
## `close_requested` on `Esc`, and `is_visible_in_tree()` gates all input while
## the host keeps the overlay hidden. The standalone launch path still creates
## and owns its own local core.
##
## Final smoke markers, printed twice with identical output:
##   SETTLEMENT_SCREEN status=ok pages=3 settlements=23 actions=13 accepted=8 rejected=5 checksum=<16 hex>
##   SETTLEMENT_SCREEN_CONNECTED status=ok settlements=1 players=2 actions=10 accepted=8 rejected=2 checksum=<16 hex>
##
## Connected mode is the server-authoritative slice: reads come from
## `settlement_list`/`player_list`, and claim, rename, tax, invite, set-role,
## remove, transfer, and release all send the D-033 commands through
## `settlement_screen_net.gd`. Every mutation re-reads the selected record and
## uses that fresh `membership_revision`; a reject keeps the cached row, the
## selection, and the world checksum unchanged and shows the stable code.
## Release is enabled for an owned record because the server owns the Admin
## ceiling: a Mayor session gets `role.forbidden` back and shows the localized
## `release requires Admin` reason. Local mode is unchanged.

const WorldI18n = preload("res://world_i18n.gd")
const SettlementScreenNet = preload("res://settlement_screen_net.gd")

const SUITE := "settlement_screen"
const DEFAULT_SEED := 2026
const PAGE_SIZE := 8
const MAX_TAX_RATE := 100
const TAX_STEP := 5
const NAME_MAX_CHARS := 48
const MEMBER_ROLE_CYCLE := ["member", "builder", "manager"]

## Visible action order and keyboard shortcuts. Number keys 1–8 and the page
## keys stay list navigation; these are the only action shortcuts.
const ACTION_ORDER := [
	["claim", "C"],
	["rename", "N"],
	["tax", "T"],
	["transfer", "Y"],
	["invite", "I"],
	["cycle_role", "M"],
	["remove_member", "Del"],
	["release", "X"],
]
const ACTION_LABEL_KEYS := {
	"claim": "base.ui.settlement_screen.action.claim",
	"rename": "base.ui.settlement_screen.action.rename",
	"tax": "base.ui.settlement_screen.action.tax",
	"transfer": "base.ui.settlement_screen.action.transfer",
	"invite": "base.ui.settlement_screen.action.invite",
	"cycle_role": "base.ui.settlement_screen.action.cycle_role",
	"remove_member": "base.ui.settlement_screen.action.remove_member",
	"release": "base.ui.settlement_screen.action.release",
}

## Fixed Tab order from docs/85 §7.2. Rows, detail, and members are skipped
## while they have no content; tax and the action bar are always present.
const FOCUS_GROUPS := ["header", "paging", "rows", "detail", "members", "tax", "actions"]
## Actions reachable from the focus model's action group. Tax is its own stop.
const FOCUS_ACTION_ORDER := [
	"claim",
	"rename",
	"transfer",
	"invite",
	"cycle_role",
	"remove_member",
	"release",
]
const MODAL_EDIT := 0
const MODAL_CANCEL := 1
const MODAL_SAVE := 2
const MODAL_FOCUS_ORDER := [MODAL_EDIT, MODAL_CANCEL, MODAL_SAVE]
## Deterministic gamepad actions. Live joypad events resolve through the input
## map first, then the local button fallbacks, so a stock map still works.
const GAMEPAD_ACTIONS := [
	"ui_up",
	"ui_down",
	"ui_left",
	"ui_right",
	"ui_accept",
	"ui_cancel",
]
const STICK_DEADZONE := 0.5
## Local rename codes are shape validation, not server rejection codes. They
## map to the `rename.*` text keys instead of leaking the raw code.
const RENAME_ERROR_KEYS := {
	"empty_name": "base.ui.settlement_screen.rename.empty",
	"name_too_long": "base.ui.settlement_screen.rename.too_long",
	"name_unchanged": "base.ui.settlement_screen.rename.unchanged",
	"invalid_name": "base.ui.settlement_screen.rename.invalid",
}

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
const FOCUS_COLOR := Color(0.36, 0.78, 1.0, 0.95)
const FOCUS_HALO_COLOR := Color(0.36, 0.78, 1.0, 0.3)

const DEFAULT_HOST := "127.0.0.1"
const DEFAULT_PORT := 47015
const DEFAULT_PLAYER_NAME := "settlement-screen"
const CONNECTED_FOUND_NAME := "Harbor"
const CONNECTED_RENAMED_NAME := "Port Harbor"
const CONNECTED_FOUND_RADIUS_M := 64
## Smoke-only second session. The main Mayor session opens first, so the roster
## is Mayor=1, Builder=2 before the screen reads it.
const CONNECTED_SEED_PLAYER := "Builder"

var smoke_mode := false
var empty_mode := false
var unavailable_mode := false
var _connected_mode := false

var _i18n
var _core = null
var _shared_core = null
var _shared := false
var _net = null
var _seed := DEFAULT_SEED
var _locale_override := ""
var _host := DEFAULT_HOST
var _port := DEFAULT_PORT
var _player_name := DEFAULT_PLAYER_NAME
var _world_checksum := ""

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

# Mutation state. `_selected_member` is a player id, so a page refresh keeps
# the member selection by identity. Pending confirmations are scoped to the
# settlement id and player id that armed them.
var _selected_member := 0
var _pending_remove_id := 0
var _pending_remove_player := 0
var _pending_release_id := 0
var _status_text := ""
var _rename_open := false
var _rename_target_id := 0
var _action_count := 0
var _accepted_count := 0
var _rejected_count := 0

# Focus model. The screen owns focus explicitly so keyboard, mouse, and the
# headless smoke all move through the same deterministic groups.
var _focus_group := "header"
var _focus_paging := 0
var _focus_action := 0
var _focus_member_index := 0
var _modal_focus := MODAL_EDIT
var _modal_opener_group := "actions"
var _modal_opener_action := "rename"

# Gamepad modality. This is presentation state only: every action routes through
# the same focus groups and handlers as the keyboard, and it never mutates the
# settlement store by itself.
var _gamepad_active := false
var _last_action := ""
var _stick_held := false
var _virtual_keyboard_requested := false
var _virtual_keyboard_requests := 0
var _virtual_keyboard_clears := 0

var _title_label: Label
var _authority_label: Label
var _world_checksum_label: Label
var _header: HBoxContainer
var _page_bar: HBoxContainer
var _page_prev: Button
var _page_next: Button
var _page_label: Label
var _list: ItemList
var _more_label: Label
var _help_label: Label
var _detail_box: VBoxContainer
var _detail_title: Label
var _detail_center: Label
var _detail_owner: Label
var _detail_tax: Label
var _detail_revision: Label
var _projection_label: Label
var _members_title: Label
var _members_box: VBoxContainer
var _member_buttons: Array[Button] = []
var _detail_empty: Label
var _status_label: Label
var _action_bar: HFlowContainer
var _action_buttons: Dictionary = {}
var _focus_ring: Control

var _rename_overlay: Control
var _rename_title: Label
var _rename_edit: LineEdit
var _rename_count: Label
var _rename_error: Label
var _rename_cancel: Button
var _rename_save: Button


## Host entry point for embedded mode. Call before `add_child`; the screen
## never frees or resets the injected core.
func configure_shared(core: Object, seed: int) -> void:
	_shared = true
	_shared_core = core
	_seed = seed


func is_shared() -> bool:
	return _shared


func _ready() -> void:
	if not _shared:
		_parse_args()
	# The headless contract is pinned in English; `--locale de` previews the
	# localized UI and the smoke switches locales itself.
	var locale := _locale_override
	if locale.is_empty() and smoke_mode:
		locale = "en"
	_i18n = WorldI18n.new(locale)
	_build_ui()
	if _shared:
		_core = _shared_core
		_refresh_player_names()
		_load_page(0)
		return
	if _connected_mode:
		if smoke_mode:
			_run_connected_smoke()
		else:
			_start_connected_session()
	elif smoke_mode:
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
		elif arg == "--connected":
			_connected_mode = true
		elif arg == "--empty":
			empty_mode = true
		elif arg == "--unavailable":
			unavailable_mode = true
		elif arg == "--host" and index + 1 < args.size():
			index += 1
			_host = args[index]
		elif arg == "--port" and index + 1 < args.size():
			index += 1
			var port_text: String = args[index]
			if port_text.is_valid_int() and int(port_text) >= 1 and int(port_text) <= 65535:
				_port = int(port_text)
		elif arg == "--name" and index + 1 < args.size():
			index += 1
			_player_name = args[index]
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
	header.mouse_filter = Control.MOUSE_FILTER_STOP
	header.gui_input.connect(_on_header_gui_input)
	_header = header
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
	_world_checksum_label = Label.new()
	_world_checksum_label.add_theme_color_override("font_color", MUTED_COLOR)
	_world_checksum_label.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT
	header.add_child(_world_checksum_label)
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

	_page_bar = HBoxContainer.new()
	_page_bar.add_theme_constant_override("separation", 6)
	left.add_child(_page_bar)
	_page_prev = Button.new()
	_page_prev.focus_mode = Control.FOCUS_NONE
	_page_prev.pressed.connect(_on_page_prev_pressed)
	_page_bar.add_child(_page_prev)
	_page_label = Label.new()
	_page_label.add_theme_color_override("font_color", MUTED_COLOR)
	_page_label.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
	_page_label.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	_page_bar.add_child(_page_label)
	_page_next = Button.new()
	_page_next.focus_mode = Control.FOCUS_NONE
	_page_next.pressed.connect(_on_page_next_pressed)
	_page_bar.add_child(_page_next)

	_list = ItemList.new()
	_list.size_flags_vertical = Control.SIZE_EXPAND_FILL
	_list.custom_minimum_size = Vector2(0, PAGE_SIZE * 30 + 20)
	_list.allow_reselect = true
	_list.focus_mode = Control.FOCUS_NONE
	_list.item_selected.connect(_on_item_selected)
	_list.item_clicked.connect(_on_item_clicked)
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

	_detail_box = VBoxContainer.new()
	_detail_box.add_theme_constant_override("separation", 6)
	_detail_box.mouse_filter = Control.MOUSE_FILTER_STOP
	_detail_box.gui_input.connect(_on_detail_gui_input)
	right.add_child(_detail_box)
	_detail_title = _detail_label(_detail_box, 20)
	_detail_center = _detail_label(_detail_box)
	_detail_owner = _detail_label(_detail_box)
	_detail_tax = _detail_label(_detail_box)
	_detail_revision = _detail_label(_detail_box)
	_projection_label = _detail_label(_detail_box)
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
	_action_bar = HFlowContainer.new()
	_action_bar.add_theme_constant_override("h_separation", 6)
	_action_bar.add_theme_constant_override("v_separation", 4)
	root.add_child(_action_bar)
	for entry in ACTION_ORDER:
		var name := str(entry[0])
		var button := Button.new()
		button.focus_mode = Control.FOCUS_NONE
		button.pressed.connect(_on_action_pressed.bind(name))
		button.gui_input.connect(_on_action_gui_input.bind(name))
		_action_bar.add_child(button)
		_action_buttons[name] = button

	_status_label = Label.new()
	_status_label.add_theme_color_override("font_color", MUTED_COLOR)
	_status_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	root.add_child(_status_label)

	# Full-rect ring drawn above the screen content. It follows the explicit
	# focus target, so mouse clicks, Tab, and render rebuilds agree.
	_focus_ring = Control.new()
	_focus_ring.set_anchors_preset(Control.PRESET_FULL_RECT)
	_focus_ring.mouse_filter = Control.MOUSE_FILTER_IGNORE
	_focus_ring.z_index = 100
	_focus_ring.draw.connect(_draw_focus_ring)
	add_child(_focus_ring)

	_build_rename_modal()


func _build_rename_modal() -> void:
	_rename_overlay = Control.new()
	_rename_overlay.set_anchors_preset(Control.PRESET_FULL_RECT)
	_rename_overlay.visible = false
	_rename_overlay.mouse_filter = Control.MOUSE_FILTER_STOP
	add_child(_rename_overlay)

	var shade := ColorRect.new()
	shade.set_anchors_preset(Control.PRESET_FULL_RECT)
	shade.color = Color(0.02, 0.03, 0.05, 0.72)
	shade.mouse_filter = Control.MOUSE_FILTER_STOP
	_rename_overlay.add_child(shade)

	var center := CenterContainer.new()
	center.set_anchors_preset(Control.PRESET_FULL_RECT)
	_rename_overlay.add_child(center)

	var panel := PanelContainer.new()
	panel.custom_minimum_size = Vector2(440, 0)
	center.add_child(panel)

	var box := VBoxContainer.new()
	box.add_theme_constant_override("separation", 8)
	panel.add_child(box)

	_rename_title = Label.new()
	_rename_title.add_theme_font_size_override("font_size", 18)
	_rename_title.add_theme_color_override("font_color", TEXT_COLOR)
	box.add_child(_rename_title)

	_rename_edit = LineEdit.new()
	_rename_edit.max_length = NAME_MAX_CHARS
	_rename_edit.select_all_on_focus = true
	_rename_edit.text_changed.connect(_on_rename_text_changed)
	_rename_edit.text_submitted.connect(func(_text: String) -> void: _save_rename())
	box.add_child(_rename_edit)

	_rename_count = Label.new()
	_rename_count.add_theme_color_override("font_color", MUTED_COLOR)
	box.add_child(_rename_count)

	_rename_error = Label.new()
	_rename_error.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	_rename_error.add_theme_color_override("font_color", Color(0.95, 0.55, 0.48))
	box.add_child(_rename_error)

	var row := HBoxContainer.new()
	row.add_theme_constant_override("separation", 8)
	box.add_child(row)
	var spacer := Control.new()
	spacer.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	row.add_child(spacer)
	_rename_cancel = Button.new()
	_rename_cancel.focus_mode = Control.FOCUS_NONE
	_rename_cancel.text = _i18n.t("base.ui.settlement_screen.rename.cancel")
	_rename_cancel.pressed.connect(_close_rename)
	row.add_child(_rename_cancel)
	_rename_save = Button.new()
	_rename_save.focus_mode = Control.FOCUS_NONE
	_rename_save.text = _i18n.t("base.ui.settlement_screen.rename.save")
	_rename_save.pressed.connect(_save_rename)
	row.add_child(_rename_save)


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
	if _shared and _shared_core != null:
		_core = _shared_core
	else:
		_core = TownsmithCore.new()
	_refresh_player_names()
	_load_page(0)


func _start_connected_session() -> void:
	_net = SettlementScreenNet.new()
	var opened: Dictionary = _net.open(_host, _port, _player_name)
	if not bool(opened.get("ok", false)):
		_show_unavailable(str(opened.get("error_code", "connect_failed")))
		_render_authority()
		return
	_refresh_player_names()
	_load_page(0)
	if _error_code.is_empty():
		_status_text = _i18n.t(
			"base.ui.settlement_screen.connected_status",
			{
				"host": _host,
				"port": str(_port),
				"name": _player_name,
			}
		)
		_render_detail()


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
	if _connected_mode:
		_refresh_connected_player_names()
		return
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


## Connected name cache: page `player_list` from cursor 0 until `has_more`
## clears. The registry is capped at 256 players, so 64 pages of 8 is bounded.
func _refresh_connected_player_names() -> void:
	if _net == null:
		return
	var after := 0
	for _page in range(64):
		var response: Dictionary = _net.request({
			"type": "player_list",
			"after_id": after,
			"limit": PAGE_SIZE,
		})
		if not bool(response.get("ok", false)):
			return
		var message: Dictionary = response.get("message", {})
		if str(message.get("type", "")) != "player_list":
			return
		var players = message.get("players", [])
		if not (players is Array):
			return
		for record in players:
			if not (record is Dictionary):
				continue
			var id := int(record.get("id", 0))
			var name := str(record.get("name", ""))
			if id > 0 and not name.is_empty():
				_player_names[id] = name
		if not bool(message.get("has_more", false)):
			return
		var next_after := int(message.get("next_after", 0))
		if next_after <= after:
			return
		after = next_after


func _load_page(index: int) -> void:
	if _connected_mode:
		_load_connected_page(index)
		return
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
	_apply_page(index)


## Connected read: `settlement_list {after_id, limit}` with the same cursor
## semantics as the local bridge page. A failed read leaves the cache intact
## and shows the localized unavailable state.
func _load_connected_page(index: int) -> void:
	if _net == null:
		_show_unavailable("no_core")
		_render_authority()
		return
	if index < 0 or index >= _cursor_stack.size():
		return
	var after := int(_cursor_stack[index])
	var response: Dictionary = _net.request({
		"type": "settlement_list",
		"after_id": after,
		"limit": PAGE_SIZE,
	})
	if not bool(response.get("ok", false)):
		_show_unavailable(str(response.get("error_code", "error")))
		_render_authority()
		return
	var message: Dictionary = response.get("message", {})
	if str(message.get("type", "")) != "settlement_list":
		_show_unavailable("protocol.malformed")
		_render_authority()
		return
	var rows = message.get("settlements", [])
	if not (rows is Array):
		_show_unavailable("protocol.malformed")
		_render_authority()
		return
	_rows = rows
	_matched = int(message.get("matched", 0))
	_has_more = bool(message.get("has_more", false))
	_next_after = int(message.get("next_after", 0))
	_checksum = str(message.get("checksum", ""))
	_apply_page(index)


## Shared page application: cursor stack, selection by id, and render.
func _apply_page(index: int) -> void:
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
	if _selected_id != keep_id:
		_reset_pending()
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
	var next_id := int((_rows[index] as Dictionary).get("id", 0))
	if next_id != _selected_id:
		_reset_pending()
		_selected_member = 0
	_selected_index = index
	_selected_id = next_id
	if not _syncing_list:
		_syncing_list = true
		_list.select(index)
		_syncing_list = false
	_render_detail()
	_sync_focus_ring()


## A pending remove/release confirmation belongs to exactly the row and player
## that armed it. Changing selection or closing the modal clears it.
func _reset_pending() -> void:
	_pending_remove_id = 0
	_pending_remove_player = 0
	_pending_release_id = 0


func _select_page_number(number: int) -> void:
	if number < 1 or number > PAGE_SIZE:
		return
	_select_index(number - 1)


func _on_item_selected(index: int) -> void:
	if _syncing_list:
		return
	_clear_gamepad_modality()
	_focus_group = "rows"
	_select_index(index)
	_sync_focus_ring()


func _on_item_clicked(index: int, _at_position: Vector2, _mouse_button_index: int) -> void:
	_clear_gamepad_modality()
	_focus_group = "rows"
	_select_index(index)
	_sync_focus_ring()


func _on_page_prev_pressed() -> void:
	if not _set_focus_group("paging"):
		return
	_focus_paging = 0
	_page_up()


func _on_page_next_pressed() -> void:
	if not _set_focus_group("paging"):
		return
	_focus_paging = 1
	_page_down()


func _on_header_gui_input(event: InputEvent) -> void:
	if event is InputEventMouseButton and event.pressed:
		if (event as InputEventMouseButton).button_index == MOUSE_BUTTON_LEFT:
			_clear_gamepad_modality()
			_set_focus_group("header")


func _on_detail_gui_input(event: InputEvent) -> void:
	if event is InputEventMouseButton and event.pressed:
		if (event as InputEventMouseButton).button_index == MOUSE_BUTTON_LEFT:
			_clear_gamepad_modality()
			_set_focus_group("detail")


func _on_action_gui_input(event: InputEvent, action: String) -> void:
	if event is InputEventMouseButton and event.pressed:
		if (event as InputEventMouseButton).button_index == MOUSE_BUTTON_LEFT:
			_clear_gamepad_modality()
			_focus_action_control(action)


# --- Focus model -----------------------------------------------------------

## Engine key path. The smoke calls `_handle_key` directly so the exact same
## branch runs without a display or an Input singleton.
func _input(event: InputEvent) -> void:
	if _handle_key(event):
		get_viewport().set_input_as_handled()


func _process(_delta: float) -> void:
	if _shared and not is_visible_in_tree():
		return
	if _focus_ring != null:
		_focus_ring.queue_redraw()


func _handle_key(event: InputEvent) -> bool:
	if _shared and not is_visible_in_tree():
		return false
	if not (event is InputEventKey):
		return false
	var key := event as InputEventKey
	if not key.pressed or key.echo:
		return false
	_gamepad_active = false
	if _rename_open:
		return _handle_modal_key(key)
	if _line_edit_has_focus():
		return false
	return _handle_screen_key(key)


func _line_edit_has_focus() -> bool:
	var owner := get_viewport().gui_get_focus_owner()
	return owner is LineEdit and owner.is_visible_in_tree()


func _handle_modal_key(key: InputEventKey) -> bool:
	match key.keycode:
		KEY_ESCAPE:
			_close_rename()
			return true
		KEY_TAB:
			_cycle_modal_focus(-1 if key.shift_pressed else 1)
			return true
		KEY_HOME:
			_set_modal_focus(MODAL_EDIT)
			return true
		KEY_END:
			_set_modal_focus(MODAL_SAVE)
			return true
	if _modal_focus == MODAL_EDIT:
		# The field owns text, Space, and Enter; text_submitted saves.
		return false
	match key.keycode:
		KEY_UP, KEY_LEFT:
			_cycle_modal_focus(-1)
		KEY_DOWN, KEY_RIGHT:
			_cycle_modal_focus(1)
		KEY_ENTER, KEY_KP_ENTER, KEY_SPACE:
			_activate_modal_focus()
		_:
			return false
	return true


func _handle_screen_key(key: InputEventKey) -> bool:
	# In embedded mode Esc closes the overlay before the local detail-clearing
	# behavior; the host owns the show/hide lifecycle.
	if _shared and key.keycode == KEY_ESCAPE:
		close_requested.emit()
		return true
	match key.keycode:
		KEY_TAB:
			_cycle_focus(-1 if key.shift_pressed else 1)
		KEY_ESCAPE:
			_handle_escape()
		KEY_UP, KEY_LEFT:
			return _move_focus(-1)
		KEY_DOWN, KEY_RIGHT:
			return _move_focus(1)
		KEY_HOME:
			return _focus_edge(true)
		KEY_END:
			return _focus_edge(false)
		KEY_PAGEUP:
			if _focus_group == "paging" or _focus_group == "rows":
				_page_up()
		KEY_PAGEDOWN:
			if _focus_group == "paging" or _focus_group == "rows":
				_page_down()
		KEY_ENTER, KEY_KP_ENTER, KEY_SPACE:
			_activate_focus()
		KEY_1, KEY_2, KEY_3, KEY_4, KEY_5, KEY_6, KEY_7, KEY_8:
			_select_page_number(key.keycode - KEY_0)
			_set_focus_group("rows")
		KEY_BRACKETLEFT:
			if _set_focus_group("members"):
				_cycle_selected_member(-1)
		KEY_BRACKETRIGHT:
			if _set_focus_group("members"):
				_cycle_selected_member(1)
		KEY_C:
			_on_action_pressed("claim")
		KEY_N:
			_on_action_pressed("rename")
		KEY_T:
			_on_action_pressed("tax")
		KEY_Y:
			_on_action_pressed("transfer")
		KEY_I:
			_on_action_pressed("invite")
		KEY_M:
			_on_action_pressed("cycle_role")
		KEY_DELETE:
			_on_action_pressed("remove_member")
		KEY_X:
			_on_action_pressed("release")
		_:
			return false
	return true


## Process one live joypad event. Keys are handled by `_input`; this only
## handles pad buttons and the left stick, and consumes what it dispatches.
func _unhandled_input(event: InputEvent) -> void:
	var action := _gamepad_action_for_event(event)
	if action.is_empty():
		return
	if _dispatch_gamepad(action):
		get_viewport().set_input_as_handled()


func _gamepad_action_for_event(event: InputEvent) -> String:
	if event is InputEventJoypadButton:
		var button := event as InputEventJoypadButton
		if not button.pressed:
			return ""
		for action in GAMEPAD_ACTIONS:
			if InputMap.event_is_action(button, action):
				return action
		match button.button_index:
			JOY_BUTTON_A:
				return "ui_accept"
			JOY_BUTTON_B:
				return "ui_cancel"
			JOY_BUTTON_DPAD_UP:
				return "ui_up"
			JOY_BUTTON_DPAD_DOWN:
				return "ui_down"
			JOY_BUTTON_DPAD_LEFT:
				return "ui_left"
			JOY_BUTTON_DPAD_RIGHT:
				return "ui_right"
		return ""
	if event is InputEventJoypadMotion:
		var motion := event as InputEventJoypadMotion
		if motion.axis != JOY_AXIS_LEFT_X and motion.axis != JOY_AXIS_LEFT_Y:
			return ""
		if absf(motion.axis_value) < STICK_DEADZONE:
			_stick_held = false
			return ""
		if _stick_held:
			return ""
		_stick_held = true
		if motion.axis == JOY_AXIS_LEFT_X:
			return "ui_right" if motion.axis_value > 0.0 else "ui_left"
		return "ui_down" if motion.axis_value > 0.0 else "ui_up"
	return ""


## Apply one named gamepad action and return the resulting snapshot. Unknown
## actions are ignored and do not change the last action.
func gamepad_action(action: String) -> Dictionary:
	_dispatch_gamepad(action)
	return gamepad_snapshot()


func gamepad_snapshot() -> Dictionary:
	return {
		"active": _gamepad_active,
		"last_action": _last_action,
		"group": _focus_group,
		"selected": _selected_id,
		"focus_paging": _focus_paging,
		"focus_action": _focus_action,
		"modal": _rename_open,
		"modal_focus": _modal_focus,
		"vk_requested": _virtual_keyboard_requested,
		"vk_requests": _virtual_keyboard_requests,
		"vk_clears": _virtual_keyboard_clears,
	}


func _dispatch_gamepad(action: String) -> bool:
	if _shared and not is_visible_in_tree():
		return false
	if not GAMEPAD_ACTIONS.has(action):
		return false
	_gamepad_active = true
	_last_action = action
	if _rename_open:
		return _dispatch_modal_gamepad(action)
	match action:
		"ui_up":
			return _gamepad_vertical(-1)
		"ui_down":
			return _gamepad_vertical(1)
		"ui_left":
			return _gamepad_horizontal(-1)
		"ui_right":
			return _gamepad_horizontal(1)
		"ui_accept":
			return _activate_focus()
		"ui_cancel":
			if _shared:
				close_requested.emit()
				return true
			return _handle_escape()
	return false


func _dispatch_modal_gamepad(action: String) -> bool:
	match action:
		"ui_up", "ui_left":
			_cycle_modal_focus(-1)
		"ui_down", "ui_right":
			_cycle_modal_focus(1)
		"ui_accept":
			_activate_modal_focus()
		"ui_cancel":
			_close_rename()
		_:
			return false
	return true


## Move within the focused group first; only at an edge move to the adjacent
## group. This keeps row selection stable when crossing groups.
func _gamepad_vertical(direction: int) -> bool:
	var before := _gamepad_focus_key()
	_move_focus(direction)
	if _gamepad_focus_key() != before:
		return true
	return _gamepad_adjacent_group(direction)


func _gamepad_horizontal(direction: int) -> bool:
	match _focus_group:
		"paging":
			_focus_paging = 0 if direction < 0 else 1
			_sync_focus_ring()
			return true
		"rows", "members":
			return _gamepad_vertical(direction)
		"actions":
			_focus_action = clampi(
				_focus_action + direction, 0, FOCUS_ACTION_ORDER.size() - 1
			)
			_sync_focus_ring()
			return true
	return false


func _gamepad_focus_key() -> String:
	match _focus_group:
		"paging":
			return "paging:%d" % _focus_paging
		"rows":
			return "rows:%d" % _selected_index
		"members":
			return "members:%d" % _focus_member_index
		"actions":
			return "actions:%d" % _focus_action
	return _focus_group


func _gamepad_adjacent_group(direction: int) -> bool:
	var start := FOCUS_GROUPS.find(_focus_group)
	if start < 0:
		start = 0
	for step in range(1, FOCUS_GROUPS.size() + 1):
		var index := posmod(start + direction * step, FOCUS_GROUPS.size())
		var group := str(FOCUS_GROUPS[index])
		if _focus_group_available(group):
			return _set_focus_group(group)
	return false


func _clear_gamepad_modality() -> void:
	_gamepad_active = false


func _virtual_keyboard_supported() -> bool:
	if DisplayServer.get_name() == "headless":
		return false
	return DisplayServer.has_feature(DisplayServer.FEATURE_VIRTUAL_KEYBOARD)


## Request or clear the platform keyboard for the rename field. Intent is
## counted on every platform so the headless smoke stays deterministic; the
## display-server call is made only when the feature exists.
func _sync_virtual_keyboard(show_keyboard: bool) -> void:
	if show_keyboard:
		if _virtual_keyboard_requested:
			return
		_virtual_keyboard_requested = true
		_virtual_keyboard_requests += 1
		if _virtual_keyboard_supported():
			DisplayServer.virtual_keyboard_show(_rename_edit.text)
		return
	if not _virtual_keyboard_requested:
		return
	_virtual_keyboard_requested = false
	_virtual_keyboard_clears += 1
	if _virtual_keyboard_supported():
		DisplayServer.virtual_keyboard_hide()


## Esc returns to the list first; a second press clears the row selection.
func _handle_escape() -> bool:
	if _focus_group != "rows":
		if _set_focus_group("rows"):
			return true
	if _selected_id > 0:
		_clear_row_selection()
	return true


func _clear_row_selection() -> void:
	_selected_id = 0
	_selected_index = -1
	_selected_member = 0
	_reset_pending()
	_status_text = ""
	_syncing_list = true
	_list.deselect_all()
	_syncing_list = false
	_render_detail()
	_sync_focus_ring()


func _move_focus(direction: int) -> bool:
	match _focus_group:
		"paging":
			_focus_paging = clampi(_focus_paging + direction, 0, 1)
		"rows":
			_move_row_focus(direction)
		"members":
			_move_member_focus(direction)
		"actions":
			_focus_action = clampi(_focus_action + direction, 0, FOCUS_ACTION_ORDER.size() - 1)
		_:
			return true
	_sync_focus_ring()
	return true


func _focus_edge(first: bool) -> bool:
	match _focus_group:
		"paging":
			_focus_paging = 0 if first else 1
		"rows":
			_select_index(0 if first else _rows.size() - 1)
		"members":
			_focus_member_edge(first)
		"actions":
			_focus_action = 0 if first else FOCUS_ACTION_ORDER.size() - 1
		_:
			return true
	_sync_focus_ring()
	return true


func _move_row_focus(direction: int) -> void:
	if _rows.is_empty():
		return
	if _selected_index < 0 or _selected_index >= _rows.size():
		_select_index(0 if direction > 0 else _rows.size() - 1)
		return
	_select_index(clampi(_selected_index + direction, 0, _rows.size() - 1))


func _move_member_focus(direction: int) -> void:
	var members := _assignable_members(_current_record())
	if members.is_empty():
		_focus_group = "tax"
		return
	var index := _member_index_of(_selected_member, members)
	_focus_selected_member(members, clampi(index + direction, 0, members.size() - 1))


func _focus_member_edge(first: bool) -> void:
	var members := _assignable_members(_current_record())
	if members.is_empty():
		return
	_focus_selected_member(members, 0 if first else members.size() - 1)


func _focus_selected_member(members: Array, index: int) -> void:
	_selected_member = int((members[index] as Dictionary).get("player", 0))
	_focus_member_index = index
	_reset_pending()
	_render_detail()
	_sync_focus_ring()


func _member_index_of(player: int, members: Array) -> int:
	for index in members.size():
		if int((members[index] as Dictionary).get("player", 0)) == player:
			return index
	return 0


func _activate_focus() -> bool:
	match _focus_group:
		"paging":
			_activate_paging()
		"rows":
			if _selected_id > 0:
				_set_focus_group("detail")
		"detail":
			if not _set_focus_group("members"):
				_set_focus_group("tax")
		"members":
			_set_focus_group("tax")
		"tax":
			_on_action_pressed("tax")
		"actions":
			_on_action_pressed(_focused_action_name())
	return true


func _activate_paging() -> void:
	if _focus_paging == 0:
		if _page_index > 0:
			_page_up()
	elif _has_more:
		_page_down()


func _focused_action_name() -> String:
	if FOCUS_ACTION_ORDER.is_empty():
		return ""
	var index := clampi(_focus_action, 0, FOCUS_ACTION_ORDER.size() - 1)
	return str(FOCUS_ACTION_ORDER[index])


func _set_focus_group(group: String) -> bool:
	if not FOCUS_GROUPS.has(group) or not _focus_group_available(group):
		return false
	_focus_group = group
	match group:
		"paging":
			_focus_paging = clampi(_focus_paging, 0, 1)
		"rows":
			if _selected_id <= 0 and not _rows.is_empty():
				_select_index(0)
		"members":
			var members := _assignable_members(_current_record())
			_focus_member_index = _member_index_of(_selected_member, members)
		"actions":
			_focus_action = clampi(_focus_action, 0, FOCUS_ACTION_ORDER.size() - 1)
	_sync_focus_ring()
	return true


func _focus_group_available(group: String) -> bool:
	match group:
		"rows":
			return not _rows.is_empty()
		"detail":
			return _selected_id > 0
		"members":
			return not _assignable_members(_current_record()).is_empty()
	return true


func _cycle_focus(direction: int) -> void:
	var start := FOCUS_GROUPS.find(_focus_group)
	if start < 0:
		start = 0
	for step in range(1, FOCUS_GROUPS.size() + 1):
		var index := posmod(start + direction * step, FOCUS_GROUPS.size())
		var group := str(FOCUS_GROUPS[index])
		if _focus_group_available(group):
			_set_focus_group(group)
			return


func _focus_action_control(action: String) -> void:
	if action == "tax":
		_set_focus_group("tax")
		return
	var index := FOCUS_ACTION_ORDER.find(action)
	if index < 0 or not _set_focus_group("actions"):
		return
	_focus_action = index
	_sync_focus_ring()


func _cycle_modal_focus(direction: int) -> void:
	var index := MODAL_FOCUS_ORDER.find(_modal_focus)
	if index < 0:
		index = 0
	_set_modal_focus(int(MODAL_FOCUS_ORDER[posmod(index + direction, MODAL_FOCUS_ORDER.size())]))


func _set_modal_focus(value: int) -> void:
	_modal_focus = clampi(value, MODAL_EDIT, MODAL_SAVE)
	if _modal_focus == MODAL_EDIT:
		_rename_edit.grab_focus()
	else:
		_rename_edit.release_focus()
	_sync_focus_ring()


func _activate_modal_focus() -> void:
	match _modal_focus:
		MODAL_CANCEL:
			_close_rename()
		MODAL_SAVE:
			_save_rename()
		_:
			_rename_edit.grab_focus()


func _restore_modal_opener() -> void:
	if _modal_opener_group == "actions":
		_focus_action_control(_modal_opener_action)
	elif _modal_opener_group == "tax":
		_set_focus_group("tax")
	elif not _set_focus_group(_modal_opener_group):
		if not _set_focus_group("rows"):
			_set_focus_group("header")


func _sync_focus_ring() -> void:
	if _focus_ring != null:
		_focus_ring.queue_redraw()


func _focus_target() -> Control:
	if _rename_open:
		match _modal_focus:
			MODAL_CANCEL:
				return _rename_cancel
			MODAL_SAVE:
				return _rename_save
			_:
				return _rename_edit
	match _focus_group:
		"header":
			return _header
		"paging":
			return _page_prev if _focus_paging == 0 else _page_next
		"rows":
			return _list
		"detail":
			return _detail_box
		"members":
			if _focus_member_index >= 0 and _focus_member_index < _member_buttons.size():
				return _member_buttons[_focus_member_index]
			return _members_box
		"tax":
			return _action_buttons.get("tax") as Control
		"actions":
			return _action_buttons.get(_focused_action_name()) as Control
	return null


func _draw_focus_ring() -> void:
	var target := _focus_target()
	if target == null or not is_instance_valid(target) or not target.is_visible_in_tree():
		return
	var rect := target.get_global_rect()
	if rect.size.x <= 0.0 or rect.size.y <= 0.0:
		return
	var origin := _focus_ring.get_global_rect().position
	var local := Rect2(rect.position - origin, rect.size).grow(2.0)
	_focus_ring.draw_rect(local, FOCUS_COLOR, false, 2.0)
	_focus_ring.draw_rect(local.grow(1.0), FOCUS_HALO_COLOR, false, 1.0)


## Deterministic state for the headless smoke. `actions_enabled` means the
## action bar currently accepts a mutation (never while a modal owns input).
func _smoke_press(key: int, shift := false) -> void:
	var event := InputEventKey.new()
	event.keycode = key
	event.physical_keycode = key
	event.pressed = true
	event.shift_pressed = shift
	_handle_key(event)


func focus_snapshot() -> Dictionary:
	var any_enabled := false
	if not _rename_open:
		for button in _action_buttons.values():
			if button is Button and not (button as Button).disabled:
				any_enabled = true
				break
	return {
		"focus": _focus_name(),
		"group": "modal" if _rename_open else _focus_group,
		"selected_id": _selected_id,
		"page": _page_index + 1,
		"has_modal": _rename_open,
		"actions_enabled": any_enabled,
	}


func _focus_name() -> String:
	if _rename_open:
		match _modal_focus:
			MODAL_CANCEL:
				return "rename.cancel"
			MODAL_SAVE:
				return "rename.save"
			_:
				return "rename.edit"
	match _focus_group:
		"header":
			return "header:authority"
		"paging":
			return "paging:prev" if _focus_paging == 0 else "paging:next"
		"rows":
			return ("rows:%d" % _selected_id) if _selected_id > 0 else "rows:none"
		"detail":
			return ("detail:%d" % _selected_id) if _selected_id > 0 else "detail:none"
		"members":
			return ("members:%d" % _selected_member) if _selected_member > 0 else "members:none"
		"tax":
			return "tax"
		"actions":
			return "actions:%s" % _focused_action_name()
	return _focus_group


# --- Actions ---------------------------------------------------------------

func _current_record() -> Dictionary:
	if _rows.is_empty() or _selected_index < 0 or _selected_index >= _rows.size():
		return {}
	var record = _rows[_selected_index]
	return record if record is Dictionary else {}


## Re-read the page that holds the selection and return the row by id.
## A mutation never uses the cached row for `membership_revision`.
func _fresh_record(id: int) -> Dictionary:
	if _connected_mode:
		return _fresh_connected_record(id)
	if _core == null or id <= 0:
		return {}
	var after := 0
	if _page_index >= 0 and _page_index < _cursor_stack.size():
		after = int(_cursor_stack[_page_index])
	var response: Dictionary = _core.settlements_page(after, PAGE_SIZE)
	if not bool(response.get("ok", false)):
		return {}
	var rows = response.get("settlements", [])
	if rows is Array:
		for row in rows:
			if row is Dictionary and int((row as Dictionary).get("id", 0)) == id:
				return row
	return {}


## Connected fresh read: current page only, no cache write. A failed read
## returns `{}` and the caller records the rejection; rows stay untouched.
func _fresh_connected_record(id: int) -> Dictionary:
	if _net == null or id <= 0:
		return {}
	var after := 0
	if _page_index >= 0 and _page_index < _cursor_stack.size():
		after = int(_cursor_stack[_page_index])
	var response: Dictionary = _net.request({
		"type": "settlement_list",
		"after_id": after,
		"limit": PAGE_SIZE,
	})
	if not bool(response.get("ok", false)):
		return {}
	var message: Dictionary = response.get("message", {})
	if str(message.get("type", "")) != "settlement_list":
		return {}
	var rows = message.get("settlements", [])
	if not (rows is Array):
		return {}
	for row in rows:
		if row is Dictionary and int((row as Dictionary).get("id", 0)) == id:
			return row
	return {}


func _fresh_selected_record() -> Dictionary:
	return _fresh_record(_selected_id)


func _revision(record: Dictionary) -> int:
	return int(record.get("membership_revision", 0))


func _record_id(record: Dictionary) -> int:
	return int(record.get("id", 0))


func _registered_ids() -> Array[int]:
	var ids: Array[int] = []
	for key in _player_names.keys():
		ids.append(int(key))
	ids.sort()
	return ids


## The local player registry assigns ascending ids at registration; the first
## id is the first registered player. Never a hardcoded actor.
func _first_player_id() -> int:
	var ids := _registered_ids()
	return ids[0] if not ids.is_empty() else 0


## Connected actor: the roster id whose name matches this session's player.
## Falls back to the selected record's owner for a just-founded record.
func _self_player_id() -> int:
	for id in _registered_ids():
		if str(_player_names.get(id, "")).to_lower() == _player_name.to_lower():
			return id
	return int(_current_record().get("owner_id", 0))


## Ascending registered player that is not the current owner.
func _transfer_target(settlement: Dictionary) -> int:
	var owner = settlement.get("owner_id")
	for id in _registered_ids():
		if owner == null or id != int(owner):
			return id
	return 0


## Ascending registered player that is neither the owner nor a member.
func _invite_target(settlement: Dictionary) -> int:
	var owner = settlement.get("owner_id")
	var member_ids: Dictionary = {}
	var members = settlement.get("members", [])
	if members is Array:
		for member in members:
			if member is Dictionary:
				member_ids[int((member as Dictionary).get("player", 0))] = true
	for id in _registered_ids():
		if owner != null and id == int(owner):
			continue
		if member_ids.has(id):
			continue
		return id
	return 0


## Members that can be targets: the owner is an ownership state, not a member
## row, and can never be removed or demoted.
func _assignable_members(settlement: Dictionary) -> Array:
	var result: Array = []
	var owner = settlement.get("owner_id")
	var members = settlement.get("members", [])
	if not (members is Array):
		return result
	for member in members:
		if not (member is Dictionary):
			continue
		if owner != null and int((member as Dictionary).get("player", 0)) == int(owner):
			continue
		result.append(member)
	return result


func _selected_member_record(settlement: Dictionary) -> Dictionary:
	var members := _assignable_members(settlement)
	if members.is_empty():
		return {}
	if _selected_member != 0:
		for member in members:
			if int((member as Dictionary).get("player", 0)) == _selected_member:
				return member
	return members[0]


func _cycle_selected_member(delta: int) -> void:
	var record := _current_record()
	var members := _assignable_members(record)
	if members.is_empty():
		_status_text = _i18n.t(
			"base.ui.settlement_screen.action.result_failed",
			{
				"action": _action_label("cycle_role"),
				"code": "no_member",
				"revision": _i18n.format_int(_revision(record)),
			}
		)
		_focus_action_control("cycle_role")
		_render_detail()
		return
	var position := 0
	for index in members.size():
		if int((members[index] as Dictionary).get("player", 0)) == _selected_member:
			position = index
			break
	position = posmod(position + delta, members.size())
	_selected_member = int((members[position] as Dictionary).get("player", 0))
	_reset_pending()
	_render_detail()


func _next_role(role: String) -> String:
	var index := MEMBER_ROLE_CYCLE.find(role)
	if index < 0:
		return "member"
	return str(MEMBER_ROLE_CYCLE[(index + 1) % MEMBER_ROLE_CYCLE.size()])


func _next_tax_rate(rate: int) -> int:
	return (clampi(rate, 0, MAX_TAX_RATE) + TAX_STEP) % (MAX_TAX_RATE + 1)


func _error(code: String) -> Dictionary:
	return {"ok": false, "error_code": code}


func _action_label(action: String) -> String:
	return _i18n.t(str(ACTION_LABEL_KEYS.get(action, action)))


func _reason_text(code: String) -> String:
	if code.is_empty():
		return ""
	var key := "base.ui.settlement_screen.reason.%s" % code
	if _i18n.has_key(key, "en") or _i18n.has_key(key, "de"):
		return _i18n.t(key)
	return code


func _local_rename_text(code: String) -> String:
	return _rename_error_text(code)


## Every rename error shows a localized reason plus the stable code; a raw
## code is never the whole message.
func _rename_error_text(code: String) -> String:
	if code.is_empty():
		code = "error"
	var reason := ""
	var key := str(RENAME_ERROR_KEYS.get(code, ""))
	if not key.is_empty() and (_i18n.has_key(key, "en") or _i18n.has_key(key, "de")):
		reason = _i18n.t(key)
	else:
		reason = _reason_text(code)
		if reason == code:
			reason = _i18n.t("base.ui.settlement_screen.rename.rejected")
	return _i18n.t(
		"base.ui.settlement_screen.rename.error",
		{"reason": reason, "code": code}
	)


## Record one attempt, print the deterministic smoke line, and apply the result.
## Failures only rewrite the status line: rows, selection, and stored revisions
## stay exactly as they were.
func _finish_action(action: String, result: Dictionary, revision: int) -> Dictionary:
	var ok := bool(result.get("ok", false))
	var code := "" if ok else str(result.get("error_code", "error"))
	if code.is_empty() and not ok:
		code = "error"
	var shown_revision := revision
	var updated: Dictionary = {}
	var raw = result.get("settlement", {})
	if ok and raw is Dictionary:
		updated = raw
		shown_revision = int(updated.get("membership_revision", revision))
	_action_count += 1
	if ok:
		_accepted_count += 1
	else:
		_rejected_count += 1
	if smoke_mode:
		print(
			"SETTLEMENT_SCREEN_ACTION action=%s ok=%s code=%s revision=%d"
			% [action, "true" if ok else "false", code if not code.is_empty() else "-", shown_revision]
		)
	if ok:
		_status_text = _i18n.t(
			"base.ui.settlement_screen.action.result_ok",
			{
				"action": _action_label(action),
				"revision": _i18n.format_int(shown_revision),
			}
		)
		# Re-read the visible page in place; selection stays by id.
		_load_page(_page_index)
	else:
		var code_text := code
		if action == "rename":
			code_text = _rename_error_text(code)
		_status_text = _i18n.t(
			"base.ui.settlement_screen.action.result_failed",
			{
				"action": _action_label(action),
				"code": code_text,
				"revision": _i18n.format_int(shown_revision),
			}
		)
		if _rename_open:
			_set_modal_focus(MODAL_EDIT)
		else:
			_focus_action_control(action)
		_render_detail()
	return result


## Re-read before every mutation. Returns {} after recording the failure.
func _mutation_record(action: String) -> Dictionary:
	if not _connected_mode and _core == null:
		_finish_action(action, _error("no_core"), 0)
		return {}
	var record := _fresh_selected_record()
	if record.is_empty():
		_finish_action(action, _error("settlement.unknown"), _revision(_current_record()))
		return {}
	return record


func _claim_selected() -> void:
	var record := _mutation_record("claim")
	if record.is_empty():
		return
	var revision := _revision(record)
	if bool(record.get("owned", false)):
		_finish_action("claim", _error("settlement.already_owned"), revision)
		return
	var actor := _self_player_id() if _connected_mode else _first_player_id()
	if actor <= 0:
		_finish_action("claim", _error("no_player"), revision)
		return
	var result: Dictionary
	if _connected_mode:
		result = _net.settlement_claim(_record_id(record), actor, revision)
	else:
		result = _core.settlement_claim(_record_id(record), actor, revision)
	_finish_action("claim", result, revision)


func _open_rename() -> void:
	if _rename_open:
		return
	var record := _fresh_selected_record()
	if record.is_empty():
		_finish_action("rename", _error("settlement.unknown"), 0)
		return
	if not bool(record.get("owned", false)):
		_finish_action("rename", _error("settlement.unowned"), _revision(record))
		return
	_modal_opener_group = _focus_group
	_modal_opener_action = "rename"
	_rename_target_id = _record_id(record)
	_rename_open = true
	_rename_title.text = _i18n.t(
		"base.ui.settlement_screen.rename.title",
		{"id": _i18n.format_int(_rename_target_id)}
	)
	_rename_edit.text = str(record.get("name", ""))
	_rename_edit.caret_column = _rename_edit.text.length()
	_rename_count.text = _i18n.t(
		"base.ui.settlement_screen.rename.count",
		{"count": _i18n.format_int(_rename_edit.text.length())}
	)
	_rename_error.text = ""
	_rename_cancel.text = _i18n.t("base.ui.settlement_screen.rename.cancel")
	_rename_save.text = _i18n.t("base.ui.settlement_screen.rename.save")
	_rename_overlay.visible = true
	_modal_focus = MODAL_EDIT
	_rename_edit.grab_focus()
	if _gamepad_active:
		_sync_virtual_keyboard(true)
	_sync_focus_ring()


func _hide_rename_modal() -> void:
	_rename_open = false
	_rename_overlay.visible = false
	_rename_target_id = 0
	_rename_edit.release_focus()
	_modal_focus = MODAL_EDIT
	_sync_virtual_keyboard(false)
	_restore_modal_opener()


func _close_rename() -> void:
	if not _rename_open:
		return
	_status_text = _i18n.t("base.ui.settlement_screen.action.cancelled")
	_hide_rename_modal()
	_render_detail()


func _save_rename() -> void:
	if not _rename_open:
		return
	# Fresh read again: the record may have changed while the field was open.
	var record := _fresh_record(_rename_target_id)
	if record.is_empty():
		_rename_error.text = _rename_error_text("settlement.unknown")
		_finish_action("rename", _error("settlement.unknown"), 0)
		return
	var revision := _revision(record)
	var name := _rename_edit.text.strip_edges()
	var local_code := _validate_rename(name, str(record.get("name", "")))
	if not local_code.is_empty():
		_rename_error.text = _rename_error_text(local_code)
		_finish_action("rename", _error(local_code), revision)
		return
	var result: Dictionary
	if _connected_mode:
		result = _connected_rename(_rename_target_id, name, revision)
	else:
		result = _core.settlement_rename(_rename_target_id, name, revision)
	if not bool(result.get("ok", false)):
		_rename_error.text = _rename_error_text(str(result.get("error_code", "error")))
		_finish_action("rename", result, revision)
		return
	_hide_rename_modal()
	_finish_action("rename", result, revision)


func _validate_rename(name: String, current_name: String) -> String:
	if name.is_empty():
		return "empty_name"
	if name.length() > NAME_MAX_CHARS:
		return "name_too_long"
	for index in name.length():
		var code := name.unicode_at(index)
		if code < 0x20 or code == 0x7f:
			return "invalid_name"
	if name == current_name:
		return "name_unchanged"
	return ""


func _on_rename_text_changed(_text: String) -> void:
	_rename_count.text = _i18n.t(
		"base.ui.settlement_screen.rename.count",
		{"count": _i18n.format_int(_rename_edit.text.length())}
	)


func _tax_selected() -> void:
	var record := _mutation_record("tax")
	if record.is_empty():
		return
	var revision := _revision(record)
	if not bool(record.get("owned", false)):
		_finish_action("tax", _error("settlement.unowned"), revision)
		return
	var rate := _next_tax_rate(int(record.get("tax_rate", 0)))
	var result: Dictionary
	if _connected_mode:
		result = _connected_set_tax(_record_id(record), rate)
	else:
		result = _core.settlement_set_tax(_record_id(record), rate)
	_finish_action("tax", result, revision)


## Connected mutations return the bridge-shaped `{ok, settlement}` result the
## page renderer already understands. A reject keeps the stable server code and
## never touches the page cache.
func _connected_update(request: Dictionary) -> Dictionary:
	if _net == null:
		return _error("no_core")
	var response: Dictionary = _net.mutation(request)
	if not bool(response.get("ok", false)):
		return _error(str(response.get("error_code", "error")))
	var record = response.get("settlement", {})
	if not (record is Dictionary):
		return _error("protocol.malformed")
	var world := str(response.get("world_checksum", ""))
	if _is_hex16(world):
		_world_checksum = world
	return {"ok": true, "settlement": record}


func _connected_rename(settlement_id: int, name: String, revision: int) -> Dictionary:
	return _connected_update({
		"type": "settlement_rename",
		"settlement_id": settlement_id,
		"name": name,
		"expected_revision": revision,
	})


func _connected_set_tax(settlement_id: int, rate: int) -> Dictionary:
	return _connected_update({
		"type": "settlement_set_tax",
		"settlement_id": settlement_id,
		"rate": rate,
	})


func _transfer_selected() -> void:
	var record := _mutation_record("transfer")
	if record.is_empty():
		return
	var revision := _revision(record)
	if not bool(record.get("owned", false)):
		_finish_action("transfer", _error("settlement.unowned"), revision)
		return
	var target := _transfer_target(record)
	if target <= 0:
		_finish_action("transfer", _error("no_recipient"), revision)
		return
	var result: Dictionary
	if _connected_mode:
		result = _net.settlement_transfer(_record_id(record), target, revision)
	else:
		result = _core.settlement_transfer(_record_id(record), target, revision)
	_finish_action("transfer", result, revision)


func _invite_selected() -> void:
	var record := _mutation_record("invite")
	if record.is_empty():
		return
	var revision := _revision(record)
	if not bool(record.get("owned", false)):
		_finish_action("invite", _error("settlement.unowned"), revision)
		return
	var target := _invite_target(record)
	if target <= 0:
		_finish_action("invite", _error("no_invitee"), revision)
		return
	var result: Dictionary
	if _connected_mode:
		result = _net.settlement_invite(_record_id(record), target, "member", revision)
	else:
		result = _core.settlement_invite(_record_id(record), target, "member", revision)
	_finish_action("invite", result, revision)


func _cycle_role() -> void:
	var record := _mutation_record("cycle_role")
	if record.is_empty():
		return
	var revision := _revision(record)
	if not bool(record.get("owned", false)):
		_finish_action("cycle_role", _error("settlement.unowned"), revision)
		return
	var member := _selected_member_record(record)
	if member.is_empty():
		_finish_action("cycle_role", _error("no_member"), revision)
		return
	var player := int(member.get("player", 0))
	_selected_member = player
	var role := _next_role(str(member.get("role", "")))
	var result: Dictionary
	if _connected_mode:
		result = _net.settlement_set_role(_record_id(record), player, role, revision)
	else:
		result = _core.settlement_set_role(_record_id(record), player, role, revision)
	_finish_action("cycle_role", result, revision)


func _remove_selected() -> void:
	var member := _selected_member_record(_current_record())
	if member.is_empty():
		_status_text = _i18n.t(
			"base.ui.settlement_screen.action.result_failed",
			{
				"action": _action_label("remove_member"),
				"code": "no_member",
				"revision": _i18n.format_int(_revision(_current_record())),
			}
		)
		_focus_action_control("remove_member")
		_render_detail()
		return
	var player := int(member.get("player", 0))
	# First press arms this exact member; the second press re-reads and removes.
	if _pending_remove_id != _selected_id or _pending_remove_player != player:
		_pending_remove_id = _selected_id
		_pending_remove_player = player
		_pending_release_id = 0
		_status_text = _i18n.t(
			"base.ui.settlement_screen.action.confirm_remove",
			{"person": _person_text(player)}
		)
		_render_detail()
		return
	_pending_remove_id = 0
	_pending_remove_player = 0
	var record := _mutation_record("remove_member")
	if record.is_empty():
		return
	_attempt_remove_member(record, player)


## Also the smoke hook for the owner-protection rejection. The bridge would
## reject the owner too; this mirrors its stable code without touching a row.
func _attempt_remove_member(record: Dictionary, player: int) -> Dictionary:
	var revision := _revision(record)
	if player <= 0:
		return _finish_action("remove_member", _error("invalid_player_id"), revision)
	if int(record.get("owner_id", -1)) == player:
		return _finish_action("remove_member", _error("settlement.owner_required"), revision)
	var result: Dictionary
	if _connected_mode:
		result = _net.settlement_remove_member(_record_id(record), player, revision)
	else:
		result = _core.settlement_remove_member(_record_id(record), player, revision)
	return _finish_action("remove_member", result, revision)


func _release_selected() -> void:
	if _selected_id <= 0:
		return
	if _pending_release_id != _selected_id:
		var cached := _current_record()
		if cached.is_empty() or not bool(cached.get("owned", false)):
			_status_text = _i18n.t(
				"base.ui.settlement_screen.action.result_failed",
				{
					"action": _action_label("release"),
					"code": "settlement.unowned",
					"revision": _i18n.format_int(_revision(cached)),
				}
			)
			_focus_action_control("release")
			_render_detail()
			return
		_pending_release_id = _selected_id
		_pending_remove_id = 0
		_pending_remove_player = 0
		_status_text = _i18n.t(
			"base.ui.settlement_screen.action.confirm_release",
			{
				"name": str(cached.get("name", "")),
				"id": _i18n.format_int(_record_id(cached)),
			}
		)
		_render_detail()
		return
	_pending_release_id = 0
	var record := _mutation_record("release")
	if record.is_empty():
		return
	var revision := _revision(record)
	if not bool(record.get("owned", false)):
		_finish_action("release", _error("settlement.unowned"), revision)
		return
	# Local/developer Admin override; connected mode asks the server and shows
	# its role rejection instead of bypassing it.
	var result: Dictionary
	if _connected_mode:
		result = _net.settlement_release(_record_id(record), revision)
	else:
		result = _core.settlement_release(_record_id(record), revision)
	_finish_action("release", result, revision)


func _on_action_pressed(action: String) -> void:
	_focus_action_control(action)
	match action:
		"claim":
			_claim_selected()
		"rename":
			_open_rename()
		"tax":
			_tax_selected()
		"transfer":
			_transfer_selected()
		"invite":
			_invite_selected()
		"cycle_role":
			_cycle_role()
		"remove_member":
			_remove_selected()
		"release":
			_release_selected()


## One localized reason per action, empty when the action is enabled.
func _availability(settlement: Dictionary) -> Dictionary:
	var reasons: Dictionary = {}
	for entry in ACTION_ORDER:
		reasons[str(entry[0])] = ""
	if _connected_mode:
		if _net == null:
			for entry in ACTION_ORDER:
				reasons[str(entry[0])] = "no_core"
			return reasons
		if settlement.is_empty():
			for entry in ACTION_ORDER:
				reasons[str(entry[0])] = "no_selection"
			return reasons
		var owned := bool(settlement.get("owned", false))
		var actor := _self_player_id()
		if owned:
			reasons["claim"] = "settlement.already_owned"
			reasons["rename"] = ""
			reasons["tax"] = ""
			reasons["release"] = ""
		else:
			reasons["claim"] = "" if actor > 0 else "no_player"
			for action in ["rename", "tax", "transfer", "invite", "cycle_role", "remove_member", "release"]:
				reasons[action] = "settlement.unowned"
			return reasons
		reasons["transfer"] = "" if _transfer_target(settlement) > 0 else "no_recipient"
		reasons["invite"] = "" if _invite_target(settlement) > 0 else "no_invitee"
		var member := _selected_member_record(settlement)
		var member_reason := "" if not member.is_empty() else "no_member"
		reasons["cycle_role"] = member_reason
		reasons["remove_member"] = member_reason
		return reasons
	if _core == null:
		for entry in ACTION_ORDER:
			reasons[str(entry[0])] = "no_core"
		return reasons
	if settlement.is_empty():
		for entry in ACTION_ORDER:
			reasons[str(entry[0])] = "no_selection"
		return reasons
	var owned := bool(settlement.get("owned", false))
	var ids := _registered_ids()
	if owned:
		reasons["claim"] = "settlement.already_owned"
	else:
		reasons["claim"] = "" if not ids.is_empty() else "no_player"
	if not owned:
		for action in ["rename", "tax", "transfer", "invite", "cycle_role", "remove_member", "release"]:
			reasons[action] = "settlement.unowned"
		return reasons
	reasons["rename"] = ""
	reasons["tax"] = ""
	reasons["release"] = ""
	reasons["transfer"] = "" if _transfer_target(settlement) > 0 else "no_recipient"
	reasons["invite"] = "" if _invite_target(settlement) > 0 else "no_invitee"
	var member := _selected_member_record(settlement)
	var member_reason := "" if not member.is_empty() else "no_member"
	reasons["cycle_role"] = member_reason
	reasons["remove_member"] = member_reason
	return reasons


func _update_action_bar(settlement: Dictionary) -> void:
	var reasons := _availability(settlement)
	for entry in ACTION_ORDER:
		var action := str(entry[0])
		var key := str(entry[1])
		var reason := str(reasons.get(action, ""))
		var label := _action_label(action)
		var button: Button = _action_buttons.get(action)
		if button == null:
			continue
		if reason.is_empty():
			button.text = _i18n.t(
				"base.ui.settlement_screen.action.button",
				{"key": key, "action": label}
			)
			button.disabled = false
			button.tooltip_text = ""
		else:
			var reason_text := _reason_text(reason)
			button.text = _i18n.t(
				"base.ui.settlement_screen.action.button_disabled",
				{"key": key, "action": label, "reason": reason_text}
			)
			button.disabled = true
			button.tooltip_text = reason_text


# --- Rendering -------------------------------------------------------------

func _render_all() -> void:
	_title_label.text = _i18n.t("base.ui.settlement_screen.title")
	_render_authority()
	_help_label.text = _i18n.t(
		"base.ui.settlement_screen.help_connected" if _connected_mode else "base.ui.settlement_screen.help"
	)
	_rename_cancel.text = _i18n.t("base.ui.settlement_screen.rename.cancel")
	_rename_save.text = _i18n.t("base.ui.settlement_screen.rename.save")
	if _rename_open:
		_rename_title.text = _i18n.t(
			"base.ui.settlement_screen.rename.title",
			{"id": _i18n.format_int(_rename_target_id)}
		)
		_rename_count.text = _i18n.t(
			"base.ui.settlement_screen.rename.count",
			{"count": _i18n.format_int(_rename_edit.text.length())}
		)
	_render_list()
	_render_detail()


## The authority mode is always visible. Connected also shows the server world
## checksum once a mutation response has carried one (`-` before that).
func _render_authority() -> void:
	if _connected_mode:
		_authority_label.text = _i18n.t(
			"base.ui.settlement_screen.authority",
			{"mode": _i18n.t("base.ui.settlement_screen.authority_connected")}
		)
		_world_checksum_label.text = _i18n.t(
			"base.ui.settlement_screen.world_checksum",
			{"checksum": _world_checksum if not _world_checksum.is_empty() else "-"}
		)
		return
	_authority_label.text = _i18n.t(
		"base.ui.settlement_screen.authority",
		{"mode": _i18n.t("base.ui.settlement_screen.authority_local")}
	)
	_world_checksum_label.text = ""


func _render_list() -> void:
	_syncing_list = true
	_list.clear()
	for row in _rows:
		_list.add_item(_row_text(row as Dictionary))
	if not _rows.is_empty() and _selected_index >= 0 and _selected_index < _rows.size():
		_list.select(_selected_index)
	else:
		_list.deselect_all()
	_syncing_list = false

	_page_prev.text = _i18n.t("base.ui.settlement_screen.paging.prev")
	_page_next.text = _i18n.t("base.ui.settlement_screen.paging.next")
	_page_prev.disabled = _page_index <= 0
	_page_next.disabled = not _has_more
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
	if _rows.is_empty() or _selected_index < 0 or _selected_index >= _rows.size():
		_detail_title.text = ""
		_detail_center.text = ""
		_detail_owner.text = ""
		_detail_tax.text = ""
		_detail_revision.text = ""
		_projection_label.text = ""
		_members_title.text = ""
		_detail_empty.text = _i18n.t("base.ui.settlement_screen.detail_empty")
		_detail_empty.visible = true
		if _focus_group == "detail" or _focus_group == "members":
			_focus_group = "rows" if not _rows.is_empty() else "header"
		if _error_code.is_empty() and _status_text.is_empty():
			_status_label.text = _i18n.t("base.ui.settlement_screen.empty")
		else:
			_status_label.text = _status_text
		_update_action_bar({})
		_sync_focus_ring()
		return
	_detail_empty.visible = false

	var settlement: Dictionary = _rows[_selected_index]
	var id := int(settlement.get("id", 0))
	var name := _display_name(settlement)
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
	var assignable := _assignable_members(settlement)
	var selection_valid := false
	for member in assignable:
		if int((member as Dictionary).get("player", 0)) == _selected_member:
			selection_valid = true
			break
	if not selection_valid:
		_selected_member = (
			int((assignable[0] as Dictionary).get("player", 0)) if not assignable.is_empty() else 0
		)
	var rendered := 0
	for member in assignable:
		var row: Dictionary = member
		var player := int(row.get("player", 0))
		var button := Button.new()
		button.flat = true
		button.focus_mode = Control.FOCUS_NONE
		button.alignment = HORIZONTAL_ALIGNMENT_LEFT
		var prefix := "▸ " if player == _selected_member else "  "
		button.text = prefix + _member_text(row)
		button.pressed.connect(_on_member_pressed.bind(player))
		_members_box.add_child(button)
		_member_buttons.append(button)
		rendered += 1
	if rendered == 0:
		var empty := Label.new()
		empty.text = _i18n.t("base.ui.settlement_screen.detail_no_members")
		empty.add_theme_color_override("font_color", MUTED_COLOR)
		_members_box.add_child(empty)
		if _focus_group == "members":
			_focus_group = "tax"
	else:
		_focus_member_index = _member_index_of(_selected_member, assignable)
	_update_action_bar(settlement)
	_status_label.text = _status_text
	_sync_focus_ring()


func _on_member_pressed(player: int) -> void:
	var members := _assignable_members(_current_record())
	_focus_group = "members"
	_focus_member_index = _member_index_of(player, members)
	_selected_member = player
	_reset_pending()
	_render_detail()
	_sync_focus_ring()


func _clear_members() -> void:
	_member_buttons.clear()
	for child in _members_box.get_children():
		_members_box.remove_child(child)
		child.free()


func _row_text(settlement: Dictionary) -> String:
	var members = settlement.get("members", [])
	var member_count := (members as Array).size() if members is Array else 0
	return _i18n.t(
		"base.ui.settlement_screen.row",
		{
			"name": _display_name(settlement),
			"id": str(int(settlement.get("id", 0))),
			"owner": _owner_text(settlement),
			"tax": _i18n.format_int(int(settlement.get("tax_rate", 0))),
			"revision": _i18n.format_int(int(settlement.get("membership_revision", 0))),
			"members": _i18n.format_int(member_count),
		}
	)


func _display_name(settlement: Dictionary) -> String:
	var name := str(settlement.get("name", ""))
	if not name.is_empty():
		return name
	return _i18n.t(
		"base.ui.settlement.default_name",
		{"index": _i18n.format_int(int(settlement.get("id", 0)))}
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


## D-039 preview. Stateless bridge helper; never mutates the store. Connected
## mode has no local editor state to preview from.
func _projection_text(rate: int) -> String:
	if _connected_mode:
		return _i18n.t("base.ui.settlement_screen.projection_connected")
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
	_status_text = ""
	_rows = []
	_matched = 0
	_selected_index = 0
	_selected_id = 0
	_reset_pending()
	_selected_member = 0
	_rename_open = false
	_rename_overlay.visible = false
	_rename_edit.release_focus()
	_modal_focus = MODAL_EDIT
	_focus_group = "header"
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
	_update_action_bar({})
	_status_label.text = _i18n.t(
		"base.ui.settlement_screen.unavailable",
		{"code": code}
	)
	_sync_focus_ring()


# --- Smoke -----------------------------------------------------------------

func _run_smoke() -> void:
	var failures: Array[String] = []
	if _shared and _shared_core != null:
		_core = _shared_core
	else:
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

	# Focus model: fixed groups, Tab/Shift+Tab, arrows/Home/End, page keys,
	# Enter activation, modal trap and focus return.
	_focus_group = "header"
	_expect(focus_snapshot().get("group") == "header", "initial focus header", failures)
	_smoke_press(KEY_TAB)
	_expect(focus_snapshot().get("group") == "paging", "tab -> paging", failures)
	_smoke_press(KEY_TAB)
	_expect(focus_snapshot().get("group") == "rows", "tab -> rows", failures)
	_smoke_press(KEY_TAB, true)
	_expect(focus_snapshot().get("group") == "paging", "shift+tab -> paging", failures)
	_smoke_press(KEY_TAB)
	_select_page_number(1)
	_smoke_press(KEY_DOWN)
	_expect(_selected_id == 2, "arrow down selects 2, got %d" % _selected_id, failures)
	_smoke_press(KEY_END)
	_expect(_selected_id == 8, "end selects 8, got %d" % _selected_id, failures)
	_smoke_press(KEY_HOME)
	_expect(_selected_id == 1, "home selects 1, got %d" % _selected_id, failures)
	_smoke_press(KEY_PAGEDOWN)
	_expect(_page_index == 1 and _focus_group == "rows", "pagedown pages rows", failures)
	_smoke_press(KEY_ENTER)
	_expect(_focus_group == "detail", "enter row -> detail, got %s" % _focus_group, failures)
	_open_rename()
	_expect(bool(focus_snapshot().get("has_modal")), "rename opens modal", failures)
	_smoke_press(KEY_TAB)
	_expect(focus_snapshot().get("focus") == "rename.cancel", "modal tab -> cancel", failures)
	_smoke_press(KEY_ESCAPE)
	_expect(not bool(focus_snapshot().get("has_modal")), "esc closes modal", failures)
	_expect(_focus_group == "detail", "esc returns to opener, got %s" % _focus_group, failures)

	# Write surface: claim the unowned record, rename, tax, invite, role,
	# remove, transfer, release, then exercise two rejections.
	_load_page(0)
	_page_down()
	_page_down()
	_select_page_number(7)
	_claim_selected()
	_open_rename()
	_rename_edit.text = ""
	_save_rename()
	_rename_edit.text = "Harbour 23"
	_save_rename()
	_tax_selected()
	_invite_selected()
	_cycle_role()
	_remove_selected()
	_remove_selected()
	_transfer_selected()
	_release_selected()
	_release_selected()

	# Rejection paths: claim an owned record, then try to remove its owner.
	_load_page(0)
	_select_page_number(2)
	_claim_selected()
	var owner_record := _current_record()
	_attempt_remove_member(owner_record, int(owner_record.get("owner_id", 0)))

	# Gamepad navigation: same focus state as the keyboard, no store mutation.
	_load_page(0)
	_select_page_number(1)
	_focus_group = "rows"
	_gamepad_active = false
	var pad := gamepad_action("ui_down")
	_expect(_selected_id == 2, "gamepad down selects next row, got %d" % _selected_id, failures)
	_expect(_focus_group == "rows", "gamepad down keeps row group", failures)
	_expect(bool(pad.get("active", false)), "gamepad marks modality active", failures)
	_expect(str(pad.get("last_action", "")) == "ui_down", "gamepad records last action", failures)
	pad = gamepad_action("ui_up")
	_expect(_selected_id == 1, "gamepad up returns to the first row", failures)
	pad = gamepad_action("ui_up")
	_expect(_focus_group == "paging", "gamepad up at the row edge enters paging", failures)
	pad = gamepad_action("ui_down")
	_expect(_focus_group == "paging" and _focus_paging == 1, "gamepad down moves paging focus", failures)
	pad = gamepad_action("ui_down")
	_expect(_focus_group == "rows", "gamepad down at the paging edge enters rows", failures)
	pad = gamepad_action("ui_accept")
	_expect(_focus_group == "detail", "gamepad accept opens detail", failures)
	pad = gamepad_action("ui_accept")
	_expect(_focus_group == "members" or _focus_group == "tax", "gamepad accept advances detail", failures)
	pad = gamepad_action("ui_cancel")
	_expect(_focus_group == "rows", "gamepad cancel returns to rows", failures)
	pad = gamepad_action("ui_cancel")
	_expect(_selected_id == 0, "gamepad cancel clears the selection", failures)
	_focus_group = "rows"
	_select_page_number(2)
	_open_rename()
	_expect(_rename_open, "gamepad modal opens", failures)
	_expect(_virtual_keyboard_requests > 0, "gamepad rename requests the virtual keyboard", failures)
	pad = gamepad_action("ui_down")
	_expect(_modal_focus == MODAL_CANCEL, "gamepad modal down moves to cancel", failures)
	pad = gamepad_action("ui_accept")
	_expect(not _rename_open, "gamepad accept closes the modal", failures)
	_expect(_virtual_keyboard_clears > 0, "gamepad modal clears the virtual keyboard", failures)
	var last_action := str(gamepad_snapshot().get("last_action", ""))
	pad = gamepad_action("bogus")
	_expect(str(pad.get("last_action", "")) == last_action, "unknown gamepad action is ignored", failures)

	print("SETTLEMENT_SCREEN_GAMEPAD status=ok checks=18")
	print("SETTLEMENT_SCREEN_FOCUS group=rows tab=paging shift=ok arrows=ok modal=trap esc=detail")
	print("SETTLEMENT_SCREEN_PAGES sizes=8/8/7 selected_last=23")
	print("SETTLEMENT_SCREEN_NAMES owner=Mayor#7 manager=Manager#11 builder=Builder#13 member=Member#12 fallback=#9001 owner_fallback=#9002")
	print("SETTLEMENT_SCREEN_PROJECTION tax0=x1.00 tax5=x1.05 tax100=x2.00 invalid=tax.projection.rate_out_of_range")
	print("SETTLEMENT_SCREEN_LOCALE en=ok de=ok")

	if failures.is_empty():
		print(
			"SETTLEMENT_SCREEN status=ok pages=%d settlements=%d selected=%d actions=%d accepted=%d rejected=%d checksum=%s"
			% [_page_count(), _matched, _selected_id, _action_count, _accepted_count, _rejected_count, _checksum]
		)
		get_tree().quit(0)
	else:
		print("SETTLEMENT_SCREEN status=failed reason=%s" % failures[0])
		for failure in failures:
			print("SETTLEMENT_SCREEN_FAIL %s" % failure)
		get_tree().quit(1)


## Connected smoke: one fresh server, one Mayor session. Founds the
## deterministic `Harbor` fixture, reads page 1 and `player_list`, renames,
## changes tax, proves a stale revision rejects without touching the cache, and
## proves a local-only action sends nothing. Prints one marker.
func _run_connected_smoke() -> void:
	var failures: Array[String] = []
	_net = SettlementScreenNet.new()
	var opened: Dictionary = _net.open(_host, _port, _player_name)
	if not bool(opened.get("ok", false)):
		failures.append("connect %s" % str(opened.get("error_code", "connect_failed")))
		_connected_smoke_fail(failures)
		return

	var found_response: Dictionary = _net.request({
		"type": "found_settlement",
		"name": CONNECTED_FOUND_NAME,
		"x": 0,
		"z": 0,
		"radius_m": CONNECTED_FOUND_RADIUS_M,
	})
	_expect(bool(found_response.get("ok", false)), "found accepted", failures)
	if not bool(found_response.get("ok", false)):
		_connected_smoke_fail(failures)
		return
	var found_message: Dictionary = found_response.get("message", {})
	_expect(str(found_message.get("type", "")) == "settlement_found", "found response", failures)
	var found_record = found_message.get("settlement", {})
	_expect(found_record is Dictionary, "found record", failures)
	if not (found_record is Dictionary):
		_connected_smoke_fail(failures)
		return
	var settlement_id := int((found_record as Dictionary).get("id", 0))
	_expect(settlement_id > 0, "found id", failures)
	_world_checksum = str(found_message.get("world_checksum", ""))
	_expect(_is_hex16(_world_checksum), "found world checksum %s" % _world_checksum, failures)

	_refresh_player_names()
	_load_page(0)
	_expect(_matched == 1, "matched 1, got %d" % _matched, failures)
	_expect(_rows.size() == 1, "page rows 1, got %d" % _rows.size(), failures)
	if _rows.is_empty():
		_connected_smoke_fail(failures)
		return
	var record := _current_record()
	_expect(_record_id(record) == settlement_id, "selected founded id", failures)
	_expect(str(record.get("name", "")) == CONNECTED_FOUND_NAME, "listed name", failures)
	_expect(_player_names.size() == 1, "players 1, got %d" % _player_names.size(), failures)
	var owner_id := int(record.get("owner_id", 0))
	_expect(_player_names.has(owner_id), "owner name cached", failures)
	var owner_name := str(_player_names.get(owner_id, ""))
	_expect(not owner_name.is_empty(), "owner name non-empty", failures)
	_expect(
		_detail_owner.text.contains(owner_name),
		"detail resolves owner name: %s" % _detail_owner.text,
		failures
	)
	_expect(
		_authority_label.text.contains(_i18n.t("base.ui.settlement_screen.authority_connected")),
		"authority Connected: %s" % _authority_label.text,
		failures
	)
	_expect(
		_world_checksum_label.text.contains(_world_checksum),
		"world checksum shown: %s" % _world_checksum_label.text,
		failures
	)
	_expect(_is_hex16(_checksum), "list checksum %s" % _checksum, failures)

	# Rename: the modal opens from the selected page record, Save re-reads and
	# sends that fresh revision. Success refreshes the page in place.
	_open_rename()
	_expect(_rename_open, "rename modal open", failures)
	if _rename_open:
		_rename_edit.text = CONNECTED_RENAMED_NAME
		_save_rename()
	var rename_ok := (
		not _rename_open
		and not _rows.is_empty()
		and str(_current_record().get("name", "")) == CONNECTED_RENAMED_NAME
	)
	_expect(rename_ok, "rename refreshed the page", failures)
	_expect(_detail_title.text.contains(CONNECTED_RENAMED_NAME), "detail renamed", failures)

	# Tax: fresh read, then `settlement_set_tax`; 0% steps to 5%.
	_tax_selected()
	var tax_ok := not _rows.is_empty() and int(_current_record().get("tax_rate", -1)) == 5
	_expect(tax_ok, "tax stepped to 5", failures)
	_expect(_detail_tax.text.contains("5"), "detail tax 5", failures)

	# Rejection: a deliberately stale revision returns the stable code, and
	# the cached row and world checksum stay exactly as they were.
	var before_name := str(_current_record().get("name", ""))
	var before_checksum := _world_checksum
	var before_list_checksum := _checksum
	var stale: Dictionary = _net.request({
		"type": "settlement_rename",
		"settlement_id": settlement_id,
		"name": "Stale Harbor",
		"expected_revision": _revision(_current_record()) + 1,
	})
	_expect(not bool(stale.get("ok", false)), "stale rename rejected", failures)
	_expect(
		str(stale.get("error_code", "")) == "settlement.revision_conflict",
		"stale code %s" % str(stale.get("error_code", "")),
		failures
	)
	_expect(str(_current_record().get("name", "")) == before_name, "cache unchanged after reject", failures)
	_expect(_world_checksum == before_checksum, "world checksum unchanged after reject", failures)
	_expect(_checksum == before_list_checksum, "list checksum unchanged after reject", failures)

	# Register a second player so invite/transfer targets resolve.
	var seed_net := SettlementScreenNet.new()
	var seed_open: Dictionary = seed_net.open(_host, _port, CONNECTED_SEED_PLAYER)
	_expect(bool(seed_open.get("ok", false)), "seed player open", failures)
	seed_net.close()
	_refresh_player_names()
	_expect(_player_names.size() == 2, "players 2, got %d" % _player_names.size(), failures)

	var before_actions := _action_count
	_invite_selected()
	_expect(_action_count > before_actions, "invite counted", failures)
	var member := _selected_member_record(_current_record())
	_expect(int(member.get("player", 0)) == 2, "member 2 invited", failures)
	var revision_after_invite := _revision(_current_record())

	_selected_member = 2
	_cycle_role()
	_expect(
		str(_selected_member_record(_current_record()).get("role", "")) == "builder",
		"role promoted to builder",
		failures
	)
	_cycle_role()
	_expect(
		str(_selected_member_record(_current_record()).get("role", "")) == "manager",
		"role promoted to manager",
		failures
	)

	_pending_remove_id = 0
	_pending_remove_player = 0
	_remove_selected()
	_remove_selected()
	_expect(
		_selected_member_record(_current_record()).is_empty(),
		"member 2 removed",
		failures
	)

	_invite_selected()
	_transfer_selected()
	_expect(int(_current_record().get("owner_id", 0)) == 2, "ownership transferred to 2", failures)
	_expect(
		_revision(_current_record()) > revision_after_invite,
		"revision advanced through mutations",
		failures
	)

	# Mayor cannot release; the server rejection is surfaced, not bypassed.
	_pending_release_id = 0
	var rejected_before := _rejected_count
	_release_selected()
	_release_selected()
	_expect(_rejected_count > rejected_before, "Mayor release rejected", failures)
	_expect(not _all_text().contains("!base.ui."), "EN key leak", failures)

	_switch_locale("de")
	_expect(not _all_text().contains("!base.ui."), "DE key leak", failures)
	_expect(
		_authority_label.text.contains(_i18n.t("base.ui.settlement_screen.authority_connected")),
		"DE authority Connected",
		failures
	)
	_switch_locale("en")

	print("SETTLEMENT_SCREEN_CONNECTED_READ settles=%d players=%d owner=%s checksum=%s" % [
		_matched,
		_player_names.size(),
		owner_name,
		_checksum,
	])
	if not failures.is_empty():
		_connected_smoke_fail(failures)
		return
	print(
		"SETTLEMENT_SCREEN_CONNECTED status=ok settlements=%d players=%d rename=%d tax=%d actions=%d accepted=%d rejected=%d checksum=%s"
		% [
			_matched,
			_player_names.size(),
			1 if rename_ok else 0,
			1 if tax_ok else 0,
			_action_count,
			_accepted_count,
			_rejected_count,
			_world_checksum,
		]
	)
	_net.close()
	get_tree().quit(0)


## Re-read the current page and names after the host world changes.
func refresh() -> void:
	if _connected_mode:
		_load_page(_page_index)
		return
	if _core == null:
		return
	_refresh_player_names()
	_load_page(_page_index)


## Select a settlement by id when it is on the current page.
func focus_settlement(id: int) -> bool:
	if id <= 0:
		return false
	for index in _rows.size():
		if _record_id(_rows[index]) == id:
			_selected_index = index
			_selected_id = id
			_syncing_list = true
			_list.select(index)
			_syncing_list = false
			_sync_focus_ring()
			_render_detail()
			return true
	return false


## Host-visible state for the smoke and embedding tests.
func embed_snapshot() -> Dictionary:
	return {
		"ok": _core != null,
		"shared": _shared,
		"seed": _seed,
		"page": _page_index,
		"selected_id": _selected_id,
		"rows": _rows.size(),
		"matched": _matched,
		"checksum": _checksum,
	}


## Hide/reset the screen when the host closes the overlay.
func on_hidden() -> void:
	_reset_pending()
	_rename_open = false
	_sync_virtual_keyboard(false)
	_gamepad_active = false
	if _rename_overlay != null:
		_rename_overlay.visible = false
	_status_text = ""
	_focus_group = "header"
	_sync_focus_ring()


func _exit_tree() -> void:
	# Never free the host's core; only drop the local reference.
	_core = null
	_shared_core = null


func _connected_smoke_fail(failures: Array) -> void:
	if _net != null:
		_net.close()
	print(
		"SETTLEMENT_SCREEN_CONNECTED status=failed reason=%s"
		% (str(failures[0]) if not failures.is_empty() else "unknown")
	)
	for failure in failures:
		print("SETTLEMENT_SCREEN_FAIL %s" % failure)
	get_tree().quit(1)


func _is_hex16(value: String) -> bool:
	if value.length() != 16:
		return false
	for index in range(value.length()):
		var character := value[index]
		if not (
			(character >= "0" and character <= "9")
			or (character >= "a" and character <= "f")
		):
			return false
	return true


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
		elif child is Button:
			parts.append((child as Button).text)
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
