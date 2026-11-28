extends PanelContainer

## Developer chat panel (client-only; no networking).
##
## Consumes the protocol `chat_sent` / `chat_log` dictionaries, validates drafts
## with the server's local rules, and composes the exact `chat` / `chat_since`
## wire commands. The owning client owns the socket: feed `set_identity`,
## `set_log`, `apply_chat_sent`, `apply_chat_log`, `apply_chat_catchup`,
## `set_status`, and `set_error`, then send whatever the panel emits on
## `command_issued`.
##
## This scene never connects, polls on a timer, or pushes anything by itself.
##
## Display strings resolve through `WorldI18n` (`base.ui.chat.*`); the wire
## commands, `_error_code`, and `snapshot()` keys stay locale-independent.
## Player names and message bodies are never translated.
##
## Run:
##   godot --path godot res://chat_panel.tscn
##   godot --headless --path godot res://chat_panel.tscn -- --smoke
##
## See docs/05 - Multiplayer.md §Chat panel (developer).

signal command_issued(command: Dictionary)

const WorldI18n = preload("res://world_i18n.gd")

## Mirrors `crates/server/src/protocol.rs` (`MAX_CHAT_CHARS`).
const MAX_CHAT_CHARS := 280
## Mirrors `CHAT_LOG_CAPACITY`; also the local transcript display cap.
const MAX_LINES := 64
## Unread counts above this render as "99+" in the header badge.
const UNREAD_DISPLAY_MAX := 99
const DEFAULT_LOCALE := "en"
const SUPPORTED_LOCALES := ["en", "de"]

## `Control.accessibility_live` values (Godot 4.7 exposes the property but not
## named enum constants). The transcript is polite so pushed lines are
## announced; the status/error line is assertive so a rejection is immediate.
const A11Y_LIVE_OFF := 0
const A11Y_LIVE_POLITE := 1
const A11Y_LIVE_ASSERTIVE := 2

## Deterministic keyboard focus order. The unread badge is display-only and
## stays out of the tab cycle; the status line is the last stop.
const FOCUS_ORDER := ["draft", "send", "transcript", "status"]

## Deterministic gamepad actions. `ui_up` / `ui_down` reuse the keyboard focus
## cycle, `ui_accept` activates the focused control, `ui_cancel` runs the Escape
## contract, and page actions move the transcript view when it owns focus.
const GAMEPAD_ACTIONS := [
	"ui_up",
	"ui_down",
	"ui_accept",
	"ui_cancel",
	"ui_page_up",
	"ui_page_down",
]

## Logical lines moved by one transcript page. Paging is a view offset: it
## never edits entries, `last_seq`, the draft, or the unread badge.
const TRANSCRIPT_PAGE_LINES := 8

## Compact glyph names for joypad buttons. Footer text reads these from the
## active InputMap bindings; this table only names the buttons.
const JOY_BUTTON_GLYPHS := {
	JOY_BUTTON_A: "A",
	JOY_BUTTON_B: "B",
	JOY_BUTTON_X: "X",
	JOY_BUTTON_Y: "Y",
	JOY_BUTTON_LEFT_SHOULDER: "LB",
	JOY_BUTTON_RIGHT_SHOULDER: "RB",
	JOY_BUTTON_BACK: "Back",
	JOY_BUTTON_START: "Start",
	JOY_BUTTON_DPAD_UP: "D-pad Up",
	JOY_BUTTON_DPAD_DOWN: "D-pad Down",
	JOY_BUTTON_DPAD_LEFT: "D-pad Left",
	JOY_BUTTON_DPAD_RIGHT: "D-pad Right",
}

const JOY_AXIS_GLYPHS := {
	JOY_AXIS_LEFT_Y: "Left Stick",
	JOY_AXIS_TRIGGER_LEFT: "LT",
	JOY_AXIS_TRIGGER_RIGHT: "RT",
}

## Local fallbacks for actions the stock InputMap leaves keyboard-only
## (`ui_accept` / `ui_cancel` / page actions). Live routing checks InputMap
## first, so a project rebinding still wins; these keep the panel usable with
## the default map and are shown in the footer, never hidden.
const JOY_BUTTON_ACTIONS := {
	JOY_BUTTON_A: "ui_accept",
	JOY_BUTTON_B: "ui_cancel",
	JOY_BUTTON_LEFT_SHOULDER: "ui_page_up",
	JOY_BUTTON_RIGHT_SHOULDER: "ui_page_down",
	JOY_BUTTON_DPAD_UP: "ui_up",
	JOY_BUTTON_DPAD_DOWN: "ui_down",
}

const JOY_AXIS_ACTIONS := {
	JOY_AXIS_TRIGGER_LEFT: "ui_page_up",
	JOY_AXIS_TRIGGER_RIGHT: "ui_page_down",
}

const ERROR_CODES := [
	"chat.empty",
	"chat.too_long",
	"chat.invalid",
	"chat.identity",
]

const ERROR_KEYS := {
	"chat.empty": "base.ui.chat.error.empty",
	"chat.too_long": "base.ui.chat.error.too_long",
	"chat.invalid": "base.ui.chat.error.invalid",
	"chat.identity": "base.ui.chat.error.identity",
}

var player_id := 0
var player_name := ""

## Highest `seq` consumed so far. `poll_command` asks for everything after it.
var last_seq := 0

var _i18n = WorldI18n.new(DEFAULT_LOCALE)
var _entries: Dictionary = {}
var _status_text := ""
var _error_code := ""
var _error_detail := ""
var _smoke_mode := false
## Owner-supplied unread count. Negative values clamp to 0; this is display
## state only and never moves the transcript or `last_seq`.
var _unread := 0
## Logical focus target. Synthetic mode moves this without touching the engine
## focus owner, so the focus contract is deterministic under `--headless`.
var _focus_id := "draft"
## Whether the last handled navigation came from a gamepad action. Keyboard,
## mouse, and API focus moves clear it; a handled gamepad action sets it.
var _gamepad_active := false
## Last accepted gamepad action name. Diagnostic only.
var _last_action := ""
## Transcript page offset: 0 is the newest page, 1 is one page older.
var _transcript_page := 0
var _trigger_left_held := false
var _trigger_right_held := false

## Platform virtual keyboard intent for the draft. Only gamepad modality asks
## for it; keyboard and mouse focus never does. Request/clear intent is counted
## on every platform so the headless smoke stays deterministic, while the
## `DisplayServer` call is made only when the feature exists and the server is
## not headless.
var _vk_requested := false
var _vk_requests := 0
var _vk_clears := 0

var _box: VBoxContainer
var _title: Label
var _badge: Label
var _transcript: RichTextLabel
var _status_label: Label
var _draft_edit: LineEdit
var _send_button: Button
var _counter_label: Label
var _help_label: Label
var _gamepad_label: Label
var _overlay: Control
var _focus_ring: Panel


func _ready() -> void:
	_parse_args()
	if _box == null:
		_build_ui()
	if _smoke_mode:
		_run_smoke()
		return
	if DisplayServer.get_name() == "headless":
		print("Usage: godot --headless --path godot res://chat_panel.tscn -- --smoke")
		get_tree().quit(2)
		return
	_refresh()
	_set_focus("draft", true)
	call_deferred("_update_focus_ring")


## Set the local identity shown in the default status line. The server owns the
## authoritative name and id echoed in chat entries.
func set_identity(id: int, display_name: String) -> void:
	player_id = id
	player_name = display_name
	_refresh()


## Switch the display locale. `en` / `de` are supported (regional codes such as
## `de_DE` map to their language); anything else falls back to English. Only
## visible labels and status/error lines refresh: entries, `last_seq`, and the
## transcript are untouched. Returns the resolved locale.
func set_locale(locale: String) -> String:
	var language := locale.strip_edges().to_lower().replace("-", "_").get_slice("_", 0)
	if not SUPPORTED_LOCALES.has(language):
		language = DEFAULT_LOCALE
	_i18n.set_locale(language)
	_refresh_labels()
	return _i18n.locale()


## Replace the transcript with a `chat_log`-style entry list. Entries are
## deduplicated by `seq`, sorted ascending, and capped at the newest 64 lines.
## `last_seq` resets to the highest sequence in the list.
func set_log(entries: Array) -> void:
	_entries.clear()
	last_seq = 0
	_merge_catchup(entries, 0)
	_refresh()


## Merge one entry into the transcript. Returns `true` when the line was added;
## a duplicate `seq`, an old line already dropped by the cap, or a malformed
## entry returns `false` without changing the transcript.
func append_entry(entry: Dictionary) -> bool:
	var normalized := _normalized_entry(entry)
	if normalized.is_empty():
		return false
	if not _merge_entry(normalized):
		return false
	_refresh()
	return true


## Show the latest server or connection status (for example "Connected").
## An error line takes precedence until `clear_error`.
func set_status(text: String) -> void:
	_status_text = text
	_refresh()


## Set the header unread badge. Negative values clamp to 0 (hidden), 1-99 show
## the number, and anything above shows "99+". Only the badge changes: the
## transcript, `last_seq`, status, error, and existing snapshot fields are
## untouched. The owning client owns resets; this panel never clears itself.
func set_unread(count: int) -> void:
	_unread = maxi(count, 0)
	_refresh_unread()


## Compact badge text: "" while hidden, `N` for 1-99, and "99+" above.
func badge_text() -> String:
	if _badge != null:
		return _badge.text
	return _unread_text(_unread)


## Whether the unread badge is visible. The mounted label mirrors the count.
func badge_visible() -> bool:
	if _badge != null:
		return _badge.visible
	return _unread > 0


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


## Validate a draft with the server's rules. Returns
## `{ok: true, command: {type: "chat", text}}` or
## `{ok: false, code, message}`. Never emits and never mutates the UI.
func draft(text: String) -> Dictionary:
	var trimmed := text.strip_edges()
	if trimmed.is_empty():
		return _failure("chat.empty")
	if trimmed.length() > MAX_CHAT_CHARS:
		return _failure("chat.too_long")
	if _has_control_character(trimmed):
		return _failure("chat.invalid")
	return {"ok": true, "command": {"type": "chat", "text": trimmed}}


## Build the poll command for everything after `last_seq`.
func poll_command() -> Dictionary:
	return {"type": "chat_since", "after": last_seq}


## Apply one `chat_sent` payload. Returns whether a new line was added. A
## malformed payload sets `chat.invalid` and leaves the transcript unchanged.
func apply_chat_sent(payload: Dictionary) -> bool:
	var normalized := _normalized_entry(payload.get("entry"))
	if normalized.is_empty():
		set_error("chat.invalid", _i18n.t("base.ui.chat.error.malformed_sent"))
		return false
	var added := _merge_entry(normalized)
	_refresh()
	return added


## Apply one `chat_log` payload and return how many new lines were added.
## Uses the same ascending catch-up merge as `apply_chat_catchup` with the
## panel's own `last_seq` as the floor: malformed entries, stale sequences, and
## duplicates are dropped, and `last_seq` never rewinds.
func apply_chat_log(payload: Dictionary) -> int:
	var entries = payload.get("entries")
	if not (entries is Array):
		set_error("chat.invalid", _i18n.t("base.ui.chat.error.malformed_log"))
		return 0
	var merged := _merge_catchup(entries, last_seq)
	if int(merged["skipped"]) > 0 and int(merged["added"]) == 0:
		set_error("chat.invalid", _i18n.t("base.ui.chat.error.malformed_entries"))
	else:
		_refresh()
	return int(merged["added"])


