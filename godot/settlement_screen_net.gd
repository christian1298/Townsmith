extends RefCounted

## Length-prefixed JSON transport for the connected settlement screen.
##
## No UI code lives here: the screen owns rendering, paging state, and action
## availability. This layer only opens a protocol v1 session, completes
## `hello`/`welcome`, sends one request at a time, and returns the decoded
## response or the stable `reject` code.
##
## Requests are synchronous by design (`request` returns the response
## dictionary). The shell is a developer/pre-production surface, and the
## connected smoke needs a deterministic request/response order. Waiting is a
## bounded poll of `StreamPeerTCP` with a 1 ms yield; there are no fixed sleeps
## and no retries beyond the deadline.
##
## `scripts/smoke_settlement_screen_connected.sh` runs this against a fresh
## `townsmith-server --listen PORT --once` per run.

const PROTOCOL_VERSION := 1
const MAX_FRAME_LEN := 1024 * 1024
# Same 64-character placeholder hash the Rust loopback harness uses.
const CONTENT_HASH := "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
const CONNECT_TIMEOUT_MS := 5000
const RESPONSE_TIMEOUT_MS := 5000
const CLOSE_TIMEOUT_MS := 1000
const POLL_INTERVAL_MS := 1

var session_id := ""
var tick := 0
var last_error := ""

var _peer: StreamPeerTCP = null
var _rx := PackedByteArray()
var _messages: Array = []
var _frame_error := ""


## Connect, send `hello`, and wait for `welcome`.
## Returns `{ok: true}` or `{ok: false, error_code: <stable code>}`.
func open(host: String, port: int, player_name: String) -> Dictionary:
	_close_socket()
	_messages.clear()
	_rx = PackedByteArray()
	_frame_error = ""
	session_id = ""
	tick = 0
	last_error = ""

	_peer = StreamPeerTCP.new()
	var error := _peer.connect_to_host(host, port)
	if error != OK:
		_close_socket()
		return _fail("connect_start_%s" % error_string(error))

	var deadline := Time.get_ticks_msec() + CONNECT_TIMEOUT_MS
	while Time.get_ticks_msec() <= deadline:
		_peer.poll()
		match _peer.get_status():
			StreamPeerTCP.STATUS_CONNECTED:
				return _hello(player_name)
			StreamPeerTCP.STATUS_ERROR:
				_close_socket()
				return _fail("connect_failed")
			StreamPeerTCP.STATUS_NONE:
				_close_socket()
				return _fail("connect_lost")
		OS.delay_msec(POLL_INTERVAL_MS)
	_close_socket()
	return _fail("connect_timeout")


## Send one command and wait for one non-`reject` response.
## A `reject` returns `{ok: false, error_code: <code>, message: <reject>}`.
## The returned `message` always includes the protocol `type`.
func request(message: Dictionary) -> Dictionary:
	if _peer == null or _peer.get_status() != StreamPeerTCP.STATUS_CONNECTED:
		return _fail("send_not_connected")
	_frame_error = ""
	if not _send(message):
		return _fail("send_failed")
	var response := _wait_response()
	if not bool(response.get("ok", false)):
		return response
	var reply: Dictionary = response.get("message", {})
	if str(reply.get("type", "")) == "reject":
		var code := str(reply.get("code", "reject"))
		return {
			"ok": false,
			"error_code": code if not code.is_empty() else "reject",
			"message": reply,
		}
	return {"ok": true, "message": reply}


## Send `disconnect`, wait briefly for `goodbye`, then close the socket.
## Never fails the caller: shutdown is best effort and bounded.
func close() -> void:
	if _peer != null and _peer.get_status() == StreamPeerTCP.STATUS_CONNECTED:
		if _send({"type": "disconnect"}):
			var deadline := Time.get_ticks_msec() + CLOSE_TIMEOUT_MS
			while Time.get_ticks_msec() <= deadline:
				_peer.poll()
				_read_frames()
				if _take_message("goodbye"):
					break
				if _peer == null or _peer.get_status() != StreamPeerTCP.STATUS_CONNECTED:
					break
				OS.delay_msec(POLL_INTERVAL_MS)
	_close_socket()


