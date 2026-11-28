extends Control

## Minimal front-end shell for the Godot project (docs/86).
##
## `main_menu.tscn` is the project's `run/main_scene`. It is presentation only:
## no save slots and no profile. Every entry opens an existing scene. The one
## thing it persists is the master-volume settings overlay
## (`user://settings.cfg`, see `menu_settings.gd`); everything else is read
## only.
##
## Forwarding: the existing scene smokes run `godot --path godot -- --smoke`
## and rely on `main.tscn` being the entry point. When the user args contain
## one of `FORWARD_FLAGS`, this menu changes to `main.tscn` immediately and
## leaves the args untouched, so every existing smoke keeps working.
##
## Membership:
##   New City        -> world_placement.tscn (the existing placement preview)
##   Blueprint Editor-> editor.tscn
##   Terrain Preview -> main.tscn (Phase 0 terrain demo)
##   Multiplayer     -> mp_world_client.tscn (needs a running server)
##   Settings        -> master-volume and mute overlay
##   Quit            -> closes the game
##
## Controls: Up/Down or D-pad moves focus, Left/Right (or the left stick, or
## the mouse) changes the volume, Enter/A activates, Esc/B cancels and closes
## the settings overlay or returns focus to the first item. A visible ring
## marks the focused control.
##
## Screen reader: every title, subtitle, menu button, hint, footer, and
## settings control gets a localized `accessibility_name` and a bounded
## `accessibility_description`. The hint label is a polite live region (it
## changes with focus); the settings status line is assertive (a save failure
## must be announced immediately). Semantics are presentation only: they add
## no control, no focus stop, and no visual element.
##
## Run:
##   godot --path godot
##   godot --headless --path godot -- --menu-smoke
##
## `--menu-smoke` prints deterministic lines and quits:
##   MAIN_MENU status=ok buttons=6 focus=new_city checks=N checksum=<8 hex>
##   MAIN_MENU_SETTINGS status=ok volume=55 muted=true checks=N
##   MAIN_MENU_A11Y status=ok checks=N
##
## The settings line is printed against an isolated
## `user://settings_smoke.cfg`, so a developer's real settings never move the
## marker and the file is removed again before quitting. The accessibility
## checks have their own counter, so the menu and settings markers (including
## the pinned `checksum=3d4a58ba`) do not move.
##
## See docs/86.

const WorldI18n = preload("res://world_i18n.gd")
const MenuSettings = preload("res://menu_settings.gd")

const MAIN_SCENE := "res://main.tscn"
## User args that must keep starting the terrain/placement scene directly.
const FORWARD_FLAGS := ["--smoke", "--volume-smoke", "--world-placement-smoke"]

const ITEMS := [
	{
		"id": "new_city",
		"label": "base.ui.menu.new_city",
		"hint": "base.ui.menu.hint.new_city",
		"scene": "res://world_placement.tscn",
	},
	{
		"id": "blueprint_editor",
		"label": "base.ui.menu.blueprint_editor",
		"hint": "base.ui.menu.hint.blueprint_editor",
		"scene": "res://editor.tscn",
	},
	{
		"id": "terrain_preview",
		"label": "base.ui.menu.terrain_preview",
		"hint": "base.ui.menu.hint.terrain_preview",
		"scene": "res://main.tscn",
	},
	{
		"id": "multiplayer",
		"label": "base.ui.menu.multiplayer",
		"hint": "base.ui.menu.hint.multiplayer",
		"scene": "res://mp_world_client.tscn",
	},
	{
		"id": "settings",
		"label": "base.ui.menu.settings",
		"hint": "base.ui.menu.hint.settings",
		"scene": "",
	},
	{
		"id": "quit",
		"label": "base.ui.menu.quit",
		"hint": "base.ui.menu.hint.quit",
		"scene": "",
	},
]

## `Control.accessibility_live` values (Godot 4.7 exposes the property but not
## named enum constants). The hint line is polite so focus changes announce
## the new destination; the settings status is assertive so a save failure is
## immediate.
const A11Y_LIVE_OFF := 0
const A11Y_LIVE_POLITE := 1
const A11Y_LIVE_ASSERTIVE := 2
## Descriptions are summaries, not transcripts: bound them so a long localized
## string never turns into a wall of speech.
const A11Y_DESCRIPTION_MAX := 160

const TITLE_COLOR := Color(0.92, 0.95, 1.0)
const TEXT_COLOR := Color(0.86, 0.9, 0.95)
const MUTED_COLOR := Color(0.62, 0.68, 0.76)
const ACCENT_COLOR := Color(0.98, 0.78, 0.25)
const BACKGROUND_COLOR := Color(0.06, 0.08, 0.11)
const BUTTON_WIDTH := 460.0
const BUTTON_HEIGHT := 46.0
const SETTINGS_WIDTH := 440.0

var _i18n
var _smoke_mode := false
var _settings_path := MenuSettings.SETTINGS_PATH
var _volume := MenuSettings.DEFAULT_VOLUME
var _muted := MenuSettings.DEFAULT_MUTED

var _title_label: Label
var _subtitle_label: Label
var _hint_label: Label
var _footer_label: Label
var _buttons: Array[Button] = []
var _ids := PackedStringArray()

var _settings_overlay: Control
var _settings_title: Label
var _settings_body: Label
var _settings_volume_label: Label
var _settings_volume_value: Label
var _settings_slider: HSlider
var _settings_mute: CheckButton
var _settings_save: Button
var _settings_cancel: Button
var _settings_status: Label
var _open_volume := MenuSettings.DEFAULT_VOLUME
var _open_muted := MenuSettings.DEFAULT_MUTED
var _applying_settings := false

var _focus_ring: Panel
var _ring_target: Control
var _focus_index := 0