## Merge a `chat_log`-style batch after a reconnect. `last_seq` is the caller's
## resume cursor; the panel never rewinds past its own `last_seq`.
##
## Entries are normalized and applied in ascending `seq` order. Malformed
## entries are counted in `skipped`; sequences at or below the floor and repeat
## sequences are counted in `duplicate`. The transcript keeps its existing
## 64-line cap. The unread badge becomes the number of newly added lines from
## other players (clamped at zero); the draft and error line are untouched.
##
## Returns `{ok, added, skipped, last_seq, duplicate}`; `ok` is false only when
## `entries` is not an array.
func apply_chat_catchup(entries: Array, last_seq: int) -> Dictionary:
	var merged := _merge_catchup(entries, last_seq)
	if not bool(merged["ok"]):
		return {
			"ok": false,
			"added": 0,
			"skipped": 0,
			"duplicate": 0,
			"last_seq": int(merged["last_seq"]),
		}
	set_unread(int(merged["unread"]))
	_refresh()
	return {
		"ok": true,
		"added": int(merged["added"]),
		"skipped": int(merged["skipped"]),
		"duplicate": int(merged["duplicate"]),
		"last_seq": int(merged["last_seq"]),
	}


## Validate and emit one `chat` command. Returns the `draft` result. A rejected
## draft sets the matching error line, returns focus to the control that asked
## (`"send"` or the draft), and emits nothing. A successful send clears the
## draft and returns focus to it.
func submit(text: String, origin: String = "draft") -> Dictionary:
	var had_focus := _smoke_mode or _focus_id != ""
	var result := draft(text)
	if not bool(result.get("ok", false)):
		set_error(str(result.get("code", "chat.invalid")), str(result.get("message", "")))
		if had_focus:
			_set_focus("send" if origin == "send" else "draft", true)
		return result
	clear_error()
	if _draft_edit != null:
		_draft_edit.text = ""
		_refresh_counter()
	# A sent message ends gamepad editing: the platform keyboard must not stay
	# over the transcript while the draft focus returns to the field.
	_sync_virtual_keyboard(false)
	if had_focus:
		_set_focus("draft", true)
	var command: Dictionary = result["command"]
	command_issued.emit(command.duplicate(true))
	return result


## Transcript lines in ascending `seq` order.
func transcript_lines() -> Array:
	var sequences := _entries.keys()
	sequences.sort()
	var lines: Array = []
	for seq in sequences:
		var entry: Dictionary = _entries[seq]
		lines.append("#%d %s: %s" % [int(entry["seq"]), str(entry["player"]), str(entry["text"])])
	return lines


## Rendered fields plus a deterministic render checksum, for headless tests.
func snapshot() -> Dictionary:
	var lines := transcript_lines()
	var render := _canonical_render(lines)
	return {
		"player_id": player_id,
		"player": player_name,
		"last_seq": last_seq,
		"unread": _unread,
		"line_count": lines.size(),
		"lines": lines,
		"transcript": "\n".join(lines),
		"status": _status_text,
		"status_line": _status_line(),
		"error_code": _error_code,
		"error_line": _error_line(),
		"render": render,
		"render_checksum": _fnv32(render),
	}


## Screen-reader semantics for the deterministic smoke. Names and descriptions
## resolve through `WorldI18n`; live modes are reported as stable strings and
## the semantic role is derived from the control's job in this panel.
func accessibility_snapshot() -> Dictionary:
	return {
		"locale": _i18n.locale(),
		"title": _a11y_entry(_title, "heading"),
		"transcript": _a11y_entry(_transcript, "log"),
		"draft": _a11y_entry(_draft_edit, "text_field"),
		"send": _a11y_entry(_send_button, "button"),
		"counter": _a11y_entry(_counter_label, "label"),
		"badge": _a11y_entry(_badge, "status"),
		"help": _a11y_entry(_help_label, "label"),
		"gamepad": _a11y_entry(_gamepad_label, "label"),
		"status": _a11y_entry(_status_label, "status"),
		"tab_order": FOCUS_ORDER.duplicate(),
	}


## Keyboard focus contract, independent of the engine focus owner. Synthetic
## mode moves `focus` logically, so the smoke needs no display or focus calls.
func focus_snapshot() -> Dictionary:
	var draft_text := _draft_edit.text if _draft_edit != null else ""
	return {
		"focus": _focus_id,
		"gamepad_focus": _gamepad_active and _focus_id != "",
		"last_action": _last_action,
		"transcript_page": _transcript_page,
		"tab_order": FOCUS_ORDER.duplicate(),
		"draft": draft_text,
		"can_send": bool(draft(draft_text).get("ok", false)),
		"unread": _unread,
		"vk_requested": _vk_requested,
		"vk_requests": _vk_requests,
		"vk_clears": _vk_clears,
	}


## Move focus to a named target, as a click on that control would. Returns
## false for an unknown target. Synthetic mode only updates the logical focus.
func focus_target(target: String) -> bool:
	if not FOCUS_ORDER.has(target):
		return false
	_set_focus(target, true)
	return true


## Advance one step through FOCUS_ORDER. `backwards` is Shift+Tab. When focus
## was released, forward starts at the draft and backwards at the last stop.
func focus_step(backwards: bool = false) -> String:
	var index := FOCUS_ORDER.find(_focus_id)
	if index < 0:
		index = FOCUS_ORDER.size() - 1 if backwards else 0
	else:
		index = posmod(index + (-1 if backwards else 1), FOCUS_ORDER.size())
	_set_focus(FOCUS_ORDER[index], true)
	return _focus_id


## Apply one keyboard command to the focus target. Used by the field handlers
## and by the synthetic smoke; returns whether the key was consumed.
func focus_key(keycode: int, shift: bool = false) -> bool:
	_gamepad_active = false
	if keycode == KEY_TAB:
		focus_step(shift)
		return true
	if keycode == KEY_ESCAPE:
		_handle_escape()
		return true
	if keycode == KEY_ENTER or keycode == KEY_KP_ENTER:
		return _submit_from_draft()
	return false


## Apply one named gamepad action and return the same shape as
## `focus_snapshot`. Unknown actions are ignored. This is the synthetic entry
## point used by the headless smoke; live joypad events call the same dispatch.
func gamepad_action(action: String) -> Dictionary:
	_dispatch_gamepad(action)
	return focus_snapshot()


## Footer text built from the current InputMap joypad glyphs. An action with no
## joypad binding falls back to its action name (plus the panel's local
## fallback button), so every reachable control stays visible.
func gamepad_footer() -> String:
	if _gamepad_label != null:
		return _gamepad_label.text
	return _gamepad_footer_text()


## Release focus to the surrounding UI. The draft is untouched; a gamepad-owned
## platform keyboard is cleared because the field no longer has focus.
func focus_release() -> void:
	if not _smoke_mode:
		var control := _focus_control(_focus_id)
		if control != null and control.is_inside_tree() and control.has_focus():
			control.release_focus()
	_focus_id = ""
	_gamepad_active = false
	_sync_virtual_keyboard(false)
	_update_focus_ring()


## One dispatch path for synthetic smoke actions and live joypad events.
## Returns whether the action applied; only accepted actions update
## `last_action` and the gamepad modality flag.
func _dispatch_gamepad(action: String) -> bool:
	if not GAMEPAD_ACTIONS.has(action):
		return false
	var handled := false
	match action:
		"ui_up":
			handled = _gamepad_step(true)
		"ui_down":
			handled = _gamepad_step(false)
		"ui_accept":
			handled = _gamepad_accept()
		"ui_cancel":
			handled = _handle_escape()
		"ui_page_up":
			handled = _page_transcript(true)
		"ui_page_down":
			handled = _page_transcript(false)
	if not handled:
		return false
	_gamepad_active = true
	_last_action = action
	_update_focus_ring()
	return true


func _gamepad_step(backwards: bool) -> bool:
	focus_step(backwards)
	_sync_virtual_keyboard(_focus_id == "draft")
	return true


## `ui_accept` activates the focused control exactly once: the draft submits
## through the same guard as Enter, and the send button uses its pressed path.
## A rejected draft keeps the field open (and the keyboard up); a sent message
## clears it.
func _gamepad_accept() -> bool:
	match _focus_id:
		"draft":
			var result := _submit_from_draft_result()
			if result.is_empty():
				return false
			if not bool(result.get("ok", false)):
				_sync_virtual_keyboard(true)
			return true
		"send":
			_on_send_pressed()
			return true
	return false


## Page the transcript only while it owns focus. Direction: older pages up,
## newer pages down; 0 is the newest page and the offset clamps at both ends.
func _page_transcript(older: bool) -> bool:
	if _focus_id != "transcript" or _entries.is_empty():
		return false
	var max_page := _transcript_max_page()
	_transcript_page = clampi(
		_transcript_page + (1 if older else -1), 0, max_page
	)
	_apply_transcript_page()
	return true


func _transcript_max_page() -> int:
	if _entries.size() <= TRANSCRIPT_PAGE_LINES:
		return 0
	return int(ceil(float(_entries.size()) / float(TRANSCRIPT_PAGE_LINES))) - 1


func _apply_transcript_page() -> void:
	if _transcript == null:
		return
	if _transcript_page <= 0:
		_transcript.scroll_following = true
		return
	_transcript.scroll_following = false
	if not _transcript.is_inside_tree():
		return
	var first_line := maxi(
		0, _entries.size() - (_transcript_page + 1) * TRANSCRIPT_PAGE_LINES
	)
	_transcript.scroll_to_line(first_line)


func _gamepad_footer_text() -> String:
	var parts: Array = []
	for action in GAMEPAD_ACTIONS:
		parts.append("%s=%s" % [action, _action_glyph(action)])
	return "Gamepad: " + "  ".join(parts)


## Prefer the first joypad glyph bound in InputMap; otherwise show the action
## name with the panel's local fallback binding in parentheses.
func _action_glyph(action: String) -> String:
	for event in InputMap.action_get_events(action):
		if event is InputEventJoypadButton:
			var button := (event as InputEventJoypadButton).button_index
			return str(JOY_BUTTON_GLYPHS.get(button, "Button %d" % button))
		if event is InputEventJoypadMotion:
			var axis := (event as InputEventJoypadMotion).axis
			if JOY_AXIS_GLYPHS.has(axis):
				return str(JOY_AXIS_GLYPHS[axis])
	return _local_fallback_label(action)


func _local_fallback_label(action: String) -> String:
	var glyphs: Array = []
	for button in JOY_BUTTON_ACTIONS:
		if str(JOY_BUTTON_ACTIONS[button]) == action:
			glyphs.append(str(JOY_BUTTON_GLYPHS.get(button, "Button %d" % int(button))))
	for axis in JOY_AXIS_ACTIONS:
		if str(JOY_AXIS_ACTIONS[axis]) == action:
			glyphs.append(str(JOY_AXIS_GLYPHS.get(axis, "Axis %d" % int(axis))))
	if glyphs.is_empty():
		return action
	return "%s (%s)" % [action, "/".join(glyphs)]


