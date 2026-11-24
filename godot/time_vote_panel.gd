extends PanelContainer

## Developer time-vote panel (client-only; no networking).
##
## Renders one protocol `TimeVoteStatus` dictionary and composes the exact D-021
## time-vote wire commands. The owning client owns the socket: feed `set_status`,
## `set_permissions`, `set_error`, and send every dictionary received on
## `command_issued`. This scene never connects, sends, or applies a vote itself.
##
## Run:
##   godot --path godot res://time_vote_panel.tscn
##   godot --headless --path godot res://time_vote_panel_smoke.tscn
##
## See docs/05 - Multiplayer.md §Time-vote client panel (developer).
##
## Labels are English by default. A small EN/DE table is included for this
## developer panel only (`set_locale`); the surrounding `mp_client.gd` remains
## raw English. Error lines are keyed by the stable protocol code rather than the
## server message, so the same code always renders the same line.

signal command_issued(command: Dictionary)

const ACTIONS := ["pause", "resume"]

const ERROR_CODES := [
	"vote.open",
	"vote.cooldown",
	"vote.min_online",
	"vote.unknown_player",
	"vote.duplicate",
	"vote.closed",
	"vote.unavailable",
	"vote.action_unsupported",
	"vote.action_required",
	"role.forbidden",
]

const TEXT := {
	"en": {
		"title": "Time vote",
		"state_waiting": "Waiting for server status",
		"state_idle": "No vote yet",
		"state_open": "Vote open",
		"state_passed": "Vote passed",
		"state_failed": "Vote failed",
		"state_cancelled": "Vote cancelled",
		"state_unknown": "Unknown state (%s)",
		"label_state": "State: %s",
		"label_target": "Target: %s",
		"label_initiator": "Started by: %s",
		"label_tally": "Yes %d / No %d / Online %d",
		"label_window_open": "Ends in %d s (tick %d)",
		"label_window_closed": "Window closed (tick %d)",
		"label_window_idle": "No active window",
		"label_cooldown_wait": "Cooldown %d s (until tick %d)",
		"label_cooldown_ready": "Cooldown ready",
		"label_clock": "Clock %d s | Paused: %s",
		"yes": "yes",
		"no": "no",
		"none": "none",
		"call_pause": "Call pause vote",
		"call_resume": "Call resume vote",
		"vote_yes": "Yes",
		"vote_no": "No",
		"cancel": "Cancel vote",
		"error_format": "Error: %s (%s)",
		"err_vote_open": "A time vote is already open.",
		"err_vote_cooldown": "Time votes are cooling down.",
		"err_vote_min_online": "At least two players must be online.",
		"err_vote_unknown_player": "You are not in this vote's roster.",
		"err_vote_duplicate": "You already voted in this vote.",
		"err_vote_closed": "No time vote is open.",
		"err_vote_unavailable": "Time votes are unavailable on this connection.",
		"err_vote_action_unsupported": "This vote target is not supported.",
		"err_vote_action_required": "Choose pause or resume before calling a vote.",
		"err_role_forbidden": "Your role cannot do that.",
		"err_unknown": "Unexpected vote error.",
	},
	"de": {
		"title": "Zeitabstimmung",
		"state_waiting": "Warte auf Serverstatus",
		"state_idle": "Keine Abstimmung",
		"state_open": "Abstimmung offen",
		"state_passed": "Abstimmung angenommen",
		"state_failed": "Abstimmung abgelehnt",
		"state_cancelled": "Abstimmung abgebrochen",
		"state_unknown": "Unbekannter Status (%s)",
		"label_state": "Status: %s",
		"label_target": "Ziel: %s",
		"label_initiator": "Gestartet von: %s",
		"label_tally": "Ja %d / Nein %d / Online %d",
		"label_window_open": "Endet in %d s (Tick %d)",
		"label_window_closed": "Zeitfenster geschlossen (Tick %d)",
		"label_window_idle": "Kein aktives Zeitfenster",
		"label_cooldown_wait": "Abklingzeit %d s (bis Tick %d)",
		"label_cooldown_ready": "Abklingzeit bereit",
		"label_clock": "Uhr %d s | Pausiert: %s",
		"yes": "ja",
		"no": "nein",
		"none": "keine",
		"call_pause": "Pause-Abstimmung starten",
		"call_resume": "Fortsetzen-Abstimmung starten",
		"vote_yes": "Ja",
		"vote_no": "Nein",
		"cancel": "Abstimmung abbrechen",
		"error_format": "Fehler: %s (%s)",
		"err_vote_open": "Eine Zeitabstimmung ist bereits offen.",
		"err_vote_cooldown": "Zeitabstimmungen kühlen ab.",
		"err_vote_min_online": "Mindestens zwei Spieler müssen online sein.",
		"err_vote_unknown_player": "Du bist nicht in der Wählerliste dieser Abstimmung.",
		"err_vote_duplicate": "Du hast in dieser Abstimmung bereits gewählt.",
		"err_vote_closed": "Keine Zeitabstimmung ist offen.",
		"err_vote_unavailable": "Zeitabstimmungen sind auf dieser Verbindung nicht verfügbar.",
		"err_vote_action_unsupported": "Dieses Abstimmungsziel wird nicht unterstützt.",
		"err_vote_action_required": "Wähle vor dem Start Pause oder Fortsetzen.",
		"err_role_forbidden": "Deine Rolle darf das nicht.",
		"err_unknown": "Unerwarteter Abstimmungsfehler.",
	},
}

