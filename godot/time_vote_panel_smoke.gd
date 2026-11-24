extends Node

## Deterministic headless synthetic smoke for `time_vote_panel.gd`.
##
## Covers idle, open -> yes -> passed, open -> no -> failed, admin cancel,
## cooldown, role-disabled controls, duplicate-ballot suppression, every stable
## `vote.*` / `vote.unavailable` error line, the EN/DE table, and the exact wire
## dictionaries. It never opens a socket and never starts a server.
##
## Run:
##   godot --headless --path godot res://time_vote_panel_smoke.tscn
##
## Final marker:
##   TIME_VOTE_UI mode=synthetic status=ok cases=N checks=N commands=5 errors=0 checksum=<8 hex>

const PanelScene := preload("res://time_vote_panel.tscn")
const EXPECTED_COMMANDS := 5

var _panel
var _commands: Array = []
var _stages: Array = []
var _failures := 0
var _checks := 0
var _command_count := 0


func _ready() -> void:
	_panel = PanelScene.instantiate()
	add_child(_panel)
	_panel.command_issued.connect(_on_command)
	_run()
	var checksum := _fnv32("\n".join(_stages))
	if _failures == 0 and _command_count == EXPECTED_COMMANDS:
		print("TIME_VOTE_UI mode=synthetic status=ok cases=%d checks=%d commands=%d errors=0 checksum=%s" % [
			_stages.size(), _checks, _command_count, checksum,
		])
		get_tree().quit(0)
	else:
		print("TIME_VOTE_UI mode=synthetic status=fail cases=%d checks=%d commands=%d errors=%d checksum=%s" % [
			_stages.size(), _checks, _command_count, _failures, checksum,
		])
		get_tree().quit(1)


func _on_command(command: Dictionary) -> void:
	_commands.append(command.duplicate(true))
	_command_count += 1


