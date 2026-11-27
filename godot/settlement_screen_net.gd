extends RefCounted

## Length-prefixed JSON transport for the connected settlement screen.
##
## No UI code lives here: the screen owns rendering, paging state, and action
## availability. This layer only opens a protocol v1 session, completes
## `hello`/`welcome`, sends one request at a time, and returns the decoded
## response or a stable transport error.
##
## Requests are synchronous by design (`request` returns the response
## dictionary). The shell is a developer/pre-production surface, and the
## connected smoke needs a deterministic request/response order. Waiting is a
## bounded poll of `StreamPeerTCP` with a 1 ms yield; there are no fixed sleeps
## and no retries after a request may have reached the wire.
##
## ## Stable failure codes
##
## Every failure result carries `ok: false`, `error_code`, and `net_code`.
## `error_code` is the stable transport code below, except for a server
## `reject`: it keeps the server code (`settlement.revision_conflict`, ...) so
## the screen can surface it, while `net_code` is `net.rejected` and
## `reject_code` repeats the server code.
##
## - `net.connect`    connect start/refused/timeout; no socket remains open.
## - `net.timeout`    hello or request deadline elapsed; socket closed.
## - `net.frame`      zero/oversized/incomplete frame or a framing read error.
## - `net.shape`      invalid JSON, non-object JSON, missing/empty `type`, a
##                    malformed reject, or the wrong response type. For a
##                    mutation this is returned before the screen applies it.
## - `net.closed`     peer closed before a complete response, or a send could
##                    not be delivered in full. The socket is closed.
## - `net.unexpected` an extra/unsolicited frame, or a resumed welcome where a
##                    fresh session was requested. The socket is closed.
## - `net.rejected`   the server sent a valid `reject` message.
## - `net.protocol`   the welcome protocol version does not match v1.
##
## There is no half-open state: every failure except a protocol `reject` closes
## the socket, and `close()` is idempotent (a second call is a no-op).
##
## ## Retry rule
##
## At most one reconnect (connect + hello) is attempted, and only before this
## request's first byte is written, when the previous session ended with a
## clean FIN (`STATUS_NONE`). A request that was (partially) written is never
## retried, so a mutation cannot be applied twice by this adapter. If the
## reconnect fails, the original request is not sent and the next call returns
## `net.closed` until `open()` is called again.
##
## `scripts/smoke_settlement_screen_net.sh` drives the whole failure matrix
## against a scripted fake server; `scripts/smoke_settlement_screen_connected.sh`
## runs the happy path against a real `townsmith-server`.

const PROTOCOL_VERSION := 1
const DEFAULT_MAX_FRAME_LEN := 1024 * 1024
# Same 64-character placeholder hash the Rust loopback harness uses.
const CONTENT_HASH := "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"

const ERR_CONNECT := "net.connect"
const ERR_TIMEOUT := "net.timeout"
const ERR_FRAME := "net.frame"
const ERR_SHAPE := "net.shape"
const ERR_CLOSED := "net.closed"
const ERR_UNEXPECTED := "net.unexpected"
const ERR_REJECTED := "net.rejected"
const ERR_PROTOCOL := "net.protocol"

# Bounded deadlines, in milliseconds. The smoke lowers these; production keeps
# the defaults.
var connect_timeout_ms := 5000
var hello_timeout_ms := 5000
var request_timeout_ms := 5000
var close_timeout_ms := 1000
var poll_interval_ms := 1
var max_frame_len := DEFAULT_MAX_FRAME_LEN
## One reconnect is allowed only on a clean close before request delivery.
var reconnect_on_clean_close := true

var session_id := ""
var tick := 0
var last_error := ""

var _peer: StreamPeerTCP = null
var _rx := PackedByteArray()
var _messages: Array = []
var _frame_error := ""
var _host := ""
var _port := 0
var _player_name := ""
var _had_session := false
var _reconnect_attempted := false
var _closed_cleanly := false


## Connect, send `hello`, and wait for `welcome`.
## Returns `{ok: true}` or a failure result with a stable `net.*` code.
func open(host: String, port: int, player_name: String) -> Dictionary:
	_close_socket()
	session_id = ""
	tick = 0
	last_error = ""
	_host = host
	_port = port
	_player_name = player_name
	_had_session = false
	_reconnect_attempted = false
	_closed_cleanly = false

	var connected := _connect()
	if not bool(connected.get("ok", false)):
		return connected
	return _hello()


