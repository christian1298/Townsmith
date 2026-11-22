extends Control

## Player-facing benchmark report screen.
##
## Reads the existing Early/Mid/Late summary JSON written by
## `scripts/bench_godot.sh` and shows three scene cards, a per-scene and
## overall rating, recommended settings, and a combined JSON export.
##
## This is a **report over existing results**, not a live scene runner and not
## the 1M gate. If a summary file is missing, the screen falls back to embedded
## deterministic fixtures so the report stays usable in a clean checkout.
##
## Run:
##   godot --path godot res://bench_screen.tscn
##   godot --path godot res://bench_screen.tscn -- --output /tmp/report.json
##
## Headless structural smoke:
##   godot --headless --path godot res://bench_screen.tscn -- --smoke
##
## See docs/15 - Benchmarks.md and docs/68 - Metropolis Gate.md §5.3.

const RESULT_DIR := "benchmarks/results"
const DEFAULT_OUTPUT := "user://benchmark_report.json"
const REPORT_SUITE := "bench_screen_report"
const REPORT_FORMAT_VERSION := 1

const SCENE_SPECS := [
	{"id": "early", "label": "Early Settlement", "file": "godot_phase0_city_early_summary.json"},
	{"id": "mid", "label": "Mid City", "file": "godot_phase0_city_mid_summary.json"},
	{"id": "late", "label": "Late Metropolis", "file": "godot_phase0_city_late_summary.json"},
]

const RATING_ORDER := {"low": 0, "playable": 1, "good": 2, "excellent": 3}

const RATING_COLORS := {
	"excellent": Color(0.36, 0.86, 0.55),
	"good": Color(0.42, 0.72, 0.98),
	"playable": Color(0.96, 0.75, 0.30),
	"low": Color(0.96, 0.42, 0.38),
}

const SETTINGS_BY_RATING := {
	"excellent": {
		"resolution_scale_percent": 100,
		"shadow_preset": "high",
		"lod_preset": "far",
		"text": "Resolution scale 100%, shadows High, LOD distance Far.",
	},
	"good": {
		"resolution_scale_percent": 100,
		"shadow_preset": "medium",
		"lod_preset": "far",
		"text": "Resolution scale 100%, shadows Medium, LOD distance Far.",
	},
	"playable": {
		"resolution_scale_percent": 85,
		"shadow_preset": "low",
		"lod_preset": "medium",
		"text": "Resolution scale 85%, shadows Low, LOD distance Medium.",
	},
	"low": {
		"resolution_scale_percent": 70,
		"shadow_preset": "low_off",
		"lod_preset": "near",
		"text": "Resolution scale 70%, shadows Low or Off, LOD distance Near; reduce agent density.",
	},
}

## Deterministic fallbacks. They use the same fields as the real summaries so
## the smoke path exercises the real parser and median-FPS derivation.
const FIXTURES := {
	"early": {
		"runs": 3,
		"results": {
			"frametime_ms_median": 12.5,
			"fps_low_1pct": 64.0,
			"hitches_16ms": 9,
		},
	},
	"mid": {
		"runs": 3,
		"results": {
			"frametime_ms_median": 20.0,
			"fps_low_1pct": 39.5,
			"hitches_16ms": 48,
		},
	},
	"late": {
		"runs": 3,
		"results": {
			"frametime_ms_median": 32.0,
			"fps_low_1pct": 24.0,
			"hitches_16ms": 96,
		},
	},
}

var smoke_mode := false
var output_path := DEFAULT_OUTPUT
var _export_status: Label
var _card_count := 0

func _ready() -> void:
	var args := OS.get_cmdline_user_args()
	smoke_mode = args.has("--smoke")
	output_path = _arg(args, "--output", DEFAULT_OUTPUT)

	var report := _build_report(smoke_mode)
	_build_ui(report)
	var exported := _export_report(report)
	if _export_status != null:
		_export_status.text = ("Exported to " if exported else "Export failed: ") + output_path

	if smoke_mode:
		_run_smoke(report, exported)
		return
	if not exported:
		push_error("Benchmark report was not written: " + output_path)
	print("BENCH_SCREEN_REPORT output=%s rating=%s scenes=%d" % [
		output_path,
		str((report.get("overall", {}) as Dictionary).get("rating", "unknown")),
		(report.get("scenes", []) as Array).size(),
	])