func _run() -> void:
	# No status yet: every control is disabled until the server speaks.
	_panel.set_permissions(true, false)
	var snap := _stage("waiting")
	_check(snap["state_label"] == "Waiting for server status", "waiting.label", str(snap["state_label"]))
	_check(not snap["call_pause_enabled"], "waiting.call_disabled")
	_check(not snap["yes_enabled"], "waiting.ballot_disabled")
	_panel.press_call("pause")
	_expect_silent("waiting.call")

	# Idle: calls are available, ballots and cancel are not.
	_panel.set_status(_status({}))
	snap = _stage("idle")
	_check(snap["state_label"] == "No vote yet", "idle.label", str(snap["state_label"]))
	_check(snap["target"] == "", "idle.target", str(snap["target"]))
	_check(int(snap["ticks_to_end"]) == 0, "idle.ticks_to_end")
	_check(int(snap["cooldown_remaining"]) == 0, "idle.cooldown")
	_check(snap["call_pause_enabled"] and snap["call_resume_enabled"], "idle.calls_enabled")
	_check(not snap["yes_enabled"] and not snap["no_enabled"], "idle.ballots_disabled")
	_check(not snap["cancel_enabled"], "idle.cancel_disabled")

	_panel.press_call("step")
	_expect_silent("idle.unsupported_target")

	_check(bool(_panel.press_call("pause")), "idle.pause_call.accepted")
	_expect_command({"type": "time_vote", "action": "call", "time": "pause"}, "idle.pause_call")
	_check(bool(_panel.press_call("resume")), "idle.resume_call.accepted")
	_expect_command({"type": "time_vote", "action": "call", "time": "resume"}, "idle.resume_call")

	_panel.press_yes()
	_expect_silent("idle.ballot_without_vote")
	_stage("idle_commands")

	# Open -> yes -> passed. The ballot locks for this vote id; a passed vote
	# starts the 120-tick cooldown, so calls stay disabled.
	_panel.set_status(_status({
		"vote_id": 1,
		"state": "open",
		"action": "pause",
		"initiator": "alice",
		"yes": 1,
		"no": 0,
		"online": 2,
		"started_tick": 10,
		"ends_tick": 70,
		"cooldown_until": 0,
		"paused": false,
		"clock": 10,
		"checksum": "1111111111111111",
	}))
	snap = _stage("open_yes")
	_check(snap["state_label"] == "Vote open", "open_yes.label", str(snap["state_label"]))
	_check(snap["target"] == "pause", "open_yes.target", str(snap["target"]))
	_check(snap["initiator"] == "alice", "open_yes.initiator", str(snap["initiator"]))
	_check(int(snap["yes"]) == 1 and int(snap["no"]) == 0 and int(snap["online"]) == 2, "open_yes.tally")
	_check(int(snap["ticks_to_end"]) == 60, "open_yes.ticks_to_end", str(snap["ticks_to_end"]))
	_check(not snap["call_pause_enabled"] and not snap["call_resume_enabled"], "open_yes.calls_disabled")
	_check(snap["yes_enabled"] and snap["no_enabled"], "open_yes.ballots_enabled")
	_check(not snap["cancel_enabled"], "open_yes.cancel_requires_admin")

	_check(bool(_panel.press_yes()), "open_yes.yes.accepted")
	_expect_command({"type": "time_vote", "action": "yes", "time": null}, "open_yes.yes")
	snap = _stage("yes_voted")
	_check(snap["voted"], "yes_voted.voted")
	_check(not snap["yes_enabled"] and not snap["no_enabled"], "yes_voted.ballot_locked")
	_panel.press_yes()
	_panel.press_no()
	_expect_silent("yes_voted.duplicate_ballots")

	_panel.set_status(_status({
		"vote_id": 1,
		"state": "passed",
		"action": "pause",
		"initiator": "alice",
		"yes": 2,
		"no": 0,
		"online": 2,
		"started_tick": 10,
		"ends_tick": 70,
		"cooldown_until": 190,
		"paused": true,
		"clock": 12,
		"checksum": "2222222222222222",
	}))
	snap = _stage("passed")
	_check(snap["state_label"] == "Vote passed", "passed.label", str(snap["state_label"]))
	_check(int(snap["cooldown_remaining"]) == 178, "passed.cooldown", str(snap["cooldown_remaining"]))
	_check(not snap["call_pause_enabled"], "passed.call_blocked_by_cooldown")
	_check(not snap["yes_enabled"] and not snap["no_enabled"], "passed.ballots_closed")
	_check(snap["paused"], "passed.paused")
	_panel.press_call("resume")
	_expect_silent("passed.cooldown_call")

	# Open -> no -> failed. A new vote id unlocks a fresh ballot even though the
	# previous vote was already voted on.
	_panel.set_status(_status({
		"vote_id": 2,
		"state": "open",
		"action": "resume",
		"initiator": "bob",
		"yes": 1,
		"no": 0,
		"online": 3,
		"started_tick": 100,
		"ends_tick": 160,
		"cooldown_until": 100,
		"paused": true,
		"clock": 100,
		"checksum": "3333333333333333",
	}))
	snap = _stage("open_no")
	_check(snap["target"] == "resume", "open_no.target", str(snap["target"]))
	_check(not snap["voted"], "open_no.new_ballot")
	_check(snap["yes_enabled"] and snap["no_enabled"], "open_no.ballots_enabled")
	_check(int(snap["ticks_to_end"]) == 60, "open_no.ticks_to_end", str(snap["ticks_to_end"]))

	_check(bool(_panel.press_no()), "open_no.no.accepted")
	_expect_command({"type": "time_vote", "action": "no", "time": null}, "open_no.no")
	snap = _stage("no_voted")
	_check(snap["voted"], "no_voted.voted")

	_panel.set_status(_status({
		"vote_id": 2,
		"state": "failed",
		"action": "resume",
		"initiator": "bob",
		"yes": 1,
		"no": 2,
		"online": 3,
		"started_tick": 100,
		"ends_tick": 160,
		"cooldown_until": 460,
		"paused": true,
		"clock": 160,
		"checksum": "4444444444444444",
	}))
	snap = _stage("failed")
	_check(snap["state_label"] == "Vote failed", "failed.label", str(snap["state_label"]))
	_check(int(snap["cooldown_remaining"]) == 300, "failed.cooldown", str(snap["cooldown_remaining"]))
	_check(not snap["call_pause_enabled"], "failed.call_blocked")

	# Admin cancel: cancel needs an open vote and the admin role. A voter (not
	# admin) keeps ballots but cannot cancel.
	_panel.set_permissions(true, true)
	_panel.set_status(_status({
		"vote_id": 3,
		"state": "open",
		"action": "pause",
		"initiator": "carol",
		"yes": 1,
		"no": 0,
		"online": 3,
		"started_tick": 200,
		"ends_tick": 260,
		"cooldown_until": 460,
		"paused": false,
		"clock": 200,
		"checksum": "5555555555555555",
	}))
	snap = _stage("admin_open")
	_check(snap["cancel_enabled"], "admin_open.cancel_enabled")
	_check(bool(_panel.press_cancel()), "admin_open.cancel.accepted")
	_expect_command({"type": "time_vote", "action": "cancel", "time": null}, "admin_open.cancel")

	_panel.set_status(_status({
		"vote_id": 3,
		"state": "cancelled",
		"action": "pause",
		"initiator": "carol",
		"yes": 1,
		"no": 0,
		"online": 3,
		"started_tick": 200,
		"ends_tick": 260,
		"cooldown_until": 502,
		"paused": false,
		"clock": 202,
		"checksum": "6666666666666666",
	}))
	snap = _stage("cancelled")
	_check(snap["state_label"] == "Vote cancelled", "cancelled.label", str(snap["state_label"]))
	_check(not snap["cancel_enabled"], "cancelled.cancel_closed")

	# Cooldown: calls disabled and suppressed until the clock reaches the tick.
	_panel.set_permissions(true, false)
	_panel.set_status(_status({
		"vote_id": 3,
		"state": "cancelled",
		"action": "pause",
		"initiator": "carol",
		"yes": 1,
		"no": 0,
		"online": 3,
		"started_tick": 200,
		"ends_tick": 260,
		"cooldown_until": 520,
		"paused": false,
		"clock": 220,
		"checksum": "7777777777777777",
	}))
	snap = _stage("cooldown")
	_check(int(snap["cooldown_remaining"]) == 300, "cooldown.remaining", str(snap["cooldown_remaining"]))
	_check(not snap["call_pause_enabled"] and not snap["call_resume_enabled"], "cooldown.calls_disabled")
	_panel.press_call("pause")
	_expect_silent("cooldown.call")

	# Role-disabled: no call, ballot, or cancel command escapes.
	_panel.set_permissions(false, false)
	_panel.set_status(_status({
		"vote_id": 4,
		"state": "open",
		"action": "pause",
		"initiator": "dave",
		"yes": 1,
		"no": 0,
		"online": 2,
		"started_tick": 600,
		"ends_tick": 660,
		"cooldown_until": 520,
		"paused": false,
		"clock": 600,
		"checksum": "8888888888888888",
	}))
	snap = _stage("role_disabled")
	_check(not snap["call_pause_enabled"], "role_disabled.call")
	_check(not snap["yes_enabled"] and not snap["no_enabled"], "role_disabled.ballots")
	_check(not snap["cancel_enabled"], "role_disabled.cancel")
	_panel.press_call("pause")
	_panel.press_call("resume")
	_panel.press_yes()
	_panel.press_no()
	_panel.press_cancel()
	_expect_silent("role_disabled.commands")

	# Non-admin voter: cancel stays disabled while ballots work.
	_panel.set_permissions(true, false)
	snap = _stage("admin_off")
	_check(not snap["cancel_enabled"], "admin_off.cancel")
	_check(snap["yes_enabled"] and snap["no_enabled"], "admin_off.ballots")

	# Developer EN/DE table; switching locale emits nothing.
	_panel.set_locale("de")
	snap = _stage("locale_de")
	_check(snap["title"] == "Zeitabstimmung", "locale_de.title", str(snap["title"]))
	_check(snap["state_label"] == "Abstimmung offen", "locale_de.state", str(snap["state_label"]))
	_check(snap["call_pause_label"] == "Pause-Abstimmung starten", "locale_de.call_label")
	_expect_silent("locale_de.no_commands")

	_panel.set_locale("en")
	snap = _stage("locale_en")
	_check(snap["title"] == "Time vote", "locale_en.title", str(snap["title"]))
	_expect_silent("locale_en.no_commands")

	# Every stable error code renders a code-keyed line, independent of the
	# server message.
	var open_line := ""
	for code in _panel.ERROR_CODES:
		_panel.set_error(code, "server detail")
		var line := str(_panel.snapshot()["error_line"])
		_check(line.begins_with("Error: "), "error." + code + ".prefix", line)
		_check(line.ends_with("(%s)" % code), "error." + code + ".code", line)
		_check(not line.contains("server detail"), "error." + code + ".stable", line)
		if code == "vote.open":
			open_line = line
	_panel.set_error("vote.open", "a different server detail")
	_check(str(_panel.snapshot()["error_line"]) == open_line, "error.stable_detail")

	_panel.set_error("vote.future", "New code detail")
	_check(str(_panel.snapshot()["error_line"]).contains("New code detail"), "error.unknown_fallback")

	_stage("errors")
	_panel.set_locale("de")
	_panel.set_error("vote.cooldown", "")
	_check(str(_panel.snapshot()["error_line"]).begins_with("Fehler: "), "error.de_prefix")
	_stage("errors_de")

	_panel.set_locale("en")
	_panel.clear_error()
	snap = _stage("error_cleared")
	_check(snap["error_code"] == "" and snap["error_line"] == "", "error_cleared.empty")

	_expect_silent("final.no_commands")