func _hello(player_name: String) -> Dictionary:
	if not _send({
		"type": "hello",
		"protocol_version": PROTOCOL_VERSION,
		"content_hash": CONTENT_HASH,
		"player_name": player_name,
	}):
		_close_socket()
		return _fail("hello_send_failed")
	var response := _wait_response()
	if not bool(response.get("ok", false)):
		_close_socket()
		return response
	var message: Dictionary = response.get("message", {})
	if str(message.get("type", "")) != "welcome":
		_close_socket()
		return _fail("welcome_expected")
	if int(message.get("protocol_version", -1)) != PROTOCOL_VERSION:
		_close_socket()
		return _fail("protocol_version")
	var welcome_session := str(message.get("session_id", ""))
	if welcome_session.is_empty():
		_close_socket()
		return _fail("welcome_session")
	if bool(message.get("resumed", false)):
		_close_socket()
		return _fail("unexpected_resumed_welcome")
	session_id = welcome_session
	tick = int(message.get("tick", 0))
	return {"ok": true}


func _wait_response() -> Dictionary:
	var deadline := Time.get_ticks_msec() + RESPONSE_TIMEOUT_MS
	while Time.get_ticks_msec() <= deadline:
		if not _frame_error.is_empty():
			return _fail(_frame_error)
		if _peer != null:
			_peer.poll()
			_read_frames()
		if not _messages.is_empty():
			return {"ok": true, "message": _messages.pop_front()}
		if _peer == null or _peer.get_status() != StreamPeerTCP.STATUS_CONNECTED:
			# Read a final reject/goodbye already buffered before treating the
			# FIN as a lost connection.
			_read_frames()
			if not _messages.is_empty():
				continue
			return _fail("connection_lost")
		OS.delay_msec(POLL_INTERVAL_MS)
	return _fail("timeout")


func _send(message: Dictionary) -> bool:
	if _peer == null or _peer.get_status() != StreamPeerTCP.STATUS_CONNECTED:
		return false
	var payload := JSON.stringify(message).to_utf8_buffer()
	if payload.is_empty() or payload.size() > MAX_FRAME_LEN:
		return false
	var frame := PackedByteArray()
	frame.resize(4)
	_write_u32_be(frame, 0, payload.size())
	frame.append_array(payload)
	return _peer.put_data(frame) == OK


func _read_frames() -> void:
	if _peer == null:
		return
	var available := _peer.get_available_bytes()
	if available > 0:
		var result := _peer.get_partial_data(available)
		if result[0] != OK:
			_frame_error = "recv_%s" % error_string(result[0])
			return
		_rx.append_array(result[1])

	while _rx.size() >= 4:
		var length := _read_u32_be(_rx, 0)
		if length == 0:
			_frame_error = "frame.zero_length"
			return
		if length > MAX_FRAME_LEN:
			_frame_error = "frame.oversized"
			return
		if _rx.size() < 4 + length:
			return
		var payload := _rx.slice(4, 4 + length)
		_rx = _rx.slice(4 + length)
		var parsed: Variant = JSON.parse_string(payload.get_string_from_utf8())
		if typeof(parsed) != TYPE_DICTIONARY:
			_frame_error = "frame.invalid_json"
			return
		_messages.append(parsed)


func _take_message(tag: String) -> bool:
	for index in _messages.size():
		if str((_messages[index] as Dictionary).get("type", "")) == tag:
			_messages.remove_at(index)
			return true
	return false


func _fail(code: String) -> Dictionary:
	last_error = code
	return {"ok": false, "error_code": code}


func _close_socket() -> void:
	if _peer != null:
		_peer.disconnect_from_host()
	_peer = null
	_rx = PackedByteArray()


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
