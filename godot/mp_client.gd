extends Control

## Multiplayer protocol v0 client.
##
## Speaks the server's length-prefixed JSON protocol end to end over
## `StreamPeerTCP`: hello -> welcome, ping -> pong, set_tax -> snapshot,
## invalid set_tax -> reject, place_road -> snapshot, set_interest ->
## snapshot + interest_view, disconnect -> goodbye.
##
## The client is deliberately a protocol probe, not a game client: it has no
## production render integration, auth, roles, entity streaming, or delta
## snapshots. The one visual is a bounded flat top-down preview of the received
## `interest_view` road set (`mp_interest_canvas.gd`), so viewport filtering is
## visible without implicating the world renderer or streaming terrain.
## v0 is loopback-only and the server serves one connection at a time.
##
## Run:
##   godot --headless --path godot res://mp_client.tscn -- \
##     --smoke --port 47015
##   godot --headless --path godot res://mp_client.tscn -- --self-test
##
## `scripts/smoke_mp_client.sh` runs the pure-helper self-test first, then starts
## `townsmith-server --listen PORT`, waits for the port with a bounded connect
## loop, runs the headless smoke, and asserts the `MP_CLIENT status=ok ...`
## marker including `interest_render=ok roads=N checksum=<16hex>`.
##
## See docs/05 - Multiplayer.md §Protocol v0.

const InterestRender = preload("res://mp_interest_render.gd")
const InterestCanvas = preload("res://mp_interest_canvas.gd")

enum Phase {
	CONNECTING,
	HELLO_SENT,
	PING_SENT,
	TAX_SENT,
	TAX_REJECT_WAIT,
	ROAD_SENT,
	INTEREST_SENT,
	INTEREST_VIEW_WAIT,
	DISCONNECT_SENT,
}

const PROTOCOL_VERSION := 0
const MAX_FRAME_LEN := 1024 * 1024
const DEFAULT_HOST := "127.0.0.1"
const DEFAULT_PORT := 47015
const DEFAULT_NAME := "godot-client"
const CONNECT_TIMEOUT_MS := 5000
const STEP_TIMEOUT_MS := 5000
const PING_NONCE := 20261123
# Same 64-character placeholder hash the Rust loopback harness uses.
const CONTENT_HASH := "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"

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
var _ok_welcome := false
var _ok_ping := false
var _ok_snapshot := false
var _ok_reject := false
var _ok_road := false
var _ok_interest := false
var _interest_roads := -1
var _interest_truncated := true
var _interest_render_checksum := ""
var _interest_rendered := -1
var _interest_cleared := false
var _session_id := ""
var _last_tick := -1
var _last_population := -1
var _last_balance := 0
var _last_checksum := ""

var _status_label: Label
var _session_label: Label
var _tick_label: Label
var _population_label: Label
var _balance_label: Label
var _checksum_label: Label
var _frames_label: Label
var _interest_label: Label
var _interest_checksum_label: Label
var _interest_canvas

func _ready() -> void:
	var args := OS.get_cmdline_user_args()
	_smoke = args.has("--smoke")
	_host = _arg(args, "--host", DEFAULT_HOST)
	_player_name = _arg(args, "--name", DEFAULT_NAME)
	_timeout_ms = _positive_int(_arg(args, "--timeout-ms", ""), STEP_TIMEOUT_MS)
	if args.has("--self-test"):
		_run_self_test()
		return
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
		# Read the goodbye or final reject before treating FIN as a failure.
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
	_phase = Phase.HELLO_SENT
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
		"pong":
			_on_pong(message)
		"snapshot":
			_on_snapshot(message)
		"interest_view":
			_on_interest_view(message)
		"reject":
			_on_reject(message)
		"goodbye":
			_on_goodbye()
		_:
			_fail("unexpected_message_%s" % (tag if tag != "" else "missing_type"))

func _on_welcome(message: Dictionary) -> void:
	if _phase != Phase.HELLO_SENT:
		_fail("welcome_in_%s" % _phase_name())
		return
	var protocol_version := int(message.get("protocol_version", -1))
	if protocol_version != PROTOCOL_VERSION:
		_fail("protocol_version_%d" % protocol_version)
		return
	_session_id = str(message.get("session_id", ""))
	if _session_id.is_empty():
		_fail("welcome_session")
		return
	_ok_welcome = true
	_set_status("Welcome %s; pinging." % _session_id)
	_refresh_ui()
	if not _send({"type": "ping", "nonce": PING_NONCE}):
		return
	_phase = Phase.PING_SENT
	_arm_deadline(_timeout_ms)

