extends Control

## Player-facing benchmark report screen.
##
## Reads the existing Early/Mid/Late summary JSON written by
## `scripts/bench_godot.sh` and shows three scene cards, a per-scene and
## overall rating, recommended settings, a compact frame-time graph per scene,
## and a combined JSON export.
##
## By default this is a **report over existing results**. With `--run` (or the
## "Run Early / Mid / Late" button) it runs `res://benchmark.tscn` once per tier,
## sequentially, writing a fresh single-run summary and refreshing the cards,
## graphs included, after each scene. It is still not the 1M/3060 gate. If a
## summary file is missing, the screen falls back to embedded deterministic
## fixtures so the report stays usable in a clean checkout.
##
## Frame-time graphs are drawn from per-run frame arrays when a summary carries
## them (`frametimes_ms`, `frame_times_ms`, `frames_ms`, or `samples_ms`, at the
## top level, under `results`, or under an array of per-run records). The
## committed summaries only keep medians, so those cards draw a deterministic
## curve derived from p50/p95/p99/max and label it `aggregated` — never raw
## samples. Fixtures are labelled `fixture`; raw arrays are labelled `summary`.
##
## Run:
##   godot --path godot res://bench_screen.tscn
##   godot --path godot res://bench_screen.tscn -- --output /tmp/report.json
##   godot --path godot res://bench_screen.tscn -- --run --duration 30 --warmup 3
##
## Headless structural smoke (no display, no scene runs):
##   godot --headless --path godot res://bench_screen.tscn -- --smoke
##
## See docs/15 - Benchmarks.md and docs/68 - Metropolis Gate.md §5.3.

const RESULT_DIR := "benchmarks/results"
const DEFAULT_OUTPUT := "user://benchmark_report.json"
const REPORT_SUITE := "bench_screen_report"
const REPORT_FORMAT_VERSION := 1
const BENCHMARK_SCENE := "res://benchmark.tscn"
const LIVE_DURATION_S := 30.0
const LIVE_WARMUP_S := 3.0

