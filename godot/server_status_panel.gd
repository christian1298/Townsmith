extends Control

## Standalone read-only server status panel (developer tool).
##
## Connects to `townsmith-server --listen PORT`, completes the v0 hello/welcome
## handshake, sends one `server_status` (Viewer), renders the authoritative
## counters, then sends `disconnect` and quits. It never mutates the world,
## saves, or changes the protocol.
##
## Run:
##   godot --path godot res://server_status_panel.tscn -- --port 47015
##   godot --headless --path godot res://server_status_panel.tscn -- \
##     --smoke --port 47015
##
## `--smoke` prints one `SERVER_STATUS_PANEL` marker. A success prints
## `status=ok ...` and exits 0; the one-shot path also prints one
## `SERVER_STATUS_PANEL_PAUSED status=ok paused=<bool> running=<bool>` line for
## the "Time" row. A failure prints `status=fail code=<stable>` and
## exits 1, where the stable code is one of:
##   status.connect  connection refused, lost while connecting, or a timeout
##   status.frame    malformed frame (length, JSON, or unexpected message)
##   status.shape    status object with missing/typed/out-of-range fields
##   status.closed   goodbye/EOF before the status answer
##   <server code>   a rejected handshake, e.g. content_hash.mismatch
## `--expect-fail CODE` turns a matching failure into a passing test (exit 0)
## and still prints the fail marker; a mismatch exits 1.
## `--fixture-json PATH` skips TCP and feeds one `server_status` object through
## the same parse/render path, so shape checks need no server.
##
## The additive `paused` field is optional: an older server that omits it
## decodes as running (`false`), and a non-boolean value is `status.shape`.
## `paused` is rendered only; it is not an integer counter and is not part of
## the watch checksum.
##
## Opt-in watch mode keeps the one connection open and polls `server_status`
## repeatedly, updating the labels in place:
##   godot --path godot res://server_status_panel.tscn -- --port 47015 --watch
##   godot --headless --path godot res://server_status_panel.tscn -- \
##     --smoke --watch --iterations 3 --interval-ms 50 --port 47015
## `--iterations` defaults to 3 and `--interval-ms` to 500. When supplied, the
## values must be integers in 1..100 and 50..5000; anything else (zero,
## negative, non-numeric, out of range) is rejected before connecting with
##   SERVER_STATUS_WATCH status=fail iteration=0 code=status.watch_args
##     arg=<flag>
## and exit 1. A watch run never reconnects: the first frame, shape, ordering,
## timeout, closed, or reject error prints
##   SERVER_STATUS_WATCH status=fail iteration=<n> code=<stable>
## and exits 1. A status wait past its deadline fails with
## `status.watch_timeout`; a connect or hello-handshake timeout stays
## `status.connect`. A length prefix left without its full payload at EOF is
## `status.frame`, not `status.closed`. Every failure path closes the one socket
## exactly once and exits; no reconnect is attempted. `server_status` is
## read-only and repeatable and does not advance the tick, so no `ping` keepalive
## is sent between polls. Without `--smoke`, watch mode keeps polling until
## Esc/Q.
##
## A clean watch run prints one aggregate marker:
##   SERVER_STATUS_WATCH status=ok iterations=N checksum=<16hex>
##     tick_first=<n> tick_last=<n>
## `checksum` is the first 16 hex characters of the SHA-256 over the ascending
## `tick|population|settlements|checksum` lines (one LF per line). The one-shot
## `SERVER_STATUS_PANEL` marker is unchanged.
##
## The panel also prints the rendered pause state once per one-shot or fixture
## status:
##   SERVER_STATUS_PANEL_PAUSED status=ok paused=<true|false> running=<...>
## Watch mode never prints that line; the watch checksum excludes `paused`, so
## the aggregate marker never moves when the world pauses or resumes.
## `scripts/smoke_server_status_panel.sh` covers the valid path twice plus the
## connect/reject/shape fixtures and a two-run watch pass/fail matrix.
##
## Screen-reader semantics: the title, every value row, and the status line
## carry `accessibility_name`/`accessibility_description`. The status line is a
## polite live region while snapshots tick and becomes assertive on failure.
## `accessibility_snapshot()` exposes the property state for the smoke. The
## panel self-checks that copy on the first rendered snapshot (watch start) and
## on every terminal path, printing one stable line:
##   SERVER_STATUS_PANEL_A11Y status=ok checks=44
## A regression in the panel's own copy prints `status=fail check=<id>` and
## fails the run with `status.a11y`; no protocol input can trigger that check.

