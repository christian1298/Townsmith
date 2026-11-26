extends Control

## Standalone developer editor for local settlement ownership and membership.
##
## This page is deliberately outside the world scene and the in-game dashboard:
## it owns one `TownsmithCore` instance and only calls the local data-only
## bridge mutations (`settlement_found`, `settlement_claim`, `settlement_rename`,
## `settlement_transfer`, `settlement_invite`, `settlement_set_role`,
## `settlement_remove_member`, `settlement_set_tax`, `settlement_release`) plus
## `settlements()` reads. No server, protocol, simulation, treasury, or save/load
## code is involved. See `docs/48 - Settlement Boundaries.md` §2.2.
##
## The record list is paged client-side (8 rows per page) so the D-033 1,000
## settlement cap stays usable. `PageUp`/`PageDown` move pages; number keys and
## mutations apply to the visible page only. Paging is display state: it never
## calls the bridge, persists anything, or changes the protocol.
##
## Run:
##   godot --path godot res://settlement_editor.tscn
##   godot --headless --path godot res://settlement_editor.tscn -- --smoke
##
## Headless smoke: starts empty, founds two deterministic settlements (one
## unowned, one claimed by player 1), invites a manager and a builder, renames,
## sets tax, transfers ownership twice, changes a role, removes a member, and
## rejects an already-owned claim, an unowned rename, a stale revision, the
## owner role, removing the owner, and player id 0. It then releases a separate
## owned record through the two-press confirmation and rejects a second
## release. Every mutation re-reads the record's `membership_revision` first and
## re-reads `settlements()` after success. Final
## marker:
##   SETTLEMENT_EDITOR status=ok actions=<n> checksum=<16 hex>
##
## The page also uses the local bridge player registry (`players_register`,
## `players_list`) as a developer address book: the header shows the roster,
## `N` registers the next deterministic `Player <n>` name, and `P` cycles the
## pending invite/transfer target so an id never has to be typed by hand. In
## `--smoke` mode only, three names (`Mayor`, `Builder`, `Auditor`) are seeded
## so the smoke can exercise name-to-id selection deterministically.
##
## Strings resolve through the `base.ui.settlement_editor.*` keys in
## `world_i18n.gd`; the standalone page follows the same EN/DE rules as the
## in-game UI. Settlement names, player ids, roles, and bridge error codes stay
## verbatim. Display numbers go through `WorldI18n.format_int` so grouping and
## separators follow `docs/16 - Localization.md`. The new roster/target labels
## stay literal English in this developer-only page because `world_i18n.gd` is
## outside this slice; they are not missing-key leaks.

const WorldI18n = preload("res://world_i18n.gd")

const SUITE := "settlement_editor"
const DEFAULT_SEED := 2026
const DEFAULT_NAME := "Settlement"
const DEFAULT_OWNER_LABEL := "wilderness"
const DEFAULT_RADIUS_M := 256

# Client-side list paging. The ItemList fits eight rows in the current panel;
# the D-033 cap is 1,000 settlements, so paging keeps every record reachable
# without one giant list. This is display-only state, never bridge state.
const PAGE_SIZE := 8
const PAGING_FIXTURE_COUNT := 23

# Smoke-only seed names. They prove name-to-id selection without wall-clock or
# RNG input; normal runs start with an empty registry.
const SMOKE_PLAYER_NAMES := ["Mayor", "Builder", "Auditor"]
const PLAYER_REGISTRY_CAP := 256

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
var _page := 0
var _page_count := 1
var _actions := 0
var _failures := 0

var _title_label: Label
var _state_label: Label
var _roster_label: Label
var _target_label: Label
var _message_label: Label
var _detail_label: Label
var _page_label: Label
var _list: ItemList
var _form_title: Label

var _name_edit: LineEdit
var _x_spin: SpinBox
var _z_spin: SpinBox
var _radius_spin: SpinBox
var _player_spin: SpinBox
var _role_option: OptionButton
var _tax_spin: SpinBox

var _players: Array = []
var _target_player_id := 1
var _target_player_name := ""
# Set by the first release press; a second press on the same record confirms.
var _pending_release_id := 0


func _ready() -> void:
	var args := OS.get_cmdline_user_args()
	smoke_mode = args.has("--smoke") or args.has("--smoke-de")
	_i18n = WorldI18n.new()
	if args.has("--smoke-de"):
		_i18n.set_locale("de")
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
	_page = 0
	_page_count = 1
	_target_player_id = 1
	_target_player_name = ""
	_pending_release_id = 0
	if reset_seed:
		_seed = DEFAULT_SEED
	if smoke_mode:
		_seed_smoke_players()
	_refresh_players()
	_refresh()
	_set_message(_i18n.t("base.ui.settlement_editor.message.new_session", {
		"seed": _i18n.format_int(_seed),
	}), false)


# --- Player registry (local bridge address book) ---------------------------

## Seed deterministic names for the smoke only. Normal runs start empty and the
## developer registers names with `N` (or through another local bridge caller).
func _seed_smoke_players() -> void:
	if _core == null:
		return
	for player_name in SMOKE_PLAYER_NAMES:
		_core.players_register(player_name)


func _refresh_players() -> void:
	_players = []
	if _core != null:
		var result: Dictionary = _core.players_list()
		if bool(result.get("ok", false)):
			var records = result.get("players", [])
			_players = records if records is Array else []
	_sync_target_name()
	_refresh_roster_label()
	_refresh_target_label()


func _sync_target_name() -> void:
	_target_player_name = ""
	for player in _players:
		if int(player.get("id", 0)) == _target_player_id:
			_target_player_name = str(player.get("name", ""))
			return


func _roster_text() -> String:
	if _players.is_empty():
		return "Players: none"
	var parts := PackedStringArray()
	for player in _players:
		parts.append("#%s %s" % [
			_i18n.format_int(int(player.get("id", 0))),
			str(player.get("name", "")),
		])
	return "Players: " + ", ".join(parts)