## Send one command and wait for one response.
##
## A `reject` returns `{ok: false, error_code: <server code>,
## net_code: "net.rejected", reject_code: <server code>, message: <reject>}`
## and keeps the session open. Every other failure closes the socket.
## The returned `message` always includes the protocol `type`.
func request(message: Dictionary) -> Dictionary:
	if not _messages.is_empty():
		# A previous response arrived with an extra frame. Do not let it be
		# mistaken for this request's response.
		return _abort(ERR_UNEXPECTED)
	if _peer != null:
		_peer.poll()
		if (
			_peer.get_status() == StreamPeerTCP.STATUS_NONE
			and _rx.is_empty()
			and _frame_error.is_empty()
		):
			_closed_cleanly = true
	if not _is_connected():
		var reconnected := _reconnect_before_delivery()
		if not bool(reconnected.get("ok", false)):
			return reconnected
	if not _is_connected():
		return _abort(ERR_CLOSED)

	var send_error := _send_frame(message)
	if not send_error.is_empty():
		return _abort(send_error)
	var response := _wait_response(request_timeout_ms)
	if not bool(response.get("ok", false)):
		return response
	var reply: Dictionary = response.get("message", {})
	if str(reply.get("type", "")) == "reject":
		return _rejected(reply)
	_reconnect_attempted = false
	_closed_cleanly = false
	return {"ok": true, "message": reply}


## Send one settlement mutation and decode its `settlement_updated` reply.
##
## Returns `{ok: true, settlement, settlement_checksum, world_checksum}` or a
## failure result. A reply of another type is a shape failure, not an accepted
## mutation, and closes the socket before the screen can apply anything.
func mutation(message: Dictionary) -> Dictionary:
	var response := request(message)
	if not bool(response.get("ok", false)):
		return response
	var reply: Dictionary = response.get("message", {})
	if str(reply.get("type", "")) != "settlement_updated":
		return _abort(ERR_SHAPE)
	var record = reply.get("settlement", {})
	if not (record is Dictionary):
		return _abort(ERR_SHAPE)
	return {
		"ok": true,
		"settlement": record,
		"settlement_checksum": str(reply.get("settlement_checksum", "")),
		"world_checksum": str(reply.get("world_checksum", "")),
	}


## D-033 connected mutations. Each sends the freshly read `membership_revision`
## and returns the decoded `settlement_updated` shape from `mutation`.
func settlement_claim(
	settlement_id: int, player_id: int, expected_revision: int
) -> Dictionary:
	return mutation({
		"type": "settlement_claim",
		"settlement_id": settlement_id,
		"player_id": player_id,
		"expected_revision": expected_revision,
	})


func settlement_transfer(
	settlement_id: int, player_id: int, expected_revision: int
) -> Dictionary:
	return mutation({
		"type": "settlement_transfer",
		"settlement_id": settlement_id,
		"player_id": player_id,
		"expected_revision": expected_revision,
	})


func settlement_release(settlement_id: int, expected_revision: int) -> Dictionary:
	return mutation({
		"type": "settlement_release",
		"settlement_id": settlement_id,
		"expected_revision": expected_revision,
	})


func settlement_invite(
	settlement_id: int, player_id: int, role: String, expected_revision: int
) -> Dictionary:
	return mutation({
		"type": "settlement_invite",
		"settlement_id": settlement_id,
		"player_id": player_id,
		"role": role,
		"expected_revision": expected_revision,
	})


func settlement_set_role(
	settlement_id: int, player_id: int, role: String, expected_revision: int
) -> Dictionary:
	return mutation({
		"type": "settlement_set_role",
		"settlement_id": settlement_id,
		"player_id": player_id,
		"role": role,
		"expected_revision": expected_revision,
	})


func settlement_remove_member(
	settlement_id: int, player_id: int, expected_revision: int
) -> Dictionary:
	return mutation({
		"type": "settlement_remove_member",
		"settlement_id": settlement_id,
		"player_id": player_id,
		"expected_revision": expected_revision,
	})


