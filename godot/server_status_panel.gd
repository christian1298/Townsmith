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
## `--smoke` prints one `SERVER_STATUS_PANEL` marker and exits non-zero on a
## malformed/missing status, timeout, reject, or unexpected goodbye.
## `scripts/smoke_server_status_panel.sh` starts a fresh `--once` server, runs
## the panel twice, and compares the markers byte for byte.

enum Phase {
	CONNECTING,
	WELCOME_WAIT,
	STATUS_SENT,
	DISCONNECT_SENT,
}

const PROTOCOL_VERSION := 1
const MAX_FRAME_LEN := 1024 * 1024
const DEFAULT_HOST := "127.0.0.1"
const DEFAULT_PORT := 47015
const DEFAULT_NAME := "status-panel"
const CONNECT_TIMEOUT_MS := 5000
const STEP_TIMEOUT_MS := 5000
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
	_build_ui()

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
		_fail("connect_start_%s" % error_string(error))
		return
	_arm_deadline(CONNECT_TIMEOUT_MS)
	_set_status("Connecting to %s:%d..." % [_host, _port])

func _process(_delta: float) -> void:
	if _finished:
		return
	if Time.get_ticks_msec() > _deadline_ms:
		_fail("timeout_%s" % _phase_name())
		return
	if _peer == null:
		_fail("peer_missing")
		return

	_peer.poll()
	if _phase == Phase.CONNECTING:
		match _peer.get_status():
			StreamPeerTCP.STATUS_CONNECTED:
				_on_connected()
			StreamPeerTCP.STATUS_ERROR:
				_fail("connect_failed")
			StreamPeerTCP.STATUS_NONE:
				_fail("connect_lost")
		return

	_drain()
	if _finished:
		return
	if _peer.get_status() != StreamPeerTCP.STATUS_CONNECTED:
		# Read the final goodbye/reject before treating FIN as a failure.
		_drain()
		if not _finished:
			_fail("connection_lost_%s" % _phase_name())

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
	var available := _peer.get_available_bytes()
	if available > 0:
		var result := _peer.get_partial_data(available)
		if result[0] != OK:
			_fail("recv_%s" % error_string(result[0]))
			return
		_rx.append_array(result[1])

	while not _finished and _rx.size() >= 4:
		var length := _read_u32_be(_rx, 0)
		if length == 0:
			_fail("frame.zero_length")
			return
		if length > MAX_FRAME_LEN:
			_fail("frame.oversized")
			return
		if _rx.size() < 4 + length:
			return
		var payload := _rx.slice(4, 4 + length)
		_rx = _rx.slice(4 + length)
		_frames_received += 1
		var parsed: Variant = JSON.parse_string(payload.get_string_from_utf8())
		if typeof(parsed) != TYPE_DICTIONARY:
			_fail("frame.invalid_json")
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
			_fail("reject", ("server_code=" + code) if not code.is_empty() else "")
		_:
			_fail("unexpected_message_%s" % (tag if tag != "" else "missing_type"))

func _on_welcome(message: Dictionary) -> void:
	if _phase != Phase.WELCOME_WAIT:
		_fail("welcome_in_%s" % _phase_name())
		return
	var protocol_version := _as_int(message.get("protocol_version"))
	if protocol_version != PROTOCOL_VERSION:
		_fail("protocol_version_%s" % str(protocol_version))
		return
	if str(message.get("session_id", "")).is_empty():
		_fail("welcome_session")
		return
	if bool(message.get("resumed", false)):
		_fail("unexpected_resumed_welcome")
		return
	_set_status("Welcome; requesting server status.")
	if not _send({"type": "server_status"}):
		return
	_phase = Phase.STATUS_SENT
	_arm_deadline(_timeout_ms)

func _on_server_status(message: Dictionary) -> void:
	if _phase != Phase.STATUS_SENT:
		_fail("server_status_in_%s" % _phase_name())
		return
	var status: Dictionary = {}
	for field in STATUS_FIELDS:
		if not message.has(field):
			_fail("malformed_status_missing_%s" % field)
			return
		var value: Variant = message[field]
		if not _is_integer(value):
			_fail("malformed_status_%s" % field)
			return
		status[field] = int(value)
	# `balance` may be negative (debt); the counters cannot be.
	for field in STATUS_FIELDS:
		if field == "balance":
			continue
		if int(status[field]) < 0:
			_fail("malformed_status_%s" % field)
			return
	if int(status["online"]) < 1:
		_fail("malformed_status_online")
		return

	var checksum_value: Variant = message.get("checksum")
	if typeof(checksum_value) != TYPE_STRING or not _is_hex16(str(checksum_value)):
		_fail("malformed_status_checksum")
		return

	_status = status
	_checksum = str(checksum_value)
	_refresh_ui()
	_set_status("Status received; disconnecting.")
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
	if not _send({"type": "disconnect"}):
		return
	_phase = Phase.DISCONNECT_SENT
	_arm_deadline(_timeout_ms)

func _on_goodbye() -> void:
	if _phase == Phase.DISCONNECT_SENT:
		_finish_ok()
		return
	# The server closed before answering the status request.
	_fail("goodbye")

func _finish_ok() -> void:
	if _finished:
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
		_fail("send_not_connected")
		return false
	var payload := JSON.stringify(message).to_utf8_buffer()
	if payload.is_empty():
		_fail("encode_empty")
		return false
	if payload.size() > MAX_FRAME_LEN:
		_fail("encode_oversized")
		return false
	var frame := PackedByteArray()
	frame.resize(4)
	_write_u32_be(frame, 0, payload.size())
	frame.append_array(payload)
	var error := _peer.put_data(frame)
	if error != OK:
		_fail("send_%s" % error_string(error))
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
	var suffix := (" " + extra) if not extra.is_empty() else ""
	print("SERVER_STATUS_PANEL status=fail code=%s%s" % [code, suffix])
	get_tree().quit(1)

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

func _phase_name() -> String:
	match _phase:
		Phase.CONNECTING:
			return "connecting"
		Phase.WELCOME_WAIT:
			return "welcome_wait"
		Phase.STATUS_SENT:
			return "status_wait"
		Phase.DISCONNECT_SENT:
			return "disconnect_wait"
		_:
			return "unknown"

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