func _target_text() -> String:
	if _target_player_name.is_empty():
		return "Target: #%s" % _i18n.format_int(_target_player_id)
	return "Target: #%s %s" % [
		_i18n.format_int(_target_player_id), _target_player_name
	]


func _refresh_roster_label() -> void:
	if _roster_label != null:
		_roster_label.text = _roster_text()


func _refresh_target_label() -> void:
	if _target_label != null:
		_target_label.text = _target_text()


func _player_count(core) -> int:
	if core == null:
		return 0
	var result: Dictionary = core.players_list()
	if bool(result.get("ok", false)):
		return int(result.get("count", 0))
	return 0


## Register a name through the local bridge. UI state only changes for the
## editor's own core; the cap probe reuses this helper on a throwaway core.
func _register_player_on(core, player_name: String) -> Dictionary:
	if core == null:
		return _no_core("register player")
	var result: Dictionary = core.players_register(player_name)
	if core == _core:
		_apply_bridge_result("register player", result)
		_refresh_players()
		if bool(result.get("ok", false)):
			var player = result.get("player", {})
			_set_message("Registered #%s %s." % [
				_i18n.format_int(int(player.get("id", 0))),
				str(player.get("name", "")),
			], false)
	return result


func _register_next_player_on(core) -> Dictionary:
	var next_name := "Player %d" % (_player_count(core) + 1)
	return _register_player_on(core, next_name)


## Cycle the pending invite/transfer target in ascending id order. With no
## registered players the previous id is kept and the failure is reported.
func _cycle_target() -> int:
	if _players.is_empty():
		_set_message("No players registered; keeping target #%s. Press N to register." % [
			_i18n.format_int(_target_player_id)
		], true)
		return _target_player_id
	var next_index := 0
	for index in range(_players.size()):
		if int(_players[index].get("id", 0)) == _target_player_id:
			next_index = (index + 1) % _players.size()
			break
	var player: Dictionary = _players[next_index]
	_target_player_id = int(player.get("id", _target_player_id))
	_target_player_name = str(player.get("name", ""))
	_refresh_target_label()
	_set_message("Target: #%s %s" % [
		_i18n.format_int(_target_player_id), _target_player_name
	], false)
	return _target_player_id


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
		_apply_bridge_result(_i18n.t("base.ui.settlement_editor.action.list"), result)
		return
	var records = result.get("settlements", [])
	_settlements = records if records is Array else []
	_checksum = str(result.get("checksum", ""))
	if _find_record(_selected_id).is_empty():
		if _selected_id != 0 and _form_title != null:
			_form_title.text = _i18n.t("base.ui.settlement_editor.no_selection")
		_selected_id = 0
	_sync_page_to_selection()
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
		return _no_core(_i18n.t("base.ui.settlement_editor.action.found"))
	var result: Dictionary = _core.settlement_found(
		_seed, name, DEFAULT_OWNER_LABEL, x, z, radius
	)
	_apply_bridge_result("%s %s" % [
		_i18n.t("base.ui.settlement_editor.action.found"), name
	], result)
	_refresh()
	return result


func _claim(id: int, player_id: int) -> Dictionary:
	if _core == null:
		return _no_core(_i18n.t("base.ui.settlement_editor.action.claim"))
	var revision := _revision_of(id)
	if revision < 0:
		return _missing(_i18n.t("base.ui.settlement_editor.action.claim"), id)
	var result: Dictionary = _core.settlement_claim(id, player_id, revision)
	return _finish_mutation("%s #%s" % [
		_i18n.t("base.ui.settlement_editor.action.claim"), _i18n.format_int(id)
	], result)


func _rename(id: int, name: String) -> Dictionary:
	if _core == null:
		return _no_core(_i18n.t("base.ui.settlement_editor.action.rename"))
	var revision := _revision_of(id)
	if revision < 0:
		return _missing(_i18n.t("base.ui.settlement_editor.action.rename"), id)
	var result: Dictionary = _core.settlement_rename(id, name, revision)
	return _finish_mutation("%s #%s" % [
		_i18n.t("base.ui.settlement_editor.action.rename"), _i18n.format_int(id)
	], result)


func _transfer(id: int, player_id: int) -> Dictionary:
	if _core == null:
		return _no_core(_i18n.t("base.ui.settlement_editor.action.transfer"))
	var revision := _revision_of(id)
	if revision < 0:
		return _missing(_i18n.t("base.ui.settlement_editor.action.transfer"), id)
	var result: Dictionary = _core.settlement_transfer(id, player_id, revision)
	return _finish_mutation("%s #%s" % [
		_i18n.t("base.ui.settlement_editor.action.transfer"), _i18n.format_int(id)
	], result)


## Admin override: clear owner and every member without an actor. The first
## press arms the selected record; the caller must press again to confirm, so a
## single key press can never release a settlement.
func _release(id: int, confirmed: bool) -> Dictionary:
	if _core == null:
		return _no_core("release")
	if not confirmed:
		var record := _fresh_record(id)
		if record.is_empty():
			_pending_release_id = 0
			return _missing("release", id)
		if not bool(record.get("owned", false)):
			_pending_release_id = 0
			_set_message("Release #%s: settlement is already unowned." % [
				_i18n.format_int(id)
			], true)
			return {
				"ok": false,
				"error_code": "settlement.unowned",
				"error": "settlement is unowned",
			}
		_pending_release_id = id
		_set_message(
			"Release #%s clears owner and members. Press Shift+X again to confirm."
				% _i18n.format_int(id),
			false
		)
		return {
			"ok": false,
			"error_code": "confirm_required",
			"error": "release confirmation required",
		}
	var revision := _revision_of(id)
	if revision < 0:
		_pending_release_id = 0
		return _missing("release", id)
	_pending_release_id = 0
	var result: Dictionary = _core.settlement_release(id, revision)
	return _finish_mutation("release #%s" % _i18n.format_int(id), result)