func _ready() -> void:
	_i18n = WorldI18n.new()
	_load_settings()
	_build_ui()
	_apply_locale(_i18n.locale())
	resized.connect(_update_focus_ring)

	var args := OS.get_cmdline_user_args()
	if forward_target(args) != "":
		# Deferred: Godot forbids replacing the current scene from inside its
		# own _ready, and the deferred call keeps the existing smokes unchanged.
		call_deferred("_forward_to_main")
		return
	if args.has("--menu-smoke"):
		_smoke_mode = true
		_run_smoke()
		return

	_set_focus(0)
	_update_focus_ring()
	call_deferred("_update_focus_ring")


## Scene the menu should hand over to for these user args, or "" when it should
## stay at the menu. Kept pure so the smoke can test the forwarding contract
## without changing scenes.
static func forward_target(args: PackedStringArray) -> String:
	for flag in FORWARD_FLAGS:
		if args.has(flag):
			return MAIN_SCENE
	return ""


func _forward_to_main() -> void:
	var error := get_tree().change_scene_to_file(MAIN_SCENE)
	if error != OK:
		push_error("main_menu: could not open %s (error %d)" % [MAIN_SCENE, error])
		get_tree().quit(1)


func _build_ui() -> void:
	var background := ColorRect.new()
	background.name = "Background"
	background.color = BACKGROUND_COLOR
	background.mouse_filter = Control.MOUSE_FILTER_IGNORE
	background.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	add_child(background)

	var center := CenterContainer.new()
	center.name = "Center"
	center.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	add_child(center)

	var panel := PanelContainer.new()
	panel.name = "MenuPanel"
	panel.add_theme_stylebox_override("panel", _panel_style())
	center.add_child(panel)

	var margin := MarginContainer.new()
	margin.add_theme_constant_override("margin_left", 36)
	margin.add_theme_constant_override("margin_top", 30)
	margin.add_theme_constant_override("margin_right", 36)
	margin.add_theme_constant_override("margin_bottom", 26)
	panel.add_child(margin)

	var column := VBoxContainer.new()
	column.add_theme_constant_override("separation", 10)
	margin.add_child(column)

	_title_label = Label.new()
	_title_label.name = "Title"
	_title_label.add_theme_font_size_override("font_size", 46)
	_title_label.add_theme_color_override("font_color", TITLE_COLOR)
	_title_label.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
	column.add_child(_title_label)

	_subtitle_label = Label.new()
	_subtitle_label.name = "Subtitle"
	_subtitle_label.add_theme_font_size_override("font_size", 14)
	_subtitle_label.add_theme_color_override("font_color", MUTED_COLOR)
	_subtitle_label.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
	column.add_child(_subtitle_label)

	var gap := Control.new()
	gap.custom_minimum_size = Vector2(0, 14)
	column.add_child(gap)

	var buttons_box := VBoxContainer.new()
	buttons_box.name = "Buttons"
	buttons_box.add_theme_constant_override("separation", 8)
	column.add_child(buttons_box)

	for index in ITEMS.size():
		var item: Dictionary = ITEMS[index]
		var button := Button.new()
		button.name = "Item_" + str(item["id"])
		button.custom_minimum_size = Vector2(BUTTON_WIDTH, BUTTON_HEIGHT)
		button.alignment = HORIZONTAL_ALIGNMENT_CENTER
		button.focus_mode = Control.FOCUS_ALL
		button.add_theme_font_size_override("font_size", 20)
		button.add_theme_color_override("font_color", TEXT_COLOR)
		button.add_theme_color_override("font_hover_color", TITLE_COLOR)
		button.add_theme_color_override("font_focus_color", TITLE_COLOR)
		button.add_theme_stylebox_override("focus", StyleBoxEmpty.new())
		button.add_theme_stylebox_override("normal", _button_style(Color(0.13, 0.16, 0.21)))
		button.add_theme_stylebox_override("hover", _button_style(Color(0.18, 0.23, 0.3)))
		button.add_theme_stylebox_override("pressed", _button_style(Color(0.2, 0.26, 0.34)))
		button.set_meta("menu_id", str(item["id"]))
		button.focus_entered.connect(_on_button_focus.bind(index))
		button.pressed.connect(_on_button_pressed.bind(str(item["id"])))
		buttons_box.add_child(button)
		_buttons.append(button)
		_ids.append(str(item["id"]))

	# Vertical focus wraps; the engine's ui_up/ui_down navigation uses these.
	for index in _buttons.size():
		var previous := _buttons[(index - 1 + _buttons.size()) % _buttons.size()]
		var next := _buttons[(index + 1) % _buttons.size()]
		_buttons[index].focus_neighbor_top = previous.get_path()
		_buttons[index].focus_neighbor_bottom = next.get_path()

	var hint_gap := Control.new()
	hint_gap.custom_minimum_size = Vector2(0, 12)
	column.add_child(hint_gap)

	_hint_label = Label.new()
	_hint_label.name = "Hint"
	_hint_label.custom_minimum_size = Vector2(BUTTON_WIDTH, 36)
	_hint_label.add_theme_font_size_override("font_size", 13)
	_hint_label.add_theme_color_override("font_color", MUTED_COLOR)
	_hint_label.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
	_hint_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	column.add_child(_hint_label)

	_footer_label = Label.new()
	_footer_label.name = "Footer"
	_footer_label.custom_minimum_size = Vector2(BUTTON_WIDTH, 20)
	_footer_label.add_theme_font_size_override("font_size", 12)
	_footer_label.add_theme_color_override("font_color", Color(0.45, 0.5, 0.58))
	_footer_label.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
	_footer_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	column.add_child(_footer_label)

	_build_settings_overlay()

	_focus_ring = Panel.new()
	_focus_ring.name = "FocusRing"
	_focus_ring.mouse_filter = Control.MOUSE_FILTER_IGNORE
	_focus_ring.z_index = 20
	_focus_ring.visible = false
	var ring_style := StyleBoxFlat.new()
	ring_style.draw_center = false
	ring_style.bg_color = Color(0, 0, 0, 0)
	ring_style.border_color = ACCENT_COLOR
	ring_style.border_width_left = 3
	ring_style.border_width_top = 3
	ring_style.border_width_right = 3
	ring_style.border_width_bottom = 3
	ring_style.corner_radius_top_left = 8
	ring_style.corner_radius_top_right = 8
	ring_style.corner_radius_bottom_left = 8
	ring_style.corner_radius_bottom_right = 8
	_focus_ring.add_theme_stylebox_override("panel", ring_style)
	add_child(_focus_ring)