enum Phase {
	CONNECTING,
	WELCOME_WAIT,
	STATUS_SENT,
	WATCH_WAIT,
	DISCONNECT_SENT,
}

const PROTOCOL_VERSION := 1
const MAX_FRAME_LEN := 1024 * 1024
const DEFAULT_HOST := "127.0.0.1"
const DEFAULT_PORT := 47015
const DEFAULT_NAME := "status-panel"
const CONNECT_TIMEOUT_MS := 5000
const STEP_TIMEOUT_MS := 5000
const WATCH_DEFAULT_ITERATIONS := 3
const WATCH_MIN_ITERATIONS := 1
const WATCH_MAX_ITERATIONS := 100
const WATCH_DEFAULT_INTERVAL_MS := 500
const WATCH_MIN_INTERVAL_MS := 50
const WATCH_MAX_INTERVAL_MS := 5000
const WATCH_ARGS_CODE := "status.watch_args"
# Same 64-character placeholder hash the Rust loopback harness uses.
const CONTENT_HASH := "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"

const STATUS_FIELDS := [
	"tick",
	"population",
	"balance",
	"roads",
	"buildings",
	"settlements",
	"placements",
	"online",
	"chat_seq",
	"locks",
]

## `Control.accessibility_live` values (Godot 4.7 exposes the property but not
## named enum constants). The ticking status snapshot is polite; a failure
## interrupts as assertive.
const A11Y_LIVE_OFF := 0
const A11Y_LIVE_POLITE := 1
const A11Y_LIVE_ASSERTIVE := 2

## Guards a description from re-reading a long value line. The row's own label
## already carries the number; the description stays a short sentence.
const A11Y_MAX_DESCRIPTION := 160

## Screen-reader copy for the value rows, keyed by the visible row id. Entry 0
## is the control name, entry 1 the sentence used before a status arrives, and
## entry 2 the same sentence with the rendered value. The panel is English-only
## like its visible labels, so this stays a literal table with no locale key.
const A11Y_ROWS := {
	"Tick": ["Tick", "Server tick.", "Server tick, currently %s."],
	"Population": ["Population", "Residents tracked by the server.", "Population, currently %s."],
	"Balance": ["Balance", "Treasury balance.", "Treasury balance, currently %s."],
	"Roads": ["Roads", "Road records in the world.", "Roads, currently %s."],
	"Buildings": ["Buildings", "Building records in the world.", "Buildings, currently %s."],
	"Settlements": ["Settlements", "Settlements in the world.", "Settlements, currently %s."],
	"Placements": ["Placements", "Blueprint placements recorded.", "Placements, currently %s."],
	"Online": ["Online players", "Players connected right now.", "Online players, currently %s."],
	"Chat seq": ["Chat sequence", "Chat message sequence number.", "Chat seq, currently %s."],
	"Locks": ["Locks", "Edit locks currently held.", "Locks, currently %s."],
	"Checksum": ["Checksum", "Stable world checksum for this snapshot; the row shows the full value.", "Stable world checksum for this snapshot; the row shows the full value."],
}

## Deterministic snapshot and self-check order: the heading, every value row,
## then the status line.
const A11Y_KEYS := [
	"title",
	"tick",
	"population",
	"balance",
	"roads",
	"buildings",
	"settlements",
	"placements",
	"online",
	"chat_seq",
	"locks",
	"time",
	"checksum",
	"status",
]

var _peer: StreamPeerTCP
var _peer_closed := false
var _phase: int = Phase.CONNECTING
var _rx := PackedByteArray()
var _deadline_ms := 0
var _finished := false

var _smoke := false
var _host := DEFAULT_HOST
var _port := DEFAULT_PORT
var _player_name := DEFAULT_NAME
var _timeout_ms := STEP_TIMEOUT_MS
var _expect_fail := ""
var _fixture_mode := false

var _watch := false
var _watch_iterations := WATCH_DEFAULT_ITERATIONS
var _watch_interval_ms := WATCH_DEFAULT_INTERVAL_MS
var _polls_done := 0
var _next_poll_ms := 0
var _watch_entries: Array = []
var _watch_tick_first := 0
var _watch_tick_last := 0
var _watch_args_invalid := false

var _frames_sent := 0
var _frames_received := 0
var _status: Dictionary = {}
var _checksum := ""
var _paused := false

var _status_label: Label
var _value_labels: Dictionary = {}
var _title: Label
var _a11y_assertive := false
var _a11y_start_emitted := false
var _a11y_in_emit := false