const SCENE_SPECS := [
	{
		"id": "early",
		"label": "Early Settlement",
		"file": "godot_phase0_city_early_summary.json",
		"route": "benchmarks/routes/phase0_city_early.json",
	},
	{
		"id": "mid",
		"label": "Mid City",
		"file": "godot_phase0_city_mid_summary.json",
		"route": "benchmarks/routes/phase0_city_mid.json",
	},
	{
		"id": "late",
		"label": "Late Metropolis",
		"file": "godot_phase0_city_late_summary.json",
		"route": "benchmarks/routes/phase0_city_late.json",
	},
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

const GRAPH_MAX_POINTS := 160
const GRAPH_REFERENCE_MS := [16.7, 33.3]
const GRAPH_SOURCE_FIXTURE := "fixture"
const GRAPH_SOURCE_AGGREGATED := "aggregated"
const GRAPH_SOURCE_SUMMARY := "summary"
const FRAME_SAMPLE_KEYS := ["frametimes_ms", "frame_times_ms", "frames_ms", "samples_ms"]

## Compact frame-time plot. One instance per scene card; redrawn on resize.
class FrameGraph extends Control:
	const BACKDROP := Color(0.052, 0.062, 0.078)
	const BORDER := Color(0.17, 0.20, 0.24)
	const REFERENCE := Color(0.32, 0.38, 0.46)
	const P95_COLOR := Color(0.96, 0.75, 0.30, 0.9)
	const P99_COLOR := Color(0.96, 0.42, 0.38, 0.9)
	const AXIS_TEXT := Color(0.50, 0.56, 0.63)
	const REFERENCE_MS := [16.7, 33.3]

	var samples := PackedFloat32Array()
	var p95_ms := 0.0
	var p99_ms := 0.0
	var y_max_ms := 33.3
	var tint := Color(0.42, 0.72, 0.98)

	func _init() -> void:
		custom_minimum_size = Vector2(0.0, 116.0)
		size_flags_horizontal = Control.SIZE_EXPAND_FILL
		mouse_filter = Control.MOUSE_FILTER_IGNORE

	func _draw() -> void:
		draw_rect(Rect2(Vector2.ZERO, size), BACKDROP, true)
		draw_rect(Rect2(Vector2.ZERO, size), BORDER, false, 1.0)
		if samples.size() < 2 or size.x < 60.0 or size.y < 50.0:
			return
		var plot_left := 32.0
		var plot_right := maxf(plot_left + 1.0, size.x - 34.0)
		var plot_top := 10.0
		var plot_bottom := maxf(plot_top + 1.0, size.y - 16.0)
		var span := maxf(y_max_ms, 0.001)
		var height := plot_bottom - plot_top
		var x_step := (plot_right - plot_left) / maxf(float(samples.size() - 1), 1.0)

		for reference in REFERENCE_MS:
			if float(reference) <= span:
				var y := plot_bottom - (float(reference) / span) * height
				_dashed_horizontal(y, plot_left, plot_right, REFERENCE)
				_label("%.1f" % float(reference), Vector2(2.0, y + 3.0), REFERENCE)
		_label("%.1f ms" % span, Vector2(2.0, plot_top + 4.0), AXIS_TEXT)
		_label("0", Vector2(2.0, plot_bottom + 3.0), AXIS_TEXT)
		_label("frames", Vector2(plot_right - 26.0, size.y - 3.0), AXIS_TEXT)

		var markers := [
			{"value": p95_ms, "color": P95_COLOR, "label": "p95"},
			{"value": p99_ms, "color": P99_COLOR, "label": "p99"},
		]
		for marker_value in markers:
			var marker: Dictionary = marker_value
			var value := float(marker.get("value", 0.0))
			if value <= 0.0 or value > span:
				continue
			var marker_y := plot_bottom - (value / span) * height
			var marker_color: Color = marker.get("color", REFERENCE)
			_dashed_horizontal(marker_y, plot_left, plot_right, marker_color)
			_label(str(marker.get("label", "")), Vector2(plot_right + 4.0, marker_y + 3.0), marker_color)

		var previous := Vector2(
			plot_left,
			plot_bottom - clampf(samples[0] / span, 0.0, 1.0) * height
		)
		for index in range(1, samples.size()):
			var point := Vector2(
				plot_left + x_step * float(index),
				plot_bottom - clampf(samples[index] / span, 0.0, 1.0) * height
			)
			draw_line(previous, point, tint, 1.4, true)
			previous = point

	func _dashed_horizontal(y: float, x0: float, x1: float, color: Color) -> void:
		var dash := 5.0
		var gap := 4.0
		var x := x0
		while x < x1:
			draw_line(Vector2(x, y), Vector2(minf(x + dash, x1), y), color, 1.0)
			x += dash + gap

	func _label(text: String, position: Vector2, color: Color) -> void:
		var font := ThemeDB.fallback_font
		if font == null:
			return
		draw_string(font, position, text, HORIZONTAL_ALIGNMENT_LEFT, -1, 9, color)

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
var run_mode := false
var output_path := DEFAULT_OUTPUT
var live_duration_s := LIVE_DURATION_S
var live_warmup_s := LIVE_WARMUP_S
var _export_status: Label
var _run_status: Label
var _run_button: Button
var _card_count := 0
var _graph_count := 0
var _report: Dictionary = {}
var _running := false
var _completed_tiers: Array[String] = []

func _ready() -> void:
	var args := OS.get_cmdline_user_args()
	smoke_mode = args.has("--smoke")
	run_mode = args.has("--run") and not smoke_mode
	output_path = _arg(args, "--output", DEFAULT_OUTPUT)
	live_duration_s = _positive_float(_arg(args, "--duration", str(LIVE_DURATION_S)), LIVE_DURATION_S)
	live_warmup_s = _positive_float(_arg(args, "--warmup", str(LIVE_WARMUP_S)), LIVE_WARMUP_S)

	_report = _build_report(smoke_mode)
	_build_ui(_report)
	var exported := _export_report(_report)
	_set_export_status(exported)

	if smoke_mode:
		_run_smoke(_report, exported)
		return

	if run_mode:
		if DisplayServer.get_name() == "headless":
			push_error("Live benchmark runs need a display; run without --headless.")
			print("BENCH_SCREEN status=display_required scenes=0 rating=unknown mode=run")
			get_tree().quit(2)
			return
		await _run_all(true)
		return

	if not exported:
		push_error("Benchmark report was not written: " + output_path)
	print("BENCH_SCREEN_REPORT output=%s rating=%s scenes=%d" % [
		output_path,
		str((_report.get("overall", {}) as Dictionary).get("rating", "unknown")),
		(_report.get("scenes", []) as Array).size(),
	])

func _arg(args: PackedStringArray, name: String, fallback: String) -> String:
	var index := args.find(name)
	if index >= 0 and index + 1 < args.size():
		return args[index + 1]
	return fallback

func _positive_float(value: String, fallback: float) -> float:
	if not value.is_valid_float():
		return fallback
	var number := float(value)
	return number if number > 0.0 else fallback

func _command_for(spec: Dictionary, duration_s: float, warmup_s: float) -> Dictionary:
	var tier := str(spec.get("id", ""))
	var route_rel := str(spec.get("route", ""))
	var summary_rel := RESULT_DIR.path_join(str(spec.get("file", "")))
	var route_abs := _project_root().path_join(route_rel)
	var summary_abs := _project_root().path_join(summary_rel)
	var raw_abs := ProjectSettings.globalize_path("user://bench_screen_%s_run.json" % tier)
	var args := PackedStringArray([
		"--path",
		ProjectSettings.globalize_path("res://"),
		BENCHMARK_SCENE,
		"--",
		"--city-tier",
		tier,
		"--route",
		route_abs,
		"--output",
		raw_abs,
		"--duration",
		str(duration_s),
		"--warmup",
		str(warmup_s),
	])
	return {
		"tier": tier,
		"label": str(spec.get("label", "")),
		"executable": OS.get_executable_path(),
		"args": args,
		"route_rel": route_rel,
		"route": route_abs,
		"summary_rel": summary_rel,
		"summary": summary_abs,
		"raw_output": raw_abs,
		"duration_s": duration_s,
		"warmup_s": warmup_s,
	}

func _run_commands(duration_s: float, warmup_s: float) -> Array:
	var commands: Array = []
	for spec_value in SCENE_SPECS:
		var spec: Dictionary = spec_value
		commands.append(_command_for(spec, duration_s, warmup_s))
	return commands

func _preflight(commands: Array) -> String:
	if OS.get_executable_path().is_empty():
		return "executable"
	if not ResourceLoader.exists(BENCHMARK_SCENE):
		return "scene"
	for command_value in commands:
		if typeof(command_value) != TYPE_DICTIONARY:
			return "command"
		var command: Dictionary = command_value
		var tier := str(command.get("tier", ""))
		if not FileAccess.file_exists(str(command.get("route", ""))):
			return "route_%s" % tier
		if str(command.get("raw_output", "")).is_empty():
			return "output_%s" % tier
	return ""

func _summary_from_run(raw: Dictionary, tier: String, runs: int) -> Dictionary:
	var results_value: Variant = raw.get("results", {})
	if typeof(results_value) != TYPE_DICTIONARY:
		return {}
	var results: Dictionary = (results_value as Dictionary).duplicate(true)
	if _median_fps(results) <= 0.0:
		return {}
	var frames := int(_number(raw.get("frames"), 0.0))
	if frames <= 0:
		frames = int(_number(results.get("frames"), 0.0))
	var summary := {
		"format_version": REPORT_FORMAT_VERSION,
		"suite": "phase0_godot_route",
		"generated_at": Time.get_datetime_string_from_system(true) + "Z",
		"generated_by": "bench_screen_live",
		"live": true,
		"tier": tier,
		"runs": maxi(runs, 1),
		"frames": frames,
		"duration_s": _number(raw.get("duration_s")),
		"warmup_s": _number(raw.get("warmup_s")),
		"headless": bool(raw.get("headless", false)),
		"hardware": raw.get("hardware", {}),
		"engine": raw.get("engine", {}),
		"route": str(raw.get("route", "")),
		"results": results,
	}
	if raw.has("city"):
		summary["city"] = raw["city"]
	if raw.has("voxel_instances"):
		summary["voxel_instances"] = raw["voxel_instances"]
	return summary

func _summary_from_run_file(path: String, tier: String) -> Dictionary:
	var text := FileAccess.get_file_as_string(path)
	if text.is_empty():
		push_error("Live benchmark run did not write a result: " + path)
		return {}
	var parsed: Variant = JSON.parse_string(text)
	if typeof(parsed) != TYPE_DICTIONARY:
		push_error("Live benchmark result is not a JSON object: " + path)
		return {}
	var raw: Dictionary = parsed
	if raw.has("city") and typeof(raw["city"]) == TYPE_DICTIONARY:
		var city: Dictionary = raw["city"]
		if city.has("tier") and str(city["tier"]) != tier:
			push_error("Live benchmark result tier mismatch: %s != %s" % [str(city["tier"]), tier])
			return {}
	return _summary_from_run(raw, tier, 1)

func _run_all(quit_when_done: bool) -> void:
	if _running:
		return
	_running = true
	var commands := _run_commands(live_duration_s, live_warmup_s)
	var problem := _preflight(commands)
	if problem != "":
		_finish_run(quit_when_done, "preflight_%s" % problem, 0)
		return
	var completed := 0
	var status := "ok"
	for command_value in commands:
		var command: Dictionary = command_value
		var tier := str(command.get("tier", ""))
		var label := str(command.get("label", tier))
		_set_run_status("Running %s (%d/%d)..." % [label, completed + 1, commands.size()])
		await get_tree().process_frame
		var raw_path := str(command.get("raw_output", ""))
		DirAccess.remove_absolute(raw_path)
		var captured: Array = []
		var args_value: Variant = command.get("args", PackedStringArray())
		var args: PackedStringArray = args_value if typeof(args_value) == TYPE_PACKED_STRING_ARRAY else PackedStringArray()
		var exit_code := OS.execute(
			str(command.get("executable", "")),
			args,
			captured,
			true
		)
		for line in captured:
			print(str(line))
		if exit_code != 0:
			status = "run_%s_exit_%d" % [tier, exit_code]
			break
		var summary := _summary_from_run_file(raw_path, tier)
		DirAccess.remove_absolute(raw_path)
		if summary.is_empty():
			status = "result_%s" % tier
			break
		if not _write_json(str(command.get("summary", "")), summary):
			status = "write_%s" % tier
			break
		completed += 1
		if not _completed_tiers.has(tier):
			_completed_tiers.append(tier)
		_report = _build_report(false, true)
		_set_export_status(_export_report(_report))
		_refresh_ui(_report, "Ran %s (%d/%d)." % [label, completed, commands.size()])
		await get_tree().process_frame
	if status == "ok" and completed != commands.size():
		status = "completed_%d" % completed
	_finish_run(quit_when_done, status, completed)

func _finish_run(quit_when_done: bool, status: String, completed: int) -> void:
	_running = false
	_report = _build_report(false, completed > 0)
	var exported := _export_report(_report)
	var overall_value: Variant = _report.get("overall", {})
	var overall: Dictionary = overall_value if typeof(overall_value) == TYPE_DICTIONARY else {}
	var rating := str(overall.get("rating", "unknown"))
	var scenes := (_report.get("scenes", []) as Array).size()
	if status == "ok":
		_refresh_ui(_report, "Live run complete: %d scenes, rating %s." % [scenes, rating])
	else:
		_refresh_ui(_report, "Live run failed: " + status)
	_set_export_status(exported)
	if status != "ok":
		push_error("Benchmark screen live run failed: " + status)
	print("BENCH_SCREEN status=%s scenes=%d rating=%s mode=run" % [status, scenes, rating])
	print("BENCH_SCREEN_LIVE status=%s completed=%d output=%s" % [status, completed, output_path])
	if quit_when_done:
		get_tree().quit(0 if status == "ok" else 1)

func _set_export_status(exported: bool) -> void:
	if _export_status != null:
		_export_status.text = ("Exported to " if exported else "Export failed: ") + output_path

func _set_run_status(text: String) -> void:
	if _run_status != null:
		_run_status.text = text

func _refresh_ui(report: Dictionary, run_status: String) -> void:
	for child in get_children():
		remove_child(child)
		child.queue_free()
	_card_count = 0
	_graph_count = 0
	_export_status = null
	_run_status = null
	_run_button = null
	_build_ui(report)
	_set_run_status(run_status)

func _write_json(path: String, data: Dictionary) -> bool:
	var absolute := _global_path(path)
	var directory_error := DirAccess.make_dir_recursive_absolute(absolute.get_base_dir())
	if directory_error != OK and directory_error != ERR_ALREADY_EXISTS:
		push_error("Could not create benchmark output directory: " + absolute.get_base_dir())
		return false
	var file := FileAccess.open(path, FileAccess.WRITE)
	if file == null:
		push_error("Could not write benchmark JSON: %s (%s)" % [
			path,
			error_string(FileAccess.get_open_error()),
		])
		return false
	file.store_string(JSON.stringify(data, "  "))
	file.close()
	return true

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
	var frames_value: Variant = summary.get("frames")
	var live_summary := bool(summary.get("live", false))
	if live_summary and _is_finite_number(frames_value) and float(frames_value) > 0.0:
		sample_count = int(float(frames_value))
		sample_unit = "frames"
	elif _is_finite_number(summary.get("runs")) and float(summary["runs"]) > 0.0:
		sample_count = int(float(summary["runs"]))
	elif _is_finite_number(frames_value) and float(frames_value) > 0.0:
		sample_count = int(float(frames_value))
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
		"frame_graph": _frame_graph_for(results, summary, source),
		"rating": rating,
		"reason": _rating_reason(rating),
		"recommended_settings": _settings_for(rating),
	}