func _invite(id: int, player_id: int, role: String) -> Dictionary:
	if _core == null:
		return _no_core(_i18n.t("base.ui.settlement_editor.action.invite"))
	var revision := _revision_of(id)
	if revision < 0:
		return _missing(_i18n.t("base.ui.settlement_editor.action.invite"), id)
	var result: Dictionary = _core.settlement_invite(id, player_id, role, revision)
	return _finish_mutation("%s #%s" % [
		_i18n.t("base.ui.settlement_editor.action.invite"), _i18n.format_int(id)
	], result)


func _set_role(id: int, player_id: int, role: String) -> Dictionary:
	if _core == null:
		return _no_core(_i18n.t("base.ui.settlement_editor.action.set_role"))
	var revision := _revision_of(id)
	if revision < 0:
		return _missing(_i18n.t("base.ui.settlement_editor.action.set_role"), id)
	var result: Dictionary = _core.settlement_set_role(id, player_id, role, revision)
	return _finish_mutation("%s #%s" % [
		_i18n.t("base.ui.settlement_editor.action.set_role"), _i18n.format_int(id)
	], result)


func _remove_member(id: int, player_id: int) -> Dictionary:
	if _core == null:
		return _no_core(_i18n.t("base.ui.settlement_editor.action.remove_member"))
	var revision := _revision_of(id)
	if revision < 0:
		return _missing(_i18n.t("base.ui.settlement_editor.action.remove_member"), id)
	var result: Dictionary = _core.settlement_remove_member(id, player_id, revision)
	return _finish_mutation("%s #%s" % [
		_i18n.t("base.ui.settlement_editor.action.remove_member"), _i18n.format_int(id)
	], result)


func _set_tax(id: int, rate: int) -> Dictionary:
	if _core == null:
		return _no_core(_i18n.t("base.ui.settlement_editor.action.set_tax"))
	var result: Dictionary = _core.settlement_set_tax(id, rate)
	return _finish_mutation("%s #%s" % [
		_i18n.t("base.ui.settlement_editor.action.set_tax"), _i18n.format_int(id)
	], result)


func _finish_mutation(context: String, result: Dictionary) -> Dictionary:
	_pending_release_id = 0
	_apply_bridge_result(context, result)
	if bool(result.get("ok", false)):
		_refresh()
	return result


func _no_core(context: String) -> Dictionary:
	_set_message(_i18n.t("base.ui.settlement_editor.message.no_core", {
		"context": context,
	}), true)
	return {"ok": false, "error_code": "no_core", "error": "bridge not loaded"}


func _missing(context: String, id: int) -> Dictionary:
	_set_message(_i18n.t("base.ui.settlement_editor.message.missing", {
		"context": context,
		"id": _i18n.format_int(id),
	}), true)
	return {"ok": false, "error_code": "settlement.unknown", "error": "unknown settlement"}


func _apply_bridge_result(context: String, result: Dictionary) -> bool:
	if bool(result.get("ok", false)):
		_set_message(_i18n.t("base.ui.settlement_editor.message.ok", {
			"context": context,
		}), false)
		return true
	var code := str(result.get("error_code", "error"))
	var message := str(result.get("error", ""))
	_set_message(_i18n.t("base.ui.settlement_editor.message.failed", {
		"context": context,
		"code": code,
		"message": message,
	}), true)
	return false


# --- Editor state ----------------------------------------------------------

func _index_of(id: int) -> int:
	for index in range(_settlements.size()):
		if int(_settlements[index].get("id", 0)) == id:
			return index
	return -1


func _find_record(id: int) -> Dictionary:
	var index := _index_of(id)
	if index < 0:
		return {}
	return _settlements[index]


# --- Paging ----------------------------------------------------------------

func _page_total() -> int:
	return maxi(1, ceili(float(_settlements.size()) / float(PAGE_SIZE)))


func _clamp_page() -> void:
	_page_count = _page_total()
	_page = clampi(_page, 0, _page_count - 1)


func _page_records() -> Array:
	if _settlements.is_empty():
		return []
	var start := _page * PAGE_SIZE
	return _settlements.slice(start, mini(start + PAGE_SIZE, _settlements.size()))


func _first_id_at(page: int) -> int:
	var index := page * PAGE_SIZE
	if index < 0 or index >= _settlements.size():
		return 0
	return int(_settlements[index].get("id", 0))


func _first_id_on_page() -> int:
	return _first_id_at(_page)


## Point `_page` at the selected record when it exists. Used after a re-read so
## a mutation or refresh never silently hides the selected row on another page.
func _sync_page_to_selection() -> void:
	var index := _index_of(_selected_id)
	if index >= 0:
		_page = int(index / float(PAGE_SIZE))
	_clamp_page()


## Move one page and keep a valid selection. When the old selection is not on
## the new page, the first visible row becomes selected. Returns false at the
## first/last page boundary so callers can detect the clamp.
func _page_by(delta: int) -> bool:
	_clamp_page()
	var next_page := clampi(_page + delta, 0, _page_count - 1)
	if next_page == _page:
		return false
	var index := _index_of(_selected_id)
	if index < 0 or int(index / float(PAGE_SIZE)) != next_page:
		var first_id := _first_id_at(next_page)
		if first_id != 0:
			# `_select` points `_page` at the record and rebuilds the list when
			# the page actually changes.
			_select(first_id)
			return true
	_page = next_page
	_rebuild_list()
	return true


## Ids currently rendered in the ItemList, i.e. the visible page only.
func _visible_ids() -> Array:
	var ids: Array = []
	if _list == null:
		return ids
	for index in range(_list.item_count):
		ids.append(int(_list.get_item_metadata(index)))
	return ids