var locale := "en"

var _can_vote := false
var _is_admin := false
var _status: Dictionary = {}
var _has_status := false
var _error_code := ""
var _error_detail := ""
var _local_voted_vote_id := -1

var _box: VBoxContainer
var _title: Label
var _state_label: Label
var _target_label: Label
var _initiator_label: Label
var _tally_label: Label
var _window_label: Label
var _cooldown_label: Label
var _clock_label: Label
var _error_label: Label
var _call_pause: Button
var _call_resume: Button
var _yes: Button
var _no: Button
var _cancel: Button


func _ready() -> void:
	if _box == null:
		_build_ui()
	_refresh()


## Set the local role flags. Calls and ballots require `can_vote`; cancel
## requires `is_admin`.
func set_permissions(can_vote: bool, is_admin: bool) -> void:
	_can_vote = can_vote
	_is_admin = is_admin
	_refresh()


## Replace the rendered status with one protocol `TimeVoteStatus` dictionary.
## Unknown keys are ignored; a new `vote_id` naturally unlocks a fresh ballot.
func set_status(status: Dictionary) -> void:
	_status = status.duplicate(true)
	_has_status = true
	_refresh()


## Show a stable, code-keyed error line. The server message is only a fallback
## for a code the panel does not know.
func set_error(code: String, message: String) -> void:
	_error_code = code
	_error_detail = message
	_refresh()


func clear_error() -> void:
	_error_code = ""
	_error_detail = ""
	_refresh()


## Switch the developer-panel label table. Unknown locales fall back to `en`.
func set_locale(code: String) -> void:
	locale = code if TEXT.has(code) else "en"
	_refresh()


## Emit a call for `pause` or `resume`. Returns whether a command was emitted.
func press_call(target: String) -> bool:
	var normalized := target.strip_edges().to_lower()
	if normalized not in ACTIONS:
		return false
	if not _call_allowed():
		return false
	command_issued.emit({"type": "time_vote", "action": "call", "time": normalized})
	return true


func press_yes() -> bool:
	if not _ballot_allowed():
		return false
	_local_voted_vote_id = _vote_id()
	_refresh()
	command_issued.emit(_ballot_command("yes"))
	return true


func press_no() -> bool:
	if not _ballot_allowed():
		return false
	_local_voted_vote_id = _vote_id()
	_refresh()
	command_issued.emit(_ballot_command("no"))
	return true


func press_cancel() -> bool:
	if not _cancel_allowed():
		return false
	command_issued.emit({"type": "time_vote", "action": "cancel", "time": null})
	return true