func _build_settings_overlay() -> void:
	_settings_overlay = Control.new()
	_settings_overlay.name = "SettingsOverlay"
	_settings_overlay.visible = false
	_settings_overlay.mouse_filter = Control.MOUSE_FILTER_STOP
	_settings_overlay.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	add_child(_settings_overlay)

	var dim := ColorRect.new()
	dim.name = "Dim"
	dim.color = Color(0, 0, 0, 0.55)
	dim.mouse_filter = Control.MOUSE_FILTER_STOP
	dim.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	_settings_overlay.add_child(dim)

	var center := CenterContainer.new()
	center.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	_settings_overlay.add_child(center)

	var panel := PanelContainer.new()
	panel.add_theme_stylebox_override("panel", _panel_style())
	center.add_child(panel)

	var margin := MarginContainer.new()
	margin.add_theme_constant_override("margin_left", 28)
	margin.add_theme_constant_override("margin_top", 24)
	margin.add_theme_constant_override("margin_right", 28)
	margin.add_theme_constant_override("margin_bottom", 22)
	panel.add_child(margin)

	var column := VBoxContainer.new()
	column.add_theme_constant_override("separation", 12)
	margin.add_child(column)

	_settings_title = Label.new()
	_settings_title.name = "SettingsTitle"
	_settings_title.add_theme_font_size_override("font_size", 26)
	_settings_title.add_theme_color_override("font_color", TITLE_COLOR)
	column.add_child(_settings_title)

	_settings_body = Label.new()
	_settings_body.name = "SettingsBody"
	_settings_body.custom_minimum_size = Vector2(SETTINGS_WIDTH, 0)
	_settings_body.add_theme_font_size_override("font_size", 14)
	_settings_body.add_theme_color_override("font_color", MUTED_COLOR)
	_settings_body.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	column.add_child(_settings_body)

	var volume_row := HBoxContainer.new()
	volume_row.name = "VolumeRow"
	volume_row.custom_minimum_size = Vector2(SETTINGS_WIDTH, 0)
	column.add_child(volume_row)

	_settings_volume_label = Label.new()
	_settings_volume_label.name = "VolumeLabel"
	_settings_volume_label.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	_settings_volume_label.add_theme_font_size_override("font_size", 16)
	_settings_volume_label.add_theme_color_override("font_color", TEXT_COLOR)
	volume_row.add_child(_settings_volume_label)

	_settings_volume_value = Label.new()
	_settings_volume_value.name = "VolumeValue"
	_settings_volume_value.add_theme_font_size_override("font_size", 16)
	_settings_volume_value.add_theme_color_override("font_color", ACCENT_COLOR)
	_settings_volume_value.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT
	volume_row.add_child(_settings_volume_value)

	_settings_slider = HSlider.new()
	_settings_slider.name = "VolumeSlider"
	_settings_slider.custom_minimum_size = Vector2(SETTINGS_WIDTH, 26)
	_settings_slider.min_value = MenuSettings.MIN_VOLUME
	_settings_slider.max_value = MenuSettings.MAX_VOLUME
	_settings_slider.step = 1
	_settings_slider.value = _volume
	_settings_slider.focus_mode = Control.FOCUS_ALL
	_settings_slider.focus_entered.connect(_on_overlay_focus.bind(_settings_slider))
	_settings_slider.value_changed.connect(_on_volume_changed)
	column.add_child(_settings_slider)

	_settings_mute = CheckButton.new()
	_settings_mute.name = "Mute"
	_settings_mute.button_pressed = _muted
	_settings_mute.focus_mode = Control.FOCUS_ALL
	_settings_mute.add_theme_font_size_override("font_size", 16)
	_settings_mute.add_theme_color_override("font_color", TEXT_COLOR)
	_settings_mute.add_theme_color_override("font_hover_color", TITLE_COLOR)
	_settings_mute.add_theme_color_override("font_focus_color", TITLE_COLOR)
	_settings_mute.add_theme_stylebox_override("focus", StyleBoxEmpty.new())
	_settings_mute.focus_entered.connect(_on_overlay_focus.bind(_settings_mute))
	_settings_mute.toggled.connect(_on_muted_toggled)
	column.add_child(_settings_mute)

	_settings_status = Label.new()
	_settings_status.name = "SettingsStatus"
	_settings_status.custom_minimum_size = Vector2(SETTINGS_WIDTH, 18)
	_settings_status.add_theme_font_size_override("font_size", 12)
	_settings_status.add_theme_color_override("font_color", ACCENT_COLOR)
	_settings_status.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	column.add_child(_settings_status)

	var buttons_row := HBoxContainer.new()
	buttons_row.name = "SettingsButtons"
	buttons_row.alignment = BoxContainer.ALIGNMENT_END
	buttons_row.add_theme_constant_override("separation", 10)
	column.add_child(buttons_row)

	_settings_cancel = _settings_button("SettingsCancel")
	_settings_cancel.pressed.connect(_on_settings_cancel)
	buttons_row.add_child(_settings_cancel)

	_settings_save = _settings_button("SettingsSave")
	_settings_save.pressed.connect(_on_settings_save)
	buttons_row.add_child(_settings_save)

	# Focus wraps inside the overlay so D-pad cannot fall through to the menu
	# behind the dim layer. The slider keeps Left/Right for its own value.
	_settings_slider.focus_neighbor_top = _settings_cancel.get_path()
	_settings_slider.focus_neighbor_bottom = _settings_mute.get_path()
	_settings_mute.focus_neighbor_top = _settings_slider.get_path()
	_settings_mute.focus_neighbor_bottom = _settings_cancel.get_path()
	_settings_cancel.focus_neighbor_top = _settings_mute.get_path()
	_settings_cancel.focus_neighbor_bottom = _settings_save.get_path()
	_settings_cancel.focus_neighbor_left = _settings_save.get_path()
	_settings_cancel.focus_neighbor_right = _settings_save.get_path()
	_settings_save.focus_neighbor_top = _settings_cancel.get_path()
	_settings_save.focus_neighbor_bottom = _settings_slider.get_path()
	_settings_save.focus_neighbor_left = _settings_cancel.get_path()
	_settings_save.focus_neighbor_right = _settings_cancel.get_path()


