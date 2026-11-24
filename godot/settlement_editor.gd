extends Control

## Standalone developer editor for local settlement ownership and membership.
##
## This page is deliberately outside the world scene and the in-game dashboard:
## it owns one `TownsmithCore` instance and only calls the local data-only
## bridge mutations (`settlement_found`, `settlement_claim`, `settlement_rename`,
## `settlement_transfer`, `settlement_invite`, `settlement_set_role`,
## `settlement_remove_member`, `settlement_set_tax`) plus `settlements()` reads.
## No server, protocol, simulation, treasury, or save/load code is involved. See
## `docs/48 - Settlement Boundaries.md` §2.2.
##
## Run:
##   godot --path godot res://settlement_editor.tscn
##   godot --headless --path godot res://settlement_editor.tscn -- --smoke
##
## Headless smoke: starts empty, founds two deterministic settlements (one
## unowned, one claimed by player 1), invites a manager and a builder, renames,
## sets tax, transfers ownership twice, changes a role, removes a member, and
## rejects an already-owned claim, an unowned rename, a stale revision, the
## owner role, removing the owner, and player id 0. Every mutation re-reads the
## record's `membership_revision` first and re-reads `settlements()` after
## success. Final
## marker:
##   SETTLEMENT_EDITOR status=ok actions=<n> checksum=<16 hex>
##
## Strings are English-only developer text: this is a pre-dashboard tool and
## the shared `base.ui.settlement.*` localization slice does not exist yet.
## Settlement names, player ids, roles, and bridge error codes stay verbatim.
## Display numbers go through `WorldI18n.format_int` so the tool already follows
## the EN/DE number rules from `docs/16 - Localization.md`.

const WorldI18n = preload("res://world_i18n.gd")

const SUITE := "settlement_editor"
const DEFAULT_SEED := 2026
const DEFAULT_NAME := "Settlement"
const DEFAULT_OWNER_LABEL := "wilderness"
const DEFAULT_RADIUS_M := 256

# Mirrors `core_city::settlement`: radius 16..2,000 m, tax 0..=100 percent.
const MIN_RADIUS_M := 16
const MAX_RADIUS_M := 2000
const MAX_TAX_RATE := 100
const MAX_PLAYER_ID := 999999999

# Deterministic, overlap-allowed defaults on a 1,024 m square ring. The smoke
# fixture uses the first two slots and never depends on wall-clock or RNG.
const DEFAULT_CENTERS := [
	Vector2i(0, 0),
	Vector2i(1024, 0),
	Vector2i(0, 1024),
	Vector2i(-1024, 0),
	Vector2i(0, -1024),
	Vector2i(1024, 1024),
	Vector2i(-1024, -1024),
	Vector2i(1024, -1024),
	Vector2i(-1024, 1024),
]

const ROLES := ["manager", "builder", "member"]

const OK_COLOR := Color(0.36, 0.86, 0.55)
const ERROR_COLOR := Color(0.96, 0.42, 0.38)
const MUTED_COLOR := Color(0.66, 0.71, 0.78)
const TEXT_COLOR := Color(0.88, 0.91, 0.95)

var smoke_mode := false

var _i18n
var _core = null
var _seed := DEFAULT_SEED
var _settlements: Array = []
var _checksum := ""
var _selected_id := 0
var _actions := 0
var _failures := 0

var _state_label: Label
var _message_label: Label
var _detail_label: Label
var _list: ItemList
var _form_title: Label

var _name_edit: LineEdit
var _x_spin: SpinBox
var _z_spin: SpinBox
var _radius_spin: SpinBox
var _player_spin: SpinBox
var _role_option: OptionButton
var _tax_spin: SpinBox


func _ready() -> void:
	var args := OS.get_cmdline_user_args()
	smoke_mode = args.has("--smoke")
	_i18n = WorldI18n.new()
	_build_ui()
	_reset_session(true)
	if smoke_mode:
		_run_smoke()