func _finite_sample_array(values: Array) -> Array:
	var samples: Array = []
	for value in values:
		if not _is_finite_number(value):
			continue
		var sample := float(value)
		if sample > 0.0:
			samples.append(sample)
	return samples

func _samples_from_container(container: Dictionary) -> Array:
	for key in FRAME_SAMPLE_KEYS:
		var value: Variant = container.get(key)
		if typeof(value) != TYPE_ARRAY:
			continue
		var samples := _finite_sample_array(value)
		if samples.size() >= 2:
			return samples
	return []

func _raw_frame_samples(summary: Dictionary, results: Dictionary) -> Array:
	var direct := _samples_from_container(summary)
	if not direct.is_empty():
		return direct
	direct = _samples_from_container(results)
	if not direct.is_empty():
		return direct
	var runs_value: Variant = summary.get("runs")
	if typeof(runs_value) != TYPE_ARRAY:
		return []
	var combined: Array = []
	for run_value in (runs_value as Array):
		if typeof(run_value) != TYPE_DICTIONARY:
			continue
		var run: Dictionary = run_value
		var part := _samples_from_container(run)
		if part.is_empty():
			var run_results: Variant = run.get("results")
			if typeof(run_results) == TYPE_DICTIONARY:
				part = _samples_from_container(run_results)
		combined.append_array(part)
	return combined if combined.size() >= 2 else []