## Rendered fields plus actual button states, for headless tests.
func snapshot() -> Dictionary:
	var render := _canonical_render()
	return {
		"locale": locale,
		"title": _tr("title"),
		"state": _state(),
		"state_label": _state_text(),
		"vote_id": _vote_id(),
		"target": _target(),
		"target_label": _target_display(),
		"initiator": _initiator(),
		"initiator_label": _initiator_display(),
		"yes": _yes_count(),
		"no": _no_count(),
		"online": _online_count(),
		"ticks_to_end": _ticks_to_end(),
		"cooldown_remaining": _cooldown_remaining(),
		"paused": _paused(),
		"clock": _clock(),
		"server_checksum": str(_status.get("checksum", "")),
		"can_vote": _can_vote,
		"is_admin": _is_admin,
		"voted": _voted(),
		"call_pause_enabled": _button_enabled(_call_pause, _call_allowed()),
		"call_resume_enabled": _button_enabled(_call_resume, _call_allowed()),
		"yes_enabled": _button_enabled(_yes, _ballot_allowed()),
		"no_enabled": _button_enabled(_no, _ballot_allowed()),
		"cancel_enabled": _button_enabled(_cancel, _cancel_allowed()),
		"call_pause_label": _tr("call_pause"),
		"call_resume_label": _tr("call_resume"),
		"yes_label": _tr("vote_yes"),
		"no_label": _tr("vote_no"),
		"cancel_label": _tr("cancel"),
		"error_code": _error_code,
		"error_line": _error_text(),
		"render": render,
		"render_checksum": _fnv32(render),
	}


func _build_ui() -> void:
	custom_minimum_size = Vector2(360, 0)

	var margin := MarginContainer.new()
	margin.name = "Margin"
	margin.add_theme_constant_override("margin_left", 12)
	margin.add_theme_constant_override("margin_top", 10)
	margin.add_theme_constant_override("margin_right", 12)
	margin.add_theme_constant_override("margin_bottom", 10)
	add_child(margin)

	_box = VBoxContainer.new()
	_box.name = "Box"
	_box.add_theme_constant_override("separation", 4)
	margin.add_child(_box)

	_title = _add_label(18)
	_state_label = _add_label(0)
	_target_label = _add_label(0)
	_initiator_label = _add_label(0)
	_tally_label = _add_label(0)
	_window_label = _add_label(0)
	_cooldown_label = _add_label(0)
	_clock_label = _add_label(0)

	_error_label = _add_label(0)
	_error_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	_error_label.add_theme_color_override("font_color", Color(0.96, 0.42, 0.38))

	_box.add_child(HSeparator.new())

	var call_row := HBoxContainer.new()
	_box.add_child(call_row)
	_call_pause = _add_button(call_row, func() -> void: press_call("pause"))
	_call_resume = _add_button(call_row, func() -> void: press_call("resume"))

	var ballot_row := HBoxContainer.new()
	_box.add_child(ballot_row)
	_yes = _add_button(ballot_row, func() -> void: press_yes())
	_no = _add_button(ballot_row, func() -> void: press_no())
	_cancel = _add_button(ballot_row, func() -> void: press_cancel())


func _add_label(font_size: int) -> Label:
	var label := Label.new()
	if font_size > 0:
		label.add_theme_font_size_override("font_size", font_size)
	_box.add_child(label)
	return label


func _add_button(parent: Control, callback: Callable) -> Button:
	var button := Button.new()
	button.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	button.pressed.connect(callback)
	parent.add_child(button)
	return button


func _refresh() -> void:
	if _box == null:
		return

	_title.text = _tr("title")
	_state_label.text = _tr("label_state") % _state_text()
	_target_label.text = _tr("label_target") % _target_display()
	_initiator_label.text = _tr("label_initiator") % _initiator_display()
	_tally_label.text = _tr("label_tally") % [_yes_count(), _no_count(), _online_count()]
	_window_label.text = _window_text()
	_cooldown_label.text = _cooldown_text()
	_clock_label.text = _clock_text()
	_error_label.text = _error_text()
	_error_label.visible = _error_code != ""

	_call_pause.text = _tr("call_pause")
	_call_resume.text = _tr("call_resume")
	_yes.text = _tr("vote_yes")
	_no.text = _tr("vote_no")
	_cancel.text = _tr("cancel")

	_call_pause.disabled = not _call_allowed()
	_call_resume.disabled = not _call_allowed()
	_yes.disabled = not _ballot_allowed()
	_no.disabled = not _ballot_allowed()
	_cancel.disabled = not _cancel_allowed()


func _call_allowed() -> bool:
	return _has_status and _can_vote and not _is_open() and _clock() >= _cooldown_until()


func _ballot_allowed() -> bool:
	return _has_status and _can_vote and _is_open() and not _voted()


func _cancel_allowed() -> bool:
	return _has_status and _is_admin and _is_open()


func _voted() -> bool:
	return _has_status and _local_voted_vote_id >= 0 and _local_voted_vote_id == _vote_id()


func _ballot_command(action: String) -> Dictionary:
	return {"type": "time_vote", "action": action, "time": null}


func _state() -> String:
	return str(_status.get("state", "")).to_lower()