func _status(overrides: Dictionary) -> Dictionary:
	var base := {
		"vote_id": 0,
		"state": "idle",
		"action": null,
		"initiator": "",
		"yes": 0,
		"no": 0,
		"online": 2,
		"started_tick": 0,
		"ends_tick": 0,
		"cooldown_until": 0,
		"paused": false,
		"clock": 0,
		"checksum": "0000000000000000",
	}
	for key in overrides.keys():
		base[key] = overrides[key]
	return base


func _stage(name: String) -> Dictionary:
	var snap: Dictionary = _panel.snapshot()
	_stages.append("%s=%s" % [name, str(snap.get("render_checksum", ""))])
	print("TIME_VOTE_UI_STAGE name=%s state=%s voted=%d call=%d ballot=%d cancel=%d error=%s checksum=%s" % [
		name,
		str(snap.get("state", "")),
		int(snap.get("voted", false)),
		int(snap.get("call_pause_enabled", false)),
		int(snap.get("yes_enabled", false)),
		int(snap.get("cancel_enabled", false)),
		str(snap.get("error_code", "")) if str(snap.get("error_code", "")) != "" else "none",
		str(snap.get("render_checksum", "")),
	])
	return snap


func _expect_command(expected: Dictionary, label: String) -> void:
	_check(_commands.size() == 1, label + ".count", "count=%d" % _commands.size())
	if _commands.size() >= 1:
		var actual: Dictionary = _commands[0]
		_check(_commands_equal(actual, expected), label + ".payload", JSON.stringify(actual))
	_commands.clear()


func _expect_silent(label: String) -> void:
	_check(_commands.is_empty(), label + ".silent", JSON.stringify(_commands))
	_commands.clear()


func _commands_equal(actual: Dictionary, expected: Dictionary) -> bool:
	if actual.size() != expected.size():
		return false
	for key in expected.keys():
		if not actual.has(key):
			return false
		if actual[key] != expected[key]:
			return false
	return true


func _check(condition: bool, label: String, detail: String = "") -> void:
	_checks += 1
	if condition:
		return
	_failures += 1
	print("TIME_VOTE_UI_FAIL %s %s" % [label, detail])


func _fnv32(text: String) -> String:
	var hash := 2166136261
	for byte in text.to_utf8_buffer():
		hash = (hash ^ int(byte)) & 0xffffffff
		hash = (hash * 16777619) & 0xffffffff
	return "%08x" % hash