func _percentile_value(sorted_samples: Array, fraction: float) -> float:
	if sorted_samples.is_empty():
		return 0.0
	var index := clampi(
		int(round(float(sorted_samples.size() - 1) * fraction)),
		0,
		sorted_samples.size() - 1
	)
	return float(sorted_samples[index])

func _graph_quantiles(results: Dictionary) -> Dictionary:
	var median_fps := _median_fps(results)
	var p50 := _number(results.get("frametime_ms_median"))
	if p50 <= 0.0 and median_fps > 0.0:
		p50 = 1000.0 / median_fps
	if p50 <= 0.0:
		return {}
	var p99 := _number(results.get("frametime_ms_p99"))
	if p99 <= 0.0:
		var low_fps := _number(results.get("fps_low_1pct", results.get("low_1pct")))
		if low_fps > 0.0:
			p99 = 1000.0 / low_fps
	if p99 < p50:
		p99 = p50 * 1.6
	var p95 := _number(results.get("frametime_ms_p95"))
	if p95 <= 0.0:
		p95 = lerpf(p50, p99, 0.62)
	p95 = clampf(p95, p50, p99)
	var max_ms := _number(results.get("frametime_ms_max"))
	if max_ms <= p99:
		max_ms = p99 * 1.15 + 0.5
	var min_ms := _number(results.get("frametime_ms_min"))
	if min_ms <= 0.0 or min_ms > p50:
		min_ms = p50 * 0.55
	return {
		"min_ms": min_ms,
		"p50_ms": p50,
		"p95_ms": p95,
		"p99_ms": p99,
		"max_ms": max_ms,
	}