func _ready() -> void:
	var args := OS.get_cmdline_user_args()
	_smoke = args.has("--smoke")
	_host = _arg(args, "--host", DEFAULT_HOST)
	_player_name = _arg(args, "--name", DEFAULT_NAME)
	_timeout_ms = _positive_int(_arg(args, "--timeout-ms", ""), STEP_TIMEOUT_MS)
	_expect_fail = _arg(args, "--expect-fail", "")
	_watch = args.has("--watch")
	_build_ui()
	if _watch:
		# Argument errors are deterministic and must not open a socket.
		var iterations_text := _arg(args, "--iterations", "")
		if not iterations_text.is_empty():
			if not _is_bounded_int(iterations_text, WATCH_MIN_ITERATIONS, WATCH_MAX_ITERATIONS):
				_watch_args_invalid = true
				_fail(WATCH_ARGS_CODE, "arg=--iterations")
				return
			_watch_iterations = iterations_text.to_int()
		var interval_text := _arg(args, "--interval-ms", "")
		if not interval_text.is_empty():
			if not _is_bounded_int(interval_text, WATCH_MIN_INTERVAL_MS, WATCH_MAX_INTERVAL_MS):
				_watch_args_invalid = true
				_fail(WATCH_ARGS_CODE, "arg=--interval-ms")
				return
			_watch_interval_ms = interval_text.to_int()

	var fixture_path := _arg(args, "--fixture-json", "")
	if not fixture_path.is_empty():
		_run_fixture(fixture_path)
		return

	var port_text := _arg(args, "--port", str(DEFAULT_PORT))
	if not port_text.is_valid_int():
		_fail("invalid_port")
		return
	_port = port_text.to_int()
	if _port < 1 or _port > 65535:
		_fail("port_out_of_range")
		return
	if _player_name.strip_edges().is_empty() or _player_name.length() > 32:
		_fail("invalid_player_name")
		return

	_peer = StreamPeerTCP.new()
	var error := _peer.connect_to_host(_host, _port)
	if error != OK:
		_fail("status.connect")
		return
	_arm_deadline(CONNECT_TIMEOUT_MS)
	_set_status("Connecting to %s:%d..." % [_host, _port])

## Feeds one status object through the live parse/render path without a socket.
## Only the test harness sets `--fixture-json`; the default path is untouched.
func _run_fixture(path: String) -> void:
	_fixture_mode = true
	_set_status("Loading fixture %s..." % path)
	if not FileAccess.file_exists(path):
		_fail("status.fixture")
		return
	var text := FileAccess.get_file_as_string(path)
	if text.is_empty():
		_fail("status.fixture")
		return
	var parsed: Variant = _parse_json_object(text)
	if typeof(parsed) != TYPE_DICTIONARY:
		# A fixture that is not one JSON object is a malformed frame payload,
		# not a structurally valid status with missing fields.
		_fail("status.frame")
		return
	_phase = Phase.STATUS_SENT
	_on_server_status(parsed)

func _process(_delta: float) -> void:
	if _finished:
		return
	if Time.get_ticks_msec() > _deadline_ms:
		_fail(_timeout_code())
		return
	if _peer == null:
		_fail("status.connect")
		return

	_peer.poll()
	if _phase == Phase.CONNECTING:
		match _peer.get_status():
			StreamPeerTCP.STATUS_CONNECTED:
				_on_connected()
			StreamPeerTCP.STATUS_ERROR:
				_fail("status.connect")
			StreamPeerTCP.STATUS_NONE:
				_fail("status.connect")
		return

	_drain()
	if _finished:
		return
	if _peer.get_status() != StreamPeerTCP.STATUS_CONNECTED:
		# Read the final goodbye/reject before treating FIN as a failure.
		_drain()
		if not _finished:
			# A buffered length prefix or partial payload is a truncated frame,
			# not a clean close between iterations.
			if _rx.is_empty():
				_fail("status.closed")
			else:
				_fail("status.frame")
		return
	# Watch mode keeps the socket open and polls on its own interval. A closed
	# socket is checked above, so a peer that vanished between polls is fatal
	# rather than something to reconnect.
	if _phase == Phase.WATCH_WAIT and Time.get_ticks_msec() >= _next_poll_ms:
		_request_status()

func _on_connected() -> void:
	_set_status("Connected; sending hello.")
	if not _send({
		"type": "hello",
		"protocol_version": PROTOCOL_VERSION,
		"content_hash": CONTENT_HASH,
		"player_name": _player_name,
	}):
		return
	_phase = Phase.WELCOME_WAIT
	_arm_deadline(_timeout_ms)

