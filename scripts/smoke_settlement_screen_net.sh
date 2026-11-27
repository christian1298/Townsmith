#!/usr/bin/env bash
set -euo pipefail

# Deterministic failure matrix for godot/settlement_screen_net.gd.
#
# A scripted Python fake server owns one listener per case (closed port,
# hello reject, bad frame length, truncated JSON, missing type, hello/request
# timeout, wrong response type, close after response, one reconnect, happy
# path). The Godot harness runs every case in one headless boot, prints one
# SETTLEMENT_NET_CASE line per case, and the whole run is repeated so the two
# marker streams can be diffed byte for byte.
#
# Expected final marker:
#   SETTLEMENT_NET_SMOKE status=ok cases=12 retries=1

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GODOT_BIN="${GODOT:-godot}"

if ! command -v "$GODOT_BIN" >/dev/null 2>&1; then
	echo "Godot binary not found: $GODOT_BIN (set GODOT=/path/to/godot)" >&2
	exit 2
fi
if ! command -v python3 >/dev/null 2>&1; then
	echo "python3 is required for the scripted fake server" >&2
	exit 2
fi

if [[ ! -d "$ROOT/godot/.godot" ]]; then
	"$GODOT_BIN" --headless --path "$ROOT/godot" --import >/dev/null 2>&1 || true
fi

SCENARIO_COUNT=11
SMOKE_DIR="$(mktemp -d)"
SERVER_PID=""

stop_server() {
	if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
		kill "$SERVER_PID" 2>/dev/null || true
	fi
	if [[ -n "$SERVER_PID" ]]; then
		wait "$SERVER_PID" 2>/dev/null || true
	fi
	SERVER_PID=""
}

cleanup() {
	stop_server
	rm -rf "$SMOKE_DIR"
}
trap cleanup EXIT

cat >"$SMOKE_DIR/fake_server.py" <<'PY'
"""Scripted protocol-v1 server for the settlement transport smoke."""
import json
import socket
import struct
import threading
import time

HOST = "127.0.0.1"
SCENARIOS = [
    "hello_reject",
    "bad_length",
    "truncated_json",
    "missing_type",
    "hello_timeout",
    "request_timeout",
    "wrong_type",
    "close_after_response",
    "reconnect",
    "success",
]

LIST_ONE = {
    "type": "settlement_list",
    "tick": 0,
    "count": 1,
    "matched": 1,
    "next_after": 1,
    "has_more": False,
    "checksum": "1111222233334444",
    "settlements": [
        {
            "id": 1,
            "name": "Harbor",
            "owner_player_id": 1,
            "membership_revision": 3,
        }
    ],
}
LIST_RECONNECTED = dict(LIST_ONE, count=7, matched=7, next_after=7, checksum="9999aaaabbbbcccc")
UPDATED = {
    "type": "settlement_updated",
    "tick": 0,
    "settlement": {
        "id": 1,
        "name": "Port Harbor",
        "owner_player_id": 1,
        "membership_revision": 4,
    },
    "settlement_checksum": "aaaabbbbccccdddd",
    "world_checksum": "eeeeffff00001111",
}


def recv_exact(conn, size):
    data = b""
    while len(data) < size:
        chunk = conn.recv(size - len(data))
        if not chunk:
            raise EOFError("peer closed")
        data += chunk
    return data


def recv_frame(conn):
    (size,) = struct.unpack(">I", recv_exact(conn, 4))
    return json.loads(recv_exact(conn, size).decode("utf-8"))


def send_frame(conn, message):
    payload = json.dumps(message, separators=(",", ":")).encode("utf-8")
    conn.sendall(struct.pack(">I", len(payload)) + payload)


def send_welcome(conn):
    send_frame(
        conn,
        {
            "type": "welcome",
            "protocol_version": 1,
            "session_id": "smoke-session",
            "tick": 0,
            "resumed": False,
        },
    )