func _settings_button(node_name: String) -> Button:
	var button := Button.new()
	button.name = node_name
	button.custom_minimum_size = Vector2(140, 40)
	button.focus_mode = Control.FOCUS_ALL
	button.add_theme_font_size_override("font_size", 16)
	button.add_theme_color_override("font_color", TEXT_COLOR)
	button.add_theme_color_override("font_hover_color", TITLE_COLOR)
	button.add_theme_color_override("font_focus_color", TITLE_COLOR)
	button.add_theme_stylebox_override("focus", StyleBoxEmpty.new())
	button.add_theme_stylebox_override("normal", _button_style(Color(0.13, 0.16, 0.21)))
	button.add_theme_stylebox_override("hover", _button_style(Color(0.18, 0.23, 0.3)))
	button.add_theme_stylebox_override("pressed", _button_style(Color(0.2, 0.26, 0.34)))
	button.focus_entered.connect(_on_overlay_focus.bind(button))
	return button


func _panel_style() -> StyleBoxFlat:
	var style := StyleBoxFlat.new()
	style.bg_color = Color(0.1, 0.12, 0.16, 0.97)
	style.border_color = Color(0.24, 0.29, 0.37)
	style.border_width_left = 1
	style.border_width_top = 1
	style.border_width_right = 1
	style.border_width_bottom = 1
	style.corner_radius_top_left = 10
	style.corner_radius_top_right = 10
	style.corner_radius_bottom_left = 10
	style.corner_radius_bottom_right = 10
	return style


func _button_style(color: Color) -> StyleBoxFlat:
	var style := StyleBoxFlat.new()
	style.bg_color = color
	style.border_color = Color(0.28, 0.34, 0.43)
	style.border_width_left = 1
	style.border_width_top = 1
	style.border_width_right = 1
	style.border_width_bottom = 1
	style.corner_radius_top_left = 6
	style.corner_radius_top_right = 6
	style.corner_radius_bottom_left = 6
	style.corner_radius_bottom_right = 6
	return style


func _apply_locale(locale: String) -> String:
	var resolved: String = _i18n.set_locale(locale)
	_title_label.text = _i18n.t("base.ui.menu.title")
	_subtitle_label.text = _i18n.t("base.ui.menu.subtitle")
	for index in _buttons.size():
		_buttons[index].text = _i18n.t(str(ITEMS[index]["label"]))
	_settings_title.text = _i18n.t("base.ui.menu.settings.title")
	_settings_body.text = _i18n.t("base.ui.menu.settings.body")
	_settings_volume_label.text = _i18n.t("base.ui.menu.settings.volume")
	_settings_mute.text = _i18n.t("base.ui.menu.settings.mute")
	_settings_save.text = _i18n.t("base.ui.menu.settings.save")
	_settings_cancel.text = _i18n.t("base.ui.menu.settings.cancel")
	if not _settings_status.text.is_empty():
		_settings_status.text = _i18n.t("base.ui.menu.settings.save_failed")
	_update_volume_value()
	_footer_label.text = _i18n.t("base.ui.menu.footer")
	_refresh_hint()
	return resolved


func _refresh_hint() -> void:
	if _hint_label == null or _focus_index < 0 or _focus_index >= ITEMS.size():
		return
	_hint_label.text = _i18n.t(str(ITEMS[_focus_index]["hint"]))
	_refresh_accessibility()