func _on_pong(message: Dictionary) -> void:
	if _phase != Phase.PING_SENT:
		_fail("pong_in_%s" % _phase_name())
		return
	if int(message.get("nonce", -1)) != PING_NONCE:
		_fail("pong_nonce")
		return
	_ok_ping = true
	_set_status("Pong; setting tax.")
	_refresh_ui()
	if not _send({"type": "set_tax", "resident_rate": 10, "business_rate": 15}):
		return
	_phase = Phase.TAX_SENT
	_arm_deadline(_timeout_ms)

func _on_snapshot(message: Dictionary) -> void:
	var tick := int(message.get("tick", -1))
	var population := int(message.get("population", -1))
	var balance := int(message.get("balance", 0))
	var checksum := str(message.get("checksum", ""))
	if tick < 1 or population < 1 or not _is_hex16(checksum):
		_fail("snapshot_fields")
		return

	if _phase == Phase.TAX_SENT:
		_ok_snapshot = true
		_last_tick = tick
		_last_population = population
		_last_balance = balance
		_last_checksum = checksum
		_set_status("Snapshot received; sending invalid tax.")
		_refresh_ui()
		# Deliberately invalid: 101 percent exceeds the v0 0-100 range.
		if not _send({"type": "set_tax", "resident_rate": 101, "business_rate": 10}):
			return
		_phase = Phase.TAX_REJECT_WAIT
		_arm_deadline(_timeout_ms)
		return

	if _phase == Phase.ROAD_SENT:
		if tick <= _last_tick:
			_fail("road_tick_did_not_advance")
			return
		_ok_road = true
		_last_tick = tick
		_last_population = population
		_last_balance = balance
		_last_checksum = checksum
		_set_status("Road accepted; requesting the interest view.")
		_refresh_ui()
		if not _send({"type": "set_interest", "center_x": 0.0, "center_z": 0.0, "radius_m": 16.0}):
			return
		_phase = Phase.INTEREST_SENT
		_arm_deadline(_timeout_ms)
		return

	if _phase == Phase.INTEREST_SENT:
		# set_interest is view-only: the snapshot must not advance the tick or
		# move the checksum, and it carries the accepted viewport.
		if tick != _last_tick:
			_fail("interest_tick_changed")
			return
		if checksum != _last_checksum:
			_fail("interest_checksum_changed")
			return
		if int(message.get("visible_roads", -1)) != 1:
			_fail("interest_visible_roads")
			return
		if not _is_pair(message.get("interest_center", null), 0, 0):
			_fail("interest_center")
			return
		if absf(float(message.get("interest_radius_m", -1.0)) - 16.0) > 0.001:
			_fail("interest_radius")
			return
		_phase = Phase.INTEREST_VIEW_WAIT
		_set_status("Interest snapshot received; awaiting the road view.")
		_arm_deadline(_timeout_ms)
		return

	_fail("snapshot_in_%s" % _phase_name())

func _on_interest_view(message: Dictionary) -> void:
	if _phase != Phase.INTEREST_VIEW_WAIT:
		_fail("interest_view_in_%s" % _phase_name())
		return
	var tick := int(message.get("tick", -1))
	var radius_m := float(message.get("radius_m", -1.0))
	var truncated := bool(message.get("truncated", true))
	var roads: Variant = message.get("roads", null)
	if tick != _last_tick:
		_fail("interest_view_tick")
		return
	if not _is_pair(message.get("center", null), 0, 0):
		_fail("interest_view_center")
		return
	if absf(radius_m - 16.0) > 0.001:
		_fail("interest_view_radius")
		return
	if typeof(roads) != TYPE_ARRAY:
		_fail("interest_view_roads")
		return
	if roads.size() != 1:
		_fail("interest_view_road_count_%d" % roads.size())
		return
	var road: Variant = roads[0]
	if typeof(road) != TYPE_DICTIONARY:
		_fail("interest_view_road")
		return
	if int(road.get("id", -1)) != 1:
		_fail("interest_view_road_id")
		return
	if not _is_pair(road.get("start", null), 0, 0):
		_fail("interest_view_road_start")
		return
	if not _is_pair(road.get("end", null), 8, 0):
		_fail("interest_view_road_end")
		return
	if int(road.get("width_m", -1)) != 6:
		_fail("interest_view_road_width")
		return
	if truncated:
		_fail("interest_view_truncated")
		return
	if _interest_canvas == null:
		_fail("interest_canvas_missing")
		return
	var view_center: Variant = message.get("center", null)
	var parsed_roads: int = _interest_canvas.set_view(
		Vector2(float(view_center[0]), float(view_center[1])), radius_m, roads, truncated
	)
	if parsed_roads != roads.size():
		_fail("interest_render_parse_%d" % parsed_roads)
		return
	var rendered: int = _interest_canvas.line_count()
	if rendered != roads.size():
		_fail("interest_render_lines_%d" % rendered)
		return
	_interest_render_checksum = _interest_canvas.road_checksum()
	if not _is_hex16(_interest_render_checksum):
		_fail("interest_render_checksum")
		return
	_ok_interest = true
	_interest_roads = roads.size()
	_interest_truncated = truncated
	_interest_rendered = rendered
	_set_status("Interest view received; disconnecting.")
	_refresh_ui()
	if not _send({"type": "disconnect"}):
		return
	_phase = Phase.DISCONNECT_SENT
	_arm_deadline(_timeout_ms)

