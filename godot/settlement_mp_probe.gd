extends Control

## Standalone connected settlement probe (developer tool, read-only reads).
##
## Connects to `townsmith-server --listen PORT`, completes the v0 hello/welcome
## handshake, founds one deterministic settlement as the Mayor session owner,
## then exercises the read commands the production settlement screen uses:
##
##   settlement_list {after_id:0, limit:8}   (first page)
##   settlement_list {after_id:next_after}   (walk to the empty page)
##   player_list {after_id:0, limit:8}       (resolve the owner name)
##   settlement_at {x:0, z:0}                (resolve the containing record)
##
## The only mutation is the D-033 `found_settlement` used to create the
## deterministic fixture; it is not a new command or field and the production
## screen is untouched. Everything after the founding is a read.
##
## Run:
##   godot --path godot res://settlement_mp_probe.tscn -- --port 47015
##   godot --headless --path godot res://settlement_mp_probe.tscn -- \
##     --smoke --port 47015
##
## `--smoke` prints one `MP_SETTLEMENT_PROBE` marker and exits non-zero on a
## reject, timeout, malformed response, or mismatched read.
## `scripts/smoke_settlement_mp_probe.sh` starts a fresh `--once` server per
## run, runs the probe twice, and compares the markers byte for byte.

enum Phase {
	CONNECTING,
	WELCOME_WAIT,
	FOUND_SENT,
	FIRST_LIST_SENT,
	SECOND_LIST_SENT,
	PLAYERS_SENT,
	AT_SENT,
	DISCONNECT_SENT,
}

const PROTOCOL_VERSION := 1
const MAX_FRAME_LEN := 1024 * 1024
const DEFAULT_HOST := "127.0.0.1"
const DEFAULT_PORT := 47015
const DEFAULT_NAME := "settlement-probe"
const CONNECT_TIMEOUT_MS := 5000
const STEP_TIMEOUT_MS := 5000
# Same 64-character placeholder hash the Rust loopback harness uses.
const CONTENT_HASH := "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"

const SETTLEMENT_NAME := "Harbor"
const SETTLEMENT_CENTER_X := 0
const SETTLEMENT_CENTER_Z := 0
const SETTLEMENT_RADIUS_M := 64
const PAGE_LIMIT := 8

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
var _published_marker := false

# Read state retained for the detail panel and the marker checksum.
var _settlements: Dictionary = {}
var _owner_id := 0
var _owner_name := ""
var _found: Dictionary = {}
var _at: Dictionary = {}
var _page_count := 0
var _page_matched := 0
var _roster_count := 0
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
		"settlement_found":
			_on_settlement_found(message)
		"settlement_list":
			_on_settlement_list(message)
		"player_list":
			_on_player_list(message)
		"settlement_at":
			_on_settlement_at(message)
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
	if _as_int(message.get("protocol_version")) != PROTOCOL_VERSION:
		_fail("protocol_version")
		return
	if str(message.get("session_id", "")).is_empty():
		_fail("welcome_session")
		return
	if bool(message.get("resumed", false)):
		_fail("unexpected_resumed_welcome")
		return
	_set_status("Welcome; founding %s." % SETTLEMENT_NAME)
	if not _send({
		"type": "found_settlement",
		"name": SETTLEMENT_NAME,
		"x": SETTLEMENT_CENTER_X,
		"z": SETTLEMENT_CENTER_Z,
		"radius_m": SETTLEMENT_RADIUS_M,
	}):
		return
	_phase = Phase.FOUND_SENT
	_arm_deadline(_timeout_ms)

func _on_settlement_found(message: Dictionary) -> void:
	if _phase != Phase.FOUND_SENT:
		_fail("settlement_found_in_%s" % _phase_name())
		return
	var record := _parse_settlement(message.get("settlement"), "found")
	if record.is_empty():
		return
	if _as_int(record.get("id")) <= 0:
		_fail("found_id")
		return
	if not bool(record.get("owned", false)) or record.get("owner_id") == null:
		_fail("found_unowned")
		return
	var owner := _as_int(record.get("owner_id"))
	if owner <= 0:
		_fail("found_owner")
		return
	if not str(record.get("name", "")) == SETTLEMENT_NAME:
		_fail("found_name")
		return
	if not _center_is(record, SETTLEMENT_CENTER_X, SETTLEMENT_CENTER_Z):
		_fail("found_center")
		return
	if _as_int(record.get("radius_m")) != SETTLEMENT_RADIUS_M:
		_fail("found_radius")
		return
	var settlement_checksum := str(message.get("settlement_checksum", ""))
	if not _is_hex16(settlement_checksum):
		_fail("found_checksum")
		return

	_found = record
	_owner_id = owner
	_store_settlement(record)
	_refresh_ui()
	_set_status("Founded record #%d; listing page 1." % _as_int(record.get("id")))
	if not _send({"type": "settlement_list", "after_id": 0, "limit": PAGE_LIMIT}):
		return
	_phase = Phase.FIRST_LIST_SENT
	_arm_deadline(_timeout_ms)