## Deterministic quantile curve through min/p50/p95/p99/max. It is a labelled
## stand-in for raw samples, never presented as measured per-frame data.
func _synthetic_samples(quantiles: Dictionary) -> Array:
	var anchors := [
		Vector2(0.0, float(quantiles.get("min_ms", 0.0))),
		Vector2(0.5, float(quantiles.get("p50_ms", 0.0))),
		Vector2(0.95, float(quantiles.get("p95_ms", 0.0))),
		Vector2(0.99, float(quantiles.get("p99_ms", 0.0))),
		Vector2(1.0, float(quantiles.get("max_ms", 0.0))),
	]
	var count := 96
	var samples: Array = []
	for index in range(count):
		var q := float(index) / float(count - 1)
		var segment := 0
		while segment < anchors.size() - 2 and q > anchors[segment + 1].x:
			segment += 1
		var left: Vector2 = anchors[segment]
		var right: Vector2 = anchors[segment + 1]
		var span := maxf(right.x - left.x, 0.0001)
		var f := clampf((q - left.x) / span, 0.0, 1.0)
		samples.append(lerpf(left.y, right.y, f))
	return samples

func _downsample_samples(samples: Array, limit: int) -> Array:
	if samples.size() <= limit:
		return samples.duplicate()
	var out: Array = []
	for index in range(limit):
		var source_index := int(round(
			float(index) * float(samples.size() - 1) / float(limit - 1)
		))
		out.append(samples[source_index])
	return out

func _frame_graph_for(results: Dictionary, summary: Dictionary, scene_source: String) -> Dictionary:
	var raw := _raw_frame_samples(summary, results)
	var source := GRAPH_SOURCE_AGGREGATED
	var samples: Array = []
	var p50 := 0.0
	var p95 := 0.0
	var p99 := 0.0
	var max_ms := 0.0
	if raw.size() >= 2:
		var sorted := raw.duplicate()
		sorted.sort()
		p50 = _percentile_value(sorted, 0.50)
		p95 = _percentile_value(sorted, 0.95)
		p99 = _percentile_value(sorted, 0.99)
		max_ms = float(sorted[sorted.size() - 1])
		samples = _downsample_samples(raw, GRAPH_MAX_POINTS)
		if scene_source == "fixture":
			source = GRAPH_SOURCE_FIXTURE
		else:
			source = GRAPH_SOURCE_SUMMARY
	else:
		var quantiles := _graph_quantiles(results)
		if quantiles.is_empty():
			return {}
		p50 = float(quantiles.get("p50_ms", 0.0))
		p95 = float(quantiles.get("p95_ms", 0.0))
		p99 = float(quantiles.get("p99_ms", 0.0))
		max_ms = float(quantiles.get("max_ms", 0.0))
		samples = _synthetic_samples(quantiles)
		if scene_source == "fixture":
			source = GRAPH_SOURCE_FIXTURE
		else:
			source = GRAPH_SOURCE_AGGREGATED
	var y_max := maxf(33.3, maxf(max_ms, p99) * 1.08)
	y_max = ceilf(y_max * 10.0) / 10.0
	return {
		"source": source,
		"synthetic": raw.size() < 2,
		"samples": samples,
		"sample_count": samples.size(),
		"p50_ms": p50,
		"p95_ms": p95,
		"p99_ms": p99,
		"max_ms": max_ms,
		"axis": {
			"x": "sample_index",
			"x_min": 0,
			"x_max": samples.size() - 1,
			"y": "frametime_ms",
			"y_min_ms": 0.0,
			"y_max_ms": y_max,
			"ascending": true,
		},
		"reference_ms": GRAPH_REFERENCE_MS.duplicate(),
	}

func _fixture_scene(spec: Dictionary, source_file: String) -> Dictionary:
	var fixture_value: Variant = FIXTURES.get(str(spec.get("id", "")), {})
	if typeof(fixture_value) != TYPE_DICTIONARY:
		return {}
	var fixture: Dictionary = fixture_value
	return _scene_record(spec, fixture, source_file, "fixture")

func _load_scene_from_path(spec: Dictionary, path: String, source_file: String) -> Dictionary:
	var text := FileAccess.get_file_as_string(path)
	if text.is_empty():
		return {}
	var parsed: Variant = JSON.parse_string(text)
	if typeof(parsed) != TYPE_DICTIONARY:
		return {}
	var summary: Dictionary = parsed
	var source := "live" if bool(summary.get("live", false)) else "summary"
	var scene := _scene_record(spec, summary, source_file, source)
	if not scene.is_empty() and _number(scene.get("median_fps")) > 0.0:
		return scene
	return {}