# --- Bridge ----------------------------------------------------------------

func _reset_session(reset_seed: bool) -> void:
	_core = TownsmithCore.new()
	_settlements = []
	_checksum = ""
	_selected_id = 0
	if reset_seed:
		_seed = DEFAULT_SEED
	_refresh()
	_set_message("New empty settlement session for seed %s." % _i18n.format_int(_seed), false)


func _refresh() -> void:
	if _core == null:
		_settlements = []
		_checksum = ""
		_rebuild_list()
		return
	var result: Dictionary = _core.settlements()
	if not bool(result.get("ok", false)):
		_settlements = []
		_checksum = ""
		_rebuild_list()
		_apply_bridge_result("List", result)
		return
	var records = result.get("settlements", [])
	_settlements = records if records is Array else []
	_checksum = str(result.get("checksum", ""))
	if _find_record(_selected_id).is_empty():
		if _selected_id != 0 and _form_title != null:
			_form_title.text = "No settlement selected"
		_selected_id = 0
	_rebuild_list()


## Re-read the layer and return one record. Never use a cached record for a
## mutation: `membership_revision` must come from the current store.
func _fresh_record(id: int) -> Dictionary:
	if _core == null:
		return {}
	var result: Dictionary = _core.settlements()
	if not bool(result.get("ok", false)):
		return {}
	var records = result.get("settlements", [])
	if records is Array:
		for record in records:
			if int(record.get("id", 0)) == id:
				return record
	return {}


func _revision_of(id: int) -> int:
	var record := _fresh_record(id)
	return int(record.get("membership_revision", -1))


func _found(name: String, x: int, z: int, radius: int) -> Dictionary:
	if _core == null:
		return _no_core("Found")
	var result: Dictionary = _core.settlement_found(
		_seed, name, DEFAULT_OWNER_LABEL, x, z, radius
	)
	_apply_bridge_result("Found %s" % name, result)
	_refresh()
	return result


func _claim(id: int, player_id: int) -> Dictionary:
	if _core == null:
		return _no_core("Claim")
	var revision := _revision_of(id)
	if revision < 0:
		return _missing("Claim", id)
	var result: Dictionary = _core.settlement_claim(id, player_id, revision)
	return _finish_mutation("Claim #%s" % _i18n.format_int(id), result)


func _rename(id: int, name: String) -> Dictionary:
	if _core == null:
		return _no_core("Rename")
	var revision := _revision_of(id)
	if revision < 0:
		return _missing("Rename", id)
	var result: Dictionary = _core.settlement_rename(id, name, revision)
	return _finish_mutation("Rename #%s" % _i18n.format_int(id), result)


func _transfer(id: int, player_id: int) -> Dictionary:
	if _core == null:
		return _no_core("Transfer")
	var revision := _revision_of(id)
	if revision < 0:
		return _missing("Transfer", id)
	var result: Dictionary = _core.settlement_transfer(id, player_id, revision)
	return _finish_mutation("Transfer #%s" % _i18n.format_int(id), result)


func _invite(id: int, player_id: int, role: String) -> Dictionary:
	if _core == null:
		return _no_core("Invite")
	var revision := _revision_of(id)
	if revision < 0:
		return _missing("Invite", id)
	var result: Dictionary = _core.settlement_invite(id, player_id, role, revision)
	return _finish_mutation("Invite #%s" % _i18n.format_int(id), result)


func _set_role(id: int, player_id: int, role: String) -> Dictionary:
	if _core == null:
		return _no_core("Set role")
	var revision := _revision_of(id)
	if revision < 0:
		return _missing("Set role", id)
	var result: Dictionary = _core.settlement_set_role(id, player_id, role, revision)
	return _finish_mutation("Set role #%s" % _i18n.format_int(id), result)