func _on_reject(message: Dictionary) -> void:
	if _phase != Phase.TAX_REJECT_WAIT:
		_fail("reject_%s_code=%s" % [_phase_name(), str(message.get("code", "missing"))])
		return
	var code := str(message.get("code", ""))
	if code != "tax.rate_out_of_range":
		_fail("reject_code_%s" % (code if code != "" else "missing"))
		return
	_ok_reject = true
	_set_status("Rejected tax as expected; placing a road.")
	_refresh_ui()
	if not _send({"type": "place_road", "start": [0, 0], "end": [8, 0]}):
		return
	_phase = Phase.ROAD_SENT
	_arm_deadline(_timeout_ms)

func _on_goodbye() -> void:
	if _phase != Phase.DISCONNECT_SENT:
		_fail("goodbye_in_%s" % _phase_name())
		return
	_finish_ok()

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

func _finish_ok() -> void:
	if _finished:
		return
	if _interest_canvas == null:
		_fail("interest_canvas_missing")
		return
	var rendered: int = _interest_canvas.line_count()
	_interest_canvas.clear_view()
	var cleared: bool = _interest_canvas.line_count() == 0 and not _interest_canvas.has_view
	if rendered != _interest_roads or _interest_render_checksum.is_empty() or not cleared:
		_fail("interest_render_teardown")
		return
	_interest_rendered = rendered
	_interest_cleared = cleared
	_finished = true
	set_process(false)
	_set_status("Protocol v0 exchange complete.")
	_refresh_ui()
	if _peer != null:
		_peer.disconnect_from_host()
	print(
		(
			"MP_CLIENT status=ok welcome=ok ping=ok snapshot=ok reject=ok road=ok interest=ok"
			+ " roads=%d truncated=%s"
			+ " interest_render=ok roads=%d checksum=%s rendered=%d cleared=%s"
			+ " tick=%d population=%d balance=%d snapshot_checksum=%s session=%s"
			+ " frames_sent=%d frames_received=%d"
		)
		% [
			_interest_roads,
			str(_interest_truncated),
			_interest_rendered,
			_interest_render_checksum,
			_interest_rendered,
			str(_interest_cleared),
			_last_tick,
			_last_population,
			_last_balance,
			_last_checksum,
			_session_id,
			_frames_sent,
			_frames_received,
		]
	)
	if _smoke:
		get_tree().quit(0)

func _fail(reason: String) -> void:
	if _finished:
		return
	_finished = true
	set_process(false)
	if _interest_canvas != null:
		_interest_canvas.clear_view()
	_set_status("Protocol v0 failed: " + reason)
	_refresh_ui()
	if _peer != null:
		_peer.disconnect_from_host()
	print(
		"MP_CLIENT status=fail %s error=%s frames_sent=%d frames_received=%d"
		% [_flags_text(), reason, _frames_sent, _frames_received]
	)
	if _smoke:
		get_tree().quit(1)

func _flags_text() -> String:
	return "welcome=%s ping=%s snapshot=%s reject=%s road=%s interest=%s" % [
		_ok_text(_ok_welcome),
		_ok_text(_ok_ping),
		_ok_text(_ok_snapshot),
		_ok_text(_ok_reject),
		_ok_text(_ok_road),
		_ok_text(_ok_interest),
	]