func _a11y_entry(control: Control, role: String) -> Dictionary:
	if control == null:
		return {
			"name": "",
			"description": "",
			"role": role,
			"class": "",
			"live": "off",
			"visible": false,
			"focus_mode": "none",
			"reachable": false,
		}
	var live := "off"
	if control.accessibility_live == A11Y_LIVE_POLITE:
		live = "polite"
	elif control.accessibility_live == A11Y_LIVE_ASSERTIVE:
		live = "assertive"
	var focus_mode := "none"
	if control.focus_mode == Control.FOCUS_ALL:
		focus_mode = "all"
	elif control.focus_mode == Control.FOCUS_CLICK:
		focus_mode = "click"
	return {
		"name": control.accessibility_name,
		"description": control.accessibility_description,
		"role": role,
		"class": control.get_class(),
		"live": live,
		"visible": control.visible,
		"focus_mode": focus_mode,
		"reachable": (
			control.is_inside_tree()
			and control.visible
			and control.focus_mode == Control.FOCUS_ALL
		),
	}


## Whether every localized name/description resolved (no `!base.ui.*!` fallback
## leaked from `WorldI18n`). Shared by the EN and DE smoke assertions.
func _a11y_no_leak(a11y: Dictionary) -> bool:
	for part in [
		"title", "transcript", "draft", "send", "counter", "badge", "help", "gamepad", "status",
	]:
		var entry: Dictionary = a11y[part]
		for field in ["name", "description"]:
			if str(entry[field]).contains("!base.ui."):
				return false
	return true


func _focus_control(target: String) -> Control:
	match target:
		"draft":
			return _draft_edit
		"send":
			return _send_button
		"transcript":
			return _transcript
		"status":
			return _status_label
	return null


func _set_focus(target: String, grab: bool) -> void:
	if not FOCUS_ORDER.has(target):
		return
	# Leaving the draft (keyboard, mouse, API, or gamepad) ends gamepad editing.
	if target != "draft":
		_sync_virtual_keyboard(false)
	_focus_id = target
	_gamepad_active = false
	_update_focus_ring()
	if not grab or _smoke_mode:
		return
	var control := _focus_control(target)
	if control != null and control.is_inside_tree() and control.is_visible_in_tree():
		control.grab_focus()


func _on_focus_entered(target: String) -> void:
	if _focus_id == target:
		return
	_focus_id = target
	_gamepad_active = false
	_update_focus_ring()


func _on_focus_exited(_target: String) -> void:
	if _smoke_mode:
		return
	call_deferred("_sync_focus_from_viewport")


## Keep the logical focus honest when a modal, popup, or surrounding UI takes
## the engine focus away from the panel.
func _sync_focus_from_viewport() -> void:
	if _smoke_mode or not is_inside_tree():
		return
	var owner := get_viewport().gui_get_focus_owner()
	var next := ""
	for target in FOCUS_ORDER:
		if _focus_control(target) == owner:
			next = target
			break
	if next == _focus_id:
		return
	_focus_id = next
	if next != "draft":
		_sync_virtual_keyboard(false)
	_update_focus_ring()


func _update_focus_ring() -> void:
	if _focus_ring == null or _overlay == null or not is_inside_tree():
		return
	var target := _focus_control(_focus_id)
	if target == null or not target.is_visible_in_tree():
		_focus_ring.visible = false
		return
	var rect := target.get_global_rect().grow(1.0)
	_focus_ring.position = rect.position - _overlay.get_global_rect().position
	_focus_ring.size = rect.size
	_focus_ring.visible = true


## The platform keyboard is opt-in: headless and platforms without the feature
## still count intent (for the deterministic smoke) but never call the server.
func _virtual_keyboard_supported() -> bool:
	if DisplayServer.get_name() == "headless":
		return false
	return DisplayServer.has_feature(DisplayServer.FEATURE_VIRTUAL_KEYBOARD)


## Request or clear the platform keyboard for the draft. Every call counts
## intent exactly once per transition; a repeat request or a clear that was
## never requested is a no-op, so the counts are deterministic everywhere.
func _sync_virtual_keyboard(show_keyboard: bool) -> void:
	if show_keyboard:
		if _vk_requested:
			return
		_vk_requested = true
		_vk_requests += 1
		if _virtual_keyboard_supported():
			DisplayServer.virtual_keyboard_show(
				_draft_edit.text if _draft_edit != null else ""
			)
		return
	if not _vk_requested:
		return
	_vk_requested = false
	_vk_clears += 1
	if _virtual_keyboard_supported():
		DisplayServer.virtual_keyboard_hide()


## Hiding the panel ends gamepad editing; the focus ring is kept in sync too.
func _on_visibility_changed() -> void:
	_update_focus_ring()
	if not visible:
		_sync_virtual_keyboard(false)


func _parse_args() -> void:
	for arg in OS.get_cmdline_user_args():
		if arg == "--smoke":
			_smoke_mode = true


func _build_ui() -> void:
	custom_minimum_size = Vector2(520, 360)

	var margin := MarginContainer.new()
	margin.name = "Margin"
	margin.add_theme_constant_override("margin_left", 12)
	margin.add_theme_constant_override("margin_top", 10)
	margin.add_theme_constant_override("margin_right", 12)
	margin.add_theme_constant_override("margin_bottom", 10)
	add_child(margin)

	_box = VBoxContainer.new()
	_box.name = "Box"
	_box.add_theme_constant_override("separation", 6)
	margin.add_child(_box)

	_title = Label.new()
	_title.text = ""
	_title.add_theme_font_size_override("font_size", 16)
	_box.add_child(_title)

	_transcript = RichTextLabel.new()
	_transcript.name = "Transcript"
	_transcript.bbcode_enabled = false
	_transcript.scroll_following = true
	_transcript.selection_enabled = true
	_transcript.scroll_active = true
	_transcript.size_flags_vertical = Control.SIZE_EXPAND_FILL
	_transcript.custom_minimum_size = Vector2(0, 240)
	_transcript.add_theme_font_size_override("normal_font_size", 13)
	_box.add_child(_transcript)

	_status_label = Label.new()
	_status_label.name = "Status"
	_status_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	_status_label.mouse_filter = Control.MOUSE_FILTER_STOP
	_box.add_child(_status_label)

	var draft_row := HBoxContainer.new()
	draft_row.name = "DraftRow"
	draft_row.add_theme_constant_override("separation", 6)
	_box.add_child(draft_row)

	_draft_edit = LineEdit.new()
	_draft_edit.name = "Draft"
	_draft_edit.placeholder_text = ""
	_draft_edit.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	_draft_edit.text_submitted.connect(_on_draft_submitted)
	_draft_edit.text_changed.connect(_on_draft_changed)
	draft_row.add_child(_draft_edit)

	_counter_label = Label.new()
	_counter_label.name = "CharCounter"
	_counter_label.add_theme_font_size_override("font_size", 11)
	_counter_label.add_theme_color_override("font_color", Color(0.62, 0.67, 0.74))
	draft_row.add_child(_counter_label)

	_send_button = Button.new()
	_send_button.name = "Send"
	_send_button.text = ""
	_send_button.pressed.connect(_on_send_pressed)
	draft_row.add_child(_send_button)

	_help_label = Label.new()
	_help_label.name = "KeyHelp"
	_help_label.text = ""
	_help_label.add_theme_font_size_override("font_size", 11)
	_help_label.add_theme_color_override("font_color", Color(0.62, 0.67, 0.74))
	_box.add_child(_help_label)

	# Gamepad footer is generated from the live InputMap, so a rebinding is
	# reflected here instead of hidden in code.
	_gamepad_label = Label.new()
	_gamepad_label.name = "GamepadHelp"
	_gamepad_label.text = ""
	_gamepad_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	_gamepad_label.add_theme_font_size_override("font_size", 11)
	_gamepad_label.add_theme_color_override("font_color", Color(0.62, 0.67, 0.74))
	_box.add_child(_gamepad_label)

	# Anchored overlay outside the VBox flow so the badge sits at the top-right
	# of the header without moving the title or the transcript.
	_overlay = Control.new()
	_overlay.name = "HeaderOverlay"
	_overlay.mouse_filter = Control.MOUSE_FILTER_IGNORE
	add_child(_overlay)

	_badge = Label.new()
	_badge.name = "UnreadBadge"
	_badge.visible = false
	_badge.mouse_filter = Control.MOUSE_FILTER_IGNORE
	_badge.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
	_badge.vertical_alignment = VERTICAL_ALIGNMENT_CENTER
	_badge.anchor_left = 1.0
	_badge.anchor_right = 1.0
	_badge.offset_left = -64.0
	_badge.offset_top = 10.0
	_badge.offset_right = -12.0
	_badge.offset_bottom = 32.0
	_badge.add_theme_font_size_override("font_size", 13)
	_badge.add_theme_color_override("font_color", Color(1.0, 0.86, 0.45))
	var pill := StyleBoxFlat.new()
	pill.bg_color = Color(0.20, 0.14, 0.06, 0.92)
	pill.set_corner_radius_all(6)
	pill.content_margin_left = 6.0
	pill.content_margin_right = 6.0
	_badge.add_theme_stylebox_override("normal", pill)
	_overlay.add_child(_badge)

	# One ring for every focus stop, including the status line (Labels do not
	# draw a themed focus box). Mouse picking stays with the controls below it.
	_focus_ring = Panel.new()
	_focus_ring.name = "FocusRing"
	_focus_ring.visible = false
	_focus_ring.mouse_filter = Control.MOUSE_FILTER_IGNORE
	_focus_ring.z_index = 20
	var ring_style := StyleBoxFlat.new()
	ring_style.bg_color = Color(0.0, 0.0, 0.0, 0.0)
	ring_style.border_color = Color(0.40, 0.72, 1.0, 0.95)
	ring_style.set_border_width_all(2)
	ring_style.set_corner_radius_all(5)
	_focus_ring.add_theme_stylebox_override("panel", ring_style)
	_overlay.add_child(_focus_ring)

	_wire_focus_targets()
	_refresh_labels()
	_refresh_unread()
	_update_focus_ring()


## Give every focus stop an explicit next/previous neighbour (Tab and
## Shift+Tab), one shared input handler, and a focus listener that keeps the
## logical target and the visible ring in sync with the engine focus owner.
func _wire_focus_targets() -> void:
	var controls: Array = []
	for target in FOCUS_ORDER:
		var control := _focus_control(target)
		if control == null:
			continue
		control.focus_mode = Control.FOCUS_ALL
		control.focus_entered.connect(_on_focus_entered.bind(target))
		control.focus_exited.connect(_on_focus_exited.bind(target))
		control.gui_input.connect(_on_focusable_gui_input.bind(target))
		control.resized.connect(_update_focus_ring)
		controls.append(control)
	for index in controls.size():
		var control: Control = controls[index]
		control.focus_next = control.get_path_to(controls[(index + 1) % controls.size()])
		control.focus_previous = control.get_path_to(
			controls[(index - 1 + controls.size()) % controls.size()]
		)
	resized.connect(_update_focus_ring)
	visibility_changed.connect(_on_visibility_changed)