def handle(name, conn):
    try:
        if name == "hello_reject":
            recv_frame(conn)
            send_frame(
                conn,
                {
                    "type": "reject",
                    "code": "content_hash.invalid",
                    "message": "content hash does not match",
                },
            )
            time.sleep(0.05)
        elif name == "bad_length":
            recv_frame(conn)
            conn.sendall(struct.pack(">I", 0xFFFFFFFF))
            time.sleep(1.2)
        elif name == "truncated_json":
            recv_frame(conn)
            payload = b'{"type":"wel'
            conn.sendall(struct.pack(">I", len(payload)) + payload)
            time.sleep(1.2)
        elif name == "missing_type":
            recv_frame(conn)
            payload = b'{"tick":1}'
            conn.sendall(struct.pack(">I", len(payload)) + payload)
            time.sleep(1.2)
        elif name == "hello_timeout":
            recv_frame(conn)
            time.sleep(1.2)
        elif name == "request_timeout":
            recv_frame(conn)
            send_welcome(conn)
            recv_frame(conn)
            time.sleep(1.2)
        elif name == "wrong_type":
            recv_frame(conn)
            send_welcome(conn)
            recv_frame(conn)
            send_frame(conn, LIST_ONE)
            time.sleep(0.05)
        elif name == "close_after_response":
            recv_frame(conn)
            send_welcome(conn)
            recv_frame(conn)
            send_frame(conn, LIST_ONE)
            time.sleep(0.05)
        elif name == "success":
            recv_frame(conn)
            send_welcome(conn)
            request = recv_frame(conn)
            if request.get("type") == "disconnect":
                send_frame(conn, {"type": "goodbye"})
                return
            send_frame(conn, LIST_ONE)
            request = recv_frame(conn)
            if request.get("type") == "disconnect":
                send_frame(conn, {"type": "goodbye"})
                return
            send_frame(conn, UPDATED)
            try:
                request = recv_frame(conn)
                if request.get("type") == "disconnect":
                    send_frame(conn, {"type": "goodbye"})
            except Exception:
                pass
            time.sleep(0.05)
    except Exception:
        pass
    finally:
        try:
            conn.close()
        except OSError:
            pass


def handle_reconnect(first, server):
    try:
        recv_frame(first)
        send_welcome(first)
        first.close()
        second, _ = server.accept()
        try:
            recv_frame(second)
            send_welcome(second)
            recv_frame(second)
            send_frame(second, LIST_RECONNECTED)
            time.sleep(0.2)
        finally:
            second.close()
    except Exception:
        pass
    finally:
        try:
            first.close()
        except OSError:
            pass


def serve(name, server):
    if name == "reconnect":
        # Exactly one first connection, then exactly one reconnect; the
        # handler itself accepts the second so no other thread races it.
        first, _ = server.accept()
        handle_reconnect(first, server)
        return
    while True:
        conn, _ = server.accept()
        threading.Thread(target=handle, args=(name, conn), daemon=True).start()


def main():
    held = []
    # Bound but never listened on: connect() gets ECONNREFUSED deterministically.
    probe = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    probe.bind((HOST, 0))
    held.append(probe)
    print(f"closed_port {probe.getsockname()[1]}", flush=True)

    for name in SCENARIOS:
        server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        server.bind((HOST, 0))
        server.listen(4)
        held.append(server)
        print(f"{name} {server.getsockname()[1]}", flush=True)
        threading.Thread(target=serve, args=(name, server), daemon=True).start()

    threading.Event().wait()


if __name__ == "__main__":
    main()
PY

cat >"$SMOKE_DIR/harness.gd" <<'GD'
extends SceneTree

const AdapterScript = preload("res://settlement_screen_net.gd")

var _ports: Dictionary = {}
var _cases := 0
var _failures := 0


func _initialize() -> void:
	if not _load_ports():
		quit(2)
		return
	_case_closed_port()
	_case_hello_reject()
	_case_bad_length()
	_case_truncated_json()
	_case_missing_type()
	_case_hello_timeout()
	_case_request_timeout()
	_case_wrong_response_type()
	_case_close_after_response()
	_case_reconnect()
	_case_success()
	_case_double_close()
	if _failures > 0:
		print("SETTLEMENT_NET_SMOKE status=failed cases=%d failures=%d" % [_cases, _failures])
		quit(1)
		return
	print("SETTLEMENT_NET_SMOKE status=ok cases=%d retries=1" % _cases)
	quit(0)


func _load_ports() -> bool:
	var path := ""
	var args := OS.get_cmdline_user_args()
	for index in args.size():
		if args[index] == "--ports" and index + 1 < args.size():
			path = args[index + 1]
	if path.is_empty():
		printerr("missing --ports")
		return false
	var file := FileAccess.open(path, FileAccess.READ)
	if file == null:
		printerr("cannot open ports file: %s" % path)
		return false
	while not file.eof_reached():
		var line := file.get_line().strip_edges()
		if line.is_empty():
			continue
		var parts := line.split(" ", false)
		if parts.size() == 2:
			_ports[parts[0]] = int(parts[1])
	return not _ports.is_empty()


func _adapter():
	var net = AdapterScript.new()
	net.connect_timeout_ms = 250
	net.hello_timeout_ms = 250
	net.request_timeout_ms = 250
	net.close_timeout_ms = 100
	return net


func _port(scenario: String) -> int:
	if not _ports.has(scenario):
		printerr("missing scenario port: %s" % scenario)
		return 1
	return int(_ports[scenario])


func _code(result: Dictionary) -> String:
	if bool(result.get("ok", false)):
		return "ok"
	return str(result.get("net_code", result.get("error_code", "?")))