## Poll the peer and report whether the socket is still connected.
func is_open() -> bool:
	if _peer == null:
		return false
	_peer.poll()
	if _peer.get_status() == StreamPeerTCP.STATUS_CONNECTED:
		return true
	if (
		_peer.get_status() == StreamPeerTCP.STATUS_NONE
		and _rx.is_empty()
		and _frame_error.is_empty()
	):
		_closed_cleanly = true
	return false


## Send `disconnect`, wait briefly for `goodbye`, then close the socket.
## Never fails the caller and is safe to call more than once.
func close() -> void:
	var peer := _peer
	if peer == null:
		return
	_had_session = false
	_closed_cleanly = false
	if peer.get_status() == StreamPeerTCP.STATUS_CONNECTED:
		_send_frame({"type": "disconnect"})
		var deadline := Time.get_ticks_msec() + maxi(1, close_timeout_ms)
		while Time.get_ticks_msec() <= deadline:
			peer.poll()
			_read_frames()
			if _take_message("goodbye"):
				break
			if _peer == null or _peer.get_status() != StreamPeerTCP.STATUS_CONNECTED:
				break
			OS.delay_msec(maxi(1, poll_interval_ms))
	_close_socket()


func _connect() -> Dictionary:
	_close_socket()
	_closed_cleanly = false
	_peer = StreamPeerTCP.new()
	var error := _peer.connect_to_host(_host, _port)
	if error != OK:
		return _abort(ERR_CONNECT)
	var deadline := Time.get_ticks_msec() + maxi(1, connect_timeout_ms)
	while Time.get_ticks_msec() <= deadline:
		_peer.poll()
		match _peer.get_status():
			StreamPeerTCP.STATUS_CONNECTED:
				return {"ok": true}
			StreamPeerTCP.STATUS_ERROR:
				return _abort(ERR_CONNECT)
			StreamPeerTCP.STATUS_NONE:
				return _abort(ERR_CLOSED)
		OS.delay_msec(maxi(1, poll_interval_ms))
	return _abort(ERR_CONNECT)


func _hello() -> Dictionary:
	var send_error := _send_frame({
		"type": "hello",
		"protocol_version": PROTOCOL_VERSION,
		"content_hash": CONTENT_HASH,
		"player_name": _player_name,
	})
	if not send_error.is_empty():
		return _abort(send_error)
	var response := _wait_response(hello_timeout_ms)
	if not bool(response.get("ok", false)):
		return response
	var message: Dictionary = response.get("message", {})
	var reply_type := str(message.get("type", ""))
	if reply_type == "reject":
		return _abort_rejected(message)
	if reply_type != "welcome":
		return _abort(ERR_SHAPE)
	if int(message.get("protocol_version", -1)) != PROTOCOL_VERSION:
		return _abort(ERR_PROTOCOL)
	var welcome_session := str(message.get("session_id", ""))
	if welcome_session.is_empty():
		return _abort(ERR_SHAPE)
	if bool(message.get("resumed", false)):
		return _abort(ERR_UNEXPECTED)
	session_id = welcome_session
	tick = int(message.get("tick", 0))
	_had_session = true
	_reconnect_attempted = false
	_closed_cleanly = false
	return {"ok": true}


## One bounded reconnect, only when the socket was already cleanly closed
## before this request wrote anything. A partial/failed write is never retried.
func _reconnect_before_delivery() -> Dictionary:
	if (
		not reconnect_on_clean_close
		or not _had_session
		or _reconnect_attempted
		or not _closed_cleanly
	):
		return _abort(ERR_CLOSED)
	if _peer != null and _peer.get_status() != StreamPeerTCP.STATUS_NONE:
		return _abort(ERR_CLOSED)
	_reconnect_attempted = true
	_closed_cleanly = false
	_close_socket()
	var connected := _connect()
	if not bool(connected.get("ok", false)):
		return connected
	return _hello()