func _arg(args: PackedStringArray, name: String, fallback: String) -> String:
	var index := args.find(name)
	if index >= 0 and index + 1 < args.size():
		return args[index + 1]
	return fallback

func _project_root() -> String:
	return ProjectSettings.globalize_path("res://").path_join("..").simplify_path()

func _is_finite_number(value: Variant) -> bool:
	if typeof(value) != TYPE_INT and typeof(value) != TYPE_FLOAT:
		return false
	var number := float(value)
	return not is_nan(number) and not is_inf(number)

func _number(value: Variant, fallback: float = 0.0) -> float:
	if _is_finite_number(value):
		return float(value)
	return fallback

func _rating_for(fps: float) -> String:
	if fps >= 60.0:
		return "excellent"
	if fps >= 45.0:
		return "good"
	if fps >= 30.0:
		return "playable"
	return "low"

func _rating_reason(rating: String) -> String:
	match rating:
		"excellent":
			return "Clears the 60 FPS target."
		"good":
			return "Near the 60 FPS target; 45+ median FPS."
		"playable":
			return "30+ median FPS; lower settings are recommended."
		_:
			return "Below 30 median FPS; lower settings are strongly recommended."

func _rating_color(rating: String) -> Color:
	return RATING_COLORS.get(rating, RATING_COLORS["low"])

func _settings_for(rating: String) -> Dictionary:
	var value: Variant = SETTINGS_BY_RATING.get(rating, SETTINGS_BY_RATING["low"])
	if typeof(value) != TYPE_DICTIONARY:
		return {}
	var settings: Dictionary = value
	return settings.duplicate(true)

func _median_fps(results: Dictionary) -> float:
	if _is_finite_number(results.get("fps_median")):
		return float(results["fps_median"])
	var median_ms := _number(results.get("frametime_ms_median"))
	if median_ms > 0.0:
		return 1000.0 / median_ms
	return _number(results.get("fps_avg", results.get("fps")))

func _scene_record(spec: Dictionary, summary: Dictionary, source_file: String, source: String) -> Dictionary:
	var results_value: Variant = summary.get("results", {})
	if typeof(results_value) != TYPE_DICTIONARY:
		return {}
	var results: Dictionary = results_value
	var median_fps := _median_fps(results)
	var low_1pct := _number(results.get("fps_low_1pct", results.get("low_1pct")))
	if low_1pct <= 0.0:
		var p99_ms := _number(results.get("frametime_ms_p99"))
		if p99_ms > 0.0:
			low_1pct = 1000.0 / p99_ms
	var hitches := _number(results.get("hitches_16ms", results.get("hitches_33ms")))
	var sample_count := 0
	var sample_unit := "runs"
	if _is_finite_number(summary.get("runs")) and float(summary["runs"]) > 0.0:
		sample_count = int(float(summary["runs"]))
	elif _is_finite_number(summary.get("frames")):
		sample_count = int(float(summary["frames"]))
		sample_unit = "frames"
	var rating := _rating_for(median_fps)
	return {
		"id": str(spec.get("id", "")),
		"label": str(spec.get("label", "")),
		"source_file": source_file,
		"source": source,
		"median_fps": median_fps,
		"fps_low_1pct": low_1pct,
		"hitches_16ms": hitches,
		"samples": sample_count,
		"sample_unit": sample_unit,
		"rating": rating,
		"reason": _rating_reason(rating),
		"recommended_settings": _settings_for(rating),
	}

func _fixture_scene(spec: Dictionary, source_file: String) -> Dictionary:
	var fixture_value: Variant = FIXTURES.get(str(spec.get("id", "")), {})
	if typeof(fixture_value) != TYPE_DICTIONARY:
		return {}
	var fixture: Dictionary = fixture_value
	return _scene_record(spec, fixture, source_file, "fixture")