func _record(name: String, expect: String, got: String, ok: bool) -> void:
	_cases += 1
	if not ok:
		_failures += 1
	print("SETTLEMENT_NET_CASE name=%s expect=%s got=%s status=%s" % [
		name, expect, got, "ok" if ok else "fail",
	])


func _check_failure(name: String, expect: String, result: Dictionary, net) -> void:
	var got := _code(result)
	var socket_closed: bool = not net.is_open()
	_record(name, expect, got, got == expect and socket_closed)
	net.close()


func _open_failed(name: String, expect: String, opened: Dictionary, net) -> bool:
	if bool(opened.get("ok", false)):
		return false
	var got := "open:%s" % _code(opened)
	_record(name, expect, got, false)
	net.close()
	return true


func _case_closed_port() -> void:
	var net = _adapter()
	var result: Dictionary = net.open("127.0.0.1", _port("closed_port"), "Mayor")
	_check_failure("closed_port", "net.connect", result, net)


func _case_hello_reject() -> void:
	var net = _adapter()
	var result: Dictionary = net.open("127.0.0.1", _port("hello_reject"), "Mayor")
	var got := _code(result)
	var extra := (
		str(result.get("net_code", "")) == "net.rejected"
		and str(result.get("reject_code", "")) == "content_hash.invalid"
	)
	_record("hello_reject", "net.rejected", got, got == "net.rejected" and extra and not net.is_open())
	net.close()


func _case_bad_length() -> void:
	var net = _adapter()
	var result: Dictionary = net.open("127.0.0.1", _port("bad_length"), "Mayor")
	_check_failure("bad_length", "net.frame", result, net)


func _case_truncated_json() -> void:
	var net = _adapter()
	var result: Dictionary = net.open("127.0.0.1", _port("truncated_json"), "Mayor")
	_check_failure("truncated_json", "net.shape", result, net)


func _case_missing_type() -> void:
	var net = _adapter()
	var result: Dictionary = net.open("127.0.0.1", _port("missing_type"), "Mayor")
	_check_failure("missing_type", "net.shape", result, net)


func _case_hello_timeout() -> void:
	var net = _adapter()
	var result: Dictionary = net.open("127.0.0.1", _port("hello_timeout"), "Mayor")
	_check_failure("hello_timeout", "net.timeout", result, net)


func _case_request_timeout() -> void:
	var net = _adapter()
	var opened: Dictionary = net.open("127.0.0.1", _port("request_timeout"), "Mayor")
	if _open_failed("request_timeout", "net.timeout", opened, net):
		return
	var result: Dictionary = net.request({"type": "settlement_list"})
	_check_failure("request_timeout", "net.timeout", result, net)


func _case_wrong_response_type() -> void:
	var net = _adapter()
	var opened: Dictionary = net.open("127.0.0.1", _port("wrong_type"), "Mayor")
	if _open_failed("wrong_response_type", "net.shape", opened, net):
		return
	var result: Dictionary = net.mutation({
		"type": "settlement_rename",
		"settlement_id": 1,
		"name": "Nope",
		"expected_revision": 3,
	})
	var got := _code(result)
	var applied := bool(result.get("ok", false))
	_record("wrong_response_type", "net.shape", got, got == "net.shape" and not applied and not net.is_open())
	net.close()


func _case_close_after_response() -> void:
	var net = _adapter()
	net.reconnect_on_clean_close = false
	var opened: Dictionary = net.open("127.0.0.1", _port("close_after_response"), "Mayor")
	if _open_failed("close_after_response", "net.closed", opened, net):
		return
	var first: Dictionary = net.request({"type": "settlement_list"})
	var first_message: Dictionary = first.get("message", {}) if bool(first.get("ok", false)) else {}
	var first_ok := (
		bool(first.get("ok", false))
		and str(first_message.get("checksum", "")) == "1111222233334444"
	)
	var deadline := Time.get_ticks_msec() + 1500
	while net.is_open() and Time.get_ticks_msec() < deadline:
		OS.delay_msec(1)
	var second: Dictionary = net.request({"type": "settlement_list"})
	var got := _code(second)
	_record("close_after_response", "net.closed", got, first_ok and got == "net.closed" and not net.is_open())
	net.close()


func _case_reconnect() -> void:
	var net = _adapter()
	var opened: Dictionary = net.open("127.0.0.1", _port("reconnect"), "Mayor")
	if _open_failed("reconnect", "ok", opened, net):
		return
	var deadline := Time.get_ticks_msec() + 1500
	while net.is_open() and Time.get_ticks_msec() < deadline:
		OS.delay_msec(1)
	var result: Dictionary = net.request({"type": "settlement_list"})
	var message: Dictionary = result.get("message", {}) if bool(result.get("ok", false)) else {}
	var got := _code(result)
	var ok := bool(result.get("ok", false)) and int(message.get("count", 0)) == 7
	_record("reconnect", "ok", got, ok)
	net.close()


