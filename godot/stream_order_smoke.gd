extends Node3D

## Deterministic terrain streaming commit-order smoke.
##
## Stages the camera at fixed chunk centers and enables `commit_barrier`, so
## every stage drains the request queue and worker pool before any ready chunk
## is committed. The committed chunk sequence is then a pure function of the
## centers and the priority order defined by `TerrainStreamer` — never worker
## completion timing.
##
## Run:
##   godot --headless --path godot res://stream_order_smoke.tscn -- \
##     --output /tmp/stream_order.json
##
## `scripts/smoke_stream_order.sh` runs this twice and compares the full
## sequence, checksum, commit count, and final loaded count.

const TerrainStreamer = preload("res://terrain_streamer.gd")
const CHUNK_METERS := 256.0
## Deterministic pass/fail bound: each stage waits at most this many process
## frames for the request queue and worker pool to drain. Frame count, not wall
## time, decides the result, so a loaded CI box cannot flake the sequence.
const STAGE_FRAME_BUDGET := 12000
## Liveness backstop only, never a passing criterion: a wedged worker thread can
## stop frames from arriving, and this turns that into a structured failure
## instead of a hung smoke. It is deliberately far above what the frame budget
## needs (a stage drains ~25 chunks) and stays under the wrapper's 120 s
## per-run `timeout` so the failure is reported by this script, not killed.
const STAGE_LIVENESS_BACKSTOP_MS := 90000
const SETTLE_FRAMES := 3

var streamer
var camera: Camera3D
var centers: Array[Vector2i] = [
	Vector2i(0, 0),
	Vector2i(1, 0),
	Vector2i(2, 0),
	Vector2i(1, 0),
]
var output_path := ""
var stages: Array[Dictionary] = []

func _ready() -> void:
	output_path = _arg(OS.get_cmdline_user_args(), "--output", "")
	_build()
	_run()

func _arg(args: PackedStringArray, name: String, fallback: String) -> String:
	var index := args.find(name)
	if index >= 0 and index + 1 < args.size():
		return args[index + 1]
	return fallback

func _build() -> void:
	streamer = TerrainStreamer.new()
	streamer.seed = 2026
	streamer.view_radius = 2
	streamer.keep_radius = 3
	streamer.builds_per_frame = 2
	streamer.commit_barrier = true
	streamer.track_commit_sequence = true
	add_child(streamer)

	camera = Camera3D.new()
	camera.current = true
	camera.far = 1500.0
	add_child(camera)
	_move_camera(centers[0])
	streamer.bind_camera(camera)

func _move_camera(center: Vector2i) -> void:
	camera.position = Vector3(
		center.x * CHUNK_METERS + CHUNK_METERS * 0.5,
		70.0,
		center.y * CHUNK_METERS + CHUNK_METERS * 0.5
	)

func _settle() -> void:
	for _i in range(SETTLE_FRAMES):
		await get_tree().process_frame

func _await_drained(status: Dictionary) -> bool:
	var deadline := Time.get_ticks_msec() + STAGE_LIVENESS_BACKSTOP_MS
	var frames := 0
	while not streamer.is_drained():
		if not streamer.is_processing():
			status["error"] = "streamer_stopped"
			return false
		frames += 1
		if frames > STAGE_FRAME_BUDGET:
			status["error"] = "frame_budget_exceeded"
			return false
		if Time.get_ticks_msec() > deadline:
			status["error"] = "liveness_timeout"
			return false
		await get_tree().process_frame
	return true

func _run() -> void:
	var started := Time.get_ticks_msec()
	var status: Dictionary = {"error": ""}

	await _settle()
	for center in centers:
		_move_camera(center)
		await _settle()
		var before_commits := int(streamer.chunks_applied)
		var drained: bool = await _await_drained(status)
		if not drained:
			_fail(str(status.get("error", "drain_failed")), started)
			return
		var stage := {
			"center": [center.x, center.y],
			"commits": int(streamer.chunks_applied) - before_commits,
			"loaded": streamer.chunk_count(),
			"checksum": streamer.commit_checksum_hex(),
		}
		stages.append(stage)
		print("STREAM_ORDER_STAGE center=%d,%d commits=%d loaded=%d checksum=%s" % [
			center.x,
			center.y,
			int(stage["commits"]),
			int(stage["loaded"]),
			str(stage["checksum"]),
		])

	var sequence: Array[String] = []
	for coord in streamer.commit_sequence:
		sequence.append("%d,%d" % [coord.x, coord.y])
	var result := {
		"format_version": 1,
		"suite": "stream_order_smoke",
		"status": "ok",
		"checksum": streamer.commit_checksum_hex(),
		"commits": int(streamer.chunks_applied),
		"loaded": streamer.chunk_count(),
		"sequence": sequence,
		"sequence_text": streamer.commit_sequence_text(),
		"stages": stages,
		"elapsed_ms": Time.get_ticks_msec() - started,
	}
	_write(result)
	print("STREAM_ORDER_SMOKE status=ok stream_order=ok checksum=%s commits=%d loaded=%d stages=%d elapsed_ms=%d" % [
		str(result["checksum"]),
		int(result["commits"]),
		int(result["loaded"]),
		stages.size(),
		int(result["elapsed_ms"]),
	])
	get_tree().quit(0)

func _fail(reason: String, started: int) -> void:
	_write({
		"format_version": 1,
		"suite": "stream_order_smoke",
		"status": "failed",
		"error": reason,
		"checksum": streamer.commit_checksum_hex() if streamer != null else "",
		"commits": int(streamer.chunks_applied) if streamer != null else 0,
		"loaded": streamer.chunk_count() if streamer != null else 0,
		"stages": stages,
		"elapsed_ms": Time.get_ticks_msec() - started,
	})
	print("STREAM_ORDER_SMOKE status=failed stream_order=failed error=%s checksum=%s commits=%d loaded=%d elapsed_ms=%d" % [
		reason,
		streamer.commit_checksum_hex() if streamer != null else "",
		int(streamer.chunks_applied) if streamer != null else 0,
		streamer.chunk_count() if streamer != null else 0,
		Time.get_ticks_msec() - started,
	])
	get_tree().quit(1)

func _write(result: Dictionary) -> void:
	if output_path == "":
		return
	var directory := output_path.get_base_dir()
	if directory != "":
		DirAccess.make_dir_recursive_absolute(directory)
	var file := FileAccess.open(output_path, FileAccess.WRITE)
	if file == null:
		push_error("Could not write stream order smoke result: " + output_path)
		return
	file.store_string(JSON.stringify(result, "  "))
	file.close()