func _refresh_labels() -> void:
	if _title != null:
		_title.text = _i18n.t("base.ui.chat.title")
	if _draft_edit != null:
		_draft_edit.placeholder_text = _i18n.t(
			"base.ui.chat.placeholder", {"max": MAX_CHAT_CHARS}
		)
	if _send_button != null:
		_send_button.text = _i18n.t("base.ui.chat.send")
	if _help_label != null:
		_help_label.text = _i18n.t("base.ui.chat.help")
	if _gamepad_label != null:
		_gamepad_label.text = _gamepad_footer_text()
	_refresh_counter()
	_refresh()
	_refresh_accessibility()


## Localized screen-reader names and descriptions for every visible part. The
## unread badge announces its compact text ("42" or "99+") only while visible;
## the status line announces the active status or the localized error.
func _refresh_accessibility() -> void:
	if _title != null:
		_title.accessibility_name = _i18n.t("base.ui.chat.title")
		_title.accessibility_description = _i18n.t("base.ui.chat.panel_description")
	if _transcript != null:
		_transcript.accessibility_name = _i18n.t("base.ui.chat.transcript")
		_transcript.accessibility_description = _i18n.t(
			"base.ui.chat.transcript_description"
		)
		_transcript.accessibility_live = A11Y_LIVE_POLITE
	if _draft_edit != null:
		_draft_edit.accessibility_name = _i18n.t("base.ui.chat.draft")
		_draft_edit.accessibility_description = _i18n.t(
			"base.ui.chat.placeholder", {"max": MAX_CHAT_CHARS}
		)
	if _send_button != null:
		_send_button.accessibility_name = _i18n.t("base.ui.chat.send")
		_send_button.accessibility_description = _i18n.t("base.ui.chat.send_hint")
	if _counter_label != null:
		_counter_label.accessibility_name = _i18n.t("base.ui.chat.char_counter_name")
		_counter_label.accessibility_description = _counter_label.text
	if _badge != null:
		_badge.accessibility_name = _i18n.t("base.ui.chat.unread_badge")
		_badge.accessibility_description = (
			_i18n.t("base.ui.chat.unread_announcement", {"count": _badge.text})
			if _badge.visible
			else ""
		)
	if _help_label != null:
		_help_label.accessibility_name = _i18n.t("base.ui.chat.help_name")
		_help_label.accessibility_description = _i18n.t("base.ui.chat.help")
	if _gamepad_label != null:
		_gamepad_label.accessibility_name = _i18n.t("base.ui.chat.gamepad_help")
		_gamepad_label.accessibility_description = _gamepad_label.text
	if _status_label != null:
		_status_label.accessibility_name = _i18n.t("base.ui.chat.status")
		_status_label.accessibility_description = _status_line()
		_status_label.accessibility_live = A11Y_LIVE_ASSERTIVE


func _refresh_counter() -> void:
	if _counter_label == null or _draft_edit == null:
		return
	_counter_label.text = _i18n.t(
		"base.ui.chat.char_counter",
		{"count": _draft_edit.text.length(), "max": MAX_CHAT_CHARS}
	)
	_refresh_accessibility()


func _refresh_unread() -> void:
	if _badge == null:
		return
	_badge.text = _unread_text(_unread)
	_badge.visible = _unread > 0
	_refresh_accessibility()


func _unread_text(count: int) -> String:
	if count <= 0:
		return ""
	if count > UNREAD_DISPLAY_MAX:
		return "99+"
	return str(count)


func _refresh() -> void:
	if _transcript == null:
		return
	var lines := transcript_lines()
	_transcript_page = mini(_transcript_page, _transcript_max_page())
	_transcript.text = (
		_i18n.t("base.ui.chat.empty_log") if lines.is_empty() else "\n".join(lines)
	)
	if _transcript_page > 0:
		_apply_transcript_page()
	else:
		_transcript.scroll_following = true
	var has_error := _error_code != ""
	_status_label.text = _status_line()
	_status_label.add_theme_color_override(
		"font_color",
		Color(0.96, 0.42, 0.38) if has_error else Color(0.78, 0.82, 0.88)
	)
	_refresh_accessibility()


func _on_draft_changed(_text: String) -> void:
	_refresh_counter()


func _on_draft_submitted(text: String) -> void:
	if not _can_submit_from_draft():
		return
	submit(text, "draft")


func _on_send_pressed() -> void:
	if _draft_edit != null:
		submit(_draft_edit.text, "send")


## A click focuses the control under the pointer; native caret/selection
## handling is left alone (the event is not accepted).
func _on_focusable_gui_input(event: InputEvent, target: String) -> void:
	if event is InputEventMouseButton:
		var mouse := event as InputEventMouseButton
		if mouse.button_index == MOUSE_BUTTON_LEFT and mouse.pressed:
			_set_focus(target, true)
		return
	if not (event is InputEventKey):
		return
	var key := event as InputEventKey
	if not key.pressed or key.echo:
		return
	if key.keycode == KEY_ESCAPE:
		_handle_escape()
		get_viewport().set_input_as_handled()


## Enter only submits while the draft owns focus. An exclusive modal or popup
## that took focus therefore blocks submission; an active IME composition is
## left to the platform instead of being sent as text.
func _can_submit_from_draft() -> bool:
	if _focus_id != "draft":
		return false
	if _smoke_mode:
		return true
	return (
		_draft_edit != null
		and _draft_edit.has_focus()
		and not _ime_composing()
	)


func _submit_from_draft() -> bool:
	if not _can_submit_from_draft():
		return false
	var text := _draft_edit.text if _draft_edit != null else ""
	submit(text, "draft")
	return true


## Same guard as `_submit_from_draft`, but returns the `submit` result so
## `ui_accept` can tell a send from a rejected draft. An empty dictionary means
## the guard refused (focus elsewhere, exclusive modal, or IME composing).
func _submit_from_draft_result() -> Dictionary:
	if not _can_submit_from_draft():
		return {}
	var text := _draft_edit.text if _draft_edit != null else ""
	return submit(text, "draft")


## First Escape clears a non-empty draft (and its error line); the next one
## releases focus to the surrounding UI. Only panel-owned input calls this.
func _handle_escape() -> bool:
	if _draft_edit != null and _draft_edit.text != "":
		_draft_edit.text = ""
		_refresh_counter()
		clear_error()
		return true
	focus_release()
	return true


func _ime_composing() -> bool:
	if DisplayServer.get_name() == "headless":
		return false
	return not DisplayServer.ime_get_text().is_empty()


## Fallback for a key event that no focused panel control consumed. Tab and
## Shift+Tab use the explicit focus_next/focus_previous chain; Escape and the
## documented Ctrl+L focus alias are handled here. No destructive shortcut.
func _unhandled_key_input(event: InputEvent) -> void:
	if not (event is InputEventKey) or not event.pressed or event.echo:
		return
	var key_event := event as InputEventKey
	if key_event.keycode == KEY_ESCAPE and _focus_id != "":
		_handle_escape()
		get_viewport().set_input_as_handled()
	elif (
		key_event.keycode == KEY_L
		and key_event.ctrl_pressed
		and _focus_id != ""
	):
		_set_focus("draft", true)
		get_viewport().set_input_as_handled()


## Joypad-only fallback. Keyboard events never enter this path, so the #464
## Tab/Enter/Esc model is unchanged and no key can fire a gamepad action.
## Focused controls and modals get first refusal because this is unhandled
## input; a released panel does not steal the pad from surrounding UI.
func _unhandled_input(event: InputEvent) -> void:
	if _smoke_mode or _focus_id == "" or not is_visible_in_tree():
		return
	var action := _gamepad_action_for_event(event)
	if action == "":
		return
	if _dispatch_gamepad(action):
		get_viewport().set_input_as_handled()


func _gamepad_action_for_event(event: InputEvent) -> String:
	if event is InputEventJoypadButton:
		var button := event as InputEventJoypadButton
		if not button.pressed:
			return ""
		for action in GAMEPAD_ACTIONS:
			if InputMap.event_is_action(button, action, true):
				return action
		return str(JOY_BUTTON_ACTIONS.get(button.button_index, ""))
	if event is InputEventJoypadMotion:
		var motion := event as InputEventJoypadMotion
		if motion.axis == JOY_AXIS_TRIGGER_LEFT:
			return _trigger_action(motion.axis_value, true)
		if motion.axis == JOY_AXIS_TRIGGER_RIGHT:
			return _trigger_action(motion.axis_value, false)
		for action in GAMEPAD_ACTIONS:
			if action != "ui_up" and action != "ui_down":
				continue
			if InputMap.event_is_action(motion, action, true):
				return action
	return ""


## Analog triggers are edge-triggered so one pull is one page. Shoulder buttons
## and PageUp/PageDown use the digital path.
func _trigger_action(value: float, left: bool) -> String:
	if left:
		if value <= 0.25:
			_trigger_left_held = false
			return ""
		if value < 0.5 or _trigger_left_held:
			return ""
		_trigger_left_held = true
		return "ui_page_up"
	if value <= 0.25:
		_trigger_right_held = false
		return ""
	if value < 0.5 or _trigger_right_held:
		return ""
	_trigger_right_held = true
	return "ui_page_down"


func _failure(code: String) -> Dictionary:
	return {
		"ok": false,
		"code": code,
		"message": _error_message(code),
	}


func _error_message(code: String) -> String:
	return _i18n.t(
		str(ERROR_KEYS.get(code, "base.ui.chat.error.invalid_fallback")),
		{"max": MAX_CHAT_CHARS}
	)


func _has_control_character(text: String) -> bool:
	for index in text.length():
		var code := text.unicode_at(index)
		if code <= 0x1f or (code >= 0x7f and code <= 0x9f):
			return true
	return false


func _merge_catchup(entries, last_seq: int) -> Dictionary:
	var floor_seq := maxi(maxi(last_seq, self.last_seq), 0)
	if not (entries is Array):
		return {
			"ok": false,
			"added": 0,
			"skipped": 0,
			"duplicate": 0,
			"last_seq": floor_seq,
			"unread": 0,
		}
	var accepted: Array = []
	var seen: Dictionary = {}
	var skipped := 0
	var duplicate := 0
	for entry in entries:
		var normalized := _normalized_entry(entry)
		if normalized.is_empty():
			skipped += 1
			continue
		var seq := int(normalized["seq"])
		if seq <= floor_seq or seen.has(seq):
			duplicate += 1
			continue
		seen[seq] = true
		accepted.append(normalized)
	accepted.sort_custom(_entry_seq_before)
	var added := 0
	var unread := 0
	for normalized in accepted:
		if not _merge_entry(normalized):
			duplicate += 1
			continue
		added += 1
		if not _is_local_entry(normalized):
			unread += 1
	self.last_seq = maxi(self.last_seq, floor_seq)
	return {
		"ok": true,
		"added": added,
		"skipped": skipped,
		"duplicate": duplicate,
		"last_seq": self.last_seq,
		"unread": unread,
	}