func _members_text(record: Dictionary) -> String:
	var members = record.get("members", [])
	if not (members is Array) or (members as Array).is_empty():
		return _i18n.t("base.ui.settlement_editor.members.none")
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
		return _i18n.t("base.ui.settlement_editor.owner.unowned")
	return _i18n.format_int(int(owner_id))


## Locale-independent smoke marker. `_owner_text` is display text; the marker
## contract stays `unowned`/raw id in both EN and DE runs.
func _owner_marker(record: Dictionary) -> String:
	var owner_id = record.get("owner_id", null)
	if not bool(record.get("owned", false)) or owner_id == null:
		return "unowned"
	return str(int(owner_id))


func _row_text(record: Dictionary) -> String:
	return _i18n.t("base.ui.settlement_editor.row", {
		"id": _i18n.format_int(int(record.get("id", 0))),
		"name": str(record.get("name", "")),
		"owner": _owner_text(record),
		"revision": _i18n.format_int(int(record.get("membership_revision", 0))),
		"tax": _i18n.format_int(int(record.get("tax_rate", 0))),
		"members": _members_text(record),
	})


func _rebuild_list() -> void:
	if _list == null:
		return
	_clamp_page()
	_list.clear()
	for record in _page_records():
		var id := int(record.get("id", 0))
		_list.add_item(_row_text(record))
		_list.set_item_metadata(_list.item_count - 1, id)
		if id == _selected_id:
			_list.select(_list.item_count - 1)
	_refresh_page_label()
	_refresh_state_label()
	_refresh_detail()


func _refresh_page_label() -> void:
	if _page_label == null:
		return
	_page_label.visible = _page_count > 1
	if _page_count <= 1:
		_page_label.text = ""
		return
	_page_label.text = "Page %d/%d · %s settlements · PageUp/PageDown" % [
		_page + 1, _page_count, _i18n.format_int(_settlements.size())
	]


func _refresh_state_label() -> void:
	if _state_label == null:
		return
	_state_label.text = _i18n.t("base.ui.settlement_editor.state", {
		"seed": _i18n.format_int(_seed),
		"count": _i18n.format_int(_settlements.size()),
		"checksum": _checksum if not _checksum.is_empty() else "—",
	})


func _refresh_detail() -> void:
	if _detail_label == null:
		return
	var record := _find_record(_selected_id)
	if record.is_empty():
		_detail_label.text = _i18n.t("base.ui.settlement_editor.detail_empty")
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
		_i18n.t("base.ui.settlement_editor.detail.identity", {
			"id": _i18n.format_int(int(record.get("id", 0))),
			"owned": _i18n.t(
				"base.ui.settlement_editor.owned.yes"
				if bool(record.get("owned", false))
				else "base.ui.settlement_editor.owned.no"
			),
			"owner": _owner_text(record),
			"label": str(record.get("owner", "")),
		}),
		_i18n.t("base.ui.settlement_editor.detail.center", {
			"center": center_text,
			"radius": _i18n.format_int(int(record.get("radius_m", 0))),
			"tax": _i18n.format_int(int(record.get("tax_rate", 0))),
		}),
		_i18n.t("base.ui.settlement_editor.detail.revision", {
			"revision": _i18n.format_int(int(record.get("membership_revision", 0))),
			"checksum": str(record.get("membership_checksum", "—")),
		}),
		_i18n.t("base.ui.settlement_editor.detail.members", {
			"members": _members_text(record),
		}),
	])


func _set_message(text: String, is_error: bool) -> void:
	if _message_label == null:
		return
	_message_label.text = text
	_message_label.add_theme_color_override("font_color", ERROR_COLOR if is_error else OK_COLOR)


func _select(id: int) -> void:
	_pending_release_id = 0
	var record_index := _index_of(id)
	if record_index < 0:
		_selected_id = 0
		_form_title.text = _i18n.t("base.ui.settlement_editor.no_selection")
		_rebuild_list()
		return
	_selected_id = id
	var page_before := _page
	_page = int(record_index / float(PAGE_SIZE))
	_clamp_page()
	var record: Dictionary = _settlements[record_index]
	_name_edit.text = str(record.get("name", ""))
	_tax_spin.set_value_no_signal(float(record.get("tax_rate", 0)))
	_form_title.text = _i18n.t("base.ui.settlement_editor.form.selected", {
		"id": _i18n.format_int(id),
	})
	if _page != page_before:
		# Selection moved off the visible page; render the new page with the
		# selected row highlighted.
		_rebuild_list()
		return
	for index in range(_list.item_count):
		if int(_list.get_item_metadata(index)) == id:
			_list.select(index)
			break
	_refresh_detail()


func _select_number(index: int) -> void:
	var records := _page_records()
	if index < 0 or index >= records.size():
		return
	_select(int(records[index].get("id", 0)))


func _selected_or_message(context: String) -> int:
	if _selected_id == 0:
		_set_message(_i18n.t("base.ui.settlement_editor.message.no_selection", {
			"context": context,
		}), true)
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
	var id := _selected_or_message(_i18n.t("base.ui.settlement_editor.action.claim"))
	if id != 0:
		_claim(id, int(_player_spin.value))


func _on_rename_pressed() -> void:
	var id := _selected_or_message(_i18n.t("base.ui.settlement_editor.action.rename"))
	if id != 0:
		_rename(id, _name_edit.text.strip_edges())


func _on_transfer_pressed() -> void:
	var id := _selected_or_message(_i18n.t("base.ui.settlement_editor.action.transfer"))
	if id != 0:
		_transfer(id, _target_player_id)


func _on_invite_pressed() -> void:
	var id := _selected_or_message(_i18n.t("base.ui.settlement_editor.action.invite"))
	if id != 0:
		_invite(id, _target_player_id, _selected_role())


func _on_set_role_pressed() -> void:
	var id := _selected_or_message(_i18n.t("base.ui.settlement_editor.action.set_role"))
	if id != 0:
		_set_role(id, int(_player_spin.value), _selected_role())


