extends Control

## Fail-closed content-mismatch overlay (client-only; no networking).
##
## When the server rejects `hello` with `content_hash.mismatch`, the client shows
## this screen instead of the generic protocol status text. It is presentation
## only: it displays the server code/message, the client's 64-hex content hash,
## and what to do next. It never reconnects, downloads, substitutes, or saves
## content. Its buttons only emit `retry_requested` / `quit_requested`; the owner
## decides what those mean.
##
## Run:
##   godot --path godot res://content_mismatch_screen.tscn -- --demo
##   godot --headless --path godot res://content_mismatch_screen.tscn -- --smoke
##
## The synthetic `--smoke` path builds two fresh instances, checks the hidden
## initial state, `set_failure`, both button signals, and prints:
##   CONTENT_MISMATCH_UI status=ok runs=2 code=content_hash.mismatch checksum=<8 hex>
##
## See docs/05 - Multiplayer.md §Content-mismatch client screen.

signal retry_requested
signal quit_requested

const TITLE := "Content mismatch"
const INSTRUCTION_TEXT := (
	"Install the server's exact content pack (the pack whose content hash matches "
	+ "the server's pin), then reconnect."
)
const SAFETY_TEXT := (
	"This screen does not download, substitute, or save content. The server "
	+ "refused hello, so no session or world state was loaded."
)
const DEMO_HASH := "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"

var _has_failure := false
var _code := ""
var _message := ""
var _client_hash := ""

var _panel: PanelContainer
var _title_label: Label
var _code_label: Label
var _message_label: Label
var _hash_label: Label
var _instruction_label: Label
var _safety_label: Label
var _retry_button: Button
var _quit_button: Button

# Smoke only: a child instance must build its UI without starting a nested run.
var _suppress_smoke := false

var _smoke_pass := ""
var _smoke_checks := 0
var _smoke_failures := 0
var _smoke_stages: Array = []


func _ready() -> void:
	_build_ui()
	if _suppress_smoke:
		visible = false
		return
	var args := OS.get_cmdline_user_args()
	if args.has("--smoke"):
		if not _has_failure:
			visible = false
		_run_smoke()
		return
	if args.has("--demo"):
		set_failure(
			"content_hash.mismatch",
			"client content registry does not match the server pin",
			DEMO_HASH
		)
		return
	if not _has_failure:
		visible = false


## Store the rejected hello details and show the overlay. Code, message, and
## hash are rendered verbatim; this screen never changes the client's content.
func set_failure(code: String, message: String, client_hash: String) -> void:
	_build_ui()
	_code = code
	_message = message
	_client_hash = client_hash
	_has_failure = true
	visible = true
	_refresh()


## Rendered state for headless tests. Intentionally the whole public contract:
## the panel text is derived from these values.
func snapshot() -> Dictionary:
	return {
		"visible": visible,
		"code": _code,
		"message": _message,
		"client_hash": _client_hash,
	}


## Emit an explicit retry request. Returns false until a failure is showing.
func press_retry() -> bool:
	if not _has_failure or not visible:
		return false
	retry_requested.emit()
	return true


## Emit an explicit quit request. Returns false until a failure is showing.
func press_quit() -> bool:
	if not _has_failure or not visible:
		return false
	quit_requested.emit()
	return true