func _remove_member(id: int, player_id: int) -> Dictionary:
	if _core == null:
		return _no_core("Remove member")
	var revision := _revision_of(id)
	if revision < 0:
		return _missing("Remove member", id)
	var result: Dictionary = _core.settlement_remove_member(id, player_id, revision)
	return _finish_mutation("Remove member #%s" % _i18n.format_int(id), result)


func _set_tax(id: int, rate: int) -> Dictionary:
	if _core == null:
		return _no_core("Set tax")
	var result: Dictionary = _core.settlement_set_tax(id, rate)
	return _finish_mutation("Set tax #%s" % _i18n.format_int(id), result)


func _finish_mutation(context: String, result: Dictionary) -> Dictionary:
	_apply_bridge_result(context, result)
	if bool(result.get("ok", false)):
		_refresh()
	return result


func _no_core(context: String) -> Dictionary:
	_set_message("%s failed: no_core (bridge not loaded)" % context, true)
	return {"ok": false, "error_code": "no_core", "error": "bridge not loaded"}


func _missing(context: String, id: int) -> Dictionary:
	_set_message(
		"%s failed: settlement.unknown (no record #%s)" % [
			context, _i18n.format_int(id)
		],
		true
	)
	return {"ok": false, "error_code": "settlement.unknown", "error": "unknown settlement"}


func _apply_bridge_result(context: String, result: Dictionary) -> bool:
	if bool(result.get("ok", false)):
		_set_message("%s: ok" % context, false)
		return true
	var code := str(result.get("error_code", "error"))
	var message := str(result.get("error", ""))
	_set_message("%s failed: %s (%s)" % [context, code, message], true)
	return false


# --- Editor state ----------------------------------------------------------

func _find_record(id: int) -> Dictionary:
	if _core == null:
		return {}
	for record in _settlements:
		if int(record.get("id", 0)) == id:
			return record
	return {}


func _members_text(record: Dictionary) -> String:
	var members = record.get("members", [])
	if not (members is Array) or (members as Array).is_empty():
		return "none"
	var parts := PackedStringArray()
	for member in members:
		parts.append(
			"%s:%s" % [
				_i18n.format_int(int(member.get("player", 0))),
				str(member.get("role", "")),
			]
		)
	return ", ".join(parts)


func _owner_text(record: Dictionary) -> String:
	var owner_id = record.get("owner_id", null)
	if not bool(record.get("owned", false)) or owner_id == null:
		return "unowned"
	return _i18n.format_int(int(owner_id))


func _row_text(record: Dictionary) -> String:
	return "#%s  %s  ·  %s  ·  rev %s  ·  tax %s%%  ·  members %s" % [
		_i18n.format_int(int(record.get("id", 0))),
		str(record.get("name", "")),
		_owner_text(record),
		_i18n.format_int(int(record.get("membership_revision", 0))),
		_i18n.format_int(int(record.get("tax_rate", 0))),
		_members_text(record),
	]


func _rebuild_list() -> void:
	if _list == null:
		return
	_list.clear()
	for record in _settlements:
		var id := int(record.get("id", 0))
		_list.add_item(_row_text(record))
		_list.set_item_metadata(_list.item_count - 1, id)
		if id == _selected_id:
			_list.select(_list.item_count - 1)
	_refresh_state_label()
	_refresh_detail()


func _refresh_state_label() -> void:
	if _state_label == null:
		return
	_state_label.text = "Seed %s · %s settlements · checksum %s" % [
		_i18n.format_int(_seed),
		_i18n.format_int(_settlements.size()),
		_checksum if not _checksum.is_empty() else "—",
	]