func _ok_text(ok: bool) -> String:
	return "ok" if ok else "pending"

func _phase_name() -> String:
	match _phase:
		Phase.CONNECTING:
			return "connecting"
		Phase.HELLO_SENT:
			return "hello_sent"
		Phase.PING_SENT:
			return "ping_sent"
		Phase.TAX_SENT:
			return "tax_sent"
		Phase.TAX_REJECT_WAIT:
			return "tax_reject_wait"
		Phase.ROAD_SENT:
			return "road_sent"
		Phase.INTEREST_SENT:
			return "interest_sent"
		Phase.INTEREST_VIEW_WAIT:
			return "interest_view_wait"
		Phase.DISCONNECT_SENT:
			return "disconnect_sent"
		_:
			return "unknown"

func _arm_deadline(duration_ms: int) -> void:
	_deadline_ms = Time.get_ticks_msec() + duration_ms

func _is_pair(value: Variant, x: int, z: int) -> bool:
	if typeof(value) != TYPE_ARRAY:
		return false
	if value.size() != 2:
		return false
	return int(value[0]) == x and int(value[1]) == z

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

## Headless pure-helper assertions. No display, server, or scene layout needed.
func _run_self_test() -> void:
	_finished = true
	set_process(false)
	var failures: Array[String] = []
	var cases := 0

	var raw: Array = []
	for index in range(InterestRender.MAX_ROADS + 6):
		raw.append({"id": index + 1, "start": [0, 0], "end": [8, 0], "width_m": 6})
	cases += 1
	var bounded: Array = InterestRender.parse_roads(raw)
	if bounded.size() != InterestRender.MAX_ROADS:
		failures.append("bounded=%d" % bounded.size())

	var mixed: Array = [
		{"id": 2, "start": [8, 0], "end": [16, 0], "width_m": 6},
		{"id": 1, "start": [0, 0], "end": [8, 0], "width_m": 6},
		{"id": 3, "start": "bad", "end": [0, 0], "width_m": 6},
		{"id": 4, "start": [0, 0], "end": [1, 0], "width_m": "wide"},
		"junk",
	]
	cases += 1
	var parsed: Array = InterestRender.parse_roads(mixed)
	if parsed.size() != 3:
		failures.append("parse=%d" % parsed.size())
	elif int(parsed[0]["id"]) != 1 or int(parsed[1]["id"]) != 2 or int(parsed[2]["id"]) != 4:
		failures.append("order")
	elif float(parsed[2]["width_m"]) != InterestRender.DEFAULT_WIDTH_M:
		failures.append("width_default")

	var sample: Array = [
		{
			"id": 1,
			"start": Vector2(0, 0),
			"end": Vector2(8, 0),
			"width_m": 6.0,
		}
	]
	cases += 1
	var checksum_a := InterestRender.checksum(sample)
	var checksum_b := InterestRender.checksum(sample.duplicate(true))
	if not _is_hex16(checksum_a):
		failures.append("checksum_format")
	elif checksum_a != checksum_b:
		failures.append("checksum_unstable")
	var changed: Array = sample.duplicate(true)
	changed[0]["end"] = Vector2(9, 0)
	if InterestRender.checksum(changed) == checksum_a:
		failures.append("checksum_sensitive")

	cases += 1
	var panel := Vector2(100, 100)
	var center_px := InterestRender.project_point(Vector2.ZERO, Vector2.ZERO, 16.0, panel, 10.0)
	if center_px.distance_to(Vector2(50, 50)) > 0.001:
		failures.append("project_center=%.3f,%.3f" % [center_px.x, center_px.y])
	var edge_px := InterestRender.project_point(Vector2(16, 0), Vector2.ZERO, 16.0, panel, 10.0)
	if absf(edge_px.x - 90.0) > 0.001 or absf(edge_px.y - 50.0) > 0.001:
		failures.append("project_edge=%.3f,%.3f" % [edge_px.x, edge_px.y])

	cases += 1
	var segments := InterestRender.segments(sample, Vector2.ZERO, 16.0, panel, 10.0)
	if segments.size() != 1:
		failures.append("segments=%d" % segments.size())
	elif float(segments[0]["width_px"]) < InterestRender.MIN_LINE_WIDTH_PX:
		failures.append("segment_width")

	if failures.is_empty():
		print(
			"MP_INTEREST_HELPER status=ok cases=%d bounded=%d checksum=%s"
			% [cases, bounded.size(), checksum_a]
		)
		get_tree().quit(0)
		return
	print(
		"MP_INTEREST_HELPER status=fail cases=%d error=%s"
		% [cases, ",".join(failures)]
	)
	get_tree().quit(1)