func _on_remove_member_pressed() -> void:
	var id := _selected_or_message(_i18n.t("base.ui.settlement_editor.action.remove_member"))
	if id != 0:
		_remove_member(id, int(_player_spin.value))


## Shift+X: first press arms, a second press on the same record confirms.
func _on_release_pressed() -> void:
	var id := _selected_or_message("release")
	if id != 0:
		_release(id, _pending_release_id == id)


func _on_set_tax_pressed() -> void:
	var id := _selected_or_message(_i18n.t("base.ui.settlement_editor.action.set_tax"))
	if id != 0:
		_set_tax(id, int(_tax_spin.value))


func _on_refresh_pressed() -> void:
	_refresh()
	_set_message(_i18n.t("base.ui.settlement_editor.message.refreshed"), false)


func _on_new_session_pressed() -> void:
	_reset_session(true)


func _on_register_player_pressed() -> void:
	_register_next_player_on(_core)


func _on_cycle_target_pressed() -> void:
	_cycle_target()


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
		KEY_PAGEUP:
			_page_by(-1)
		KEY_PAGEDOWN:
			_page_by(1)
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
			if key.shift_pressed:
				_on_set_role_pressed()
			else:
				_on_cycle_target_pressed()
		KEY_X:
			if key.shift_pressed:
				_on_release_pressed()
			else:
				_on_remove_member_pressed()
		KEY_M:
			_on_set_tax_pressed()
		KEY_N:
			if key.shift_pressed:
				_on_new_session_pressed()
			else:
				_on_register_player_pressed()
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


func _smoke_player_registry() -> void:
	var seeded_ok := _players.size() == 3
	if seeded_ok:
		for index in range(SMOKE_PLAYER_NAMES.size()):
			seeded_ok = seeded_ok \
				and int(_players[index].get("id", 0)) == index + 1 \
				and str(_players[index].get("name", "")) == SMOKE_PLAYER_NAMES[index]
	print("SETTLEMENT_EDITOR_PLAYERS phase=seed count=%s roster=%s" % [
		_i18n.format_int(_players.size()), _roster_text()
	])
	_check(
		"players_seeded",
		seeded_ok \
			and _roster_label.text == "Players: #1 Mayor, #2 Builder, #3 Auditor"
	)

	var fourth := _register_next_player_on(_core)
	_expect_ok("register_player_4", fourth)
	var fourth_player = fourth.get("player", {})
	print("SETTLEMENT_EDITOR_PLAYERS phase=register count=%s roster=%s" % [
		_i18n.format_int(_players.size()), _roster_text()
	])
	_check(
		"player_four_registered",
		int(fourth_player.get("id", 0)) == 4
			and str(fourth_player.get("name", "")) == "Player 4"
			and _players.size() == 4
			and _roster_label.text
				== "Players: #1 Mayor, #2 Builder, #3 Auditor, #4 Player 4"
	)

	var cycle_one := _cycle_target()
	print("SETTLEMENT_EDITOR_TARGET id=%s name=%s" % [
		_i18n.format_int(cycle_one), _target_player_name
	])
	_check(
		"cycle_target_builder",
		cycle_one == 2 and _target_player_name == "Builder"
	)
	var cycle_two := _cycle_target()
	print("SETTLEMENT_EDITOR_TARGET id=%s name=%s" % [
		_i18n.format_int(cycle_two), _target_player_name
	])
	_check(
		"cycle_target_auditor",
		cycle_two == 3 and _target_player_name == "Auditor"
	)

	# Empty registry: keep the previous id and report it without crashing.
	var saved_players := _players
	var saved_name := _target_player_name
	_players = []
	var kept := _cycle_target()
	var empty_message := _message_label.text.begins_with("No players registered")
	_players = saved_players
	_target_player_name = saved_name
	_refresh_roster_label()
	_refresh_target_label()
	print("SETTLEMENT_EDITOR_TARGET_EMPTY kept=%s message=%s" % [
		_i18n.format_int(kept), "1" if empty_message else "0"
	])
	_check(
		"cycle_target_empty_keeps_target",
		kept == 3 and empty_message
	)

	# Cap and invalid-name rejections on a throwaway core; the editor roster
	# must stay untouched.
	var cap_core = TownsmithCore.new()
	for index in range(PLAYER_REGISTRY_CAP):
		cap_core.players_register("Player %d" % (index + 1))
	_expect_error("player_limit", _register_next_player_on(cap_core), "player.limit")
	_expect_error(
		"player_invalid_name",
		_register_player_on(cap_core, ""),
		"player.invalid_name"
	)