func _on_settlement_list(message: Dictionary) -> void:
	if _phase != Phase.FIRST_LIST_SENT and _phase != Phase.SECOND_LIST_SENT:
		_fail("settlement_list_in_%s" % _phase_name())
		return
	var tick := _as_int(message.get("tick"))
	var count := _as_int(message.get("count"))
	var matched := _as_int(message.get("matched"))
	var next_after := _as_int(message.get("next_after"))
	var has_more: Variant = message.get("has_more")
	var page: Variant = message.get("settlements")
	var checksum := str(message.get("checksum", ""))
	if tick < 0 or count < 0 or matched < 0 or next_after < 0:
		_fail("list_fields")
		return
	if typeof(has_more) != TYPE_BOOL:
		_fail("list_has_more")
		return
	if typeof(page) != TYPE_ARRAY:
		_fail("list_page")
		return
	if not _is_hex16(checksum):
		_fail("list_checksum")
		return
	if page.size() != count:
		_fail("list_count")
		return

	if _phase == Phase.FIRST_LIST_SENT:
		if count != 1 or matched != 1:
			_fail("list_first_page")
			return
		var record := _parse_settlement(page[0], "list")
		if record.is_empty():
			return
		if has_more:
			_fail("list_first_has_more")
			return
		if next_after != _as_int(record.get("id")):
			_fail("list_first_next")
			return
		_store_settlement(record)
		_page_count = count
		_page_matched = matched
		_refresh_ui()
		_set_status("Page 1 held %d; walking cursor %d." % [count, next_after])
		if not _send({
			"type": "settlement_list",
			"after_id": next_after,
			"limit": PAGE_LIMIT,
		}):
			return
		_phase = Phase.SECOND_LIST_SENT
		_arm_deadline(_timeout_ms)
		return

	# Second page: the fixture has one settlement, so the walked page is empty.
	if count != 0 or matched != 0 or has_more:
		_fail("list_second_page")
		return
	if next_after != _found_id():
		_fail("list_second_next")
		return
	_page_count += count
	_page_matched += matched
	_refresh_ui()
	_set_status("Cursor exhausted; reading players.")
	if not _send({"type": "player_list", "after_id": 0, "limit": PAGE_LIMIT}):
		return
	_phase = Phase.PLAYERS_SENT
	_arm_deadline(_timeout_ms)

func _on_player_list(message: Dictionary) -> void:
	if _phase != Phase.PLAYERS_SENT:
		_fail("player_list_in_%s" % _phase_name())
		return
	var tick := _as_int(message.get("tick"))
	var count := _as_int(message.get("count"))
	var matched := _as_int(message.get("matched"))
	var next_after := _as_int(message.get("next_after"))
	var has_more: Variant = message.get("has_more")
	var raw_players: Variant = message.get("players")
	var checksum := str(message.get("checksum", ""))
	if tick < 0 or count < 0 or matched < 0 or next_after < 0:
		_fail("players_fields")
		return
	if typeof(has_more) != TYPE_BOOL or typeof(raw_players) != TYPE_ARRAY:
		_fail("players_shape")
		return
	if not _is_hex16(checksum):
		_fail("players_checksum")
		return
	if raw_players.size() != count or count < 1:
		_fail("players_count")
		return

	var owner_name := ""
	for entry in raw_players:
		if typeof(entry) != TYPE_DICTIONARY:
			_fail("players_entry")
			return
		var player_id := _as_int(entry.get("id"))
		var name_value: Variant = entry.get("name")
		if player_id <= 0 or typeof(name_value) != TYPE_STRING:
			_fail("players_entry_fields")
			return
		if player_id == _owner_id:
			owner_name = str(name_value)
	if owner_name.is_empty():
		_fail("players_owner_missing")
		return
	if owner_name != _player_name:
		_fail("players_owner_name")
		return

	_owner_name = owner_name
	_roster_count = count
	_refresh_ui()
	_set_status("Owner #%d resolved; querying (0,0)." % _owner_id)
	if not _send({
		"type": "settlement_at",
		"x": SETTLEMENT_CENTER_X,
		"z": SETTLEMENT_CENTER_Z,
	}):
		return
	_phase = Phase.AT_SENT
	_arm_deadline(_timeout_ms)

