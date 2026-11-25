extends PanelContainer

## Developer chat panel (client-only; no networking).
##
## Consumes the protocol `chat_sent` / `chat_log` dictionaries, validates drafts
## with the server's local rules, and composes the exact `chat` / `chat_since`
## wire commands. The owning client owns the socket: feed `set_identity`,
## `set_log`, `apply_chat_sent`, `apply_chat_log`, `set_status`, and `set_error`,
## then send whatever the panel emits on `command_issued`.
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
const DEFAULT_LOCALE := "en"
const SUPPORTED_LOCALES := ["en", "de"]

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

var _box: VBoxContainer
var _title: Label
var _transcript: RichTextLabel
var _status_label: Label
var _draft_edit: LineEdit
var _send_button: Button
var _counter_label: Label
var _help_label: Label


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
	_merge_entries(entries)
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
## Duplicates, out-of-order sequences, and entries past the 64-line cap are
## handled locally; `last_seq` always advances to the highest sequence seen.
func apply_chat_log(payload: Dictionary) -> int:
	var entries = payload.get("entries")
	if not (entries is Array):
		set_error("chat.invalid", _i18n.t("base.ui.chat.error.malformed_log"))
		return 0
	var added := 0
	var malformed := 0
	for entry in entries:
		var normalized := _normalized_entry(entry)
		if normalized.is_empty():
			malformed += 1
			continue
		if _merge_entry(normalized):
			added += 1
	_refresh()
	if malformed > 0 and added == 0:
		set_error("chat.invalid", _i18n.t("base.ui.chat.error.malformed_entries"))
	return added


## Validate and emit one `chat` command. Returns the `draft` result. A rejected
## draft sets the matching error line and emits nothing.
func submit(text: String) -> Dictionary:
	var result := draft(text)
	if not bool(result.get("ok", false)):
		set_error(str(result.get("code", "chat.invalid")), str(result.get("message", "")))
		return result
	clear_error()
	if _draft_edit != null:
		_draft_edit.text = ""
		_refresh_counter()
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

	_refresh_labels()


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
	_refresh_counter()
	_refresh()


func _refresh_counter() -> void:
	if _counter_label == null or _draft_edit == null:
		return
	_counter_label.text = _i18n.t(
		"base.ui.chat.char_counter",
		{"count": _draft_edit.text.length(), "max": MAX_CHAT_CHARS}
	)


func _refresh() -> void:
	if _transcript == null:
		return
	var lines := transcript_lines()
	_transcript.text = (
		_i18n.t("base.ui.chat.empty_log") if lines.is_empty() else "\n".join(lines)
	)
	var has_error := _error_code != ""
	_status_label.text = _status_line()
	_status_label.add_theme_color_override(
		"font_color",
		Color(0.96, 0.42, 0.38) if has_error else Color(0.78, 0.82, 0.88)
	)


func _on_draft_changed(_text: String) -> void:
	_refresh_counter()


func _on_draft_submitted(text: String) -> void:
	submit(text)


func _on_send_pressed() -> void:
	if _draft_edit != null:
		submit(_draft_edit.text)


func _unhandled_key_input(event: InputEvent) -> void:
	if not (event is InputEventKey) or not event.pressed or event.echo:
		return
	var key_event := event as InputEventKey
	if key_event.keycode == KEY_ESCAPE:
		if _draft_edit != null:
			_draft_edit.text = ""
			_refresh_counter()
		clear_error()
		get_viewport().set_input_as_handled()
	elif key_event.keycode == KEY_L and key_event.ctrl_pressed:
		if _draft_edit != null:
			_draft_edit.grab_focus()
		get_viewport().set_input_as_handled()


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


func _merge_entries(entries: Array) -> int:
	var added := 0
	for entry in entries:
		var normalized := _normalized_entry(entry)
		if normalized.is_empty():
			continue
		if _merge_entry(normalized):
			added += 1
	return added


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

	set_error("chat.future", "new server detail")
	_check(str(snapshot()["error_line"]).contains("new server detail"), "error.fallback")
	clear_error()
	set_status("Connected as alice")
	snap = _case("status")
	_check(str(snap["status_line"]) == "Connected as alice", "status.line", str(snap["status_line"]))
	_check(str(snap["error_line"]) == "", "status.no_error")
	set_status("")
	_check(str(snapshot()["status_line"]) == "Signed in as alice (#7)", "status.identity_fallback")
	_case("final")

	_check(_commands.size() == 2, "final.command_count", str(_commands.size()))
	_check(_same_command(_commands[-1], {"type": "chat", "text": "padded"}), "final.last_payload", JSON.stringify(_commands[-1]))
	_check(_same_command(_commands[0], {"type": "chat", "text": "hello world"}), "final.first_payload", JSON.stringify(_commands[0]))

	var checksum := _fnv32("\n".join(_stages))
	if _failures == 0 and _commands.size() == 2:
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