## Localized screen-reader names, bounded descriptions, and live modes for
## every visible part of the menu and its settings overlay. The overlay is
## labelled even while closed, so the semantics are ready before it opens.
func _refresh_accessibility() -> void:
	if _title_label == null or _i18n == null:
		return
	_a11y_set(_title_label, _i18n.t("base.ui.menu.title"), _i18n.t("base.ui.menu.a11y.menu"))
	_a11y_set(
		_subtitle_label,
		_i18n.t("base.ui.menu.subtitle"),
		_i18n.t("base.ui.menu.a11y.menu")
	)
	for index in _buttons.size():
		var item: Dictionary = ITEMS[index]
		_a11y_set(
			_buttons[index],
			_buttons[index].text,
			_i18n.t(str(item["hint"]))
		)
	_a11y_set(
		_hint_label,
		_i18n.t("base.ui.menu.a11y.hint"),
		_hint_label.text
	)
	_hint_label.accessibility_live = A11Y_LIVE_POLITE
	_a11y_set(
		_footer_label,
		_i18n.t("base.ui.menu.a11y.footer"),
		_footer_label.text
	)

	if _settings_title == null:
		return
	_a11y_set(
		_settings_title,
		_i18n.t("base.ui.menu.settings.title"),
		_settings_body.text
	)
	_a11y_set(
		_settings_body,
		_i18n.t("base.ui.menu.a11y.settings_body"),
		_settings_body.text
	)
	_a11y_set(
		_settings_volume_label,
		_settings_volume_label.text,
		_i18n.t("base.ui.menu.a11y.volume_hint")
	)
	_a11y_set(
		_settings_volume_value,
		_i18n.t("base.ui.menu.a11y.volume_value"),
		_settings_volume_value.text
	)
	_a11y_set(
		_settings_slider,
		_settings_volume_label.text,
		_i18n.t("base.ui.menu.a11y.volume_hint")
	)
	_a11y_set(
		_settings_mute,
		_settings_mute.text,
		_i18n.t("base.ui.menu.a11y.mute_hint")
	)
	_a11y_set(
		_settings_save,
		_settings_save.text,
		_i18n.t("base.ui.menu.a11y.save_hint")
	)
	_a11y_set(
		_settings_cancel,
		_settings_cancel.text,
		_i18n.t("base.ui.menu.a11y.cancel_hint")
	)
	_a11y_set(
		_settings_status,
		_i18n.t("base.ui.menu.a11y.status"),
		(
			_settings_status.text
			if not _settings_status.text.is_empty()
			else _i18n.t("base.ui.menu.a11y.status_idle")
		)
	)
	_settings_status.accessibility_live = A11Y_LIVE_ASSERTIVE


## Assign a bounded name/description pair. Null controls are ignored so the
## helper is safe during construction order changes.
func _a11y_set(control: Control, name_text: String, description_text: String) -> void:
	if control == null:
		return
	control.accessibility_name = _a11y_clamp(name_text)
	control.accessibility_description = _a11y_clamp(description_text)


func _a11y_clamp(text: String) -> String:
	var trimmed := text.strip_edges()
	if trimmed.length() <= A11Y_DESCRIPTION_MAX:
		return trimmed
	return trimmed.substr(0, A11Y_DESCRIPTION_MAX).strip_edges()


## Read-only screen-reader model for the headless smoke. Entries carry the
## same name/description/role/live/reachable shape as the other UI slices.
func accessibility_snapshot() -> Dictionary:
	var buttons: Array = []
	for index in _buttons.size():
		var entry := _a11y_entry(_buttons[index], "button")
		entry["id"] = _ids[index]
		buttons.append(entry)
	return {
		"locale": _i18n.locale() if _i18n != null else "",
		"title": _a11y_entry(_title_label, "heading"),
		"subtitle": _a11y_entry(_subtitle_label, "label"),
		"buttons": buttons,
		"hint": _a11y_entry(_hint_label, "status"),
		"footer": _a11y_entry(_footer_label, "label"),
		"settings": {
			"title": _a11y_entry(_settings_title, "heading"),
			"body": _a11y_entry(_settings_body, "group"),
			"volume_label": _a11y_entry(_settings_volume_label, "label"),
			"volume_value": _a11y_entry(_settings_volume_value, "status"),
			"slider": _a11y_entry(_settings_slider, "slider"),
			"mute": _a11y_entry(_settings_mute, "checkbox"),
			"save": _a11y_entry(_settings_save, "button"),
			"cancel": _a11y_entry(_settings_cancel, "button"),
			"status": _a11y_entry(_settings_status, "status"),
		},
		"live": {
			"hint": _a11y_live_name(_hint_label),
			"settings_status": _a11y_live_name(_settings_status),
		},
		"menu_order": _ids.duplicate(),
	}


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
		"live": _a11y_live_name(control),
		"visible": control.visible,
		"focus_mode": focus_mode,
		"reachable": (
			control.is_inside_tree()
			and control.visible
			and control.focus_mode == Control.FOCUS_ALL
		),
	}


func _a11y_live_name(control: Control) -> String:
	if control == null:
		return "off"
	if control.accessibility_live == A11Y_LIVE_POLITE:
		return "polite"
	if control.accessibility_live == A11Y_LIVE_ASSERTIVE:
		return "assertive"
	return "off"


## Whether every name/description resolved and none leaked a raw
## `!base.ui.*!` fallback. Walks the nested snapshot.
func _a11y_no_leak(value) -> bool:
	if value is Array:
		for child in value:
			if not _a11y_no_leak(child):
				return false
		return true
	if value is Dictionary:
		for key in value:
			if str(key) == "name" or str(key) == "description":
				if str(value[key]).contains("!base.ui."):
					return false
			elif not _a11y_no_leak(value[key]):
				return false
		return true
	return true


func _a11y_descriptions_bounded(value) -> bool:
	if value is Array:
		for child in value:
			if not _a11y_descriptions_bounded(child):
				return false
		return true
	if value is Dictionary:
		for key in value:
			if str(key) == "description":
				if str(value[key]).length() > A11Y_DESCRIPTION_MAX:
					return false
			elif not _a11y_descriptions_bounded(value[key]):
				return false
		return true
	return true


func _target_scene(item_id: String) -> String:
	for item in ITEMS:
		if str(item["id"]) == item_id:
			return str(item["scene"])
	return ""


func _index_of(item_id: String) -> int:
	return _ids.find(item_id)


func _set_focus(index: int) -> void:
	if _buttons.is_empty():
		return
	_focus_index = wrapi(index, 0, _buttons.size())
	_ring_target = _buttons[_focus_index]
	_buttons[_focus_index].grab_focus()
	_refresh_hint()
	_update_focus_ring()
	call_deferred("_update_focus_ring")


func _on_button_focus(index: int) -> void:
	_focus_index = index
	_ring_target = _buttons[index]
	_refresh_hint()
	_update_focus_ring()