func _entry_seq_before(a: Dictionary, b: Dictionary) -> bool:
	return int(a["seq"]) < int(b["seq"])


## Whether an entry was authored by the signed-in local player. The server id is
## authoritative; the display name only covers entries without a `player_id`.
func _is_local_entry(entry: Dictionary) -> bool:
	var entry_id := int(entry.get("player_id", 0))
	if entry_id > 0 and player_id > 0:
		return entry_id == player_id
	if entry_id > 0:
		return false
	return player_name != "" and str(entry.get("player", "")) == player_name


func _merge_entry(normalized: Dictionary) -> bool:
	var seq := int(normalized.get("seq", 0))
	if seq <= 0 or _entries.has(seq):
		return false
	_entries[seq] = normalized
	if seq > last_seq:
		last_seq = seq
	_trim_lines()
	return true


func _trim_lines() -> void:
	while _entries.size() > MAX_LINES:
		var lowest := 0
		for seq in _entries.keys():
			if lowest == 0 or int(seq) < lowest:
				lowest = int(seq)
		_entries.erase(lowest)


func _normalized_entry(value) -> Dictionary:
	if not (value is Dictionary):
		return {}
	if not (value.has("seq") and value.has("player") and value.has("text")):
		return {}
	if not _is_number(value["seq"]) or not (value["player"] is String) or not (value["text"] is String):
		return {}
	var seq := int(value["seq"])
	var text := str(value["text"])
	if seq <= 0 or text.strip_edges().is_empty():
		return {}
	var entry := {
		"seq": seq,
		"tick": 0,
		"player_id": 0,
		"player": str(value["player"]),
		"text": text,
	}
	if value.has("tick") and _is_number(value["tick"]):
		entry["tick"] = int(value["tick"])
	if value.has("player_id") and _is_number(value["player_id"]):
		entry["player_id"] = int(value["player_id"])
	return entry


func _is_number(value) -> bool:
	return typeof(value) == TYPE_INT or typeof(value) == TYPE_FLOAT


func _status_line() -> String:
	if _error_code != "":
		return _error_line()
	if _status_text != "":
		return _status_text
	if player_name != "" or player_id != 0:
		return _i18n.t(
			"base.ui.chat.status_signed_in",
			{"name": player_name, "id": player_id}
		)
	return _i18n.t("base.ui.chat.status_no_identity")


func _error_line() -> String:
	if _error_code == "":
		return ""
	var body := ""
	if ERROR_KEYS.has(_error_code):
		body = _error_message(_error_code)
	if body == "":
		body = _error_detail if _error_detail != "" else _i18n.t("base.ui.chat.error.unknown")
	return _i18n.t("base.ui.chat.error.line", {"message": body, "code": _error_code})


func _canonical_render(lines: Array) -> String:
	var header := "\n".join([
		"identity=%d:%s" % [player_id, player_name],
		"status=" + _status_text,
		"error=" + _error_line(),
		"last_seq=%d" % last_seq,
		"lines=%d" % lines.size(),
	])
	if lines.is_empty():
		return header
	return header + "\n" + "\n".join(lines)


func _fnv32(text: String) -> String:
	var hash := 2166136261
	for byte in text.to_utf8_buffer():
		hash = (hash ^ int(byte)) & 0xffffffff
		hash = (hash * 16777619) & 0xffffffff
	return "%08x" % hash


# --- Headless synthetic smoke ----------------------------------------------

var _commands: Array = []
var _stages: Array = []
var _failures := 0
var _checks := 0
var _cases := 0