func _drain() -> void:
	# A peer that already reported EOF can no longer be read; requesting the
	# available byte count on a closed socket logs an engine error. Buffered
	# frames in `_rx` are still parsed below.
	if _peer != null and _peer.get_status() == StreamPeerTCP.STATUS_CONNECTED:
		var available := _peer.get_available_bytes()
		if available > 0:
			var result := _peer.get_partial_data(available)
			if result[0] != OK:
				_fail("status.closed")
				return
			_rx.append_array(result[1])

	while not _finished and _rx.size() >= 4:
		var length := _read_u32_be(_rx, 0)
		if length == 0:
			_fail("status.frame")
			return
		if length > MAX_FRAME_LEN:
			_fail("status.frame")
			return
		if _rx.size() < 4 + length:
			return
		var payload := _rx.slice(4, 4 + length)
		_rx = _rx.slice(4 + length)
		_frames_received += 1
		var parsed: Variant = _parse_json_object(payload.get_string_from_utf8())
		if typeof(parsed) != TYPE_DICTIONARY:
			_fail("status.frame")
			return
		_handle_message(parsed)

func _handle_message(message: Dictionary) -> void:
	var tag := str(message.get("type", ""))
	match tag:
		"welcome":
			_on_welcome(message)
		"server_status":
			_on_server_status(message)
		"goodbye":
			_on_goodbye()
		"reject":
			var code := str(message.get("code", ""))
			_fail(code if not code.is_empty() else "status.reject")
		_:
			_fail("status.frame")

func _on_welcome(message: Dictionary) -> void:
	if _phase != Phase.WELCOME_WAIT:
		_fail("status.frame")
		return
	var protocol_version := _as_int(message.get("protocol_version"))
	if protocol_version != PROTOCOL_VERSION:
		_fail("status.frame")
		return
	if str(message.get("session_id", "")).is_empty():
		_fail("status.frame")
		return
	if bool(message.get("resumed", false)):
		_fail("status.frame")
		return
	_set_status("Welcome; requesting server status.")
	if not _request_status():
		return

## Sends one `server_status` request and waits for its answer. Both one-shot and
## watch mode use this path; watch mode schedules the next request after a reply.
func _request_status() -> bool:
	if _watch:
		_set_status("Watching: request %d." % (_polls_done + 1))
	elif _phase != Phase.WELCOME_WAIT:
		_set_status("Requesting server status.")
	if not _send({"type": "server_status"}):
		return false
	_phase = Phase.STATUS_SENT
	_arm_deadline(_timeout_ms)
	return true

func _on_server_status(message: Dictionary) -> void:
	if _phase != Phase.STATUS_SENT:
		_fail("status.frame")
		return
	var shape_error := _read_status(message)
	if not shape_error.is_empty():
		_fail(shape_error)
		return

	_refresh_ui()
	# The first rendered snapshot is the watch start (and the only snapshot of a
	# one-shot or fixture run); later polls update the polite live region without
	# printing another marker.
	if not _a11y_start_emitted:
		_a11y_start_emitted = true
		_emit_a11y()
	if _watch and not _fixture_mode:
		_on_watch_status()
		return

	_set_status("Status received; disconnecting.")
	if _expect_fail.is_empty():
		print(
			(
				"SERVER_STATUS_PANEL status=ok tick=%d population=%d"
				+ " settlements=%d checksum=%s"
			)
			% [
				int(_status["tick"]),
				int(_status["population"]),
				int(_status["settlements"]),
				_checksum,
			]
		)
		print(
			"SERVER_STATUS_PANEL_PAUSED status=ok paused=%s running=%s"
			% [
				"true" if _paused else "false",
				"false" if _paused else "true",
			]
		)
	if _fixture_mode:
		_finish_ok()
		return
	if not _send({"type": "disconnect"}):
		return
	_phase = Phase.DISCONNECT_SENT
	_arm_deadline(_timeout_ms)

## Records one successful watch poll, then either schedules the next request or
## (in smoke mode, after N polls) asks the server to close the session normally.
func _on_watch_status() -> void:
	var tick := int(_status["tick"])
	if _polls_done > 0 and tick < _watch_tick_last:
		# A server that advances the world never moves the tick backwards.
		_fail("status.watch_order")
		return
	_record_poll()
	if _smoke and _polls_done >= _watch_iterations:
		_set_status("Watch complete; disconnecting.")
		if not _send({"type": "disconnect"}):
			return
		_phase = Phase.DISCONNECT_SENT
		_arm_deadline(_timeout_ms)
		return
	_next_poll_ms = Time.get_ticks_msec() + _watch_interval_ms
	# A stalled frame longer than interval + step timeout is a failure, not a
	# reason to reconnect.
	_arm_deadline(_watch_interval_ms + _timeout_ms)
	_set_status(
		"Watching: %d poll(s), next in %d ms." % [_polls_done, _watch_interval_ms]
	)
	_phase = Phase.WATCH_WAIT