func _build_ui() -> void:
	set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)

	var background := ColorRect.new()
	background.color = Color(0.055, 0.065, 0.08)
	background.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	background.mouse_filter = Control.MOUSE_FILTER_IGNORE
	add_child(background)

	var margin := MarginContainer.new()
	margin.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	margin.add_theme_constant_override("margin_left", 36)
	margin.add_theme_constant_override("margin_top", 28)
	margin.add_theme_constant_override("margin_right", 36)
	margin.add_theme_constant_override("margin_bottom", 28)
	add_child(margin)

	var root := VBoxContainer.new()
	root.add_theme_constant_override("separation", 10)
	margin.add_child(root)

	var title := Label.new()
	title.text = "Multiplayer protocol v0"
	title.add_theme_font_size_override("font_size", 32)
	root.add_child(title)

	var subtitle := Label.new()
	subtitle.text = "Loopback TCP, length-prefixed JSON. Protocol probe with a bounded flat viewport preview; no world renderer, auth, roles, entity streaming, or deltas."
	subtitle.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	subtitle.add_theme_color_override("font_color", Color(0.70, 0.75, 0.82))
	subtitle.add_theme_font_size_override("font_size", 15)
	root.add_child(subtitle)

	_status_label = _add_row(root, "Status", "Connecting...")
	_session_label = _add_row(root, "Session", "-")
	_tick_label = _add_row(root, "Tick", "-")
	_population_label = _add_row(root, "Population", "-")
	_balance_label = _add_row(root, "Balance", "-")
	_checksum_label = _add_row(root, "Checksum", "-")
	_frames_label = _add_row(root, "Frames", "0 sent / 0 received")
	_interest_label = _add_row(root, "Interest view", "roads=0 truncated=false")
	_interest_checksum_label = _add_row(root, "Interest checksum", "-")

	_interest_canvas = InterestCanvas.new()
	_interest_canvas.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	root.add_child(_interest_canvas)

func _add_row(parent: VBoxContainer, key: String, value: String) -> Label:
	var row := HBoxContainer.new()
	row.add_theme_constant_override("separation", 16)
	parent.add_child(row)

	var key_label := Label.new()
	key_label.text = key
	key_label.custom_minimum_size = Vector2(140.0, 0.0)
	key_label.add_theme_color_override("font_color", Color(0.66, 0.71, 0.78))
	key_label.add_theme_font_size_override("font_size", 16)
	row.add_child(key_label)

	var value_label := Label.new()
	value_label.text = value
	value_label.add_theme_font_size_override("font_size", 16)
	row.add_child(value_label)
	return value_label

func _set_status(text: String) -> void:
	if _status_label != null:
		_status_label.text = text

func _refresh_ui() -> void:
	if _session_label == null:
		return
	_session_label.text = _session_id if not _session_id.is_empty() else "-"
	_tick_label.text = str(_last_tick) if _last_tick >= 0 else "-"
	_population_label.text = str(_last_population) if _last_population >= 0 else "-"
	_balance_label.text = str(_last_balance) if _last_tick >= 0 else "-"
	_checksum_label.text = _last_checksum if not _last_checksum.is_empty() else "-"
	_frames_label.text = "%d sent / %d received" % [_frames_sent, _frames_received]
	if _interest_canvas != null:
		_interest_label.text = _interest_canvas.summary()
		_interest_checksum_label.text = (
			_interest_canvas.road_checksum()
			if _interest_canvas.has_view
			else (_interest_render_checksum if not _interest_render_checksum.is_empty() else "-")
		)

func _read_u32_be(buffer: PackedByteArray, offset: int) -> int:
	return (
		(int(buffer[offset]) << 24)
		| (int(buffer[offset + 1]) << 16)
		| (int(buffer[offset + 2]) << 8)
		| int(buffer[offset + 3])
	)

func _write_u32_be(buffer: PackedByteArray, offset: int, value: int) -> void:
	buffer[offset] = (value >> 24) & 0xff
	buffer[offset + 1] = (value >> 16) & 0xff
	buffer[offset + 2] = (value >> 8) & 0xff
	buffer[offset + 3] = value & 0xff