func _load_scene(spec: Dictionary) -> Dictionary:
	var source_file := RESULT_DIR.path_join(str(spec.get("file", "")))
	var path := _project_root().path_join(source_file)
	var text := FileAccess.get_file_as_string(path)
	if not text.is_empty():
		var parsed: Variant = JSON.parse_string(text)
		if typeof(parsed) == TYPE_DICTIONARY:
			var summary: Dictionary = parsed
			var scene := _scene_record(spec, summary, source_file, "summary")
			if not scene.is_empty() and _number(scene.get("median_fps")) > 0.0:
				return scene
	return _fixture_scene(spec, source_file)

func _overall(scenes: Array) -> Dictionary:
	var worst_rank := 99
	var worst: Dictionary = {}
	var worst_fps := INF
	for value in scenes:
		if typeof(value) != TYPE_DICTIONARY:
			continue
		var scene: Dictionary = value
		var rank := int(RATING_ORDER.get(str(scene.get("rating", "low")), 0))
		var fps := _number(scene.get("median_fps"))
		if rank < worst_rank or (rank == worst_rank and fps < worst_fps):
			worst_rank = rank
			worst = scene
			worst_fps = fps
	var rating := str(worst.get("rating", "low"))
	var reason := "All three scenes clear the 60 FPS target."
	if rating != "excellent":
		reason = "Limited by %s at %.1f median FPS." % [
			str(worst.get("label", "the slowest scene")),
			_number(worst.get("median_fps")),
		]
	return {
		"rating": rating,
		"reason": reason,
		"limiting_scene": str(worst.get("id", "")),
	}

func _build_report(use_fixtures: bool) -> Dictionary:
	var scenes: Array = []
	for spec_value in SCENE_SPECS:
		var spec: Dictionary = spec_value
		var source_file := RESULT_DIR.path_join(str(spec.get("file", "")))
		if use_fixtures:
			scenes.append(_fixture_scene(spec, source_file))
		else:
			scenes.append(_load_scene(spec))
	var overall := _overall(scenes)
	return {
		"format_version": REPORT_FORMAT_VERSION,
		"suite": REPORT_SUITE,
		"generated_at": Time.get_datetime_string_from_system(true) + "Z",
		"headless": DisplayServer.get_name() == "headless",
		"report_only": true,
		"note": "Report over existing summary JSON; this screen does not run benchmark scenes.",
		"rating_policy": "overall is the worst scene rating",
		"rating_thresholds_fps": {"excellent": 60, "good": 45, "playable": 30},
		"overall": overall,
		"recommended_settings": _settings_for(str(overall.get("rating", "low"))),
		"scenes": scenes,
	}

func _global_path(path: String) -> String:
	if path.begins_with("user://") or path.begins_with("res://"):
		return ProjectSettings.globalize_path(path)
	return path

func _export_report(report: Dictionary) -> bool:
	var absolute := _global_path(output_path)
	var directory_error := DirAccess.make_dir_recursive_absolute(absolute.get_base_dir())
	if directory_error != OK and directory_error != ERR_ALREADY_EXISTS:
		push_error("Could not create benchmark report directory: " + absolute.get_base_dir())
		return false
	var file := FileAccess.open(output_path, FileAccess.WRITE)
	if file == null:
		push_error("Could not write benchmark report: %s (%s)" % [
			output_path,
			error_string(FileAccess.get_open_error()),
		])
		return false
	file.store_string(JSON.stringify(report, "  "))
	file.close()
	return true