func _refresh_detail() -> void:
	if _detail_label == null:
		return
	var record := _find_record(_selected_id)
	if record.is_empty():
		_detail_label.text = (
			"No settlement selected.\n"
			+ "Select a row (or press its number) to inspect name, id, ownership, "
			+ "revision, tax, and members."
		)
		return
	var center = record.get("center", [])
	var center_text := "?"
	if center is PackedInt32Array and center.size() >= 2:
		center_text = "(%s, %s)" % [
			_i18n.format_int(int(center[0])), _i18n.format_int(int(center[1]))
		]
	elif center is Array and center.size() >= 2:
		center_text = "(%s, %s)" % [
			_i18n.format_int(int(center[0])), _i18n.format_int(int(center[1]))
		]
	_detail_label.text = "\n".join([
		str(record.get("name", "")),
		"id %s · owned %s · owner id %s · owner label \"%s\"" % [
			_i18n.format_int(int(record.get("id", 0))),
			"yes" if bool(record.get("owned", false)) else "no",
			_owner_text(record),
			str(record.get("owner", "")),
		],
		"center %s · radius %s m · tax %s%%" % [
			center_text,
			_i18n.format_int(int(record.get("radius_m", 0))),
			_i18n.format_int(int(record.get("tax_rate", 0))),
		],
		"membership revision %s · checksum %s" % [
			_i18n.format_int(int(record.get("membership_revision", 0))),
			str(record.get("membership_checksum", "—")),
		],
		"members: %s" % _members_text(record),
	])


func _set_message(text: String, is_error: bool) -> void:
	if _message_label == null:
		return
	_message_label.text = text
	_message_label.add_theme_color_override("font_color", ERROR_COLOR if is_error else OK_COLOR)


func _select(id: int) -> void:
	if _find_record(id).is_empty():
		_selected_id = 0
		_form_title.text = "No settlement selected"
		_rebuild_list()
		return
	_selected_id = id
	var record := _find_record(id)
	_name_edit.text = str(record.get("name", ""))
	_tax_spin.set_value_no_signal(float(record.get("tax_rate", 0)))
	_form_title.text = "Selected #%s" % _i18n.format_int(id)
	for index in range(_list.item_count):
		if int(_list.get_item_metadata(index)) == id:
			_list.select(index)
			break
	_refresh_detail()


func _select_number(index: int) -> void:
	if index < 0 or index >= _settlements.size():
		return
	_select(int(_settlements[index].get("id", 0)))


func _selected_or_message(context: String) -> int:
	if _selected_id == 0:
		_set_message("%s failed: no_selection (select a settlement first)" % context, true)
	return _selected_id


# --- Actions ---------------------------------------------------------------

func _on_found_pressed() -> void:
	var name := _name_edit.text.strip_edges()
	if name.is_empty():
		name = "%s %s" % [DEFAULT_NAME, _i18n.format_int(_settlements.size() + 1)]
	_found(name, int(_x_spin.value), int(_z_spin.value), int(_radius_spin.value))


func _on_fixture_pressed() -> void:
	# Two settlements: #1 stays unowned, #2 is claimed by player 1. This is the
	# same fixture the smoke uses, so a human can replay the script by hand.
	_reset_session(true)
	_found("Smoke Commons", 0, 0, 256)
	_found("Smoke Keep", 1024, 0, 256)
	var second := _find_record(2)
	if not second.is_empty():
		_select(2)


func _on_claim_pressed() -> void:
	var id := _selected_or_message("Claim")
	if id != 0:
		_claim(id, int(_player_spin.value))


func _on_rename_pressed() -> void:
	var id := _selected_or_message("Rename")
	if id != 0:
		_rename(id, _name_edit.text.strip_edges())


func _on_transfer_pressed() -> void:
	var id := _selected_or_message("Transfer")
	if id != 0:
		_transfer(id, int(_player_spin.value))


func _on_invite_pressed() -> void:
	var id := _selected_or_message("Invite")
	if id != 0:
		_invite(id, int(_player_spin.value), _selected_role())


func _on_set_role_pressed() -> void:
	var id := _selected_or_message("Set role")
	if id != 0:
		_set_role(id, int(_player_spin.value), _selected_role())