func _on_settlement_at(message: Dictionary) -> void:
	if _phase != Phase.AT_SENT:
		_fail("settlement_at_in_%s" % _phase_name())
		return
	if not bool(message.get("found", false)):
		_fail("at_not_found")
		return
	var record := _parse_settlement(message.get("settlement"), "at")
	if record.is_empty():
		return
	if _as_int(record.get("id")) != _found_id():
		_fail("at_id")
		return
	if not str(record.get("name", "")) == SETTLEMENT_NAME:
		_fail("at_name")
		return
	if not _center_is(record, SETTLEMENT_CENTER_X, SETTLEMENT_CENTER_Z):
		_fail("at_center")
		return

	_at = record
	_checksum = _settlement_checksum()
	if _checksum.is_empty():
		_fail("checksum")
		return
	_refresh_ui()
	_set_status("Reads verified; disconnecting.")
	if _smoke:
		print(
			(
				"MP_SETTLEMENT_PROBE status=ok settlements=%d players=%d"
				+ " at=%d owner=%d revision=%d checksum=%s"
			)
			% [
				_page_count,
				_roster_count,
				_found_id(),
				_owner_id,
				_as_int(_found.get("membership_revision")),
				_checksum,
			]
		)
		_published_marker = true
	if not _send({"type": "disconnect"}):
		return
	_phase = Phase.DISCONNECT_SENT
	_arm_deadline(_timeout_ms)

func _on_goodbye() -> void:
	if _phase != Phase.DISCONNECT_SENT:
		_fail("goodbye_in_%s" % _phase_name())
		return
	_finish_ok()

func _finish_ok() -> void:
	if _finished:
		return
	if _smoke and not _published_marker:
		_fail("marker_not_published")
		return
	if not _smoke and not _published_marker:
		print("MP_SETTLEMENT_PROBE status=ok rendered=true")
	_finished = true
	set_process(false)
	_set_status("Disconnected.")
	if _peer != null:
		_peer.disconnect_from_host()
	get_tree().quit(0)

func _fail(code: String, extra := "") -> void:
	if _finished:
		return
	_finished = true
	set_process(false)
	_set_status("Failed: " + code)
	if _peer != null:
		_peer.disconnect_from_host()
	var suffix := (" " + extra) if not extra.is_empty() else ""
	print("MP_SETTLEMENT_PROBE status=fail code=%s%s" % [code, suffix])
	get_tree().quit(1)

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

## Validate one `SettlementView` and return it, or `{}` after failing.
func _parse_settlement(raw: Variant, context: String) -> Dictionary:
	if typeof(raw) != TYPE_DICTIONARY:
		_fail("%s_settlement" % context)
		return {}
	var record: Dictionary = raw
	if _as_int(record.get("id")) <= 0:
		_fail("%s_id" % context)
		return {}
	if typeof(record.get("name")) != TYPE_STRING:
		_fail("%s_name" % context)
		return {}
	if typeof(record.get("owner")) != TYPE_STRING:
		_fail("%s_owner_label" % context)
		return {}
	if typeof(record.get("owned")) != TYPE_BOOL:
		_fail("%s_owned" % context)
		return {}
	var owner_value: Variant = record.get("owner_id")
	if owner_value != null and _as_int(owner_value) <= 0:
		_fail("%s_owner_id" % context)
		return {}
	var center: Variant = record.get("center")
	if typeof(center) != TYPE_ARRAY or center.size() != 2:
		_fail("%s_center" % context)
		return {}
	if _as_int(center[0]) < -1000000 or _as_int(center[1]) < -1000000:
		_fail("%s_center_value" % context)
		return {}
	var radius: Variant = record.get("radius_m")
	if _as_int(radius) < 0:
		_fail("%s_radius" % context)
		return {}
	var revision: Variant = record.get("membership_revision")
	if _as_int(revision) < 0:
		_fail("%s_revision" % context)
		return {}
	var tax: Variant = record.get("tax_rate")
	if _as_int(tax) < 0 or _as_int(tax) > 100:
		_fail("%s_tax" % context)
		return {}
	var members: Variant = record.get("members")
	if typeof(members) != TYPE_ARRAY:
		_fail("%s_members" % context)
		return {}
	for member in members:
		if typeof(member) != TYPE_DICTIONARY:
			_fail("%s_member" % context)
			return {}
		if _as_int(member.get("player")) <= 0:
			_fail("%s_member_player" % context)
			return {}
		if typeof(member.get("role")) != TYPE_STRING:
			_fail("%s_member_role" % context)
			return {}
	return record