func _record_poll() -> void:
	var tick := int(_status["tick"])
	var line := (
		"%d|%d|%d|%s"
		% [
			tick,
			int(_status["population"]),
			int(_status["settlements"]),
			_checksum,
		]
	)
	_watch_entries.append({"tick": tick, "line": line})
	if _polls_done == 0:
		_watch_tick_first = tick
	_watch_tick_last = tick
	_polls_done += 1

## First 16 hex of SHA-256 over ascending `tick|population|settlements|checksum`
## lines, each terminated by LF. Ticks are sorted numerically; equal ticks keep
## a deterministic line order.
func _watch_checksum() -> String:
	var ordered := _watch_entries.duplicate()
	ordered.sort_custom(
		func(a: Dictionary, b: Dictionary) -> bool:
			if int(a["tick"]) != int(b["tick"]):
				return int(a["tick"]) < int(b["tick"])
			return str(a["line"]) < str(b["line"])
	)
	var text := ""
	for entry in ordered:
		text += str(entry["line"]) + "\n"
	var context := HashingContext.new()
	context.start(HashingContext.HASH_SHA256)
	context.update(text.to_utf8_buffer())
	return context.finish().hex_encode().substr(0, 16)

func _finish_watch_ok() -> void:
	if _finished:
		return
	if _smoke and _polls_done != _watch_iterations:
		# The success marker promises exactly the configured number of polls.
		_fail("status.watch_incomplete")
		return
	set_process(false)
	_set_status("Watch complete.")
	# Terminal a11y self-check (the watch stop line). It may fail the run, so
	# only mark the run finished afterwards.
	_emit_a11y()
	if _finished:
		return
	_finished = true
	_close_peer()
	print(
		(
			"SERVER_STATUS_WATCH status=ok iterations=%d checksum=%s"
			+ " tick_first=%d tick_last=%d"
		)
		% [_polls_done, _watch_checksum(), _watch_tick_first, _watch_tick_last]
	)
	get_tree().quit(0)

## Esc/Q stops an interactive watch run. It prints the same aggregate marker
## using the polls completed so far.
func _quit_watch() -> void:
	if _finished:
		return
	_finish_watch_ok()

func _input(event: InputEvent) -> void:
	if not _watch or _smoke or _finished:
		return
	if event is InputEventKey:
		var key_event := event as InputEventKey
		if not key_event.pressed or key_event.echo:
			return
		if key_event.keycode == KEY_ESCAPE or key_event.keycode == KEY_Q:
			_quit_watch()

## Validates and stores one `server_status` object. Returns "" on success or
## the stable failure code. Unknown fields are tolerated: the protocol is
## forward-compatible and the panel only renders the fields it knows.
func _read_status(message: Dictionary) -> String:
	var status: Dictionary = {}
	for field in STATUS_FIELDS:
		if not message.has(field):
			return "status.shape"
		var value: Variant = message[field]
		if not _is_integer(value):
			return "status.shape"
		status[field] = int(value)
	# `balance` may be negative (debt); the counters cannot be.
	for field in STATUS_FIELDS:
		if field == "balance":
			continue
		if int(status[field]) < 0:
			return "status.shape"
	if int(status["online"]) < 1:
		return "status.shape"

	var checksum_value: Variant = message.get("checksum")
	if typeof(checksum_value) != TYPE_STRING or not _is_hex16(str(checksum_value)):
		return "status.shape"

	# `paused` is additive and optional: a missing key is a pre-pause server and
	# decodes as running. A present value must be a JSON boolean, so it never
	# enters the integer counter loop above.
	var paused := false
	if message.has("paused"):
		var paused_value: Variant = message["paused"]
		if typeof(paused_value) != TYPE_BOOL:
			return "status.shape"
		paused = bool(paused_value)

	_status = status
	_checksum = str(checksum_value)
	_paused = paused
	return ""

func _on_goodbye() -> void:
	if _phase == Phase.DISCONNECT_SENT:
		if _watch and not _fixture_mode:
			_finish_watch_ok()
		else:
			_finish_ok()
		return
	# The server closed before answering the status request.
	_fail("status.closed")

func _finish_ok() -> void:
	if _finished:
		return
	if not _expect_fail.is_empty():
		_fail("status.unexpected_ok")
		return
	set_process(false)
	_set_status("Disconnected.")
	# Terminal a11y self-check (the stop line for a one-shot or fixture run).
	# It may fail the run, so only mark the run finished afterwards.
	_emit_a11y()
	if _finished:
		return
	_finished = true
	_close_peer()
	if not _smoke:
		print("SERVER_STATUS_PANEL status=ok rendered=true")
	get_tree().quit(0)