func _on_remove_member_pressed() -> void:
	var id := _selected_or_message("Remove member")
	if id != 0:
		_remove_member(id, int(_player_spin.value))


func _on_set_tax_pressed() -> void:
	var id := _selected_or_message("Set tax")
	if id != 0:
		_set_tax(id, int(_tax_spin.value))


func _on_refresh_pressed() -> void:
	_refresh()
	_set_message("List refreshed.", false)


func _on_new_session_pressed() -> void:
	_reset_session(true)


func _on_list_selected(index: int) -> void:
	_select(int(_list.get_item_metadata(index)))


func _selected_role() -> String:
	var index := _role_option.selected
	if index < 0 or index >= ROLES.size():
		return "member"
	return ROLES[index]


func _default_center(index: int) -> Vector2i:
	return DEFAULT_CENTERS[index % DEFAULT_CENTERS.size()]


func _unhandled_key_input(event: InputEvent) -> void:
	var key := event as InputEventKey
	if key == null or not key.pressed or key.echo:
		return
	var handled := true
	match key.keycode:
		KEY_1, KEY_2, KEY_3, KEY_4, KEY_5, KEY_6, KEY_7, KEY_8, KEY_9:
			_select_number(key.keycode - KEY_1)
		KEY_F:
			_on_found_pressed()
		KEY_C:
			_on_claim_pressed()
		KEY_R:
			_on_rename_pressed()
		KEY_T:
			_on_transfer_pressed()
		KEY_I:
			_on_invite_pressed()
		KEY_P:
			_on_set_role_pressed()
		KEY_X:
			_on_remove_member_pressed()
		KEY_M:
			_on_set_tax_pressed()
		KEY_N:
			_on_new_session_pressed()
		_: handled = false
	if handled:
		get_viewport().set_input_as_handled()


# --- Smoke -----------------------------------------------------------------

func _expect_ok(label: String, result: Dictionary) -> bool:
	_actions += 1
	var ok := bool(result.get("ok", false))
	var code := "-" if ok else str(result.get("error_code", "error"))
	print(
		"SETTLEMENT_EDITOR_ACTION seq=%s name=%s ok=%s code=%s" % [
			_i18n.format_int(_actions), label, "1" if ok else "0", code
		]
	)
	if not ok:
		_failures += 1
		print("SETTLEMENT_EDITOR_FAIL seq=%s name=%s code=%s" % [
			_i18n.format_int(_actions), label, code
		])
	return ok


func _expect_error(label: String, result: Dictionary, expected: String) -> bool:
	_actions += 1
	var ok := bool(result.get("ok", false))
	var code := str(result.get("error_code", ""))
	var matched := (not ok) and code == expected
	print(
		"SETTLEMENT_EDITOR_ERROR seq=%s name=%s code=%s expected=%s match=%s" % [
			_i18n.format_int(_actions), label, code, expected, "1" if matched else "0"
		]
	)
	if not matched:
		_failures += 1
		print("SETTLEMENT_EDITOR_FAIL seq=%s name=%s code=%s expected=%s" % [
			_i18n.format_int(_actions), label, code, expected
		])
	return matched


func _check(label: String, condition: bool) -> bool:
	_actions += 1
	print(
		"SETTLEMENT_EDITOR_CHECK seq=%s name=%s ok=%s" % [
			_i18n.format_int(_actions), label, "1" if condition else "0"
		]
	)
	if not condition:
		_failures += 1
		print("SETTLEMENT_EDITOR_FAIL seq=%s name=%s code=check" % [
			_i18n.format_int(_actions), label
		])
	return condition