func _build_ui(report: Dictionary) -> void:
	set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)

	var background := ColorRect.new()
	background.color = Color(0.055, 0.065, 0.08)
	background.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	background.mouse_filter = Control.MOUSE_FILTER_IGNORE
	add_child(background)

	var margin := MarginContainer.new()
	margin.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	margin.add_theme_constant_override("margin_left", 48)
	margin.add_theme_constant_override("margin_top", 36)
	margin.add_theme_constant_override("margin_right", 48)
	margin.add_theme_constant_override("margin_bottom", 32)
	add_child(margin)

	var root := VBoxContainer.new()
	root.add_theme_constant_override("separation", 14)
	margin.add_child(root)

	var title := Label.new()
	title.text = "Benchmark report"
	title.add_theme_font_size_override("font_size", 34)
	root.add_child(title)

	var subtitle := Label.new()
	subtitle.text = "Early / Mid / Late summary files. Report only: no benchmark scenes are run."
	subtitle.add_theme_color_override("font_color", Color(0.70, 0.75, 0.82))
	subtitle.add_theme_font_size_override("font_size", 16)
	root.add_child(subtitle)

	root.add_child(_build_overall_panel(report))

	var cards := HBoxContainer.new()
	cards.add_theme_constant_override("separation", 16)
	cards.size_flags_vertical = Control.SIZE_EXPAND_FILL
	root.add_child(cards)

	var scenes_value: Variant = report.get("scenes", [])
	if typeof(scenes_value) == TYPE_ARRAY:
		var scenes: Array = scenes_value
		for scene_value in scenes:
			if typeof(scene_value) != TYPE_DICTIONARY:
				continue
			var scene: Dictionary = scene_value
			cards.add_child(_build_scene_card(scene))
			_card_count += 1

	var footer := HBoxContainer.new()
	footer.add_theme_constant_override("separation", 12)
	root.add_child(footer)

	var export_button := Button.new()
	export_button.text = "Export JSON"
	export_button.pressed.connect(_on_export_pressed)
	footer.add_child(export_button)

	_export_status = Label.new()
	_export_status.text = "Exported to " + output_path
	_export_status.add_theme_color_override("font_color", Color(0.70, 0.75, 0.82))
	_export_status.add_theme_font_size_override("font_size", 14)
	footer.add_child(_export_status)

func _build_overall_panel(report: Dictionary) -> PanelContainer:
	var overall_value: Variant = report.get("overall", {})
	var overall: Dictionary = overall_value if typeof(overall_value) == TYPE_DICTIONARY else {}
	var rating := str(overall.get("rating", "low"))

	var panel := PanelContainer.new()
	var style := StyleBoxFlat.new()
	style.bg_color = Color(0.09, 0.105, 0.125)
	style.border_color = _rating_color(rating)
	style.border_width_left = 3
	style.border_width_top = 3
	style.border_width_right = 3
	style.border_width_bottom = 3
	style.corner_radius_top_left = 10
	style.corner_radius_top_right = 10
	style.corner_radius_bottom_left = 10
	style.corner_radius_bottom_right = 10
	style.content_margin_left = 22.0
	style.content_margin_top = 16.0
	style.content_margin_right = 22.0
	style.content_margin_bottom = 16.0
	panel.add_theme_stylebox_override("panel", style)

	var row := HBoxContainer.new()
	row.add_theme_constant_override("separation", 24)
	panel.add_child(row)

	var rating_label := Label.new()
	rating_label.text = rating.to_upper()
	rating_label.add_theme_font_size_override("font_size", 42)
	rating_label.add_theme_color_override("font_color", _rating_color(rating))
	row.add_child(rating_label)

	var detail := VBoxContainer.new()
	detail.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	detail.add_theme_constant_override("separation", 4)
	row.add_child(detail)

	var reason := Label.new()
	reason.text = "Overall: " + str(overall.get("reason", ""))
	reason.add_theme_font_size_override("font_size", 20)
	reason.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	detail.add_child(reason)

	var settings_value: Variant = report.get("recommended_settings", {})
	var settings: Dictionary = settings_value if typeof(settings_value) == TYPE_DICTIONARY else {}
	var settings_label := Label.new()
	settings_label.text = "Recommended: " + str(settings.get("text", "Choose settings after a live run."))
	settings_label.add_theme_color_override("font_color", Color(0.74, 0.79, 0.86))
	settings_label.add_theme_font_size_override("font_size", 16)
	settings_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	detail.add_child(settings_label)
	return panel