func _wait_response(timeout_ms: int) -> Dictionary:
	var deadline := Time.get_ticks_msec() + maxi(1, timeout_ms)
	while true:
		if not _frame_error.is_empty():
			return _abort(_frame_error)
		if _peer == null:
			return _abort(ERR_CLOSED)
		var was_connected := _peer.get_status() == StreamPeerTCP.STATUS_CONNECTED
		if was_connected:
			_peer.poll()
			_read_frames()
		if not _messages.is_empty():
			var message: Dictionary = _messages.pop_front()
			if not _messages.is_empty():
				# One response per request: a second frame is unsolicited.
				return _abort(ERR_UNEXPECTED)
			return {"ok": true, "message": message}
		if not was_connected or _peer.get_status() != StreamPeerTCP.STATUS_CONNECTED:
			if not _rx.is_empty():
				return _abort(ERR_FRAME)
			if _frame_error.is_empty() and _peer.get_status() == StreamPeerTCP.STATUS_NONE:
				_closed_cleanly = true
			return _abort(ERR_CLOSED)
		if Time.get_ticks_msec() > deadline:
			return _abort(ERR_TIMEOUT)
		OS.delay_msec(maxi(1, poll_interval_ms))
	return _abort(ERR_TIMEOUT)


## Returns an empty string on a complete write, otherwise a stable error code.
func _send_frame(message: Dictionary) -> String:
	if not _is_connected():
		return ERR_CLOSED
	if str(message.get("type", "")).is_empty():
		return ERR_SHAPE
	var payload := JSON.stringify(message).to_utf8_buffer()
	if payload.is_empty():
		return ERR_SHAPE
	if payload.size() > max_frame_len:
		return ERR_FRAME
	var frame := PackedByteArray()
	frame.resize(4)
	_write_u32_be(frame, 0, payload.size())
	frame.append_array(payload)
	var result := _peer.put_partial_data(frame)
	if int(result[0]) != OK or int(result[1]) != frame.size():
		return ERR_CLOSED
	return ""


func _read_frames() -> void:
	if _peer == null or _peer.get_status() != StreamPeerTCP.STATUS_CONNECTED:
		return
	var available := _peer.get_available_bytes()
	if available > 0:
		var result := _peer.get_partial_data(available)
		if result[0] != OK:
			_frame_error = ERR_CLOSED
			return
		_rx.append_array(result[1])

	while _rx.size() >= 4:
		var length := _read_u32_be(_rx, 0)
		if length == 0 or length > max_frame_len:
			_frame_error = ERR_FRAME
			return
		if _rx.size() < 4 + length:
			return
		var payload := _rx.slice(4, 4 + length)
		_rx = _rx.slice(4 + length)
		var json := JSON.new()
		if json.parse(payload.get_string_from_utf8()) != OK:
			_frame_error = ERR_SHAPE
			return
		var parsed: Variant = json.data
		if typeof(parsed) != TYPE_DICTIONARY:
			_frame_error = ERR_SHAPE
			return
		var message: Dictionary = parsed
		if str(message.get("type", "")).is_empty():
			_frame_error = ERR_SHAPE
			return
		_messages.append(message)


func _take_message(tag: String) -> bool:
	for index in _messages.size():
		if str((_messages[index] as Dictionary).get("type", "")) == tag:
			_messages.remove_at(index)
			return true
	return false


func _rejected(message: Dictionary) -> Dictionary:
	var code := str(message.get("code", ""))
	if code.is_empty():
		return _abort(ERR_SHAPE)
	last_error = code
	return {
		"ok": false,
		"error_code": code,
		"net_code": ERR_REJECTED,
		"reject_code": code,
		"message": message,
	}


func _abort_rejected(message: Dictionary) -> Dictionary:
	var rejected := _rejected(message)
	_close_socket()
	return rejected


func _abort(code: String) -> Dictionary:
	_close_socket()
	return _fail(code)


func _fail(code: String) -> Dictionary:
	last_error = code
	return {"ok": false, "error_code": code, "net_code": code}


func _is_connected() -> bool:
	return _peer != null and _peer.get_status() == StreamPeerTCP.STATUS_CONNECTED


## Idempotent: detaches the peer first, then disconnects it once.
func _close_socket() -> void:
	var peer := _peer
	_peer = null
	_rx = PackedByteArray()
	_messages.clear()
	_frame_error = ""
	if peer != null:
		peer.disconnect_from_host()


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