func _smoke_paging() -> void:
	_reset_session(true)
	var empty_hidden := _page_count == 1 and not _page_label.visible and _list.item_count == 0
	print("SETTLEMENT_EDITOR_PAGING phase=empty page=1/1 hidden=%s rows=%s" % [
		"1" if not _page_label.visible else "0",
		_i18n.format_int(_list.item_count),
	])
	_check("paging_empty_hidden", empty_hidden)

	_found("Paged 1", 0, 0, 256)
	var single_hidden := _page_count == 1 and not _page_label.visible and _list.item_count == 1
	print("SETTLEMENT_EDITOR_PAGING phase=single page=1/1 hidden=%s rows=%s" % [
		"1" if not _page_label.visible else "0",
		_i18n.format_int(_list.item_count),
	])
	_check("paging_single_hidden", single_hidden)

	for index in range(1, PAGING_FIXTURE_COUNT):
		_found("Paged %d" % (index + 1), 0, 0, 256)
	var seeded_ok := _settlements.size() == PAGING_FIXTURE_COUNT \
		and _page_count == 3 and _page == 0 and _list.item_count == PAGE_SIZE \
		and _page_label.visible \
		and _page_label.text == "Page 1/3 · 23 settlements · PageUp/PageDown"
	print("SETTLEMENT_EDITOR_PAGING phase=seed count=%s page=1/%s hidden=%s rows=%s" % [
		_i18n.format_int(_settlements.size()), _i18n.format_int(_page_count),
		"0" if _page_label.visible else "1", _i18n.format_int(_list.item_count),
	])
	_check("paging_seeded_pages", seeded_ok)

	# Off-page fallback: selecting id 1 and moving to page 2 must select the
	# first visible row (id 9), not keep an invisible selection.
	_select(1)
	_page_by(1)
	var down_ids := _visible_ids()
	print("SETTLEMENT_EDITOR_PAGING phase=down page=%s/%s selected=%s rows=%s first=%s last=%s" % [
		_i18n.format_int(_page + 1), _i18n.format_int(_page_count),
		_i18n.format_int(_selected_id), _i18n.format_int(down_ids.size()),
		_i18n.format_int(int(down_ids[0])), _i18n.format_int(int(down_ids[-1])),
	])
	_check(
		"paging_page_down",
		_page == 1 and _selected_id == 9 and down_ids.size() == 8 \
			and int(down_ids[0]) == 9 and int(down_ids[-1]) == 16
	)

	_page_by(1)
	var last_ids := _visible_ids()
	print("SETTLEMENT_EDITOR_PAGING phase=down_last page=%s/%s selected=%s rows=%s first=%s last=%s" % [
		_i18n.format_int(_page + 1), _i18n.format_int(_page_count),
		_i18n.format_int(_selected_id), _i18n.format_int(last_ids.size()),
		_i18n.format_int(int(last_ids[0])), _i18n.format_int(int(last_ids[-1])),
	])
	_check(
		"paging_page_down_last",
		_page == 2 and _selected_id == 17 and last_ids.size() == 7 \
			and int(last_ids[0]) == 17 and int(last_ids[-1]) == 23
	)

	# PageDown at the last page clamps and leaves the visible selection alone.
	var clamped := not _page_by(1)
	print("SETTLEMENT_EDITOR_PAGING phase=clamp page=%s/%s selected=%s clamped=%s" % [
		_i18n.format_int(_page + 1), _i18n.format_int(_page_count),
		_i18n.format_int(_selected_id), "1" if clamped else "0",
	])
	_check("paging_clamp_end", clamped and _page == 2 and _selected_id == 17)

	_page_by(-1)
	var up_ids := _visible_ids()
	print("SETTLEMENT_EDITOR_PAGING phase=up page=%s/%s selected=%s rows=%s first=%s last=%s" % [
		_i18n.format_int(_page + 1), _i18n.format_int(_page_count),
		_i18n.format_int(_selected_id), _i18n.format_int(up_ids.size()),
		_i18n.format_int(int(up_ids[0])), _i18n.format_int(int(up_ids[-1])),
	])
	_check(
		"paging_page_up",
		_page == 1 and _selected_id == 9 and up_ids.size() == 8 \
			and int(up_ids[0]) == 9 and int(up_ids[-1]) == 16
	)

	# Number keys index the visible page, not the global array.
	_select_number(2)
	print("SETTLEMENT_EDITOR_PAGING phase=number page=%s/%s index=3 selected=%s" % [
		_i18n.format_int(_page + 1), _i18n.format_int(_page_count),
		_i18n.format_int(_selected_id),
	])
	_check("paging_number_key", _page == 1 and _selected_id == 11)

	# An external/refresh selection on another page moves the page to it, and
	# the id survives a re-read.
	_select(23)
	var jump_ids := _visible_ids()
	print("SETTLEMENT_EDITOR_PAGING phase=jump page=%s/%s selected=%s rows=%s" % [
		_i18n.format_int(_page + 1), _i18n.format_int(_page_count),
		_i18n.format_int(_selected_id), _i18n.format_int(jump_ids.size()),
	])
	_check(
		"paging_jump_selected",
		_page == 2 and _selected_id == 23 and jump_ids.size() == 7 \
			and int(jump_ids[-1]) == 23
	)
	_refresh()
	print("SETTLEMENT_EDITOR_PAGING phase=refresh page=%s/%s selected=%s checked=%s" % [
		_i18n.format_int(_page + 1), _i18n.format_int(_page_count),
		_i18n.format_int(_selected_id),
		"1" if _visible_ids().has(23) else "0",
	])
	_check(
		"paging_refresh_stable",
		_page == 2 and _selected_id == 23 and _visible_ids().has(23)
	)

	# Walk every page once and prove each seeded id is reachable exactly once.
	_select(1)
	var walked: Array = []
	var rows := PackedStringArray()
	while true:
		var ids := _visible_ids()
		rows.append(str(ids.size()))
		for id in ids:
			walked.append(id)
		if not _page_by(1):
			break
	var unique := {}
	for id in walked:
		unique[id] = true
	var sorted_ids := walked.duplicate()
	sorted_ids.sort()
	var expected: Array = []
	for index in range(PAGING_FIXTURE_COUNT):
		expected.append(index + 1)
	print("SETTLEMENT_EDITOR_PAGING phase=walk pages=%s ids=%s unique=%s rows=%s" % [
		_i18n.format_int(_page_count), _i18n.format_int(walked.size()),
		_i18n.format_int(unique.size()), "/".join(rows),
	])
	_check(
		"paging_walk_unique",
		walked.size() == PAGING_FIXTURE_COUNT
			and unique.size() == PAGING_FIXTURE_COUNT
			and sorted_ids == expected
			and "/".join(rows) == "8/8/7"
	)