func _run_smoke() -> void:
	command_issued.connect(_on_smoke_command)

	var initial_focus := focus_snapshot()
	_check(str(initial_focus["focus"]) == "draft", "focus.initial", str(initial_focus["focus"]))
	_check(initial_focus["tab_order"] == FOCUS_ORDER, "focus.initial_order", JSON.stringify(initial_focus["tab_order"]))
	_check(str(initial_focus["draft"]) == "", "focus.initial_draft", str(initial_focus["draft"]))
	_check(not bool(initial_focus["can_send"]), "focus.initial_can_send")

	set_identity(7, "alice")
	var snap := _case("identity")
	_check(int(snap["player_id"]) == 7, "identity.player_id")
	_check(str(snap["player"]) == "alice", "identity.player")
	_check(int(snap["last_seq"]) == 0, "identity.last_seq")
	_check(int(snap["line_count"]) == 0, "identity.empty_transcript")
	_check(str(snap["status_line"]) == "Signed in as alice (#7)", "identity.status_line", str(snap["status_line"]))
	_check(_same_command(poll_command(), {"type": "chat_since", "after": 0}), "poll.initial", JSON.stringify(poll_command()))

	set_log([
		{"seq": 1, "tick": 10, "player_id": 2, "player": "bob", "text": "hello town"},
		{"seq": 2, "tick": 11, "player_id": 7, "player": "alice", "text": "hi bob"},
	])
	snap = _case("log")
	_check(int(snap["line_count"]) == 2, "log.count", str(snap["line_count"]))
	_check(int(snap["last_seq"]) == 2, "log.last_seq", str(snap["last_seq"]))
	_check(str(snap["lines"][0]).contains("bob: hello town"), "log.first", str(snap["lines"][0]))
	_check(_same_command(poll_command(), {"type": "chat_since", "after": 2}), "poll.after_log")

	_check(append_entry({"seq": 3, "tick": 12, "player_id": 3, "player": "carol", "text": "third"}), "append.added")
	_check(not append_entry({"seq": 3, "tick": 12, "player_id": 3, "player": "carol", "text": "third"}), "append.duplicate")
	snap = _case("append")
	_check(int(snap["line_count"]) == 3, "append.count", str(snap["line_count"]))
	_check(int(snap["last_seq"]) == 3, "append.last_seq", str(snap["last_seq"]))
	_check(_same_command(poll_command(), {"type": "chat_since", "after": 3}), "poll.after_append")

	var empty := draft("")
	_check(not bool(empty["ok"]) and str(empty["code"]) == "chat.empty", "draft.empty", JSON.stringify(empty))
	var blank := draft("   ")
	_check(not bool(blank["ok"]) and str(blank["code"]) == "chat.empty", "draft.blank", JSON.stringify(blank))
	var long := draft("x".repeat(MAX_CHAT_CHARS + 1))
	_check(not bool(long["ok"]) and str(long["code"]) == "chat.too_long", "draft.too_long", JSON.stringify(long))
	var boundary := draft("x".repeat(MAX_CHAT_CHARS))
	_check(bool(boundary["ok"]), "draft.max_boundary")
	var newline := draft("bad\nline")
	_check(not bool(newline["ok"]) and str(newline["code"]) == "chat.invalid", "draft.control_newline")
	var bell := draft("bad\u0007line")
	_check(not bool(bell["ok"]) and str(bell["code"]) == "chat.invalid", "draft.control_bell")
	var del := draft("bad\u007fline")
	_check(not bool(del["ok"]) and str(del["code"]) == "chat.invalid", "draft.control_del")
	_check(_commands.is_empty(), "draft.no_commands", JSON.stringify(_commands))
	_case("draft")

	var first := submit("  hello world  ")
	_check(bool(first["ok"]), "send.first.ok", JSON.stringify(first))
	_check(_commands.size() == 1, "send.first.count", str(_commands.size()))
	_check(_same_command(_commands[-1], {"type": "chat", "text": "hello world"}), "send.first.payload", JSON.stringify(_commands[-1]))
	_check(_error_code == "", "send.first.error_cleared", _error_line())

	var second := submit("padded")
	_check(bool(second["ok"]), "send.second.ok", JSON.stringify(second))
	_check(_commands.size() == 2, "send.second.count", str(_commands.size()))
	_check(_same_command(_commands[-1], {"type": "chat", "text": "padded"}), "send.second.payload", JSON.stringify(_commands[-1]))
	_case("send")

	submit("   ")
	_check(_commands.size() == 2, "send.rejected.silent", str(_commands.size()))
	_check(_error_code == "chat.empty", "send.rejected.error", _error_code)
	clear_error()
	_check(_error_code == "", "send.error_cleared")
	_check(int(snapshot()["line_count"]) == 3, "send.no_transcript_change")

	_check(apply_chat_sent({
		"type": "chat_sent",
		"tick": 40,
		"entry": {"seq": 4, "tick": 40, "player_id": 7, "player": "alice", "text": "hello world"},
	}), "sent.applied")
	snap = _case("chat_sent")
	_check(int(snap["last_seq"]) == 4, "sent.last_seq", str(snap["last_seq"]))
	_check(int(snap["line_count"]) == 4, "sent.count", str(snap["line_count"]))
	_check(str(snap["lines"][3]).contains("#4 alice: hello world"), "sent.line", str(snap["lines"][3]))
	_check(not apply_chat_sent({
		"type": "chat_sent",
		"tick": 40,
		"entry": {"seq": 4, "tick": 40, "player_id": 7, "player": "alice", "text": "hello world"},
	}), "sent.duplicate")
	_check(_same_command(poll_command(), {"type": "chat_since", "after": 4}), "poll.after_sent")

	var log_payload := {
		"type": "chat_log",
		"tick": 50,
		"entries": [
			{"seq": 4, "tick": 40, "player_id": 7, "player": "alice", "text": "hello world"},
			{"seq": 5, "tick": 50, "player_id": 2, "player": "bob", "text": "welcome"},
			{"seq": 6, "tick": 50, "player_id": 3, "player": "carol", "text": "hey"},
		],
	}
	_check(apply_chat_log(log_payload) == 2, "log.added")
	_check(apply_chat_log(log_payload) == 0, "log.dedupe")
	snap = _case("chat_log")
	_check(int(snap["line_count"]) == 6, "log.merged_count", str(snap["line_count"]))
	_check(int(snap["last_seq"]) == 6, "log.merged_last_seq", str(snap["last_seq"]))
	_check(_same_command(poll_command(), {"type": "chat_since", "after": 6}), "poll.after_log")

	_check(apply_chat_log({
		"type": "chat_log",
		"tick": 60,
		"entries": [
			{"seq": 8, "tick": 60, "player_id": 2, "player": "bob", "text": "eight"},
			{"seq": 7, "tick": 59, "player_id": 3, "player": "carol", "text": "seven"},
		],
	}) == 2, "log.out_of_order.added")
	snap = _case("log_order")
	_check(int(snap["line_count"]) == 8, "log.order_count", str(snap["line_count"]))
	_check(int(snap["last_seq"]) == 8, "log.order_last_seq", str(snap["last_seq"]))
	_check(str(snap["lines"][6]).contains("#7 carol: seven"), "log.order_seven", str(snap["lines"][6]))
	_check(str(snap["lines"][7]).contains("#8 bob: eight"), "log.order_eight", str(snap["lines"][7]))

	var bulk: Array = []
	for index in range(100, 181):
		bulk.append({
			"seq": index,
			"tick": 100 + index,
			"player_id": 2 + (index % 3),
			"player": "bulk%d" % (index % 3),
			"text": "line %d" % index,
		})
	_check(apply_chat_log({"type": "chat_log", "tick": 400, "entries": bulk}) == 81, "cap.added")
	snap = _case("cap")
	_check(int(snap["line_count"]) == MAX_LINES, "cap.display", str(snap["line_count"]))
	_check(int(snap["last_seq"]) == 180, "cap.last_seq", str(snap["last_seq"]))
	_check(str(snap["lines"][0]).contains("#117 "), "cap.oldest", str(snap["lines"][0]))
	_check(str(snap["lines"][MAX_LINES - 1]).contains("#180 "), "cap.newest", str(snap["lines"][MAX_LINES - 1]))
	_check(_same_command(poll_command(), {"type": "chat_since", "after": 180}), "poll.after_cap")

	var reset: Array = []
	for index in range(20, 24):
		reset.append({
			"seq": index,
			"tick": 500 + index,
			"player_id": 7,
			"player": "alice",
			"text": "reset %d" % index,
		})
	set_log(reset)
	snap = _case("set_log_reset")
	_check(int(snap["line_count"]) == 4, "reset.count", str(snap["line_count"]))
	_check(int(snap["last_seq"]) == 23, "reset.last_seq", str(snap["last_seq"]))
	_check(_same_command(poll_command(), {"type": "chat_since", "after": 23}), "poll.after_reset")

	var before_malformed := snapshot()
	_check(not apply_chat_sent({"type": "chat_sent"}), "malformed.sent")
	_check(_error_code == "chat.invalid", "malformed.sent_code", _error_code)
	_check(_same_command(poll_command(), {"type": "chat_since", "after": 23}), "malformed.sent_poll")
	clear_error()
	_check(apply_chat_log({"type": "chat_log", "tick": 600, "entries": "nope"}) == 0, "malformed.log")
	_check(_error_code == "chat.invalid", "malformed.log_code", _error_code)
	clear_error()
	_check(apply_chat_log({
		"type": "chat_log",
		"tick": 600,
		"entries": [{"seq": 0, "player": "bob", "text": "zero"}],
	}) == 0, "malformed.entry")
	_check(_error_code == "chat.invalid", "malformed.entry_code", _error_code)
	_check(int(snapshot()["line_count"]) == int(before_malformed["line_count"]), "malformed.no_lines")
	clear_error()

	set_error("chat.too_long", "server detail")
	var line := str(snapshot()["error_line"])
	_check(line.begins_with("Error: ") and line.ends_with("(chat.too_long)"), "error.stable", line)
	_check(not line.contains("server detail"), "error.hides_detail", line)
	var lines_before_locale := int(snapshot()["line_count"])

	_check(set_locale("de_DE") == "de", "locale.de_resolves")
	var de_error := str(snapshot()["error_line"])
	_check(de_error.begins_with("Fehler: ") and de_error.contains("zu lang"), "locale.de_error", de_error)
	clear_error()
	_check(str(snapshot()["status_line"]).contains("Angemeldet als alice (#7)"), "locale.de_status", str(snapshot()["status_line"]))
	_check(set_locale("fr") == "en", "locale.unsupported_falls_back")
	_check(str(snapshot()["status_line"]).begins_with("Signed in as alice (#7)"), "locale.fallback_en", str(snapshot()["status_line"]))
	_check(set_locale("en") == "en", "locale.en_resolves")
	_check(int(snapshot()["line_count"]) == lines_before_locale, "locale.transcript_unchanged")
	_case("locale")
	print("CHAT_PANEL_LOCALE de=ok fr=en en=ok lines=%d" % lines_before_locale)

	var unread_before := snapshot()
	set_unread(0)
	snap = _case("unread_zero")
	_check(int(snap["unread"]) == 0, "unread.zero", str(snap["unread"]))
	_check(not badge_visible(), "unread.zero_hidden")
	_check(badge_text() == "", "unread.zero_text", badge_text())
	_check(_badge != null and not _badge.visible, "unread.zero_label", badge_text())
	_check(str(snap["transcript"]) == str(unread_before["transcript"]), "unread.transcript")
	_check(str(snap["status_line"]) == str(unread_before["status_line"]), "unread.status")
	_check(str(snap["render_checksum"]) == str(unread_before["render_checksum"]), "unread.render")

	set_unread(1)
	snap = _case("unread_one")
	_check(int(snap["unread"]) == 1, "unread.one", str(snap["unread"]))
	_check(badge_visible(), "unread.one_visible")
	_check(badge_text() == "1", "unread.one_text", badge_text())
	_check(_badge != null and _badge.visible and _badge.text == "1", "unread.one_label", badge_text())

	set_unread(42)
	snap = _case("unread_42")
	_check(int(snap["unread"]) == 42, "unread.42", str(snap["unread"]))
	_check(badge_visible(), "unread.42_visible")
	_check(badge_text() == "42", "unread.42_text", badge_text())
	_check(_badge != null and _badge.text == "42", "unread.42_label", badge_text())

	set_unread(1000)
	snap = _case("unread_1000")
	_check(int(snap["unread"]) == 1000, "unread.1000", str(snap["unread"]))
	_check(badge_visible(), "unread.1000_visible")
	_check(badge_text() == "99+", "unread.1000_text", badge_text())
	_check(_badge != null and _badge.text == "99+", "unread.1000_label", badge_text())
	_check(str(snap["transcript"]) == str(unread_before["transcript"]), "unread.1000_transcript")

	set_unread(-7)
	_check(int(snapshot()["unread"]) == 0, "unread.negative", str(snapshot()["unread"]))
	_check(not badge_visible(), "unread.negative_hidden")
	_check(badge_text() == "", "unread.negative_text", badge_text())
	set_unread(0)

	set_error("chat.future", "new server detail")
	_check(str(snapshot()["error_line"]).contains("new server detail"), "error.fallback")
	clear_error()
	set_status("Connected as alice")
	snap = _case("status")
	_check(str(snap["status_line"]) == "Connected as alice", "status.line", str(snap["status_line"]))
	_check(str(snap["error_line"]) == "", "status.no_error")
	set_status("")
	_check(str(snapshot()["status_line"]) == "Signed in as alice (#7)", "status.identity_fallback")

	# --- Reconnect catch-up merge (deterministic, local seq math) ---
	var failures_before_catchup := _failures
	set_log([])
	set_unread(0)
	_draft_edit.text = "draft survives catch-up"

	var tail := apply_chat_catchup([
		{"seq": 1, "tick": 1, "player_id": 2, "player": "bob", "text": "one"},
		{"seq": 2, "tick": 2, "player_id": 7, "player": "alice", "text": "two"},
		{"seq": 3, "tick": 3, "player_id": 3, "player": "carol", "text": "three"},
	], 2)
	snap = _case("catchup_tail")
	_check(bool(tail.get("ok", false)), "catchup.tail.ok", JSON.stringify(tail))
	_check(int(tail.get("added", -1)) == 1, "catchup.tail.added", JSON.stringify(tail))
	_check(int(tail.get("skipped", -1)) == 0, "catchup.tail.skipped", JSON.stringify(tail))
	_check(int(tail.get("duplicate", -1)) == 2, "catchup.tail.duplicate", JSON.stringify(tail))
	_check(int(tail.get("last_seq", -1)) == 3, "catchup.tail.last_seq", JSON.stringify(tail))
	_check(int(snap["line_count"]) == 1, "catchup.tail.count", str(snap["line_count"]))
	_check(str(snap["lines"][0]).contains("#3 carol: three"), "catchup.tail.line", str(snap["lines"][0]))
	_check(int(snap["unread"]) == 1, "catchup.tail.unread", str(snap["unread"]))
	_check(_draft_edit.text == "draft survives catch-up", "catchup.tail.draft", _draft_edit.text)

	set_log([])
	set_unread(0)
	var ordered := apply_chat_catchup([
		{"seq": 9, "tick": 9, "player_id": 2, "player": "bob", "text": "nine"},
		{"seq": 7, "tick": 7, "player_id": 3, "player": "carol", "text": "seven"},
		{"seq": 8, "tick": 8, "player_id": 2, "player": "bob", "text": "eight"},
		{"seq": 8, "tick": 8, "player_id": 2, "player": "bob", "text": "eight again"},
	], 6)
	snap = _case("catchup_order")
	_check(bool(ordered.get("ok", false)), "catchup.order.ok", JSON.stringify(ordered))
	_check(int(ordered.get("added", -1)) == 3, "catchup.order.added", JSON.stringify(ordered))
	_check(int(ordered.get("skipped", -1)) == 0, "catchup.order.skipped", JSON.stringify(ordered))
	_check(int(ordered.get("duplicate", -1)) == 1, "catchup.order.duplicate", JSON.stringify(ordered))
	_check(int(ordered.get("last_seq", -1)) == 9, "catchup.order.last_seq", JSON.stringify(ordered))
	_check(int(snap["line_count"]) == 3, "catchup.order.count", str(snap["line_count"]))
	_check(str(snap["lines"][0]).contains("#7 carol: seven"), "catchup.order.first", str(snap["lines"][0]))
	_check(str(snap["lines"][1]).contains("#8 bob: eight"), "catchup.order.second", str(snap["lines"][1]))
	_check(str(snap["lines"][2]).contains("#9 bob: nine"), "catchup.order.third", str(snap["lines"][2]))

	set_log([])
	set_unread(0)
	var malformed_catchup := apply_chat_catchup([
		{"seq": 4, "tick": 4, "player_id": 2, "player": "bob", "text": "valid"},
		{"seq": 5, "tick": 5, "player_id": 2, "player": "bob"},
		"not an entry",
		{"seq": 0, "player": "bob", "text": "zero"},
	], 0)
	snap = _case("catchup_malformed")
	_check(bool(malformed_catchup.get("ok", false)), "catchup.malformed.ok", JSON.stringify(malformed_catchup))
	_check(int(malformed_catchup.get("added", -1)) == 1, "catchup.malformed.added", JSON.stringify(malformed_catchup))
	_check(int(malformed_catchup.get("skipped", -1)) == 3, "catchup.malformed.skipped", JSON.stringify(malformed_catchup))
	_check(int(malformed_catchup.get("duplicate", -1)) == 0, "catchup.malformed.duplicate", JSON.stringify(malformed_catchup))
	_check(int(malformed_catchup.get("last_seq", -1)) == 4, "catchup.malformed.last_seq", JSON.stringify(malformed_catchup))
	_check(int(snap["line_count"]) == 1, "catchup.malformed.count", str(snap["line_count"]))
	_check(str(snap["lines"][0]).contains("#4 bob: valid"), "catchup.malformed.line", str(snap["lines"][0]))

	set_log([])
	set_unread(3)
	var mixed := apply_chat_catchup([
		{"seq": 1, "tick": 1, "player_id": 7, "player": "alice", "text": "mine one"},
		{"seq": 2, "tick": 2, "player_id": 2, "player": "bob", "text": "theirs one"},
		{"seq": 3, "tick": 3, "player_id": 7, "player": "alice", "text": "mine two"},
		{"seq": 4, "tick": 4, "player_id": 3, "player": "carol", "text": "theirs two"},
	], 0)
	snap = _case("catchup_identity")
	_check(int(mixed.get("added", -1)) == 4, "catchup.identity.added", JSON.stringify(mixed))
	_check(int(mixed.get("last_seq", -1)) == 4, "catchup.identity.last_seq", JSON.stringify(mixed))
	_check(int(snap["unread"]) == 2, "catchup.identity.unread", str(snap["unread"]))
	_check(badge_text() == "2", "catchup.identity.badge", badge_text())

	set_log([])
	set_unread(0)
	var own_only := apply_chat_catchup([
		{"seq": 5, "tick": 5, "player_id": 7, "player": "alice", "text": "mine three"},
	], 4)
	_check(int(own_only.get("added", -1)) == 1, "catchup.own.added", JSON.stringify(own_only))
	_check(int(own_only.get("last_seq", -1)) == 5, "catchup.own.last_seq", JSON.stringify(own_only))
	_check(int(snapshot()["unread"]) == 0, "catchup.own.unread", str(snapshot()["unread"]))

	set_log([])
	set_unread(0)
	var catchup_bulk: Array = []
	for index in range(1, MAX_LINES + 7):
		catchup_bulk.append({
			"seq": index,
			"tick": index,
			"player_id": 2,
			"player": "bob",
			"text": "bulk %d" % index,
		})
	var capped := apply_chat_catchup(catchup_bulk, 0)
	snap = _case("catchup_cap")
	_check(int(capped.get("added", -1)) == MAX_LINES + 6, "catchup.cap.added", JSON.stringify(capped))
	_check(int(capped.get("last_seq", -1)) == MAX_LINES + 6, "catchup.cap.last_seq", JSON.stringify(capped))
	_check(int(snap["line_count"]) == MAX_LINES, "catchup.cap.display", str(snap["line_count"]))
	_check(str(snap["lines"][0]).contains("#7 "), "catchup.cap.oldest", str(snap["lines"][0]))
	_check(str(snap["lines"][MAX_LINES - 1]).contains("#70 "), "catchup.cap.newest", str(snap["lines"][MAX_LINES - 1]))

	var ahead := apply_chat_catchup([], MAX_LINES + 16)
	snap = _case("catchup_ahead")
	_check(bool(ahead.get("ok", false)), "catchup.ahead.ok", JSON.stringify(ahead))
	_check(int(ahead.get("added", -1)) == 0, "catchup.ahead.added", JSON.stringify(ahead))
	_check(int(ahead.get("last_seq", -1)) == MAX_LINES + 16, "catchup.ahead.last_seq", JSON.stringify(ahead))
	_check(int(snap["last_seq"]) == MAX_LINES + 16, "catchup.ahead.panel_last_seq", str(snap["last_seq"]))
	_check(int(snap["unread"]) == 0, "catchup.ahead.unread", str(snap["unread"]))

	var catchup_added := (
		int(tail["added"]) + int(ordered["added"]) + int(malformed_catchup["added"])
		+ int(mixed["added"]) + int(own_only["added"]) + int(capped["added"])
		+ int(ahead["added"])
	)
	var catchup_skipped := (
		int(tail["skipped"]) + int(ordered["skipped"]) + int(malformed_catchup["skipped"])
		+ int(mixed["skipped"]) + int(own_only["skipped"]) + int(capped["skipped"])
		+ int(ahead["skipped"])
	)
	if _failures == failures_before_catchup:
		print("MP_CHAT_CATCHUP status=ok added=%d skipped=%d last_seq=%d unread=%d" % [
			catchup_added, catchup_skipped, int(snap["last_seq"]), int(snap["unread"]),
		])
	else:
		print("MP_CHAT_CATCHUP status=fail added=0 skipped=0 last_seq=0 unread=0")

	# --- Deterministic keyboard focus (logical, no engine focus calls) ---
	var focus_failures := _failures
	var focus_checks_before := _checks

	_check(focus_target("draft"), "focus.reset.ok")
	_draft_edit.text = ""
	_refresh_counter()
	var focus := focus_snapshot()
	_check(str(focus["focus"]) == "draft", "focus.reset.draft", str(focus["focus"]))
	_check(focus["tab_order"] == FOCUS_ORDER, "focus.reset.order", JSON.stringify(focus["tab_order"]))

	var moved := focus_step()
	_check(moved == "send", "focus.tab.draft_to_send", moved)
	moved = focus_step()
	_check(moved == "transcript", "focus.tab.send_to_log", moved)
	moved = focus_step()
	_check(moved == "status", "focus.tab.log_to_status", moved)
	moved = focus_step()
	_check(moved == "draft", "focus.tab.status_wraps", moved)

	moved = focus_step(true)
	_check(moved == "status", "focus.shift_tab.draft_to_status", moved)
	moved = focus_step(true)
	_check(moved == "transcript", "focus.shift_tab.status_to_log", moved)

	_check(focus_target("send"), "focus.click.send.ok")
	_check(str(focus_snapshot()["focus"]) == "send", "focus.click.send")
	_check(focus_target("transcript"), "focus.click.log.ok")
	_check(str(focus_snapshot()["focus"]) == "transcript", "focus.click.log")
	_check(not focus_target("badge"), "focus.click.badge_not_focusable")
	_check(str(focus_snapshot()["focus"]) == "transcript", "focus.click.unknown_unchanged")

	_check(focus_target("draft"), "focus.enter.focus_ok")
	_draft_edit.text = "focus enter"
	_refresh_counter()
	focus = focus_snapshot()
	_check(
		str(focus["draft"]) == "focus enter" and bool(focus["can_send"]),
		"focus.enter.ready",
		JSON.stringify(focus)
	)
	_check(focus_key(KEY_ENTER), "focus.enter.consumed")
	focus = focus_snapshot()
	_check(str(focus["focus"]) == "draft", "focus.enter.focus_draft", str(focus["focus"]))
	_check(str(focus["draft"]) == "", "focus.enter.cleared", str(focus["draft"]))
	_check(_commands.size() == 3, "focus.enter.command_count", str(_commands.size()))
	_check(
		_same_command(_commands[-1], {"type": "chat", "text": "focus enter"}),
		"focus.enter.payload",
		JSON.stringify(_commands[-1])
	)
	_check(_error_code == "", "focus.enter.error_cleared", _error_code)

	focus_target("send")
	var commands_before := _commands.size()
	_check(not focus_key(KEY_ENTER), "focus.enter.send_does_not_submit")
	_check(_commands.size() == commands_before, "focus.enter.send_silent", str(_commands.size()))

	focus_target("draft")
	_check(focus_key(KEY_ENTER), "focus.enter.empty_consumed")
	_check(_error_code == "chat.empty", "focus.enter.empty_rejected", _error_code)
	_check(str(focus_snapshot()["focus"]) == "draft", "focus.enter.empty_focus")
	clear_error()

	focus_target("draft")
	_draft_edit.text = "unsent"
	_refresh_counter()
	_check(focus_key(KEY_ESCAPE), "focus.esc.consumed")
	focus = focus_snapshot()
	_check(str(focus["draft"]) == "", "focus.esc.cleared", str(focus["draft"]))
	_check(str(focus["focus"]) == "draft", "focus.esc.keeps_focus", str(focus["focus"]))
	_check(_error_code == "", "focus.esc.error_cleared", _error_code)
	_check(focus_key(KEY_ESCAPE), "focus.esc.release_consumed")
	_check(str(focus_snapshot()["focus"]) == "", "focus.esc.released", str(focus_snapshot()["focus"]))
	_check(focus_step() == "draft", "focus.released_tab_starts_draft", str(focus_snapshot()["focus"]))
	focus_release()
	_check(focus_step(true) == "status", "focus.released_shift_tab_starts_last", str(focus_snapshot()["focus"]))

	focus_target("send")
	_draft_edit.text = "   "
	_refresh_counter()
	var rejected := submit(_draft_edit.text, "send")
	_check(not bool(rejected["ok"]), "focus.reject.send.ok", JSON.stringify(rejected))
	_check(_error_code == "chat.empty", "focus.reject.send.error", _error_code)
	_check(str(focus_snapshot()["focus"]) == "send", "focus.reject.send.focus", str(focus_snapshot()["focus"]))
	focus_target("draft")
	rejected = submit(_draft_edit.text, "draft")
	_check(not bool(rejected["ok"]), "focus.reject.draft.ok", JSON.stringify(rejected))
	_check(str(focus_snapshot()["focus"]) == "draft", "focus.reject.draft.focus", str(focus_snapshot()["focus"]))
	clear_error()
	_draft_edit.text = ""
	_refresh_counter()

	focus_target("transcript")
	set_unread(5)
	focus = focus_snapshot()
	_check(int(focus["unread"]) == 5, "focus.unread.count", str(focus["unread"]))
	_check(badge_text() == "5" and badge_visible(), "focus.unread.badge", badge_text())
	_check(str(focus["focus"]) == "transcript", "focus.unread.keeps_focus", str(focus["focus"]))
	set_unread(0)
	focus = focus_snapshot()
	_check(int(focus["unread"]) == 0, "focus.unread.reset_count", str(focus["unread"]))
	_check(not badge_visible() and badge_text() == "", "focus.unread.reset_badge", badge_text())
	_check(str(focus["focus"]) == "transcript", "focus.unread.reset_keeps_focus", str(focus["focus"]))
	focus_target("draft")

	var focus_check_count := _checks - focus_checks_before
	if _failures == focus_failures:
		print("CHAT_PANEL_FOCUS status=ok checks=%d" % focus_check_count)
	else:
		print("CHAT_PANEL_FOCUS status=fail checks=%d errors=%d" % [
			focus_check_count, _failures - focus_failures,
		])

	# --- Deterministic gamepad navigation (no commands, no render cases) ---
	var gamepad_failures := _failures
	var gamepad_checks_before := _checks
	_check(focus_target("draft"), "gamepad.reset.ok")
	_draft_edit.text = ""
	_refresh_counter()
	var pad := gamepad_action("ui_down")
	_check(str(pad["focus"]) == "send", "gamepad.down.draft_send", str(pad["focus"]))
	_check(bool(pad["gamepad_focus"]), "gamepad.down.active", str(pad["gamepad_focus"]))
	pad = gamepad_action("ui_down")
	_check(str(pad["focus"]) == "transcript", "gamepad.down.send_log", str(pad["focus"]))
	pad = gamepad_action("ui_up")
	_check(str(pad["focus"]) == "send", "gamepad.up.log_send", str(pad["focus"]))
	_check(str(pad["last_action"]) == "ui_up", "gamepad.last_action", str(pad["last_action"]))
	_check(str(gamepad_footer()).contains("ui_accept"), "gamepad.footer.accept", gamepad_footer())
	_check(str(gamepad_footer()).contains("ui_cancel"), "gamepad.footer.cancel", gamepad_footer())
	pad = gamepad_action("ui_accept")
	_check(_commands.size() == 3, "gamepad.accept.no_send", str(_commands.size()))
	_check(str(pad["focus"]) == "send", "gamepad.accept.send_focus", str(pad["focus"]))
	pad = gamepad_action("ui_cancel")
	_check(["", "send", "draft"].has(str(pad["focus"])), "gamepad.cancel.no_draft", str(pad["focus"]))
	_check(not bool(pad["gamepad_focus"]), "gamepad.cancel.released", str(pad["gamepad_focus"]))
	_check(focus_target("draft"), "gamepad.keyboard_reset.ok")
	_check(not bool(focus_snapshot()["gamepad_focus"]), "gamepad.keyboard_clears", str(focus_snapshot()["gamepad_focus"]))
	if _failures == gamepad_failures:
		print("CHAT_PANEL_GAMEPAD status=ok checks=%d" % (_checks - gamepad_checks_before))
	else:
		print("CHAT_PANEL_GAMEPAD status=fail checks=%d" % (_checks - gamepad_checks_before))

	# --- Deterministic virtual-keyboard intent (draft + gamepad) ---
	# Intent is counted on every platform; the DisplayServer call is skipped
	# headless, so this block runs without a display. No render case is added,
	# which keeps the CHAT_PANEL checksum at the pre-VK baseline.
	var vk_failures := _failures
	var vk_checks_before := _checks

	# Keyboard modality never requests the platform keyboard for the draft.
	focus_target("draft")
	var vk := focus_snapshot()
	_check(not bool(vk["vk_requested"]), "vk.keyboard.not_requested", JSON.stringify(vk))
	_check(int(vk["vk_requests"]) == 0, "vk.keyboard.no_request", str(vk["vk_requests"]))
	_check(int(vk["vk_clears"]) == 0, "vk.keyboard.no_clear", str(vk["vk_clears"]))

	# Leaving the draft before any request must not count a clear.
	vk = gamepad_action("ui_down")
	_check(str(vk["focus"]) == "send", "vk.leave.focus", str(vk["focus"]))
	_check(not bool(vk["vk_requested"]), "vk.leave.not_requested", JSON.stringify(vk))
	_check(int(vk["vk_clears"]) == 0, "vk.leave.no_clear", str(vk["vk_clears"]))

	# Gamepad navigation onto the draft requests it exactly once.
	vk = gamepad_action("ui_up")
	_check(str(vk["focus"]) == "draft", "vk.focus.draft", str(vk["focus"]))
	_check(bool(vk["vk_requested"]), "vk.focus.requested", JSON.stringify(vk))
	_check(int(vk["vk_requests"]) == 1, "vk.focus.requests", str(vk["vk_requests"]))
	_check(int(vk["vk_clears"]) == 0, "vk.focus.no_clear", str(vk["vk_clears"]))

	# A rejected `ui_accept` keeps the field open; a repeat request is a no-op.
	vk = gamepad_action("ui_accept")
	_check(str(vk["focus"]) == "draft", "vk.duplicate.draft", str(vk["focus"]))
	_check(bool(vk["vk_requested"]), "vk.duplicate.requested", JSON.stringify(vk))
	_check(int(vk["vk_requests"]) == 1, "vk.duplicate.requests", str(vk["vk_requests"]))
	_check(_error_code == "chat.empty", "vk.duplicate.error", _error_code)
	clear_error()

	# A successful send clears the keyboard and returns focus to the draft.
	_draft_edit.text = "vk send"
	_refresh_counter()
	var commands_before_vk := _commands.size()
	vk = gamepad_action("ui_accept")
	_check(_commands.size() == commands_before_vk + 1, "vk.send.emitted", str(_commands.size()))
	_check(
		_same_command(_commands[-1], {"type": "chat", "text": "vk send"}),
		"vk.send.payload",
		JSON.stringify(_commands[-1])
	)
	_check(str(vk["focus"]) == "draft", "vk.send.focus_draft", str(vk["focus"]))
	_check(not bool(vk["vk_requested"]), "vk.send.cleared", JSON.stringify(vk))
	_check(int(vk["vk_clears"]) == 1, "vk.send.clears", str(vk["vk_clears"]))

	# Cancel from an empty draft releases focus and clears the keyboard.
	gamepad_action("ui_down")
	vk = gamepad_action("ui_up")
	_check(bool(vk["vk_requested"]), "vk.reopen.requested", JSON.stringify(vk))
	_check(int(vk["vk_requests"]) == 2, "vk.reopen.requests", str(vk["vk_requests"]))
	vk = gamepad_action("ui_cancel")
	_check(str(vk["focus"]) == "", "vk.cancel.focus_released", str(vk["focus"]))
	_check(not bool(vk["vk_requested"]), "vk.cancel.cleared", JSON.stringify(vk))
	_check(int(vk["vk_clears"]) == 2, "vk.cancel.clears", str(vk["vk_clears"]))

	# Hiding the panel clears a requested keyboard.
	vk = gamepad_action("ui_down")
	_check(str(vk["focus"]) == "draft", "vk.hide.focus_draft", str(vk["focus"]))
	_check(bool(vk["vk_requested"]), "vk.hide.requested", JSON.stringify(vk))
	hide()
	vk = focus_snapshot()
	_check(not bool(vk["vk_requested"]), "vk.hide.cleared", JSON.stringify(vk))
	_check(int(vk["vk_clears"]) == 3, "vk.hide.clears", str(vk["vk_clears"]))
	show()

	if _failures == vk_failures:
		print("CHAT_PANEL_VK status=ok checks=%d" % (_checks - vk_checks_before))
	else:
		print("CHAT_PANEL_VK status=fail checks=%d errors=%d" % [
			_checks - vk_checks_before, _failures - vk_failures,
		])

	_case("final")

	# The VK block contributes the fourth and last command ("vk send"); the
	# render stages (and therefore the CHAT_PANEL checksum) are unchanged.
	_check(_commands.size() == 4, "final.command_count", str(_commands.size()))
	_check(_same_command(_commands[-1], {"type": "chat", "text": "vk send"}), "final.last_payload", JSON.stringify(_commands[-1]))
	_check(_same_command(_commands[0], {"type": "chat", "text": "hello world"}), "final.first_payload", JSON.stringify(_commands[0]))

	# --- Screen-reader semantics (no render case, no checksum weight) ---
	# Every check runs after the final render stage, so the transcript, status,
	# and locale changes here cannot move the pinned `CHAT_PANEL` checksum.
	var a11y_failures := _failures
	var a11y_checks_before := _checks
	_draft_edit.text = ""
	_refresh_counter()
	set_unread(42)
	var a11y := accessibility_snapshot()
	_check(str(a11y["locale"]) == "en", "a11y.locale_en", str(a11y["locale"]))
	for part in [
		"title", "transcript", "draft", "send", "counter", "badge", "help", "gamepad", "status",
	]:
		var entry: Dictionary = a11y[part]
		_check(not str(entry["name"]).is_empty(), "a11y.%s.name" % part, str(entry["name"]))
		_check(
			not str(entry["description"]).is_empty(),
			"a11y.%s.description" % part,
			str(entry["description"])
		)
	_check(str(a11y["transcript"]["live"]) == "polite", "a11y.transcript.live", str(a11y["transcript"]["live"]))
	_check(str(a11y["status"]["live"]) == "assertive", "a11y.status.live", str(a11y["status"]["live"]))
	_check(bool(a11y["draft"]["reachable"]), "a11y.draft.reachable", JSON.stringify(a11y["draft"]))
	_check(bool(a11y["send"]["reachable"]), "a11y.send.reachable", JSON.stringify(a11y["send"]))
	_check(str(a11y["draft"]["role"]) == "text_field", "a11y.draft.role", str(a11y["draft"]["role"]))
	_check(str(a11y["send"]["role"]) == "button", "a11y.send.role", str(a11y["send"]["role"]))
	_check(
		str(a11y["badge"]["description"]).contains(badge_text()),
		"a11y.badge.announced",
		str(a11y["badge"]["description"])
	)
	_check(_a11y_no_leak(a11y), "a11y.en.no_missing_keys", JSON.stringify(a11y))
	_check(str(a11y["title"]["name"]) == "Chat (developer panel)", "a11y.en.title", str(a11y["title"]["name"]))

	var en_title := str(a11y["title"]["name"])
	var en_transcript := str(a11y["transcript"]["name"])
	_check(set_locale("de") == "de", "a11y.locale_de")
	a11y = accessibility_snapshot()
	_check(str(a11y["title"]["name"]) == "Chat (Entwicklerpanel)", "a11y.de.title", str(a11y["title"]["name"]))
	_check(str(a11y["transcript"]["name"]) == "Chatnachrichten", "a11y.de.transcript", str(a11y["transcript"]["name"]))
	_check(str(a11y["send"]["name"]) == "Senden", "a11y.de.send", str(a11y["send"]["name"]))
	_check(str(a11y["title"]["name"]) != en_title, "a11y.de.title_switched")
	_check(str(a11y["transcript"]["name"]) != en_transcript, "a11y.de.transcript_switched")
	_check(_a11y_no_leak(a11y), "a11y.de.no_missing_keys", JSON.stringify(a11y))
	_check(set_locale("en") == "en", "a11y.locale_en_restored")
	_check(str(accessibility_snapshot()["title"]["name"]) == en_title, "a11y.en.title_restored")
	set_error("chat.empty", "")
	a11y = accessibility_snapshot()
	_check(str(a11y["status"]["live"]) == "assertive", "a11y.status.error_live", str(a11y["status"]["live"]))
	_check(
		str(a11y["status"]["description"]).contains("Error:"),
		"a11y.status.error_description",
		str(a11y["status"]["description"])
	)
	clear_error()
	set_unread(0)

	if _failures == a11y_failures:
		print("CHAT_PANEL_A11Y status=ok checks=%d" % (_checks - a11y_checks_before))
	else:
		print("CHAT_PANEL_A11Y status=fail checks=%d errors=%d" % [
			_checks - a11y_checks_before, _failures - a11y_failures,
		])

	var checksum := _fnv32("\n".join(_stages))
	if _failures == 0 and _commands.size() == 4:
		print("CHAT_PANEL status=ok cases=%d checks=%d checksum=%s" % [_cases, _checks, checksum])
		get_tree().quit(0)
	else:
		print("CHAT_PANEL status=fail cases=%d checks=%d errors=%d checksum=%s" % [
			_cases, _checks, _failures, checksum,
		])
		get_tree().quit(1)


func _on_smoke_command(command: Dictionary) -> void:
	_commands.append(command.duplicate(true))


func _case(name: String) -> Dictionary:
	_cases += 1
	var snap := snapshot()
	_stages.append("%s=%s" % [name, str(snap.get("render_checksum", ""))])
	print("CHAT_PANEL_STAGE name=%s lines=%d last_seq=%d checksum=%s" % [
		name,
		int(snap.get("line_count", 0)),
		int(snap.get("last_seq", 0)),
		str(snap.get("render_checksum", "")),
	])
	return snap


func _check(condition: bool, label: String, detail: String = "") -> void:
	_checks += 1
	if condition:
		return
	_failures += 1
	print("CHAT_PANEL_FAIL %s %s" % [label, detail])


func _same_command(actual, expected: Dictionary) -> bool:
	if not (actual is Dictionary):
		return false
	var left: Dictionary = actual
	if left.size() != expected.size():
		return false
	for key in expected.keys():
		if not left.has(key) or left[key] != expected[key]:
			return false
	return true