func _on_overlay_focus(control: Control) -> void:
	_ring_target = control
	_update_focus_ring()


func _on_button_pressed(item_id: String) -> void:
	if _smoke_mode:
		return
	if item_id == "settings":
		_open_settings()
		return
	if item_id == "quit":
		get_tree().quit(0)
		return
	var scene := _target_scene(item_id)
	if scene.is_empty():
		return
	var error := get_tree().change_scene_to_file(scene)
	if error != OK:
		push_error("main_menu: could not open %s (error %d)" % [scene, error])


func _load_settings() -> void:
	var state := MenuSettings.load_from(_settings_path)
	_volume = int(state["volume"])
	_muted = bool(state["muted"])
	MenuSettings.apply(_volume, _muted)


func _set_menu_focusable(enabled: bool) -> void:
	for button in _buttons:
		button.focus_mode = Control.FOCUS_ALL if enabled else Control.FOCUS_NONE


func _update_volume_value() -> void:
	if _settings_volume_value == null:
		return
	_settings_volume_value.text = _i18n.t(
		"base.ui.menu.settings.volume_value", {"value": _i18n.format_int(_volume)}
	)
	_refresh_accessibility()


func _on_volume_changed(value: float) -> void:
	_volume = MenuSettings.sanitize_volume(value)
	_update_volume_value()
	if _applying_settings:
		return
	MenuSettings.apply(_volume, _muted)


func _on_muted_toggled(pressed: bool) -> void:
	_muted = pressed
	if _applying_settings:
		return
	MenuSettings.apply(_volume, _muted)


func _open_settings() -> void:
	if _settings_overlay == null:
		return
	_open_volume = _volume
	_open_muted = _muted
	_applying_settings = true
	_settings_slider.value = _volume
	_settings_mute.button_pressed = _muted
	_applying_settings = false
	_settings_status.text = ""
	_update_volume_value()
	_refresh_accessibility()
	_settings_overlay.visible = true
	_set_menu_focusable(false)
	_ring_target = _settings_slider
	_settings_slider.grab_focus()
	_update_focus_ring()
	call_deferred("_update_focus_ring")


func _close_settings() -> void:
	if _settings_overlay == null:
		return
	_settings_overlay.visible = false
	_set_menu_focusable(true)
	_set_focus(_index_of("settings"))


func _on_settings_save() -> void:
	var error := MenuSettings.save_to(_settings_path, _volume, _muted)
	if error != OK:
		_settings_status.text = _i18n.t("base.ui.menu.settings.save_failed")
		_refresh_accessibility()
		return
	_close_settings()


## Cancel restores the values snapshotted when the overlay opened, including
## the live preview, then leaves the menu exactly where it was.
func _on_settings_cancel() -> void:
	_volume = _open_volume
	_muted = _open_muted
	MenuSettings.apply(_volume, _muted)
	_close_settings()


## Esc / B: cancel the settings overlay, otherwise return focus to the first
## item. The menu is the root of the scene stack, so there is no scene to go
## back to.
func _back() -> void:
	if _settings_overlay != null and _settings_overlay.visible:
		_on_settings_cancel()
		return
	_set_focus(0)


func _unhandled_input(event: InputEvent) -> void:
	if _settings_overlay != null and event.is_action_pressed("ui_cancel"):
		_back()
		get_viewport().set_input_as_handled()


func _update_focus_ring() -> void:
	if _focus_ring == null or _ring_target == null or not is_inside_tree():
		return
	if not _ring_target.is_inside_tree() or not _ring_target.is_visible_in_tree():
		_focus_ring.visible = false
		return
	var target_rect := _ring_target.get_global_rect().grow(2.0)
	var root_rect := get_global_rect()
	_focus_ring.position = target_rect.position - root_rect.position
	_focus_ring.size = target_rect.size
	_focus_ring.visible = target_rect.size.x > 0.0 and target_rect.size.y > 0.0


func _fnv1a32(text: String) -> String:
	var hash := 0x811c9dc5
	for byte in text.to_utf8_buffer():
		hash ^= int(byte)
		hash = (hash * 0x01000193) & 0xffffffff
	return "%08x" % hash


# --- smoke -----------------------------------------------------------------