func _run_smoke() -> void:
	print("SETTLEMENT_EDITOR_SMOKE start seed=%s" % _i18n.format_int(_seed))
	_reset_session(true)

	var empty: Dictionary = _core.settlements()
	_check(
		"empty",
		bool(empty.get("ok", false))
			and not bool(empty.get("open", true))
			and int(empty.get("count", -1)) == 0
			and str(empty.get("checksum", "")) == "0000000000000000"
	)

	# --- Fixture: one unowned, one claimed by player 1. ---
	var alpha := _found("Smoke Commons", 0, 0, 256)
	_expect_ok("found_alpha", alpha)
	_select(1)
	var beta := _found("Smoke Keep", 1024, 0, 256)
	_expect_ok("found_beta", beta)

	var claimed := _claim(2, 1)
	_expect_ok("claim_beta_p1", claimed)
	var beta_record := _fresh_record(2)
	_check(
		"claim_revision",
		int(beta_record.get("membership_revision", -1)) == 1
			and bool(beta_record.get("owned", false))
			and int(beta_record.get("owner_id", 0)) == 1
	)
	_check("alpha_unowned", not bool(_fresh_record(1).get("owned", true)))

	_expect_ok("invite_p2_manager", _invite(2, 2, "manager"))
	_expect_ok("invite_p3_builder", _invite(2, 3, "builder"))
	var members := _fresh_record(2)
	_check(
		"members_after_invite",
		_members_text(members) == "1:owner, 2:manager, 3:builder"
			and int(members.get("membership_revision", -1)) == 3
	)

	_expect_ok("rename_beta", _rename(2, "Smoke Keep Renamed"))
	_expect_ok("set_tax_beta", _set_tax(2, 12))
	_check(
		"tax_without_revision_bump",
		int(_fresh_record(2).get("membership_revision", -1)) == 4
			and int(_fresh_record(2).get("tax_rate", -1)) == 12
	)

	_expect_ok("transfer_beta_p2", _transfer(2, 2))
	_check(
		"transfer_demotes_previous_owner",
		_members_text(_fresh_record(2)) == "1:manager, 2:owner, 3:builder"
			and int(_fresh_record(2).get("owner_id", 0)) == 2
	)

	_expect_ok("set_role_p3_member", _set_role(2, 3, "member"))
	_expect_ok("remove_p3", _remove_member(2, 3))
	_check(
		"members_after_remove",
		_members_text(_fresh_record(2)) == "1:manager, 2:owner"
	)

	_expect_ok("transfer_beta_p3", _transfer(2, 3))

	# --- Rejections; none may mutate the store. ---
	var revision_before := _revision_of(2)
	var checksum_before := str(_core.settlements().get("checksum", ""))
	_expect_error("claim_already_owned", _claim(2, 4), "settlement.already_owned")
	_expect_error("rename_unowned", _rename(1, "Nope"), "settlement.unowned")
	_expect_error(
		"claim_player_zero",
		_core.settlement_claim(2, 0, _revision_of(2)),
		"invalid_player_id"
	)
	# A stale revision is supplied directly: `_rename` always re-reads.
	_expect_error(
		"stale_revision",
		_core.settlement_rename(2, "Stale", 1),
		"settlement.revision_conflict"
	)
	_expect_error(
		"owner_role_rejected",
		_invite(2, 4, "owner"),
		"settlement.role_invalid"
	)
	_expect_error(
		"remove_owner_rejected",
		_remove_member(2, 3),
		"settlement.owner_required"
	)
	_check(
		"rejections_leave_store_unchanged",
		_revision_of(2) == revision_before
			and str(_core.settlements().get("checksum", "")) == checksum_before
	)

	var final_record := _fresh_record(2)
	print(
		"SETTLEMENT_EDITOR_STATE count=%s id=2 owned=%s owner=%s revision=%s tax=%s members=%s" % [
			_i18n.format_int(_settlements.size()),
			"1" if bool(final_record.get("owned", false)) else "0",
			_owner_text(final_record),
			_i18n.format_int(int(final_record.get("membership_revision", 0))),
			_i18n.format_int(int(final_record.get("tax_rate", 0))),
			_members_text(final_record),
		]
	)
	print(
		"SETTLEMENT_EDITOR_UNOWNED id=1 owned=%s owner=%s revision=%s" % [
			"1" if bool(_fresh_record(1).get("owned", false)) else "0",
			_owner_text(_fresh_record(1)),
			_i18n.format_int(int(_fresh_record(1).get("membership_revision", 0))),
		]
	)
	if _failures > 0:
		print("SETTLEMENT_EDITOR status=failed actions=%s failures=%s checksum=%s" % [
			_i18n.format_int(_actions),
			_i18n.format_int(_failures),
			_checksum,
		])
		get_tree().quit(1)
		return
	print("SETTLEMENT_EDITOR status=ok actions=%s checksum=%s" % [
		_i18n.format_int(_actions), _checksum
	])
	get_tree().quit(0)


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
	title.text = "Settlement ownership editor"
	title.add_theme_font_size_override("font_size", 30)
	root.add_child(title)

	var subtitle := Label.new()
	subtitle.text = (
		"Developer tool over the local data-only bridge (docs/48 §2.2). "
		+ "No server, protocol, simulation, or save changes: this edits the "
		+ "in-memory settlement layer for one seed. Player ids are explicit and "
		+ "bridge error codes are authoritative."
	)
	subtitle.add_theme_color_override("font_color", MUTED_COLOR)
	subtitle.add_theme_font_size_override("font_size", 14)
	subtitle.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	root.add_child(subtitle)

	root.add_child(_build_list_panel())

	var columns := HBoxContainer.new()
	columns.add_theme_constant_override("separation", 20)
	columns.size_flags_vertical = Control.SIZE_EXPAND_FILL
	root.add_child(columns)
	columns.add_child(_build_detail_panel())
	columns.add_child(_build_form_panel())

	_message_label = Label.new()
	_message_label.text = "Ready."
	_message_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	root.add_child(_message_label)

	var help := Label.new()
	help.text = (
		"Keys: 1–9 select · F found · C claim · R rename · T transfer · "
		+ "I invite · P set role · X remove member · M set tax · N new session"
	)
	help.add_theme_color_override("font_color", MUTED_COLOR)
	help.add_theme_font_size_override("font_size", 13)
	help.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	root.add_child(help)

	var footer := Label.new()
	footer.text = "Local bridge only · never contacts a server · errors never abort the tool."
	footer.add_theme_color_override("font_color", MUTED_COLOR)
	footer.add_theme_font_size_override("font_size", 12)
	root.add_child(footer)