func _run_smoke() -> void:
	print("SETTLEMENT_EDITOR_SMOKE start seed=%s" % _i18n.format_int(_seed))
	_reset_session(true)
	print("SETTLEMENT_EDITOR_UI locale=%s title=%s state=%s" % [
		_i18n.locale(), _title_label.text, _state_label.text
	])

	var empty: Dictionary = _core.settlements()
	_check(
		"empty",
		bool(empty.get("ok", false))
			and not bool(empty.get("open", true))
			and int(empty.get("count", -1)) == 0
			and str(empty.get("checksum", "")) == "0000000000000000"
	)

	_smoke_player_registry()

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
			_owner_marker(final_record),
			_i18n.format_int(int(final_record.get("membership_revision", 0))),
			_i18n.format_int(int(final_record.get("tax_rate", 0))),
			_members_text(final_record),
		]
	)
	print(
		"SETTLEMENT_EDITOR_UNOWNED id=1 owned=%s owner=%s revision=%s" % [
			"1" if bool(_fresh_record(1).get("owned", false)) else "0",
			_owner_marker(_fresh_record(1)),
			_i18n.format_int(int(_fresh_record(1).get("membership_revision", 0))),
		]
	)

	# --- Release probe on a throwaway owned settlement. The first press only
	# arms the record; the second clears owner + members and bumps revision. ---
	var release_found := _found("Release Keep", 0, -1024, 256)
	_expect_ok("release_probe_found", release_found)
	var release_id := int(release_found.get("settlement", {}).get("id", -1))
	_expect_ok("release_probe_claim", _claim(release_id, 1))
	_expect_ok("release_probe_invite", _invite(release_id, 2, "manager"))
	var release_revision := _revision_of(release_id)
	var release_members := _members_text(_fresh_record(release_id))
	_expect_error("release_arm", _release(release_id, false), "confirm_required")
	_check(
		"release_arm_unchanged",
		_revision_of(release_id) == release_revision
			and _members_text(_fresh_record(release_id)) == release_members
	)
	var released := _release(release_id, true)
	_check("release_confirm", bool(released.get("ok", false)))
	var released_record := _fresh_record(release_id)
	_check(
		"release_clears_owner_and_members",
		not bool(released_record.get("owned", false))
			and released_record.get("owner_id", null) == null
			and _members_text(released_record) == _members_text({})
			and int(released_record.get("membership_revision", 0)) == release_revision + 1
	)
	_expect_error("release_repeat", _release(release_id, true), "settlement.unowned")
	print(
		"SETTLEMENT_EDITOR_RELEASE phase=arm code=confirm_required revision=%s members=%s" % [
			_i18n.format_int(release_revision), release_members
		]
	)
	print(
		"SETTLEMENT_EDITOR_RELEASE phase=confirm owned=%s owner=%s members=%s revision=%s checksum=%s" % [
			"1" if bool(released_record.get("owned", false)) else "0",
			_owner_marker(released_record),
			_members_text(released_record),
			_i18n.format_int(int(released_record.get("membership_revision", 0))),
			_checksum,
		]
	)
	print(
		"SETTLEMENT_EDITOR_RELEASE phase=repeat code=settlement.unowned revision=%s" % (
			_i18n.format_int(int(_fresh_record(release_id).get("membership_revision", 0)))
		)
	)

	# --- Name-based invite path on a throwaway settlement. The main fixture
	# markers and checksum above stay pinned; this probe proves that `P` set
	# the pending id and `_on_invite_pressed` used that id. ---
	var main_checksum := _checksum
	var probe_found := _found("Registry Probe", 0, 1024, 256)
	_expect_ok("registry_probe_found", probe_found)
	var probe_id := int(probe_found.get("settlement", {}).get("id", -1))
	_expect_ok("registry_probe_claim", _claim(probe_id, 1))
	_select(probe_id)
	_target_player_id = 1
	_target_player_name = "Mayor"
	_refresh_target_label()
	var invite_target := _cycle_target()
	_check(
		"target_before_registry_invite",
		invite_target == 2 and _target_player_name == "Builder"
	)
	_on_invite_pressed()
	var probe_record := _fresh_record(probe_id)
	print("SETTLEMENT_EDITOR_INVITE settlement=%s target=2 name=Builder members=%s" % [
		_i18n.format_int(probe_id), _members_text(probe_record)
	])
	_check(
		"invite_uses_selected_name_id",
		_members_text(probe_record) == "1:owner, 2:manager"
			and int(probe_record.get("membership_revision", -1)) == 2
	)

	# --- Client-side paging over a 23-record fixture (8 per page). Runs last so
	# the pinned two-settlement state, release, and registry markers above are
	# unaffected; `main_checksum` is captured before the throwaway probes. ---
	_smoke_paging()

	if _failures > 0:
		print("SETTLEMENT_EDITOR status=failed actions=%s failures=%s checksum=%s" % [
			_i18n.format_int(_actions),
			_i18n.format_int(_failures),
			main_checksum,
		])
		get_tree().quit(1)
		return
	print("SETTLEMENT_EDITOR status=ok actions=%s checksum=%s" % [
		_i18n.format_int(_actions), main_checksum
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

	_title_label = Label.new()
	_title_label.text = _i18n.t("base.ui.settlement_editor.title")
	_title_label.add_theme_font_size_override("font_size", 30)
	root.add_child(_title_label)

	var subtitle := Label.new()
	subtitle.text = _i18n.t("base.ui.settlement_editor.subtitle")
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
	_message_label.text = _i18n.t("base.ui.settlement_editor.ready")
	_message_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	root.add_child(_message_label)

	var help := Label.new()
	help.text = _i18n.t("base.ui.settlement_editor.help") + "\n" + \
		"PageUp/PageDown page · " + \
		"Players: N register · P cycle target · Shift+P set role · " + \
		"Shift+X release (press twice) · Shift+N new session"
	help.add_theme_color_override("font_color", MUTED_COLOR)
	help.add_theme_font_size_override("font_size", 13)
	help.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	root.add_child(help)

	var footer := Label.new()
	footer.text = _i18n.t("base.ui.settlement_editor.footer")
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
	seed_label.text = _i18n.t("base.ui.settlement_editor.seed", {
		"seed": _i18n.format_int(DEFAULT_SEED),
	})
	seed_label.add_theme_color_override("font_color", MUTED_COLOR)
	toolbar.add_child(seed_label)

	toolbar.add_child(_button("%s (F)" % _i18n.t(
		"base.ui.settlement_editor.action.found"
	), _on_found_pressed))
	toolbar.add_child(_button(_i18n.t(
		"base.ui.settlement_editor.button.fixture"
	), _on_fixture_pressed))
	toolbar.add_child(_button(_i18n.t(
		"base.ui.settlement_editor.button.refresh"
	), _on_refresh_pressed))
	toolbar.add_child(_button(_i18n.t(
		"base.ui.settlement_editor.button.new_session"
	), _on_new_session_pressed))

	_state_label = Label.new()
	_state_label.add_theme_color_override("font_color", TEXT_COLOR)
	panel.add_child(_state_label)

	var registry_row := HBoxContainer.new()
	registry_row.add_theme_constant_override("separation", 18)
	panel.add_child(registry_row)
	_roster_label = Label.new()
	_roster_label.add_theme_color_override("font_color", TEXT_COLOR)
	registry_row.add_child(_roster_label)
	_target_label = Label.new()
	_target_label.add_theme_color_override("font_color", MUTED_COLOR)
	registry_row.add_child(_target_label)

	_page_label = Label.new()
	_page_label.add_theme_color_override("font_color", MUTED_COLOR)
	_page_label.visible = false
	panel.add_child(_page_label)

	_list = ItemList.new()
	_list.size_flags_vertical = Control.SIZE_EXPAND_FILL
	_list.custom_minimum_size = Vector2(0, 180)
	_list.item_selected.connect(_on_list_selected)
	panel.add_child(_list)
	return panel


func _build_detail_panel() -> Control:
	var panel := VBoxContainer.new()
	panel.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	panel.add_theme_constant_override("separation", 6)

	var title := Label.new()
	title.text = _i18n.t("base.ui.settlement_editor.selected_record")
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
	_form_title.text = _i18n.t("base.ui.settlement_editor.no_selection")
	_form_title.add_theme_font_size_override("font_size", 20)
	panel.add_child(_form_title)

	var grid := GridContainer.new()
	grid.columns = 2
	grid.add_theme_constant_override("h_separation", 10)
	grid.add_theme_constant_override("v_separation", 6)
	panel.add_child(grid)

	grid.add_child(_label(_i18n.t("base.ui.settlement_editor.label.name")))
	_name_edit = LineEdit.new()
	_name_edit.text = DEFAULT_NAME
	_name_edit.custom_minimum_size = Vector2(240, 0)
	grid.add_child(_name_edit)

	grid.add_child(_label(_i18n.t("base.ui.settlement_editor.label.center_x")))
	_x_spin = _spin(-1000000, 1000000, 0)
	grid.add_child(_x_spin)

	grid.add_child(_label(_i18n.t("base.ui.settlement_editor.label.center_z")))
	_z_spin = _spin(-1000000, 1000000, 0)
	grid.add_child(_z_spin)

	grid.add_child(_label(_i18n.t("base.ui.settlement_editor.label.radius_m")))
	_radius_spin = _spin(MIN_RADIUS_M, MAX_RADIUS_M, DEFAULT_RADIUS_M)
	grid.add_child(_radius_spin)

	grid.add_child(_label(_i18n.t("base.ui.settlement_editor.label.player_id")))
	_player_spin = _spin(1, MAX_PLAYER_ID, 1)
	grid.add_child(_player_spin)

	grid.add_child(_label(_i18n.t("base.ui.settlement_editor.label.role")))
	_role_option = OptionButton.new()
	for role in ROLES:
		_role_option.add_item(role)
	grid.add_child(_role_option)

	grid.add_child(_label(_i18n.t("base.ui.settlement_editor.label.tax_percent")))
	_tax_spin = _spin(0, MAX_TAX_RATE, 0)
	grid.add_child(_tax_spin)

	var actions := HBoxContainer.new()
	actions.add_theme_constant_override("separation", 6)
	panel.add_child(actions)
	actions.add_child(_button("%s (C)" % _i18n.t(
		"base.ui.settlement_editor.action.claim"
	), _on_claim_pressed))
	actions.add_child(_button("%s (R)" % _i18n.t(
		"base.ui.settlement_editor.action.rename"
	), _on_rename_pressed))
	actions.add_child(_button("%s (T)" % _i18n.t(
		"base.ui.settlement_editor.action.transfer"
	), _on_transfer_pressed))

	var actions_two := HBoxContainer.new()
	actions_two.add_theme_constant_override("separation", 6)
	panel.add_child(actions_two)
	actions_two.add_child(_button("%s (I)" % _i18n.t(
		"base.ui.settlement_editor.action.invite"
	), _on_invite_pressed))
	actions_two.add_child(_button("%s (P)" % _i18n.t(
		"base.ui.settlement_editor.action.set_role"
	), _on_set_role_pressed))
	actions_two.add_child(_button("%s (X)" % _i18n.t(
		"base.ui.settlement_editor.action.remove_member"
	), _on_remove_member_pressed))
	actions_two.add_child(_button("%s (M)" % _i18n.t(
		"base.ui.settlement_editor.action.set_tax"
	), _on_set_tax_pressed))

	var actions_three := HBoxContainer.new()
	actions_three.add_theme_constant_override("separation", 6)
	panel.add_child(actions_three)
	actions_three.add_child(_button("Release (Shift+X)", _on_release_pressed))

	var registry_actions := HBoxContainer.new()
	registry_actions.add_theme_constant_override("separation", 6)
	panel.add_child(registry_actions)
	registry_actions.add_child(_button("Register player (N)", _on_register_player_pressed))
	registry_actions.add_child(_button("Next target (P)", _on_cycle_target_pressed))

	var note := Label.new()
	note.text = _i18n.t("base.ui.settlement_editor.note")
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