func _send(message: Dictionary) -> bool:
	if _peer == null or _peer.get_status() != StreamPeerTCP.STATUS_CONNECTED:
		_fail("status.closed")
		return false
	var payload := JSON.stringify(message).to_utf8_buffer()
	if payload.is_empty():
		_fail("status.frame")
		return false
	if payload.size() > MAX_FRAME_LEN:
		_fail("status.frame")
		return false
	var frame := PackedByteArray()
	frame.resize(4)
	_write_u32_be(frame, 0, payload.size())
	frame.append_array(payload)
	var error := _peer.put_data(frame)
	if error != OK:
		_fail("status.closed")
		return false
	_frames_sent += 1
	return true

func _fail(code: String, extra := "") -> void:
	if _finished:
		return
	_finished = true
	set_process(false)
	# A failure is the assertive state for the status live region; the terminal
	# self-check asserts that mode and the fallback row copy.
	_a11y_assertive = true
	_set_status("Failed: " + code)
	_emit_a11y()
	_close_peer()
	if _watch and not _fixture_mode:
		# Watch mode never reconnects. The marker is the test surface: one
		# stable line naming the iteration that failed and the stable code.
		var suffix := (" " + extra) if not extra.is_empty() else ""
		print(
			"SERVER_STATUS_WATCH status=fail iteration=%d code=%s%s"
			% [_watch_failure_iteration(), code, suffix]
		)
		get_tree().quit(1)
		return
	var suffix := (" " + extra) if not extra.is_empty() else ""
	if not _expect_fail.is_empty() and _expect_fail != code:
		print(
			"SERVER_STATUS_PANEL_EXPECT status=mismatch expected=%s actual=%s"
			% [_expect_fail, code]
		)
	print("SERVER_STATUS_PANEL status=fail code=%s%s" % [code, suffix])
	# A matching --expect-fail is the test passing; otherwise a failure is fatal.
	get_tree().quit(0 if not _expect_fail.is_empty() and _expect_fail == code else 1)

## Connect/hello timeouts stay `status.connect`; once the handshake is done, a
## watch poll or disconnect wait that runs out is its own stable code.
func _timeout_code() -> String:
	if _watch and not _fixture_mode:
		if _phase != Phase.CONNECTING and _phase != Phase.WELCOME_WAIT:
			return "status.watch_timeout"
	return "status.connect"

## Closes the single watch connection at most once. Every terminal path calls
## this before quitting, so no socket is left behind.
func _close_peer() -> void:
	if _peer == null or _peer_closed:
		return
	_peer_closed = true
	_peer.disconnect_from_host()
	_peer = null

## Iteration (1-based) whose request failed. Smoke runs clamp to the configured
## count so a missing goodbye after the last answer still reports iteration=N.
## Argument validation fails before iteration 1 and reports 0.
func _watch_failure_iteration() -> int:
	if _watch_args_invalid:
		return 0
	var iteration := _polls_done + 1
	if _smoke:
		return clampi(iteration, 1, _watch_iterations)
	return maxi(iteration, 1)

func _build_ui() -> void:
	var margin := MarginContainer.new()
	margin.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	margin.add_theme_constant_override("margin_left", 12)
	margin.add_theme_constant_override("margin_top", 12)
	margin.add_theme_constant_override("margin_right", 12)
	margin.add_theme_constant_override("margin_bottom", 12)
	add_child(margin)

	var root := VBoxContainer.new()
	root.add_theme_constant_override("separation", 8)
	margin.add_child(root)

	_title = Label.new()
	_title.text = "Server status"
	_title.add_theme_font_size_override("font_size", 18)
	root.add_child(_title)

	_status_label = Label.new()
	_status_label.text = "Starting..."
	root.add_child(_status_label)

	var grid := GridContainer.new()
	grid.columns = 2
	grid.add_theme_constant_override("h_separation", 16)
	grid.add_theme_constant_override("v_separation", 2)
	root.add_child(grid)

	for field in [
		["Tick", "Tick"],
		["Population", "Population"],
		["Balance", "Balance"],
		["Roads", "Roads"],
		["Buildings", "Buildings"],
		["Settlements", "Settlements"],
		["Placements", "Placements"],
		["Online", "Online"],
		["Chat seq", "Chat seq"],
		["Locks", "Locks"],
		["Time", "Time"],
		["Checksum", "Checksum"],
	]:
		var label := Label.new()
		label.text = field[0]
		grid.add_child(label)
		var value := Label.new()
		value.text = "-"
		value.add_theme_color_override("font_color", Color(0.75, 0.85, 1.0))
		grid.add_child(value)
		_value_labels[field[1]] = value