func _build_list_panel() -> Control:
	var panel := VBoxContainer.new()
	panel.custom_minimum_size = Vector2(620, 220)
	panel.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	panel.add_theme_constant_override("separation", 6)

	var toolbar := HBoxContainer.new()
	toolbar.add_theme_constant_override("separation", 8)
	panel.add_child(toolbar)

	var seed_label := Label.new()
	seed_label.text = "Seed %s" % _i18n.format_int(DEFAULT_SEED)
	seed_label.add_theme_color_override("font_color", MUTED_COLOR)
	toolbar.add_child(seed_label)

	toolbar.add_child(_button("Found (F)", _on_found_pressed))
	toolbar.add_child(_button("Fixture", _on_fixture_pressed))
	toolbar.add_child(_button("Refresh", _on_refresh_pressed))
	toolbar.add_child(_button("New session (N)", _on_new_session_pressed))

	_state_label = Label.new()
	_state_label.add_theme_color_override("font_color", TEXT_COLOR)
	panel.add_child(_state_label)

	_list = ItemList.new()
	_list.size_flags_vertical = Control.SIZE_EXPAND_FILL
	_list.custom_minimum_size = Vector2(0, 160)
	_list.item_selected.connect(_on_list_selected)
	panel.add_child(_list)
	return panel