func _build_scene_card(scene: Dictionary) -> PanelContainer:
	var rating := str(scene.get("rating", "low"))
	var color := _rating_color(rating)

	var card := PanelContainer.new()
	card.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	card.custom_minimum_size = Vector2(0.0, 330.0)
	var style := StyleBoxFlat.new()
	style.bg_color = Color(0.095, 0.11, 0.13)
	style.border_color = color
	style.border_width_left = 2
	style.border_width_top = 2
	style.border_width_right = 2
	style.border_width_bottom = 2
	style.corner_radius_top_left = 8
	style.corner_radius_top_right = 8
	style.corner_radius_bottom_left = 8
	style.corner_radius_bottom_right = 8
	style.content_margin_left = 20.0
	style.content_margin_top = 18.0
	style.content_margin_right = 20.0
	style.content_margin_bottom = 18.0
	card.add_theme_stylebox_override("panel", style)

	var box := VBoxContainer.new()
	box.add_theme_constant_override("separation", 6)
	card.add_child(box)

	var name_label := Label.new()
	name_label.text = str(scene.get("label", ""))
	name_label.add_theme_font_size_override("font_size", 22)
	box.add_child(name_label)

	var fps_label := Label.new()
	fps_label.text = "%.1f" % _number(scene.get("median_fps"))
	fps_label.add_theme_font_size_override("font_size", 48)
	fps_label.add_theme_color_override("font_color", color)
	box.add_child(fps_label)

	var fps_caption := Label.new()
	fps_caption.text = "median FPS"
	fps_caption.add_theme_color_override("font_color", Color(0.66, 0.71, 0.78))
	fps_caption.add_theme_font_size_override("font_size", 14)
	box.add_child(fps_caption)

	box.add_child(_card_row("1% low", "%.1f FPS" % _number(scene.get("fps_low_1pct"))))
	box.add_child(_card_row("Hitches >16 ms", "%.0f" % _number(scene.get("hitches_16ms"))))
	box.add_child(_card_row("Samples", "%d %s" % [
		int(_number(scene.get("samples"))),
		str(scene.get("sample_unit", "runs")),
	]))

	var rating_label := Label.new()
	rating_label.text = "Rating: " + rating.capitalize()
	rating_label.add_theme_font_size_override("font_size", 18)
	rating_label.add_theme_color_override("font_color", color)
	box.add_child(rating_label)

	var reason := Label.new()
	reason.text = str(scene.get("reason", ""))
	reason.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	reason.add_theme_color_override("font_color", Color(0.70, 0.75, 0.82))
	reason.add_theme_font_size_override("font_size", 13)
	box.add_child(reason)

	var source := Label.new()
	var source_kind := "Summary" if str(scene.get("source", "")) == "summary" else "Embedded fixture"
	source.text = "%s: %s" % [source_kind, str(scene.get("source_file", ""))]
	source.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	source.add_theme_color_override("font_color", Color(0.55, 0.60, 0.68))
	source.add_theme_font_size_override("font_size", 12)
	box.add_child(source)
	return card

func _card_row(key: String, value: String) -> HBoxContainer:
	var row := HBoxContainer.new()
	var key_label := Label.new()
	key_label.text = key
	key_label.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	key_label.add_theme_color_override("font_color", Color(0.66, 0.71, 0.78))
	key_label.add_theme_font_size_override("font_size", 15)
	row.add_child(key_label)
	var value_label := Label.new()
	value_label.text = value
	value_label.add_theme_font_size_override("font_size", 15)
	row.add_child(value_label)
	return row

func _on_export_pressed() -> void:
	var report := _build_report(false)
	var exported := _export_report(report)
	if _export_status != null:
		_export_status.text = ("Exported to " if exported else "Export failed: ") + output_path