func _refresh_ui() -> void:
	var keys := [
		["Tick", "tick"],
		["Population", "population"],
		["Balance", "balance"],
		["Roads", "roads"],
		["Buildings", "buildings"],
		["Settlements", "settlements"],
		["Placements", "placements"],
		["Online", "online"],
		["Chat seq", "chat_seq"],
		["Locks", "locks"],
	]
	for entry in keys:
		if _value_labels.has(entry[0]) and _status.has(entry[1]):
			_value_labels[entry[0]].text = str(_status[entry[1]])
	if _value_labels.has("Time"):
		# The panel is English-only (no locale table), so keep the row literal.
		_value_labels["Time"].text = "Paused" if _paused else "Running"
	if _value_labels.has("Checksum"):
		_value_labels["Checksum"].text = _checksum
	_refresh_accessibility()

func _set_status(text: String) -> void:
	if _status_label != null:
		_status_label.text = text
		_refresh_accessibility()


## Screen-reader state for the deterministic smoke. Each value row exposes the
## name and concise description a screen reader would announce; `status` also
## exposes whether the ticking snapshot is polite or an assertive failure.
func accessibility_snapshot() -> Dictionary:
	return {
		"title": _a11y_entry(_title, "heading"),
		"tick": _a11y_entry(_value_labels.get("Tick"), "status"),
		"population": _a11y_entry(_value_labels.get("Population"), "status"),
		"balance": _a11y_entry(_value_labels.get("Balance"), "status"),
		"roads": _a11y_entry(_value_labels.get("Roads"), "status"),
		"buildings": _a11y_entry(_value_labels.get("Buildings"), "status"),
		"settlements": _a11y_entry(_value_labels.get("Settlements"), "status"),
		"placements": _a11y_entry(_value_labels.get("Placements"), "status"),
		"online": _a11y_entry(_value_labels.get("Online"), "status"),
		"chat_seq": _a11y_entry(_value_labels.get("Chat seq"), "status"),
		"locks": _a11y_entry(_value_labels.get("Locks"), "status"),
		"time": _a11y_entry(_value_labels.get("Time"), "status"),
		"checksum": _a11y_entry(_value_labels.get("Checksum"), "status"),
		"status": _a11y_entry(_status_label, "status"),
	}


## English screen-reader copy for every visible part. The panel has no locale
## table, so names and descriptions stay literals next to its visible labels.
## The status line is a polite live region while snapshots tick; a failure
## switches it to assertive so the stable error code interrupts.
func _refresh_accessibility() -> void:
	if _title != null:
		_title.accessibility_name = "Server status"
		_title.accessibility_description = (
			"Read-only snapshot of the authoritative server counters."
		)
	for field in A11Y_ROWS:
		if not _value_labels.has(field):
			continue
		var value_label: Label = _value_labels[field]
		var copy: Array = A11Y_ROWS[field]
		value_label.accessibility_name = str(copy[0])
		value_label.accessibility_description = _a11y_row_description(field, copy)
	if _value_labels.has("Time"):
		var time_label: Label = _value_labels["Time"]
		time_label.accessibility_name = "Time"
		if _status.is_empty():
			time_label.accessibility_description = (
				"Server clock state is not known yet."
			)
		else:
			time_label.accessibility_description = (
				"Server clock is paused; simulation is frozen."
				if _paused
				else "Server clock is running; simulation advances."
			)
	if _status_label != null:
		_status_label.accessibility_name = "Server status"
		_status_label.accessibility_description = _status_label.text
		_status_label.accessibility_live = (
			A11Y_LIVE_ASSERTIVE if _a11y_assertive else A11Y_LIVE_POLITE
		)


func _a11y_row_description(field: String, copy: Array) -> String:
	if field == "Checksum" or _status.is_empty():
		return str(copy[1])
	return str(copy[2]) % str(_value_labels[field].text)