func _load_scene(spec: Dictionary) -> Dictionary:
	var source_file := RESULT_DIR.path_join(str(spec.get("file", "")))
	var path := _project_root().path_join(source_file)
	var scene := _load_scene_from_path(spec, path, source_file)
	if not scene.is_empty():
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

func _scenes_have_live(scenes: Array) -> bool:
	for value in scenes:
		if typeof(value) == TYPE_DICTIONARY and str((value as Dictionary).get("source", "")) == "live":
			return true
	return false

func _build_report(use_fixtures: bool, live_hint: bool = false) -> Dictionary:
	var scenes: Array = []
	for spec_value in SCENE_SPECS:
		var spec: Dictionary = spec_value
		var source_file := RESULT_DIR.path_join(str(spec.get("file", "")))
		if use_fixtures:
			scenes.append(_fixture_scene(spec, source_file))
		else:
			scenes.append(_load_scene(spec))
	var overall := _overall(scenes)
	var live := live_hint or _scenes_have_live(scenes)
	var report := {
		"format_version": REPORT_FORMAT_VERSION,
		"suite": REPORT_SUITE,
		"generated_at": Time.get_datetime_string_from_system(true) + "Z",
		"headless": DisplayServer.get_name() == "headless",
		"report_only": not live,
		"mode": "run" if live else "report",
		"note": (
			"Live single-run results produced by this screen; one run per scene."
			if live
			else "Report over existing summary JSON; this screen does not run benchmark scenes."
		),
		"rating_policy": "overall is the worst scene rating",
		"rating_thresholds_fps": {"excellent": 60, "good": 45, "playable": 30},
		"overall": overall,
		"recommended_settings": _settings_for(str(overall.get("rating", "low"))),
		"scenes": scenes,
	}
	if live:
		report["live_runs"] = _completed_tiers.duplicate()
		report["live_duration_s"] = live_duration_s
		report["live_warmup_s"] = live_warmup_s
	return report

func _global_path(path: String) -> String:
	if path.begins_with("user://") or path.begins_with("res://"):
		return ProjectSettings.globalize_path(path)
	return path

func _export_report(report: Dictionary) -> bool:
	if not _write_json(output_path, report):
		push_error("Benchmark report was not written: " + output_path)
		return false
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
	subtitle.text = "Early / Mid / Late summaries. Report only until you press Run; live runs need a display and take one run per scene."
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

	_run_button = Button.new()
	_run_button.text = "Run Early / Mid / Late"
	_run_button.pressed.connect(_on_run_pressed)
	_run_button.disabled = _running or smoke_mode or DisplayServer.get_name() == "headless"
	footer.add_child(_run_button)

	_export_status = Label.new()
	_export_status.text = "Exported to " + output_path
	_export_status.add_theme_color_override("font_color", Color(0.70, 0.75, 0.82))
	_export_status.add_theme_font_size_override("font_size", 14)
	footer.add_child(_export_status)

	_run_status = Label.new()
	if _running:
		_run_status.text = "Running live scenes..."
	elif DisplayServer.get_name() == "headless":
		_run_status.text = "Live runs need a display."
	else:
		_run_status.text = "Report only. Live runs are sequential and are not the 1M gate."
	_run_status.add_theme_color_override("font_color", Color(0.70, 0.75, 0.82))
	_run_status.add_theme_font_size_override("font_size", 14)
	footer.add_child(_run_status)

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
	card.custom_minimum_size = Vector2(0.0, 500.0)
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

	var graph_value: Variant = scene.get("frame_graph", {})
	if typeof(graph_value) == TYPE_DICTIONARY:
		box.add_child(_build_frame_graph(graph_value, color))

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
	var source_kind := "Embedded fixture"
	match str(scene.get("source", "")):
		"summary":
			source_kind = "Summary"
		"live":
			source_kind = "Live run"
	source.text = "%s: %s" % [source_kind, str(scene.get("source_file", ""))]
	source.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	source.add_theme_color_override("font_color", Color(0.55, 0.60, 0.68))
	source.add_theme_font_size_override("font_size", 12)
	box.add_child(source)
	return card

func _build_frame_graph(graph: Dictionary, tint: Color) -> VBoxContainer:
	var container := VBoxContainer.new()
	container.add_theme_constant_override("separation", 3)

	var source := str(graph.get("source", ""))
	var caption := Label.new()
	caption.text = "Frame time · %s · %s" % [source, _frame_graph_note(graph)]
	caption.add_theme_color_override("font_color", Color(0.66, 0.71, 0.78))
	caption.add_theme_font_size_override("font_size", 12)
	container.add_child(caption)

	var plot := FrameGraph.new()
	plot.name = "FrameGraph"
	plot.tint = tint
	var axis_value: Variant = graph.get("axis", {})
	var axis: Dictionary = axis_value if typeof(axis_value) == TYPE_DICTIONARY else {}
	plot.y_max_ms = _number(axis.get("y_max_ms"), 33.3)
	plot.p95_ms = _number(graph.get("p95_ms"))
	plot.p99_ms = _number(graph.get("p99_ms"))
	var samples_value: Variant = graph.get("samples", [])
	var samples: Array = samples_value if typeof(samples_value) == TYPE_ARRAY else []
	var packed := PackedFloat32Array()
	for value in samples:
		packed.append(float(value))
	plot.samples = packed
	container.add_child(plot)
	_graph_count += 1

	var stats := Label.new()
	stats.text = "p50 %.2f ms · p95 %.2f · p99 %.2f · max %.2f" % [
		_number(graph.get("p50_ms")),
		_number(graph.get("p95_ms")),
		_number(graph.get("p99_ms")),
		_number(graph.get("max_ms")),
	]
	stats.add_theme_color_override("font_color", Color(0.55, 0.60, 0.68))
	stats.add_theme_font_size_override("font_size", 12)
	container.add_child(stats)
	return container