func _run_smoke() -> void:
	var checks := 0
	var failures := PackedStringArray()

	# Let the containers solve their layout before checking ring geometry.
	await get_tree().process_frame
	await get_tree().process_frame

	checks += _smoke_check(_buttons.size() == 6, "buttons=%d" % _buttons.size(), failures)
	checks += _smoke_check(_ids == PackedStringArray([
		"new_city", "blueprint_editor", "terrain_preview",
		"multiplayer", "settings", "quit",
	]), "id_order=%s" % ",".join(_ids), failures)

	_apply_locale("en")
	var expected_labels := [
		"New City", "Blueprint Editor", "Terrain Preview",
		"Multiplayer", "Settings", "Quit",
	]
	var labels_ok := true
	for index in expected_labels.size():
		if _buttons[index].text != expected_labels[index]:
			labels_ok = false
	checks += _smoke_check(labels_ok, "en_labels", failures)
	checks += _smoke_check(_title_label.text == "Townsmith", "en_title", failures)

	_apply_locale("de")
	checks += _smoke_check(
		_buttons[0].text == "Neue Stadt" and _buttons[5].text == "Beenden",
		"de_labels",
		failures
	)
	checks += _smoke_check(_footer_label.text.contains("Esc"), "de_footer", failures)

	checks += _smoke_check(
		_target_scene("new_city") == "res://world_placement.tscn"
		and _target_scene("blueprint_editor") == "res://editor.tscn"
		and _target_scene("terrain_preview") == "res://main.tscn"
		and _target_scene("multiplayer") == "res://mp_world_client.tscn",
		"targets",
		failures
	)
	checks += _smoke_check(
		_target_scene("settings").is_empty() and _target_scene("quit").is_empty(),
		"non_scene_items",
		failures
	)

	# Focus starts on the first item and the ring tracks it.
	_set_focus(0)
	var first_rect := _buttons[0].get_global_rect()
	checks += _smoke_check(
		_focus_index == 0
		and _focus_ring.visible
		and absf(_focus_ring.size.x - (first_rect.size.x + 4.0)) <= 2.0
		and absf(_focus_ring.size.y - (first_rect.size.y + 4.0)) <= 2.0,
		"focus_ring",
		failures
	)

	# Vertical focus wraps in both directions.
	_set_focus(-1)
	var wrapped_up := _focus_index == 5
	_set_focus(6)
	var wrapped_down := _focus_index == 0
	checks += _smoke_check(wrapped_up and wrapped_down, "focus_wrap", failures)
	checks += _smoke_check(
		_buttons[0].focus_neighbor_top == _buttons[5].get_path()
		and _buttons[5].focus_neighbor_bottom == _buttons[0].get_path(),
		"focus_neighbors",
		failures
	)

	# Multiplayer hint names the server requirement.
	_set_focus(_index_of("multiplayer"))
	checks += _smoke_check(
		_hint_label.text.contains("server") and _hint_label.text.contains("listen"),
		"multiplayer_hint",
		failures
	)

	# Settings overlay: slider default, live preview, cancel restores, save
	# persists, and the values clamp.
	_settings_path = "user://settings_smoke.cfg"
	var absolute_settings := ProjectSettings.globalize_path(_settings_path)
	if FileAccess.file_exists(_settings_path):
		DirAccess.remove_absolute(absolute_settings)
	_load_settings()
	_open_settings()
	var opened := (
		_settings_overlay.visible
		and _ring_target == _settings_slider
		and _focus_ring.visible
		and _settings_slider.value == 100.0
	)
	checks += _smoke_check(opened, "settings_open", failures)
	_settings_slider.value = 37.0
	_settings_mute.button_pressed = true
	_on_settings_save()
	checks += _smoke_check(
		not _settings_overlay.visible
		and _focus_index == _index_of("settings")
		and _volume == 37
		and _muted,
		"settings_save",
		failures
	)
	_open_settings()
	_settings_slider.value = 88.0
	_back()
	checks += _smoke_check(
		not _settings_overlay.visible and _volume == 37 and _muted,
		"settings_cancel",
		failures
	)
	_load_settings()
	checks += _smoke_check(
		_volume == 37 and _muted and MenuSettings.sanitize_volume(150) == 100
		and MenuSettings.sanitize_volume(-10) == 0
		and MenuSettings.sanitize_volume(37.6) == 38,
		"settings_load_clamp",
		failures
	)
	if FileAccess.file_exists(_settings_path):
		DirAccess.remove_absolute(absolute_settings)
	print("MAIN_MENU_SETTINGS status=ok volume=%d muted=%s checks=9" % [
		_volume, "true" if _muted else "false",
	])

	# Every menu string is keyed in both launch locales.
	_apply_locale("en")
	var keyed := true
	for key in _menu_keys():
		if not _i18n.has_key(key, "en") or not _i18n.has_key(key, "de"):
			keyed = false
	checks += _smoke_check(keyed, "i18n_keys", failures)

	# Forwarding keeps the existing terrain/placement smokes unchanged.
	var forwards_ok := true
	for flag in FORWARD_FLAGS:
		if forward_target(PackedStringArray([flag])) != "res://main.tscn":
			forwards_ok = false
	checks += _smoke_check(
		forwards_ok
		and forward_target(PackedStringArray(["--menu-smoke"])).is_empty()
		and forward_target(PackedStringArray()).is_empty(),
		"forward_args",
		failures
	)

	# --- Screen-reader semantics (own counter; markers stay pinned) ---
	# These checks run after the menu/settings markers are printed, so the
	# locale switches and overlay error state cannot move `checksum=3d4a58ba`
	# or `MAIN_MENU_SETTINGS checks=9`.
	var a11y_checks := 0
	var a11y_failures := PackedStringArray()
	_apply_locale("en")
	_set_focus(0)
	var a11y := accessibility_snapshot()

	var parts_ok := true
	for part in ["title", "subtitle", "hint", "footer"]:
		var part_entry: Dictionary = a11y[part]
		if str(part_entry["name"]).is_empty() or str(part_entry["description"]).is_empty():
			parts_ok = false
	a11y_checks += _smoke_check(parts_ok, "a11y.parts", a11y_failures)

	var button_entries: Array = a11y["buttons"]
	var buttons_ok: bool = button_entries.size() == ITEMS.size()
	for entry in button_entries:
		if str(entry["name"]).is_empty() or str(entry["description"]).is_empty():
			buttons_ok = false
	a11y_checks += _smoke_check(buttons_ok, "a11y.buttons", a11y_failures)
	a11y_checks += _smoke_check(
		str(a11y["title"]["role"]) == "heading"
		and str(a11y["subtitle"]["role"]) == "label"
		and str(a11y["hint"]["role"]) == "status"
		and str(a11y["footer"]["role"]) == "label"
		and str(a11y["buttons"][0]["role"]) == "button"
		and str(a11y["menu_order"]) == str(_ids),
		"a11y.roles",
		a11y_failures
	)

	var settings_ok := true
	for part in [
		"title", "body", "volume_label", "volume_value",
		"slider", "mute", "save", "cancel", "status",
	]:
		var setting_entry: Dictionary = a11y["settings"][part]
		if str(setting_entry["name"]).is_empty() or str(setting_entry["description"]).is_empty():
			settings_ok = false
	a11y_checks += _smoke_check(settings_ok, "a11y.settings", a11y_failures)
	a11y_checks += _smoke_check(
		str(a11y["settings"]["slider"]["role"]) == "slider"
		and str(a11y["settings"]["mute"]["role"]) == "checkbox"
		and str(a11y["settings"]["save"]["role"]) == "button"
		and str(a11y["settings"]["cancel"]["role"]) == "button"
		and str(a11y["settings"]["status"]["role"]) == "status",
		"a11y.settings_roles",
		a11y_failures
	)
	a11y_checks += _smoke_check(
		str(a11y["live"]["hint"]) == "polite"
		and str(a11y["live"]["settings_status"]) == "assertive",
		"a11y.live",
		a11y_failures
	)
	a11y_checks += _smoke_check(
		_a11y_descriptions_bounded(a11y),
		"a11y.bounded",
		a11y_failures
	)
	a11y_checks += _smoke_check(_a11y_no_leak(a11y), "a11y.en.no_missing_keys", a11y_failures)
	a11y_checks += _smoke_check(
		str(a11y["settings"]["slider"]["name"]) == "Master volume"
		and str((a11y["buttons"] as Array)[4]["name"]) == "Settings",
		"a11y.en.names",
		a11y_failures
	)

	# The hint is a polite live region: focus changes announce the new hint.
	_set_focus(_index_of("multiplayer"))
	a11y = accessibility_snapshot()
	a11y_checks += _smoke_check(
		str(a11y["hint"]["description"]).contains("server")
		and str(a11y["hint"]["description"]).contains("listen")
		and str(a11y["hint"]["live"]) == "polite",
		"a11y.hint_live",
		a11y_failures
	)
	_set_focus(0)

	# DE switches the announced names without changing the controls.
	_apply_locale("de")
	var de_a11y := accessibility_snapshot()
	a11y_checks += _smoke_check(
		str(de_a11y["locale"]) == "de"
		and str((de_a11y["buttons"] as Array)[0]["name"]) == "Neue Stadt"
		and str(de_a11y["settings"]["volume_label"]["name"]) == "Gesamtlautstärke",
		"a11y.de.names",
		a11y_failures
	)
	a11y_checks += _smoke_check(_a11y_no_leak(de_a11y), "a11y.de.no_missing_keys", a11y_failures)
	_apply_locale("en")

	# A failed save stays in the overlay and announces the localized error.
	_settings_path = ""
	_open_settings()
	_on_settings_save()
	a11y = accessibility_snapshot()
	a11y_checks += _smoke_check(
		_settings_overlay.visible
		and str(a11y["live"]["settings_status"]) == "assertive"
		and str(a11y["settings"]["status"]["description"])
			== _i18n.t("base.ui.menu.settings.save_failed"),
		"a11y.settings_save_error",
		a11y_failures
	)
	_back()
	_settings_path = "user://settings_smoke.cfg"

	# Semantics add no focus stop and no new navigation.
	a11y_checks += _smoke_check(
		_buttons[0].focus_mode == Control.FOCUS_ALL
		and _settings_slider.focus_mode == Control.FOCUS_ALL
		and _settings_mute.focus_mode == Control.FOCUS_ALL
		and _settings_save.focus_mode == Control.FOCUS_ALL
		and _settings_cancel.focus_mode == Control.FOCUS_ALL
		and str(accessibility_snapshot()["menu_order"]) == str(_ids),
		"a11y.no_navigation_change",
		a11y_failures
	)
	print("MAIN_MENU_A11Y status=%s checks=%d" % [
		"ok" if a11y_failures.is_empty() else "fail",
		a11y_checks,
	])
	failures.append_array(a11y_failures)

	# The marker line is deterministic: English labels, first item focused.
	_set_focus(0)
	var parts := PackedStringArray()
	for index in ITEMS.size():
		parts.append("%s:%s:%s" % [
			_ids[index],
			_buttons[index].text,
			_target_scene(_ids[index]),
		])
	var checksum := _fnv1a32("|".join(parts))
	var status := "ok" if failures.is_empty() else "failed"
	print("MAIN_MENU status=%s buttons=%d focus=%s checks=%d checksum=%s" % [
		status,
		_buttons.size(),
		_ids[_focus_index],
		checks,
		checksum,
	])
	for failure in failures:
		print("MAIN_MENU_FAIL ", failure)
	get_tree().quit(0 if status == "ok" else 1)