func _build_ui() -> void:
	if _panel != null:
		return

	set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)

	var background := ColorRect.new()
	background.name = "Background"
	background.color = Color(0.02, 0.025, 0.03, 0.96)
	background.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	background.mouse_filter = Control.MOUSE_FILTER_STOP
	add_child(background)

	var center := CenterContainer.new()
	center.name = "Center"
	center.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	add_child(center)

	_panel = PanelContainer.new()
	_panel.name = "Panel"
	_panel.custom_minimum_size = Vector2(760, 0)
	var style := StyleBoxFlat.new()
	style.bg_color = Color(0.085, 0.095, 0.115)
	style.border_color = Color(0.96, 0.42, 0.38)
	style.set_border_width_all(2)
	style.set_corner_radius_all(10)
	style.set_content_margin_all(28.0)
	_panel.add_theme_stylebox_override("panel", style)
	center.add_child(_panel)

	var box := VBoxContainer.new()
	box.name = "Box"
	box.add_theme_constant_override("separation", 14)
	_panel.add_child(box)

	_title_label = _add_label(box, 30)
	_title_label.add_theme_color_override("font_color", Color(0.98, 0.55, 0.50))

	_code_label = _add_label(box, 16)
	_message_label = _add_label(box, 16)
	_message_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART

	_hash_label = _add_label(box, 16)
	_hash_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART

	box.add_child(HSeparator.new())

	_instruction_label = _add_label(box, 17)
	_instruction_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART

	_safety_label = _add_label(box, 14)
	_safety_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	_safety_label.add_theme_color_override("font_color", Color(0.66, 0.71, 0.78))

	var buttons := HBoxContainer.new()
	buttons.name = "Buttons"
	buttons.alignment = BoxContainer.ALIGNMENT_END
	buttons.add_theme_constant_override("separation", 12)
	box.add_child(buttons)

	_retry_button = _add_button(buttons, "Retry", func() -> void: press_retry())
	_quit_button = _add_button(buttons, "Quit", func() -> void: press_quit())

	_refresh()


func _add_label(parent: VBoxContainer, font_size: int) -> Label:
	var label := Label.new()
	label.add_theme_font_size_override("font_size", font_size)
	parent.add_child(label)
	return label


func _add_button(parent: HBoxContainer, text: String, callback: Callable) -> Button:
	var button := Button.new()
	button.text = text
	button.custom_minimum_size = Vector2(140, 42)
	button.pressed.connect(callback)
	parent.add_child(button)
	return button


func _refresh() -> void:
	if _panel == null:
		return
	_title_label.text = TITLE
	_code_label.text = "Server code: %s" % _display(_code)
	_message_label.text = "Server message: %s" % _display(_message)
	_hash_label.text = "Client content hash: %s" % _display(_client_hash)
	_instruction_label.text = INSTRUCTION_TEXT
	_safety_label.text = SAFETY_TEXT


func _display(value: String) -> String:
	return value if value != "" else "(none)"


func _render_checksum(target) -> String:
	var snap: Dictionary = target.snapshot()
	return _fnv32("\n".join([
		"visible=" + str(snap.get("visible", false)),
		"code=" + str(snap.get("code", "")),
		"message=" + str(snap.get("message", "")),
		"client_hash=" + str(snap.get("client_hash", "")),
		"title=" + str(target._title_label.text),
		"code_line=" + str(target._code_label.text),
		"message_line=" + str(target._message_label.text),
		"hash_line=" + str(target._hash_label.text),
		"instruction=" + str(target._instruction_label.text),
	]))


func _stage(target, name: String) -> void:
	var checksum := _render_checksum(target)
	_smoke_stages.append("%s.%s=%s" % [_smoke_pass, name, checksum])
	print(
		"CONTENT_MISMATCH_UI_STAGE pass=%s stage=%s checksum=%s"
		% [_smoke_pass, name, checksum]
	)


func _run_smoke() -> void:
	_run_smoke_pass(self, "first")

	var second: Node = (get_script() as GDScript).new()
	second.set("_suppress_smoke", true)
	add_child(second)
	_run_smoke_pass(second, "second")

	var checksum := _fnv32("\n".join(_smoke_stages))
	if _smoke_failures == 0:
		print(
			"CONTENT_MISMATCH_UI status=ok runs=2 code=content_hash.mismatch checksum=%s"
			% checksum
		)
		get_tree().quit(0)
		return
	print(
		(
			"CONTENT_MISMATCH_UI status=fail runs=2 code=content_hash.mismatch"
			+ " checks=%d errors=%d checksum=%s"
		)
		% [_smoke_checks, _smoke_failures, checksum]
	)
	get_tree().quit(1)