func _frame_graph_note(graph: Dictionary) -> String:
	if bool(graph.get("synthetic", false)):
		return "synthetic from p50/p95/p99, not raw frames"
	return "%d frames" % int(_number(graph.get("sample_count")))

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
	_set_export_status(exported)

func _on_run_pressed() -> void:
	if _running:
		return
	var commands := _run_commands(live_duration_s, live_warmup_s)
	var problem := _preflight(commands)
	if problem != "":
		_set_run_status("Cannot run: " + problem)
		push_error("Cannot run benchmark scenes: " + problem)
		return
	_run_all(false)

func _graph_problem(graph_value: Variant, expected_source: String, index: int) -> String:
	if typeof(graph_value) != TYPE_DICTIONARY:
		return "graph_type_%d" % index
	var graph: Dictionary = graph_value
	if str(graph.get("source", "")) != expected_source:
		return "graph_source_%d" % index
	var samples_value: Variant = graph.get("samples", [])
	if typeof(samples_value) != TYPE_ARRAY:
		return "graph_samples_%d" % index
	var samples: Array = samples_value
	if samples.size() < 2:
		return "graph_count_%d" % index
	var axis_value: Variant = graph.get("axis", {})
	if typeof(axis_value) != TYPE_DICTIONARY:
		return "graph_axis_%d" % index
	var axis: Dictionary = axis_value
	if int(_number(axis.get("x_min"), -1.0)) != 0:
		return "graph_x_min_%d" % index
	if int(_number(axis.get("x_max"), -1.0)) != samples.size() - 1:
		return "graph_x_max_%d" % index
	var y_min := _number(axis.get("y_min_ms"), -1.0)
	var y_max := _number(axis.get("y_max_ms"), 0.0)
	if y_min != 0.0 or y_max <= y_min:
		return "graph_y_axis_%d" % index
	var p50 := _number(graph.get("p50_ms"), -1.0)
	var p95 := _number(graph.get("p95_ms"), -1.0)
	var p99 := _number(graph.get("p99_ms"), -1.0)
	var max_ms := _number(graph.get("max_ms"), -1.0)
	if p50 <= 0.0 or p95 < p50 or p99 < p95 or max_ms < p99:
		return "graph_percentiles_%d" % index
	if max_ms > y_max:
		return "graph_max_range_%d" % index
	var previous := -1.0
	for sample in samples:
		if not _is_finite_number(sample):
			return "graph_sample_finite_%d" % index
		var value := float(sample)
		if value <= 0.0 or value > y_max:
			return "graph_sample_range_%d" % index
		if bool(graph.get("synthetic", false)) and value + 0.000001 < previous:
			return "graph_sample_order_%d" % index
		previous = value
	var references_value: Variant = graph.get("reference_ms", [])
	if typeof(references_value) != TYPE_ARRAY:
		return "graph_refs_%d" % index
	var references: Array = references_value
	if not references.has(16.7) or not references.has(33.3):
		return "graph_refs_%d" % index
	return ""

## Exercises all three graph source labels: fixture fallback, aggregate-only
## summary (synthetic curve), and a summary carrying raw frame arrays.
func _smoke_graph_sources() -> String:
	var aggregate_summary := {
		"runs": 3,
		"results": {
			"frametime_ms_median": 16.0,
			"frametime_ms_p95": 24.0,
			"frametime_ms_p99": 31.0,
			"fps_low_1pct": 32.2,
		},
	}
	var aggregate_scene := _scene_record(SCENE_SPECS[0], aggregate_summary, "crafted", "summary")
	var problem := _graph_problem(aggregate_scene.get("frame_graph", {}), GRAPH_SOURCE_AGGREGATED, 0)
	if problem != "":
		return "resolver_aggregate_%s" % problem

	var raw_samples: Array = []
	for index in range(40):
		raw_samples.append(9.0 + float(index % 8) * 1.5)
	var raw_summary := aggregate_summary.duplicate(true)
	raw_summary["frametimes_ms"] = raw_samples
	var raw_scene := _scene_record(SCENE_SPECS[0], raw_summary, "crafted", "summary")
	problem = _graph_problem(raw_scene.get("frame_graph", {}), GRAPH_SOURCE_SUMMARY, 0)
	if problem != "":
		return "resolver_summary_%s" % problem

	var fixture_scene := _fixture_scene(SCENE_SPECS[0], "crafted")
	problem = _graph_problem(fixture_scene.get("frame_graph", {}), GRAPH_SOURCE_FIXTURE, 0)
	if problem != "":
		return "resolver_fixture_%s" % problem
	return ""