func _is_open() -> bool:
	return _state() == "open"


func _state_text() -> String:
	if not _has_status:
		return _tr("state_waiting")
	match _state():
		"idle":
			return _tr("state_idle")
		"open":
			return _tr("state_open")
		"passed":
			return _tr("state_passed")
		"failed":
			return _tr("state_failed")
		"cancelled":
			return _tr("state_cancelled")
		_:
			return _tr("state_unknown") % _state()


func _target() -> String:
	var action = _status.get("action", null)
	return "" if action == null else str(action)


func _target_display() -> String:
	var target := _target()
	return target if target != "" else _tr("none")


func _initiator() -> String:
	return str(_status.get("initiator", ""))


func _initiator_display() -> String:
	var initiator := _initiator()
	return initiator if initiator != "" else _tr("none")


func _window_text() -> String:
	if not _has_status:
		return _tr("label_window_idle")
	if _is_open():
		return _tr("label_window_open") % [_ticks_to_end(), _int_field("ends_tick")]
	if _state() in ["passed", "failed", "cancelled"]:
		return _tr("label_window_closed") % _int_field("ends_tick")
	return _tr("label_window_idle")


func _cooldown_text() -> String:
	var remaining := _cooldown_remaining()
	if remaining > 0:
		return _tr("label_cooldown_wait") % [remaining, _cooldown_until()]
	return _tr("label_cooldown_ready")


func _clock_text() -> String:
	return _tr("label_clock") % [_clock(), _yes_no(_paused())]


func _yes_no(value: bool) -> String:
	return _tr("yes") if value else _tr("no")


func _ticks_to_end() -> int:
	if not _is_open():
		return 0
	return maxi(_int_field("ends_tick") - _clock(), 0)


func _cooldown_remaining() -> int:
	if not _has_status:
		return 0
	return maxi(_cooldown_until() - _clock(), 0)


func _cooldown_until() -> int:
	return _int_field("cooldown_until")


func _clock() -> int:
	return _int_field("clock")


func _vote_id() -> int:
	return _int_field("vote_id")


func _yes_count() -> int:
	return _int_field("yes")


func _no_count() -> int:
	return _int_field("no")


func _online_count() -> int:
	return _int_field("online")


func _paused() -> bool:
	var value = _status.get("paused", false)
	return bool(value) if value != null else false


func _int_field(key: String) -> int:
	var value = _status.get(key, 0)
	return 0 if value == null else int(value)


func _error_text() -> String:
	if _error_code == "":
		return ""
	var table: Dictionary = TEXT.get(locale, TEXT["en"])
	var key := "err_" + _error_code.replace(".", "_")
	var body: String
	if table.has(key):
		body = str(table[key])
	elif _error_detail != "":
		body = _error_detail
	else:
		body = str(table["err_unknown"])
	return str(table["error_format"]) % [body, _error_code]


func _tr(key: String) -> String:
	var table: Dictionary = TEXT.get(locale, TEXT["en"])
	if table.has(key):
		return str(table[key])
	return str(TEXT["en"].get(key, key))


func _button_enabled(button: Button, allowed: bool) -> bool:
	if button == null:
		return allowed
	return not button.disabled


func _canonical_render() -> String:
	var buttons := "%d%d%d%d%d" % [
		int(_button_enabled(_call_pause, _call_allowed())),
		int(_button_enabled(_call_resume, _call_allowed())),
		int(_button_enabled(_yes, _ballot_allowed())),
		int(_button_enabled(_no, _ballot_allowed())),
		int(_button_enabled(_cancel, _cancel_allowed())),
	]
	return "\n".join([
		"locale=" + locale,
		"title=" + _tr("title"),
		"state=" + _state(),
		"state_label=" + _state_text(),
		"target=" + _target(),
		"target_label=" + _target_display(),
		"initiator=" + _initiator(),
		"tally=%d/%d/%d" % [_yes_count(), _no_count(), _online_count()],
		"window=" + _window_text(),
		"cooldown=" + _cooldown_text(),
		"clock=" + _clock_text(),
		"error=" + _error_text(),
		"buttons=" + buttons,
	])


func _fnv32(text: String) -> String:
	var hash := 2166136261
	for byte in text.to_utf8_buffer():
		hash = (hash ^ int(byte)) & 0xffffffff
		hash = (hash * 16777619) & 0xffffffff
	return "%08x" % hash