func _run_smoke_pass(target, pass_name: String) -> void:
	_smoke_pass = pass_name
	var events: Array = []
	target.connect("retry_requested", func() -> void: events.append("retry"))
	target.connect("quit_requested", func() -> void: events.append("quit"))

	_check(not bool(target.snapshot()["visible"]), "initial.hidden")
	_check(str(target.snapshot()["code"]) == "", "initial.code")
	_check(str(target.snapshot()["message"]) == "", "initial.message")
	_check(str(target.snapshot()["client_hash"]) == "", "initial.hash")
	_stage(target, "initial")

	var message := "client content registry does not match the server pin"
	target.set_failure("content_hash.mismatch", message, DEMO_HASH)
	var snap: Dictionary = target.snapshot()
	_check(bool(snap["visible"]), "failure.visible")
	_check(str(snap["code"]) == "content_hash.mismatch", "failure.code", str(snap["code"]))
	_check(str(snap["message"]) == message, "failure.message", str(snap["message"]))
	_check(str(snap["client_hash"]) == DEMO_HASH, "failure.hash", str(snap["client_hash"]))
	_check(_is_hex64(str(snap["client_hash"])), "failure.hash_shape", str(snap["client_hash"]))
	_check(str(target._title_label.text) == TITLE, "failure.title", str(target._title_label.text))
	_check(
		str(target._code_label.text).contains("content_hash.mismatch"),
		"failure.code_line",
		str(target._code_label.text)
	)
	_check(
		str(target._message_label.text).contains(message),
		"failure.message_line",
		str(target._message_label.text)
	)
	_check(
		str(target._hash_label.text).contains(DEMO_HASH),
		"failure.hash_line",
		str(target._hash_label.text)
	)
	_check(
		str(target._instruction_label.text).contains("exact content pack"),
		"failure.instruction",
		str(target._instruction_label.text)
	)
	_check(
		str(target._instruction_label.text).contains("reconnect"),
		"failure.reconnect",
		str(target._instruction_label.text)
	)
	_check(events.is_empty(), "failure.no_auto_signal", str(events))
	_stage(target, "failure")

	target.set_failure("content_hash.mismatch", "updated server detail", DEMO_HASH)
	_check(
		str(target.snapshot()["message"]) == "updated server detail",
		"replacement.message",
		str(target.snapshot()["message"])
	)
	_check(bool(target.snapshot()["visible"]), "replacement.visible")
	_stage(target, "replacement")

	_check(bool(target.press_retry()), "retry.accepted")
	_check(events.size() == 1 and events[0] == "retry", "retry.signal", str(events))
	_check(bool(target.snapshot()["visible"]), "retry.no_auto_hide")
	_stage(target, "retry")

	_check(bool(target.press_quit()), "quit.accepted")
	_check(events.size() == 2 and events[1] == "quit", "quit.signal", str(events))
	_check(bool(target.snapshot()["visible"]), "quit.no_auto_hide")
	_stage(target, "quit")


func _check(condition: bool, label: String, detail: String = "") -> void:
	_smoke_checks += 1
	if condition:
		return
	_smoke_failures += 1
	print("CONTENT_MISMATCH_UI_FAIL pass=%s %s %s" % [_smoke_pass, label, detail])


func _is_hex64(value: String) -> bool:
	if value.length() != 64:
		return false
	for index in range(value.length()):
		var character := value[index]
		if not (
			(character >= "0" and character <= "9")
			or (character >= "a" and character <= "f")
		):
			return false
	return true


func _fnv32(text: String) -> String:
	var hash := 2166136261
	for byte in text.to_utf8_buffer():
		hash = (hash ^ int(byte)) & 0xffffffff
		hash = (hash * 16777619) & 0xffffffff
	return "%08x" % hash
