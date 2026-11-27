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
## `status=ok ...` and exits 0. A failure prints `status=fail code=<stable>` and
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
## Opt-in watch mode keeps the one connection open and polls `server_status`
## repeatedly, updating the labels in place:
##   godot --path godot res://server_status_panel.tscn -- --port 47015 --watch
##   godot --headless --path godot res://server_status_panel.tscn -- \
##     --smoke --watch --iterations 3 --interval-ms 50 --port 47015
## `--iterations` defaults to 3 (clamped 1..100) and `--interval-ms` to 500
## (clamped 50..5000). A watch run never reconnects: the first frame, shape, or
## status error prints
##   SERVER_STATUS_WATCH status=fail iteration=<n> code=<stable>
## and exits 1. `server_status` is read-only and repeatable and does not advance
## the tick, so no `ping` keepalive is sent between polls. Without `--smoke`,
## watch mode keeps polling until Esc/Q.
##
## A clean watch run prints one aggregate marker:
##   SERVER_STATUS_WATCH status=ok iterations=N checksum=<16hex>
##     tick_first=<n> tick_last=<n>
## `checksum` is the first 16 hex characters of the SHA-256 over the ascending
## `tick|population|settlements|checksum` lines (one LF per line). The one-shot
## `SERVER_STATUS_PANEL` marker is unchanged.
## `scripts/smoke_server_status_panel.sh` covers the valid path twice plus the
## connect/reject/shape fixtures and a two-run watch pass/fail matrix.

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

var _peer: StreamPeerTCP
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

var _frames_sent := 0
var _frames_received := 0
var _status: Dictionary = {}
var _checksum := ""

var _status_label: Label
var _value_labels: Dictionary = {}

func _ready() -> void:
	var args := OS.get_cmdline_user_args()
	_smoke = args.has("--smoke")
	_host = _arg(args, "--host", DEFAULT_HOST)
	_player_name = _arg(args, "--name", DEFAULT_NAME)
	_timeout_ms = _positive_int(_arg(args, "--timeout-ms", ""), STEP_TIMEOUT_MS)
	_expect_fail = _arg(args, "--expect-fail", "")
	_watch = args.has("--watch")
	_watch_iterations = _bounded_int(
		_arg(args, "--iterations", ""),
		WATCH_DEFAULT_ITERATIONS,
		WATCH_MIN_ITERATIONS,
		WATCH_MAX_ITERATIONS
	)
	_watch_interval_ms = _bounded_int(
		_arg(args, "--interval-ms", ""),
		WATCH_DEFAULT_INTERVAL_MS,
		WATCH_MIN_INTERVAL_MS,
		WATCH_MAX_INTERVAL_MS
	)
	_build_ui()

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
	var parsed: Variant = JSON.parse_string(text)
	if typeof(parsed) != TYPE_DICTIONARY:
		_fail("status.shape")
		return
	_phase = Phase.STATUS_SENT
	_on_server_status(parsed)

func _process(_delta: float) -> void:
	if _finished:
		return
	if Time.get_ticks_msec() > _deadline_ms:
		# Refused, lost, or silent: all mean the session never became usable.
		_fail("status.connect")
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
			_fail("status.closed")
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
		var parsed: Variant = JSON.parse_string(payload.get_string_from_utf8())
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
	_finished = true
	set_process(false)
	_set_status("Watch complete.")
	if _peer != null:
		_peer.disconnect_from_host()
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

	_status = status
	_checksum = str(checksum_value)
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
	_finished = true
	set_process(false)
	_set_status("Disconnected.")
	if _peer != null:
		_peer.disconnect_from_host()
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
	_set_status("Failed: " + code)
	if _peer != null:
		_peer.disconnect_from_host()
	if _watch and not _fixture_mode:
		# Watch mode never reconnects. The marker is the test surface: one
		# stable line naming the iteration that failed and the stable code.
		print(
			"SERVER_STATUS_WATCH status=fail iteration=%d code=%s"
			% [_watch_failure_iteration(), code]
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

## Iteration (1-based) whose request failed. Smoke runs clamp to the configured
## count so a missing goodbye after the last answer still reports iteration=N.
func _watch_failure_iteration() -> int:
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

	var title := Label.new()
	title.text = "Server status"
	title.add_theme_font_size_override("font_size", 18)
	root.add_child(title)

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
	if _value_labels.has("Checksum"):
		_value_labels["Checksum"].text = _checksum

func _set_status(text: String) -> void:
	if _status_label != null:
		_status_label.text = text

func _arm_deadline(duration_ms: int) -> void:
	_deadline_ms = Time.get_ticks_msec() + duration_ms

func _is_integer(value: Variant) -> bool:
	if typeof(value) == TYPE_INT:
		return true
	if typeof(value) != TYPE_FLOAT:
		return false
	var number := float(value)
	return is_finite(number) and number == floor(number)

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

func _bounded_int(value: String, fallback: int, minimum: int, maximum: int) -> int:
	if not value.is_valid_int():
		return fallback
	return clampi(value.to_int(), minimum, maximum)

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