func _case_success() -> void:
	var net = _adapter()
	var opened: Dictionary = net.open("127.0.0.1", _port("success"), "Mayor")
	if _open_failed("success", "ok", opened, net):
		return
	var listed: Dictionary = net.request({"type": "settlement_list"})
	var list_message: Dictionary = listed.get("message", {}) if bool(listed.get("ok", false)) else {}
	var list_ok := (
		bool(listed.get("ok", false))
		and str(list_message.get("type", "")) == "settlement_list"
		and int(list_message.get("count", 0)) == 1
	)
	var renamed: Dictionary = net.mutation({
		"type": "settlement_rename",
		"settlement_id": 1,
		"name": "Port Harbor",
		"expected_revision": 3,
	})
	var record: Dictionary = renamed.get("settlement", {}) if bool(renamed.get("ok", false)) else {}
	var rename_ok := (
		bool(renamed.get("ok", false))
		and str(record.get("name", "")) == "Port Harbor"
		and str(renamed.get("world_checksum", "")) == "eeeeffff00001111"
	)
	var got := "ok" if list_ok and rename_ok else (_code(listed) if not list_ok else _code(renamed))
	_record("success", "ok", got, list_ok and rename_ok)
	net.close()


func _case_double_close() -> void:
	var net = _adapter()
	var opened: Dictionary = net.open("127.0.0.1", _port("success"), "Mayor")
	net.close()
	net.close()
	var ok: bool = bool(opened.get("ok", false)) and not net.is_open()
	_record("double_close", "closed", "closed" if not net.is_open() else "open", ok)
GD

pick_ports() {
	: >"$SMOKE_DIR/ports.tsv"
	python3 "$SMOKE_DIR/fake_server.py" >"$SMOKE_DIR/ports.tsv" 2>"$SMOKE_DIR/server.log" &
	SERVER_PID=$!
	local deadline=$((SECONDS + 10))
	while [[ "$(wc -l <"$SMOKE_DIR/ports.tsv")" -lt "$SCENARIO_COUNT" ]]; do
		if ! kill -0 "$SERVER_PID" 2>/dev/null; then
			echo "fake server exited during startup" >&2
			cat "$SMOKE_DIR/server.log" >&2 || true
			exit 1
		fi
		if ((SECONDS > deadline)); then
			echo "fake server did not report $SCENARIO_COUNT ports" >&2
			cat "$SMOKE_DIR/server.log" >&2 || true
			exit 1
		fi
		sleep 0.02
	done
}

run_harness() {
	local output="$1"
	timeout 120s env -u DISPLAY -u WAYLAND_DISPLAY \
		"$GODOT_BIN" --headless --path "$ROOT/godot" \
		--script "$SMOKE_DIR/harness.gd" -- \
		--ports "$SMOKE_DIR/ports.tsv" >"$output" 2>&1 || {
		cat "$output" >&2
		echo "settlement net harness exited non-zero" >&2
		return 1
	}
	if grep -q '^ERROR' "$output"; then
		cat "$output" >&2
		echo "settlement net harness logged an engine error" >&2
		return 1
	fi
	if [[ "$(grep -c '^SETTLEMENT_NET_CASE .* status=ok$' "$output" || true)" -ne 12 ]]; then
		cat "$output" >&2
		echo "expected 12 passing SETTLEMENT_NET_CASE lines" >&2
		return 1
	fi
	if grep -q '^SETTLEMENT_NET_CASE .* status=fail$' "$output"; then
		cat "$output" >&2
		echo "a settlement net case failed" >&2
		return 1
	fi
	if [[ "$(grep -c '^SETTLEMENT_NET_SMOKE status=ok cases=12 retries=1$' "$output" || true)" -ne 1 ]]; then
		cat "$output" >&2
		echo "missing or unexpected SETTLEMENT_NET_SMOKE marker" >&2
		return 1
	fi
	grep '^SETTLEMENT_NET_' "$output" >"$output.markers"
}

pick_ports
if ! run_harness "$SMOKE_DIR/first.log"; then
	exit 1
fi
stop_server
pick_ports
if ! run_harness "$SMOKE_DIR/second.log"; then
	exit 1
fi
stop_server

if ! diff -u "$SMOKE_DIR/first.log.markers" "$SMOKE_DIR/second.log.markers"; then
	echo "settlement net smoke failed: marker streams differ between runs" >&2
	exit 1
fi

echo "$(grep '^SETTLEMENT_NET_SMOKE ' "$SMOKE_DIR/first.log.markers")"