## Self-check behind `SERVER_STATUS_PANEL_A11Y`. Returns the first failing
## `<entry>.<property>` id, or "" when every name/description is present and
## bounded, the Time row agrees with the rendered pause state, and the status
## live mode matches success (polite) or failure (assertive).
func _a11y_verify() -> String:
	var snapshot := accessibility_snapshot()
	for key in A11Y_KEYS:
		var entry: Dictionary = snapshot[key]
		if str(entry["name"]).strip_edges().is_empty():
			return "%s.name" % key
		var description := str(entry["description"]).strip_edges()
		if description.is_empty():
			return "%s.description" % key
		if description.length() > A11Y_MAX_DESCRIPTION:
			return "%s.length" % key
	var status_entry: Dictionary = snapshot["status"]
	var expected_live := "assertive" if _a11y_assertive else "polite"
	if str(status_entry["live"]) != expected_live:
		return "status.live"
	var time_entry: Dictionary = snapshot["time"]
	var time_text := str(time_entry["description"]).to_lower()
	if _status.is_empty():
		if not time_text.contains("not known"):
			return "time.state"
	elif not time_text.contains("paused" if _paused else "running"):
		return "time.state"
	return ""


## 3 checks per snapshot entry (name, description, bounded length) plus the
## status live mode and the Time row state. `checks` stays a fixed number so the
## marker never depends on the world's counters.
func _a11y_check_count() -> int:
	return A11Y_KEYS.size() * 3 + 2


## Prints one stable a11y marker. Called on the first rendered snapshot (watch
## start) and on every terminal path (one-shot stop, watch stop, failure). An
## internal copy regression fails the run with `status.a11y`; a re-entrant call
## from `_fail()` is suppressed so only one fail line is printed.
func _emit_a11y() -> void:
	if _a11y_in_emit:
		return
	_a11y_in_emit = true
	_refresh_accessibility()
	var failure := _a11y_verify()
	if failure.is_empty():
		print("SERVER_STATUS_PANEL_A11Y status=ok checks=%d" % _a11y_check_count())
	else:
		print(
			"SERVER_STATUS_PANEL_A11Y status=fail check=%s checks=%d"
			% [failure, _a11y_check_count()]
		)
		if not _finished:
			_fail("status.a11y")
	_a11y_in_emit = false


func _a11y_entry(control: Control, role: String) -> Dictionary:
	if control == null:
		return {
			"name": "",
			"description": "",
			"role": role,
			"class": "",
			"live": "off",
			"visible": false,
			"reachable": false,
		}
	var live := "off"
	if control.accessibility_live == A11Y_LIVE_POLITE:
		live = "polite"
	elif control.accessibility_live == A11Y_LIVE_ASSERTIVE:
		live = "assertive"
	return {
		"name": control.accessibility_name,
		"description": control.accessibility_description,
		"role": role,
		"class": control.get_class(),
		"live": live,
		"visible": control.visible,
		"reachable": control.is_inside_tree() and control.visible,
	}

func _arm_deadline(duration_ms: int) -> void:
	_deadline_ms = Time.get_ticks_msec() + duration_ms

func _is_integer(value: Variant) -> bool:
	if typeof(value) == TYPE_INT:
		return true
	if typeof(value) != TYPE_FLOAT:
		return false
	var number := float(value)
	return is_finite(number) and number == floor(number)

## Parses one JSON value without emitting an engine error for malformed input.
## Both a parse failure and a JSON `null` return null and fail the object check.
func _parse_json_object(text: String) -> Variant:
	var parser := JSON.new()
	if parser.parse(text) != OK:
		return null
	return parser.data

func _as_int(value: Variant) -> int:
	return int(value) if _is_integer(value) else -1

func _is_hex16(value: String) -> bool:
	if value.length() != 16:
		return false
	for index in range(value.length()):
		var character := value[index]
		if not (
			(character >= "0" and character <= "9")
			or (character >= "a" and character <= "f")
		):
			return false
	return true

func _arg(args: PackedStringArray, name: String, fallback: String) -> String:
	var index := args.find(name)
	if index >= 0 and index + 1 < args.size():
		return args[index + 1]
	return fallback

func _positive_int(value: String, fallback: int) -> int:
	if not value.is_valid_int():
		return fallback
	var number := value.to_int()
	return number if number > 0 else fallback

func _is_bounded_int(value: String, minimum: int, maximum: int) -> bool:
	if not value.is_valid_int():
		return false
	var number := value.to_int()
	return number >= minimum and number <= maximum

func _read_u32_be(buffer: PackedByteArray, offset: int) -> int:
	return (
		(buffer[offset] << 24)
		| (buffer[offset + 1] << 16)
		| (buffer[offset + 2] << 8)
		| buffer[offset + 3]
	)

func _write_u32_be(buffer: PackedByteArray, offset: int, value: int) -> void:
	buffer[offset] = (value >> 24) & 0xFF
	buffer[offset + 1] = (value >> 16) & 0xFF
	buffer[offset + 2] = (value >> 8) & 0xFF
	buffer[offset + 3] = value & 0xFF