func _smoke_check(ok: bool, label: String, failures: PackedStringArray) -> int:
	if not ok:
		failures.append(label)
	return 1


func _menu_keys() -> PackedStringArray:
	var keys := PackedStringArray([
		"base.ui.menu.title",
		"base.ui.menu.subtitle",
		"base.ui.menu.settings.title",
		"base.ui.menu.settings.body",
		"base.ui.menu.settings.volume",
		"base.ui.menu.settings.volume_value",
		"base.ui.menu.settings.mute",
		"base.ui.menu.settings.save",
		"base.ui.menu.settings.cancel",
		"base.ui.menu.footer",
		"base.ui.menu.settings.save_failed",
		"base.ui.menu.a11y.menu",
		"base.ui.menu.a11y.hint",
		"base.ui.menu.a11y.footer",
		"base.ui.menu.a11y.settings_body",
		"base.ui.menu.a11y.volume_hint",
		"base.ui.menu.a11y.volume_value",
		"base.ui.menu.a11y.mute_hint",
		"base.ui.menu.a11y.save_hint",
		"base.ui.menu.a11y.cancel_hint",
		"base.ui.menu.a11y.status",
		"base.ui.menu.a11y.status_idle",
	])
	for item in ITEMS:
		keys.append(str(item["label"]))
		keys.append(str(item["hint"]))
	return keys