func _run_smoke(report: Dictionary, exported: bool) -> void:
	var status := "ok"
	var scenes_value: Variant = report.get("scenes", [])
	var scenes: Array = scenes_value if typeof(scenes_value) == TYPE_ARRAY else []
	var overall_value: Variant = report.get("overall", {})
	var overall: Dictionary = overall_value if typeof(overall_value) == TYPE_DICTIONARY else {}
	var expected_ids := ["early", "mid", "late"]
	var expected_ratings := ["excellent", "good", "playable"]

	if _card_count != 3:
		status = "cards=%d" % _card_count
	elif scenes.size() != 3:
		status = "scenes=%d" % scenes.size()

	var previous_fps := INF
	for index in range(scenes.size()):
		if status != "ok":
			break
		var scene_value: Variant = scenes[index]
		if typeof(scene_value) != TYPE_DICTIONARY:
			status = "scene_type_%d" % index
			break
		var scene: Dictionary = scene_value
		var median_fps := _number(scene.get("median_fps"), -1.0)
		var low_1pct := _number(scene.get("fps_low_1pct"), -1.0)
		var hitches := _number(scene.get("hitches_16ms"), -1.0)
		var samples := int(_number(scene.get("samples"), 0.0))
		if str(scene.get("id", "")) != expected_ids[index]:
			status = "id_%d" % index
		elif median_fps <= 0.0:
			status = "median_fps_%d" % index
		elif low_1pct <= 0.0:
			status = "low_1pct_%d" % index
		elif hitches < 0.0:
			status = "hitches_%d" % index
		elif samples <= 0:
			status = "samples_%d" % index
		elif str(scene.get("rating", "")) != expected_ratings[index]:
			status = "rating_%d" % index
		elif str(scene.get("source_file", "")) == "":
			status = "source_%d" % index
		elif index > 0 and median_fps > previous_fps:
			status = "order_%d" % index
		previous_fps = median_fps

	if status == "ok" and str(overall.get("rating", "")) != "playable":
		status = "overall_%s" % str(overall.get("rating", "missing"))
	elif status == "ok" and str(overall.get("limiting_scene", "")) != "late":
		status = "limiting_%s" % str(overall.get("limiting_scene", "missing"))

	if status == "ok" and _rating_for(60.0) != "excellent":
		status = "boundary_60"
	elif status == "ok" and _rating_for(45.0) != "good":
		status = "boundary_45"
	elif status == "ok" and _rating_for(30.0) != "playable":
		status = "boundary_30"
	elif status == "ok" and _rating_for(29.9) != "low":
		status = "boundary_below_30"

	if status == "ok" and not exported:
		status = "export_failed"
	if status == "ok":
		var text := FileAccess.get_file_as_string(output_path)
		var parsed: Variant = JSON.parse_string(text)
		if typeof(parsed) != TYPE_DICTIONARY:
			status = "export_parse"
		else:
			var exported_report: Dictionary = parsed
			var exported_scenes_value: Variant = exported_report.get("scenes", [])
			var exported_scenes: Array = exported_scenes_value if typeof(exported_scenes_value) == TYPE_ARRAY else []
			var exported_overall_value: Variant = exported_report.get("overall", {})
			var exported_overall: Dictionary = exported_overall_value if typeof(exported_overall_value) == TYPE_DICTIONARY else {}
			if exported_scenes.size() != 3:
				status = "export_scenes=%d" % exported_scenes.size()
			elif str(exported_overall.get("rating", "")) != str(overall.get("rating", "")):
				status = "export_rating"
			elif not exported_report.has("recommended_settings"):
				status = "export_settings"
			else:
				for index in range(3):
					var original: Dictionary = scenes[index]
					var exported_scene_value: Variant = exported_scenes[index]
					if typeof(exported_scene_value) != TYPE_DICTIONARY:
						status = "export_scene_type_%d" % index
						break
					var exported_scene: Dictionary = exported_scene_value
					if not exported_scene.has("source_file") or str(exported_scene.get("source_file", "")) == "":
						status = "export_source_%d" % index
					elif absf(_number(original.get("median_fps")) - _number(exported_scene.get("median_fps"), -1.0)) > 0.001:
						status = "export_roundtrip_%d" % index
					elif absf(_number(original.get("fps_low_1pct")) - _number(exported_scene.get("fps_low_1pct"), -1.0)) > 0.001:
						status = "export_roundtrip_low_%d" % index
					elif absf(_number(original.get("hitches_16ms")) - _number(exported_scene.get("hitches_16ms"), -1.0)) > 0.001:
						status = "export_roundtrip_hitches_%d" % index
					elif not exported_scene.has("recommended_settings"):
						status = "export_scene_settings_%d" % index

	if scenes.size() == 3:
		print("BENCH_SCREEN_VALUES early=%.1f/%s mid=%.1f/%s late=%.1f/%s" % [
			_number((scenes[0] as Dictionary).get("median_fps")),
			str((scenes[0] as Dictionary).get("rating", "")),
			_number((scenes[1] as Dictionary).get("median_fps")),
			str((scenes[1] as Dictionary).get("rating", "")),
			_number((scenes[2] as Dictionary).get("median_fps")),
			str((scenes[2] as Dictionary).get("rating", "")),
		])
	print("BENCH_SCREEN status=%s scenes=%d rating=%s" % [
		status,
		scenes.size(),
		str(overall.get("rating", "unknown")),
	])
	get_tree().quit(0 if status == "ok" else 1)