func _store_settlement(record: Dictionary) -> void:
	_settlements[_as_int(record.get("id"))] = record

func _found_id() -> int:
	return _as_int(_found.get("id"))

func _center_is(record: Dictionary, x: int, z: int) -> bool:
	var center: Variant = record.get("center")
	if typeof(center) != TYPE_ARRAY or center.size() != 2:
		return false
	return _as_int(center[0]) == x and _as_int(center[1]) == z

## SHA-256 over ascending records, then the first 16 hex.
## One line per record: `id|name|owner_id|revision|tax|member_ids`.
func _settlement_checksum() -> String:
	if _settlements.is_empty():
		return ""
	var ids: Array = _settlements.keys()
	ids.sort()
	var context := HashingContext.new()
	if context.start(HashingContext.HASH_SHA256) != OK:
		return ""
	for settlement_id in ids:
		var record: Dictionary = _settlements[settlement_id]
		var owner := _as_int(record.get("owner_id"))
		var member_ids: Array = []
		for member in record.get("members", []):
			member_ids.append(_as_int(member.get("player")))
		member_ids.sort()
		var member_text := ""
		for member_id in member_ids:
			if not member_text.is_empty():
				member_text += ","
			member_text += str(member_id)
		var line := "%d|%s|%d|%d|%d|%s" % [
			settlement_id,
			str(record.get("name", "")),
			owner,
			_as_int(record.get("membership_revision")),
			_as_int(record.get("tax_rate")),
			member_text,
		]
		context.update((line + "\n").to_utf8_buffer())
	return context.finish().hex_encode().substr(0, 16)

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
	title.text = "Connected settlement probe"
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
		["Name", "Name"],
		["Id", "Id"],
		["Owner", "Owner"],
		["Revision", "Revision"],
		["Tax", "Tax"],
		["Members", "Members"],
		["Roster", "Roster"],
		["At (0,0)", "At"],
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
	if _found.is_empty():
		return
	_set_value("Name", str(_found.get("name", "")))
	_set_value("Id", "#%d" % _found_id())
	var owner_text := "none"
	if _owner_id > 0:
		owner_text = "#%d" % _owner_id
		if not _owner_name.is_empty():
			owner_text += " " + _owner_name
	_set_value("Owner", owner_text)
	_set_value("Revision", str(_as_int(_found.get("membership_revision"))))
	_set_value("Tax", "%d%%" % _as_int(_found.get("tax_rate")))
	var members: Variant = _found.get("members", [])
	_set_value("Members", str(members.size() if typeof(members) == TYPE_ARRAY else 0))
	_set_value("Roster", str(_roster_count))
	var at_text := "pending"
	if not _at.is_empty():
		at_text = "#%d %s" % [_as_int(_at.get("id")), str(_at.get("name", ""))]
	_set_value("At", at_text)
	if not _checksum.is_empty():
		_set_value("Checksum", _checksum)

func _set_value(key: String, value: String) -> void:
	if _value_labels.has(key):
		_value_labels[key].text = value

func _set_status(text: String) -> void:
	if _status_label != null:
		_status_label.text = text

func _phase_name() -> String:
	match _phase:
		Phase.CONNECTING:
			return "connecting"
		Phase.WELCOME_WAIT:
			return "welcome_wait"
		Phase.FOUND_SENT:
			return "found_wait"
		Phase.FIRST_LIST_SENT:
			return "list_first_wait"
		Phase.SECOND_LIST_SENT:
			return "list_second_wait"
		Phase.PLAYERS_SENT:
			return "players_wait"
		Phase.AT_SENT:
			return "at_wait"
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
	if _is_integer(value):
		return int(value)
	return -1

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