func _export_graph_problem(original: Dictionary, exported: Dictionary, index: int) -> String:
	if not exported.has("frame_graph"):
		return "export_graph_%d" % index
	var exported_value: Variant = exported.get("frame_graph", {})
	if typeof(exported_value) != TYPE_DICTIONARY:
		return "export_graph_type_%d" % index
	var exported_graph: Dictionary = exported_value
	var original_value: Variant = original.get("frame_graph", {})
	var original_graph: Dictionary = original_value if typeof(original_value) == TYPE_DICTIONARY else {}
	var exported_source := str(exported_graph.get("source", ""))
	if exported_source == "" or exported_source != str(original_graph.get("source", "")):
		return "export_graph_source_%d" % index
	var exported_samples_value: Variant = exported_graph.get("samples", [])
	var original_samples_value: Variant = original_graph.get("samples", [])
	if typeof(exported_samples_value) != TYPE_ARRAY or typeof(original_samples_value) != TYPE_ARRAY:
		return "export_graph_samples_%d" % index
	var exported_samples: Array = exported_samples_value
	var original_samples: Array = original_samples_value
	if exported_samples.size() < 2 or exported_samples.size() != original_samples.size():
		return "export_graph_samples_%d" % index
	for key in ["p50_ms", "p95_ms", "p99_ms", "max_ms"]:
		var original_value_number := _number(original_graph.get(key), -1.0)
		var exported_value_number := _number(exported_graph.get(key), -1.0)
		if original_value_number <= 0.0 or exported_value_number <= 0.0:
			return "export_graph_%s_missing_%d" % [str(key), index]
		if absf(original_value_number - exported_value_number) > 0.001:
			return "export_graph_%s_%d" % [str(key), index]
	return ""

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

	if status == "ok" and _graph_count != 3:
		status = "graph_controls_%d" % _graph_count
	if status == "ok":
		for index in range(scenes.size()):
			var scene: Dictionary = scenes[index]
			var source := str(scene.get("source", GRAPH_SOURCE_FIXTURE))
			var problem := _graph_problem(scene.get("frame_graph", {}), source, index)
			if problem != "":
				status = problem
				break
	if status == "ok":
		var resolver_problem := _smoke_graph_sources()
		if resolver_problem != "":
			status = resolver_problem

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
					else:
						var graph_problem := _export_graph_problem(original, exported_scene, index)
						if graph_problem != "":
							status = graph_problem

	if status == "ok":
		var commands := _run_commands(30.0, 3.0)
		if commands.size() != 3:
			status = "run_commands=%d" % commands.size()
		for index in range(commands.size()):
			if status != "ok":
				break
			var command: Dictionary = commands[index]
			var args: PackedStringArray = command.get("args", PackedStringArray())
			var route_path := str(command.get("route", ""))
			var args_ok := args.has("--city-tier") and args.has("--route") and args.has("--output")
			args_ok = args_ok and args.has("--duration") and args.has("--warmup")
			if not args_ok:
				status = "run_args_%d" % index
			elif str(command.get("tier", "")) != expected_ids[index]:
				status = "run_tier_%d" % index
			elif not FileAccess.file_exists(route_path):
				status = "run_route_%d" % index
			else:
				print("BENCH_SCREEN_RUN tier=%s scene=%s route=%s output=%s duration=%.1f warmup=%.1f" % [
					str(command.get("tier", "")),
					BENCHMARK_SCENE,
					route_path,
					str(command.get("raw_output", "")),
					float(command.get("duration_s", 0.0)),
					float(command.get("warmup_s", 0.0)),
				])
	if status == "ok":
		var graph_sources: Array = []
		for scene_value in scenes:
			if typeof(scene_value) == TYPE_DICTIONARY:
				var graph_value: Variant = (scene_value as Dictionary).get("frame_graph", {})
				if typeof(graph_value) == TYPE_DICTIONARY:
					graph_sources.append(str((graph_value as Dictionary).get("source", "")))
		print("BENCH_SCREEN_GRAPH_SOURCES %s" % ",".join(PackedStringArray(graph_sources)))
		print("BENCH_SCREEN_GRAPHS OK scenes=3")
	if scenes.size() == 3:
		print("BENCH_SCREEN_VALUES early=%.1f/%s mid=%.1f/%s late=%.1f/%s" % [
			_number((scenes[0] as Dictionary).get("median_fps")),
			str((scenes[0] as Dictionary).get("rating", "")),
			_number((scenes[1] as Dictionary).get("median_fps")),
			str((scenes[1] as Dictionary).get("rating", "")),
			_number((scenes[2] as Dictionary).get("median_fps")),
			str((scenes[2] as Dictionary).get("rating", "")),
		])
	print("BENCH_SCREEN status=%s scenes=%d rating=%s mode=smoke" % [
		status,
		scenes.size(),
		str(overall.get("rating", "unknown")),
	])
	get_tree().quit(0 if status == "ok" else 1)