func _build_detail_panel() -> Control:
	var panel := VBoxContainer.new()
	panel.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	panel.add_theme_constant_override("separation", 6)

	var title := Label.new()
	title.text = "Selected record"
	title.add_theme_font_size_override("font_size", 20)
	panel.add_child(title)

	_detail_label = Label.new()
	_detail_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	_detail_label.size_flags_vertical = Control.SIZE_EXPAND_FILL
	panel.add_child(_detail_label)
	return panel


func _build_form_panel() -> Control:
	var panel := VBoxContainer.new()
	panel.custom_minimum_size = Vector2(420, 0)
	panel.add_theme_constant_override("separation", 8)

	_form_title = Label.new()
	_form_title.text = "No settlement selected"
	_form_title.add_theme_font_size_override("font_size", 20)
	panel.add_child(_form_title)

	var grid := GridContainer.new()
	grid.columns = 2
	grid.add_theme_constant_override("h_separation", 10)
	grid.add_theme_constant_override("v_separation", 6)
	panel.add_child(grid)

	grid.add_child(_label("Name"))
	_name_edit = LineEdit.new()
	_name_edit.text = "Settlement"
	_name_edit.custom_minimum_size = Vector2(240, 0)
	grid.add_child(_name_edit)

	grid.add_child(_label("Centre X"))
	_x_spin = _spin(-1000000, 1000000, 0)
	grid.add_child(_x_spin)

	grid.add_child(_label("Centre Z"))
	_z_spin = _spin(-1000000, 1000000, 0)
	grid.add_child(_z_spin)

	grid.add_child(_label("Radius m"))
	_radius_spin = _spin(MIN_RADIUS_M, MAX_RADIUS_M, DEFAULT_RADIUS_M)
	grid.add_child(_radius_spin)

	grid.add_child(_label("Player id"))
	_player_spin = _spin(1, MAX_PLAYER_ID, 1)
	grid.add_child(_player_spin)

	grid.add_child(_label("Role"))
	_role_option = OptionButton.new()
	for role in ROLES:
		_role_option.add_item(role)
	grid.add_child(_role_option)

	grid.add_child(_label("Tax %"))
	_tax_spin = _spin(0, MAX_TAX_RATE, 0)
	grid.add_child(_tax_spin)

	var actions := HBoxContainer.new()
	actions.add_theme_constant_override("separation", 6)
	panel.add_child(actions)
	actions.add_child(_button("Claim (C)", _on_claim_pressed))
	actions.add_child(_button("Rename (R)", _on_rename_pressed))
	actions.add_child(_button("Transfer (T)", _on_transfer_pressed))

	var actions_two := HBoxContainer.new()
	actions_two.add_theme_constant_override("separation", 6)
	panel.add_child(actions_two)
	actions_two.add_child(_button("Invite (I)", _on_invite_pressed))
	actions_two.add_child(_button("Set role (P)", _on_set_role_pressed))
	actions_two.add_child(_button("Remove member (X)", _on_remove_member_pressed))
	actions_two.add_child(_button("Set tax (M)", _on_set_tax_pressed))

	var note := Label.new()
	note.text = (
		"Claim needs an explicit positive player id. Rename/transfer/invite/"
		+ "set-role/remove/tax use the current owner as actor; an unowned "
		+ "settlement rejects them. The revision is re-read before every call."
	)
	note.add_theme_color_override("font_color", MUTED_COLOR)
	note.add_theme_font_size_override("font_size", 12)
	note.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	panel.add_child(note)
	return panel


func _button(text: String, callback: Callable) -> Button:
	var button := Button.new()
	button.text = text
	button.pressed.connect(callback)
	return button


func _label(text: String) -> Label:
	var label := Label.new()
	label.text = text
	return label


func _spin(minimum: int, maximum: int, value: int) -> SpinBox:
	var spin := SpinBox.new()
	spin.min_value = minimum
	spin.max_value = maximum
	spin.step = 1
	spin.value = value
	spin.custom_minimum_size = Vector2(140, 0)
	return spin
