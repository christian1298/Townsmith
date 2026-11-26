extends Node3D

## Debug 3D multiplayer world client.
##
## This is the first client render integration for the live protocol v0 path:
## it connects to a loopback `townsmith-server`, sends `hello` and a
## deterministic `set_interest`, then draws the server's bounded `interest_view`
## road and building records in 3D world space (1 unit = 1 m). It is a debug
## client, not gameplay networking: there is no prediction, interpolation,
## terrain/chunk/entity/agent streaming, delta snapshot, or production camera.
##
## Flow:
##   hello -> welcome
##   six deterministic place_road commands -> six snapshots (server-authoritative)
##   set_interest {center 0,0 radius 96 m} -> snapshot + interest_view
##   build the 3D scene from the received view
##   --smoke only: set_interest {center 32,32 radius 16 m} -> snapshot +
##     interest_view with zero roads/buildings, proving the render clears
##   optional --place-hash: place_blueprint {blueprint_hash, x, z, rotation} ->
##     place_blueprint_result; render the authoritative placement box
##   disconnect -> goodbye
##
## The scene draws a ground plane, a world-aligned grid with world axes, the
## interest ring, one flat box per road, and one box per bounded building
## record. Roads are 6 m wide and 0.24 m thick at their centreline; building
## boxes use the server's `radius_m` render hint as a footprint diameter (not an
## authoritative footprint). A HUD shows tick, population, balance, road and
## building counts, both truncation flags, and the server checksum.
##
## Placement is opt-in: only `--place-hash <blake3:…|64 hex>` sends the D-032
## command after the zero-view probe. The box uses the authoritative
## `placement.center`, `placement.footprint_m`, and `placement.bounds_m` on the
## client ground (bottom at y=0); the server ground placeholder is 40 m, so the
## Y value is a render hint, never a server height. Without the flag the flow,
## markers, and frame counts are unchanged.
##
## Placement UI is a second opt-in path: `--place-ui --library <abs dir>
## [--place-ui-script "..."]`. It loads the local catalog through
## `TownsmithCore.blueprint_library_entries` (instantiated via `ClassDB`, never
## referenced statically), draws a translucent `BoxMesh` ghost from the entry's
## `bounds_m` on the 1 m grid, and keeps placement mode active through
## authoritative results and rejects. The zero-view probe is skipped; the first
## view and the existing `MP_WORLD_CLIENT` marker still run. A scripted
## `place` sends `place_blueprint`, renders the authoritative box, and refreshes
## with `set_interest`; `reject` records the server code and continues.
## Keyboard (interactive): Tab / Shift+Tab cycle entries, wheel cycles, arrows
## nudge 1 m, R rotates 90°, Enter places, Esc cancels. Esc/q cancels. Without
## `--place-ui` none of this code runs.
##
## Chat is a third opt-in path: `--chat [--chat-script "..."]`. After the main
## interest view it skips the zero-view probe, sends `chat_since {after: 0}`,
## runs `say:<text>` / `poll` actions in order, and disconnects. The developer
## `godot/chat_panel.tscn` is instantiated as the transcript model; on a
## non-headless, non-smoke run it is also mounted bottom-left in its own
## `CanvasLayer`. The extra `MP_CHAT status=ok entries=N checksum=...` marker is
## printed before the normal done marker. Without `--chat` no chat code runs.
##
## `--chat-listen --chat-expect "<text>"` is the passive push probe: after
## `hello` it sends nothing (no `chat`, no `chat_since`) and waits for a
## server-pushed `chat_sent` from another session. On a matching text it prints
## `MP_CHAT_PUSH ...` and disconnects. A timeout or close before the match fails
## with `chat_push_timeout`. It cannot be combined with `--chat-script` or
## `--place-ui`. With `--chat`, incoming pushed lines are always applied to the
## panel and never advance the script; `chat_since` remains the catch-up path.
##
## `--chat-reconnect` (used with `--chat`) proves chat catch-up across a real
## session resume. The first connection sends one chat line (default
## `before-resume`, override with `--chat-text`), records its entry seq and the
## session token, then disconnects. The second connection sends
## `resume {session_id, last_tick}`; after the resumed welcome and snapshot it
## drops its in-memory transcript and sends `chat_since {after: 0}` so the
## server's retained log is the only catch-up source. It must return exactly the
## one pre-resume entry, which is merged and printed as
## `MP_CHAT_RECONNECT status=ok entries=1 last_seq=1 token_reused=true
## checksum=...` instead of `MP_CHAT`; the normal done marker still follows.
## The first message is never re-sent. It cannot be combined with
## `--chat-listen` (`chat_mode_conflict`) or `--chat-script`.
##
## `godot/mp_interest_render.gd` is reused for road parsing, the 64-road wire
## cap, and the deterministic 16-hex road checksum. Buildings are parsed
## defensively here (cap 64, ascending id, malformed entries skipped) with a
## separate 16-hex checksum. Headless Godot returns identity transforms from
## `MultiMesh`, so the smoke asserts CPU-side transform mirrors, never the
## GPU-side copies.
##
## Generic reconnect/resume stays out of scope: `godot/mp_client.gd`
## (`--reconnect`) covers the resume wire path, and this scene is a one-shot
## render probe. `--chat-reconnect` only resumes so it can prove that a chat
## line sent before the disconnect is recovered from the server log; it does
## not fake or replay authoritative world state.
##
## Run:
##   godot --path godot res://mp_world_client.tscn -- --port 47015
##   godot --headless --path godot res://mp_world_client.tscn -- --smoke --port 47015
##   godot --headless --path godot res://mp_world_client.tscn -- --smoke --port 47015 \
##     --place-hash blake3:<64 hex> --place-x 32 --place-z 32 --place-rotation 0
##
## `scripts/smoke_mp_world_client.sh` starts a fresh `--sessions 1` server per
## run and asserts identical `MP_WORLD_CLIENT` / `MP_WORLD_CLIENT_ZERO` markers
## across two runs. `scripts/smoke_mp_placement.sh` seeds a client fixture
## library, runs the same scene with `--place-hash`, and asserts the
## `MP_PLACEMENT` result plus 11/13 frame counts across two runs.
## `scripts/smoke_mp_placement_ui.sh` seeds separate client and server fixture
## libraries, runs `--place-ui` with a scripted place/overlap/cancel sequence,
## and asserts identical `MP_PLACE_UI*` markers across two fresh runs.
##
## See docs/05 - Multiplayer.md §Interest view 3D world client.

const InterestRender = preload("res://mp_interest_render.gd")
const PlacementUI = preload("res://mp_placement_ui.gd")
const ChatScene = preload("res://chat_panel.tscn")

enum Phase {
	CONNECTING,
	HELLO_SENT,
	ROAD_SENT,
	INTEREST_SENT,
	INTEREST_VIEW_WAIT,
	ZERO_INTEREST_SENT,
	ZERO_VIEW_WAIT,
	UI_SNAPSHOT_SENT,
	UI_VIEW_WAIT,
	UI_PLACE_WAIT,
	UI_LOCK_WAIT,
	UI_UNLOCK_WAIT,
	UI_ACTIVE,
	PLACE_SENT,
	CHAT_ACTIVE,
	CHAT_LISTEN,
	CHAT_RECONNECT_WAIT_SENT,
	CHAT_RECONNECT_WAIT_DISCONNECT,
	CHAT_RECONNECT_CONNECTING,
	CHAT_RESUME_WELCOME_WAIT,
	CHAT_RESUME_SNAPSHOT_WAIT,
	CHAT_RECONNECT_CATCHUP_WAIT,
	DISCONNECT_SENT,
}

const PROTOCOL_VERSION := 1
const MAX_FRAME_LEN := 1024 * 1024
const DEFAULT_HOST := "127.0.0.1"
const DEFAULT_PORT := 47015
const DEFAULT_NAME := "mp-world-client"
const CONNECT_TIMEOUT_MS := 5000
const STEP_TIMEOUT_MS := 5000
# Same 64-character placeholder hash the Rust loopback harness uses.
const CONTENT_HASH := "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"

const DEFAULT_CENTER := Vector2(0.0, 0.0)
const DEFAULT_RADIUS_M := 96.0
# Zero/narrow-viewport probe: no road midpoint or synthetic building centre is
# within 16 m of this point.
const ZERO_CENTER := Vector2(32.0, 32.0)
const ZERO_RADIUS_M := 16.0
const MAX_RADIUS_M := 2048.0

# Optional D-032 placement. The hash must be a canonical `blake3:` + 64
# lowercase hex, or bare 64 lowercase hex normalized to that form.
const BLUEPRINT_HASH_PREFIX := "blake3:"
const DEFAULT_PLACE_X := 32.0
const DEFAULT_PLACE_Z := 32.0
const DEFAULT_PLACE_ROTATION := 0

# Six axis-aligned roads forming a square with a cross. Every midpoint is
# inside the 96 m interest circle and each segment is a legal 128-cell road.
const ROAD_PLAN := [
	{"start": [-64, 0], "end": [64, 0]},
	{"start": [0, -64], "end": [0, 64]},
	{"start": [-64, -64], "end": [-64, 64]},
	{"start": [64, -64], "end": [64, 64]},
	{"start": [-64, -64], "end": [64, -64]},
	{"start": [-64, 64], "end": [64, 64]},
]

const MAX_BUILDINGS := 64
const DEFAULT_BUILDING_RADIUS_M := 8.0
const MIN_BUILDING_RADIUS_M := 0.5
const MAX_BUILDING_RADIUS_M := 256.0
const BUILDING_HEIGHT_RATIO := 0.5
const MIN_BUILDING_HEIGHT_M := 1.0
const MAX_BUILDING_HEIGHT_M := 12.0

const MIN_WIDTH_M := 0.5
const MAX_WIDTH_M := 64.0
const ROAD_HEIGHT_M := 0.24
const ROAD_Y_M := 0.12
const MIN_ROAD_LENGTH_M := 0.5
const GROUND_MARGIN := 3.0
const GRID_STEP_M := 16.0
const GRID_MIN_EXTENT_M := 64.0
const RING_SEGMENTS := 96
const CAMERA_FOV := 50.0

const COLOR_GROUND := Color(0.065, 0.082, 0.105)
const COLOR_GRID := Color(0.18, 0.23, 0.29)
const COLOR_AXIS_X := Color(0.90, 0.34, 0.30)
const COLOR_AXIS_Y := Color(0.36, 0.86, 0.46)
const COLOR_AXIS_Z := Color(0.32, 0.56, 0.96)
const COLOR_RING := Color(0.32, 0.44, 0.58)
const COLOR_CENTER := Color(1.0, 0.92, 0.62)
const COLOR_INSIDE := Color(0.96, 0.78, 0.32)
const COLOR_OUTSIDE := Color(0.92, 0.28, 0.22)
const COLOR_BUILDING_INSIDE := Color(0.30, 0.82, 0.92)
const COLOR_BUILDING_OUTSIDE := Color(0.86, 0.36, 0.94)
const COLOR_PLACEMENT := Color(0.98, 0.64, 0.24)
const COLOR_UI_GHOST := Color(0.40, 0.96, 0.72, 0.42)
const COLOR_UI_PLACEMENT := Color(0.98, 0.64, 0.24)
const UI_MOVE_STEP_M := 1.0
const UI_MAX_ACTIONS := 256

# Opt-in chat (`--chat`). Pipe-separated actions: `say:<text>` or `poll`.
# Text is validated through the chat panel's server-mirrored rules.
const DEFAULT_CHAT_SCRIPT := "say:hello|poll|say:world"
# `--chat-reconnect` sends exactly one line before the resume. The value is
# validated through the same panel `draft` rules as a scripted `say`.
const DEFAULT_CHAT_RECONNECT_TEXT := "before-resume"
const CHAT_MAX_ACTIONS := 64
const CHAT_LAYER := 2
const CHAT_MARGIN := 12.0
const CHAT_PANEL_WIDTH := 540.0
const CHAT_PANEL_HEIGHT := 430.0
const CHAT_UNREAD_MAX := 999

var _peer: StreamPeerTCP
var _phase: int = Phase.CONNECTING
var _rx := PackedByteArray()
var _deadline_ms := 0
var _finished := false
var _status_text := "Connecting..."

var _smoke := false
var _host := DEFAULT_HOST
var _port := DEFAULT_PORT
var _player_name := DEFAULT_NAME
var _timeout_ms := STEP_TIMEOUT_MS
var _center := DEFAULT_CENTER
var _radius_m := DEFAULT_RADIUS_M

var _frames_sent := 0
var _frames_received := 0
var _road_index := 0
var _session_id := ""
var _session_token := ""
var _last_tick := -1
var _last_population := -1
var _last_balance := 0
var _last_checksum := ""
var _visible_roads := -1

var _container: Node3D
var _hud_layer: CanvasLayer
var _hud_label: Label
var _camera: Camera3D
var _road_multimesh: MultiMesh
var _building_multimesh: MultiMesh
var _roads: Array = []
var _buildings: Array = []
var _road_transforms: Array = []
var _building_transforms: Array = []
var _truncated := false
var _buildings_truncated := false
var _road_checksum := ""
var _building_checksum := ""
var _inside := 0
var _outside := 0
var _buildings_inside := 0
var _buildings_outside := 0
var _view_tick := -1
var _view_center := Vector2.ZERO
var _view_radius_m := 0.0
var _has_view := false
var _grid_lines := 0
var _main_marker := ""
var _zero_marker := ""

var _place_hash := ""
var _place_x := DEFAULT_PLACE_X
var _place_z := DEFAULT_PLACE_Z
var _place_rotation := DEFAULT_PLACE_ROTATION
var _has_placement := false
var _placement_id := 0
var _placement_hash := ""
var _placement_checksum := ""
var _placement_world_checksum := ""
var _placement_center := Vector2.ZERO
var _placement_footprint := Vector2.ZERO
var _placement_bounds := Vector3.ZERO
var _placement_transform := Transform3D.IDENTITY
var _placement_size := Vector3.ZERO
var _placement_marker := ""

# Opt-in placement UI (`--place-ui`). Everything below is inert without the
# flag, so the legacy markers, frame counts, and checksums stay unchanged.
var _place_ui := false
var _library_root := ""
var _place_ui_script := ""
var _ui_actions: Array = []
var _ui_action_index := 0
var _ui_actions_run := 0
var _ui_core: Object = null
var _ui_entries: Array = []
var _ui_catalog_checksum := ""
var _ui_selected := 0
var _ui_center := Vector2.ZERO
var _ui_rotation := 0
var _ui_root: Node3D
var _ui_ghost: MeshInstance3D
var _ui_ghost_transform := Transform3D.IDENTITY
var _ui_ghost_size := Vector3.ZERO
var _ui_placements: Array = []
var _ui_placement_transforms: Array = []
var _ui_accepted := 0
var _ui_rejected := 0
var _ui_last_error := ""
var _ui_pending_hash := ""
var _ui_pending_center := Vector2.ZERO
var _ui_pending_rotation := 0
# Advisory blueprint locks (`blueprint_locks` once after welcome). The map is
# hash -> {player_id, player, tick, expires_tick}; `_ui_own_locks` marks hashes
# this session locked, so the HUD can say "LOCKED you" without trusting a
# display name.
var _ui_locks := {}
var _ui_own_locks := {}
var _ui_lock_checksum := ""
var _ui_lock_pending_hash := ""
var _ui_locks_taken := 0
var _ui_locks_released := 0
# Lock/unlock results carry only the lock-table checksum, not the world
# checksum; the next placement refresh may legitimately move it.
var _ui_checksum_stale := false
var _ui_main_marker := ""
var _ui_done_marker := ""
var _ui_place_markers: Array = []
var _ui_reject_markers: Array = []

# Opt-in chat (`--chat`) and the passive push listener (`--chat-listen`).
# Inert without either flag.
var _chat_enabled := false
var _chat_listen := false
var _chat_reconnect := false
var _chat_expect := ""
var _chat_script_given := false
var _chat_script := DEFAULT_CHAT_SCRIPT
var _chat_reconnect_text := DEFAULT_CHAT_RECONNECT_TEXT
var _chat_reconnect_first_seq := 0
var _chat_reconnect_token_reused := false
var _chat_actions: Array = []
var _chat_action_index := 0
var _chat_awaiting := ""
var _chat_panel = null
var _chat_marker := ""
# Pushed `chat_sent` lines observed while the transcript is not being read.
# Debug English only; this client is not localized.
var _chat_unread := 0
var _chat_pushes := 0
var _chat_unread_label: Label

func _ready() -> void:
	var args := OS.get_cmdline_user_args()
	_smoke = args.has("--smoke")
	_host = _arg(args, "--host", DEFAULT_HOST)
	_player_name = _arg(args, "--name", DEFAULT_NAME)
	_timeout_ms = _positive_int(_arg(args, "--timeout-ms", ""), STEP_TIMEOUT_MS)
	_center = Vector2(
		_float_arg(args, "--center-x", DEFAULT_CENTER.x),
		_float_arg(args, "--center-z", DEFAULT_CENTER.y)
	)
	_radius_m = _float_arg(args, "--radius-m", DEFAULT_RADIUS_M)
	var place_hash_text := _arg(args, "--place-hash", "")
	_place_hash = _canonical_place_hash(place_hash_text)
	_place_x = _float_arg(args, "--place-x", DEFAULT_PLACE_X)
	_place_z = _float_arg(args, "--place-z", DEFAULT_PLACE_Z)
	_place_rotation = _int_arg(args, "--place-rotation", DEFAULT_PLACE_ROTATION)

	_place_ui = args.has("--place-ui")
	_library_root = _arg(args, "--library", "")
	_place_ui_script = _arg(args, "--place-ui-script", "")

	_chat_listen = args.has("--chat-listen")
	_chat_reconnect = args.has("--chat-reconnect")
	_chat_expect = _arg(args, "--chat-expect", "")
	_chat_script_given = args.has("--chat-script")
	_chat_enabled = args.has("--chat") or _chat_listen or _chat_reconnect
	_chat_script = _arg(args, "--chat-script", DEFAULT_CHAT_SCRIPT)
	_chat_reconnect_text = _arg(args, "--chat-text", DEFAULT_CHAT_RECONNECT_TEXT)

	_container = Node3D.new()
	_container.name = "WorldClientRoot"
	add_child(_container)
	if _place_ui:
		_ui_root = Node3D.new()
		_ui_root.name = "WorldClientPlacementUI"
		add_child(_ui_root)
	_build_hud()

	if _chat_enabled and not _prepare_chat():
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
	if not place_hash_text.is_empty() and _place_hash.is_empty():
		_fail("invalid_place_hash")
		return
	if not _place_hash.is_empty() and (
		not is_finite(_place_x) or not is_finite(_place_z) or _place_rotation % 90 != 0
	):
		_fail("invalid_placement")
		return
	if not _is_valid_view(_center, _radius_m):
		_fail("invalid_interest")
		return
	if _place_ui and not _prepare_place_ui():
		return

	_peer = StreamPeerTCP.new()
	var error := _peer.connect_to_host(_host, _port)
	if error != OK:
		_fail("connect_start_%s" % error_string(error))
		return
	_arm_deadline(CONNECT_TIMEOUT_MS)
	_set_status("Connecting to %s:%d..." % [_host, _port])

## Validate the opt-in placement UI arguments before opening the socket. The
## script is parsed up front so a typo fails fast with a stable marker.
func _prepare_place_ui() -> bool:
	if not _place_hash.is_empty():
		_fail("place_ui_conflict")
		return false
	if _library_root.strip_edges().is_empty():
		_fail("place_ui_library_missing")
		return false
	if (
		not is_finite(_place_x)
		or not is_finite(_place_z)
		or absf(_place_x) > 1000000.0
		or absf(_place_z) > 1000000.0
	):
		_fail("place_ui_center_invalid")
		return false
	var rotation := PlacementUI.canonical_rotation(_place_rotation)
	if rotation < 0:
		_fail("place_ui_rotation_invalid")
		return false
	var parsed := _parse_place_ui_script(_place_ui_script)
	if not bool(parsed.get("ok", false)):
		_fail("place_ui_script_%s" % str(parsed.get("error", "invalid")))
		return false
	_ui_actions = parsed.get("actions", [])
	if _ui_actions.size() > UI_MAX_ACTIONS:
		_fail("place_ui_script_too_long")
		return false
	if _smoke and _ui_actions.is_empty():
		_fail("place_ui_script_empty")
		return false
	_ui_center = Vector2(roundf(_place_x), roundf(_place_z))
	_ui_rotation = rotation
	return true

## Extend the shared `PlacementUI.parse_script` grammar with `lock` and
## `unlock`. The tokens are parsed as `cancel` so the shared parser owns every
## other validation and error index, then restored by position. Empty segments
## are skipped by the shared parser, so only non-empty tokens are walked.
func _parse_place_ui_script(text: String) -> Dictionary:
	var source: Array = text.split(";", true)
	var transformed: Array = []
	var kinds: Array = []
	for raw in source:
		var token := str(raw).strip_edges()
		if token == "lock" or token == "unlock":
			transformed.append("cancel")
			kinds.append(token)
		else:
			transformed.append(raw)
			kinds.append("")
	var parsed := PlacementUI.parse_script(";".join(transformed))
	if not bool(parsed.get("ok", false)):
		return parsed
	var actions: Array = parsed.get("actions", [])
	var kind_index := 0
	for token_index in range(source.size()):
		if str(source[token_index]).strip_edges().is_empty():
			continue
		if kind_index >= actions.size():
			return {"ok": false, "error": "action_%d_invalid" % token_index, "actions": []}
		if not str(kinds[token_index]).is_empty():
			actions[kind_index] = {"kind": str(kinds[token_index])}
		kind_index += 1
	return {"ok": true, "error": "", "actions": actions}

## Validate the opt-in chat arguments and build the transcript model before the
## socket opens. A bad script fails fast with `chat_script_invalid`; the chat
## probe cannot be combined with either placement flow because it skips the
## zero-view step those flows depend on. Listener mode requires an expected
## text and rejects the scripted/placement combinations it cannot run.
func _prepare_chat() -> bool:
	if _chat_reconnect and _chat_listen:
		_fail("chat_mode_conflict")
		return false
	if _chat_reconnect:
		if _place_ui or not _place_hash.is_empty():
			_fail("chat_conflict")
			return false
		if _chat_script_given:
			_fail("chat_reconnect_script")
			return false
		_chat_panel = ChatScene.instantiate()
		var reconnect_result: Dictionary = _chat_panel.draft(_chat_reconnect_text)
		if not bool(reconnect_result.get("ok", false)):
			_fail("chat_reconnect_text_invalid")
			return false
		_chat_reconnect_text = str(
			reconnect_result.get("command", {}).get("text", "")
		)
		_chat_panel.command_issued.connect(_on_chat_command)
		if not _smoke and DisplayServer.get_name() != "headless":
			_mount_chat_panel()
		return true
	if _chat_listen:
		if _place_ui or not _place_hash.is_empty() or _chat_script_given:
			_fail("chat_listen_conflict")
			return false
		if _chat_expect.strip_edges().is_empty():
			_fail("chat_expect_missing")
			return false
		_chat_panel = ChatScene.instantiate()
		var expect_result: Dictionary = _chat_panel.draft(_chat_expect)
		if not bool(expect_result.get("ok", false)):
			_fail("chat_expect_invalid")
			return false
		_chat_expect = str(expect_result.get("command", {}).get("text", ""))
		_chat_panel.command_issued.connect(_on_chat_command)
		return true
	_chat_panel = ChatScene.instantiate()
	var parsed := _parse_chat_script(_chat_script)
	if not bool(parsed.get("ok", false)):
		_fail("chat_script_invalid")
		return false
	_chat_actions = parsed.get("actions", [])
	if _place_ui or not _place_hash.is_empty():
		_fail("chat_conflict")
		return false
	_chat_panel.command_issued.connect(_on_chat_command)
	if not _smoke and DisplayServer.get_name() != "headless":
		_mount_chat_panel()
	return true

## Parse `say:<text>|poll|...` into ordered actions. Each `say` text is checked
## with the panel's `draft`, which mirrors the server's trim/length/control
## rules, so an invalid script never reaches the wire.
func _parse_chat_script(text: String) -> Dictionary:
	var actions: Array = []
	if text.strip_edges().is_empty():
		return {"ok": false, "actions": actions}
	for raw in text.split("|", true):
		var action := raw.strip_edges()
		if action == "poll":
			actions.append({"type": "poll"})
			continue
		if not action.begins_with("say:"):
			return {"ok": false, "actions": actions}
		var message := action.substr(4).strip_edges()
		if message.is_empty():
			return {"ok": false, "actions": actions}
		var draft: Dictionary = _chat_panel.draft(message)
		if not bool(draft.get("ok", false)):
			return {"ok": false, "actions": actions}
		var command: Dictionary = draft.get("command", {})
		actions.append({"type": "say", "text": str(command.get("text", ""))})
	if actions.is_empty() or actions.size() > CHAT_MAX_ACTIONS:
		return {"ok": false, "actions": actions}
	return {"ok": true, "actions": actions}

## Mount the developer panel bottom-left. `chat_panel.gd` runs its own headless
## smoke when `_ready` sees `--smoke`, so the smoke path keeps the panel as a
## tree-less model and only a real display gets a mounted Control.
func _mount_chat_panel() -> void:
	var layer := CanvasLayer.new()
	layer.name = "WorldClientChat"
	layer.layer = CHAT_LAYER
	add_child(layer)
	_chat_panel.name = "WorldClientChatPanel"
	layer.add_child(_chat_panel)
	_chat_panel.anchor_left = 0.0
	_chat_panel.anchor_top = 1.0
	_chat_panel.anchor_right = 0.0
	_chat_panel.anchor_bottom = 1.0
	_chat_panel.offset_left = CHAT_MARGIN
	_chat_panel.offset_top = -CHAT_PANEL_HEIGHT - CHAT_MARGIN
	_chat_panel.offset_right = CHAT_MARGIN + CHAT_PANEL_WIDTH
	_chat_panel.offset_bottom = -CHAT_MARGIN

func _process(_delta: float) -> void:
	if _finished:
		return
	if Time.get_ticks_msec() > _deadline_ms:
		# Interactive placement mode has no scripted clock; keep the session
		# alive while the player moves the ghost. Scripted/network waits still
		# time out normally.
		if _place_ui and _phase == Phase.UI_ACTIVE and _place_ui_script.is_empty():
			_arm_deadline(_timeout_ms)
		elif _chat_listen and _phase == Phase.CHAT_LISTEN:
			_fail("chat_push_timeout")
			return
		else:
			_fail("timeout_%s" % _phase_name())
			return
	if _peer == null:
		_fail("peer_missing")
		return

	_peer.poll()
	if _phase == Phase.CONNECTING or _phase == Phase.CHAT_RECONNECT_CONNECTING:
		var first := _phase == Phase.CONNECTING
		match _peer.get_status():
			StreamPeerTCP.STATUS_CONNECTED:
				_on_connected()
			StreamPeerTCP.STATUS_ERROR:
				_fail("connect_failed" if first else "chat_reconnect_failed")
			StreamPeerTCP.STATUS_NONE:
				_fail("connect_lost" if first else "chat_reconnect_lost")
		return

	_drain()
	if _finished or _phase == Phase.CHAT_RECONNECT_CONNECTING:
		# The first-session goodbye already replaced the peer; the next poll
		# connects the resume socket.
		return
	if _peer.get_status() != StreamPeerTCP.STATUS_CONNECTED:
		# Read the goodbye or final reject before treating FIN as a failure.
		_drain()
		if not _finished:
			if _chat_listen and _phase == Phase.CHAT_LISTEN:
				_fail("chat_push_timeout")
			else:
				_fail("connection_lost_%s" % _phase_name())

func _on_connected() -> void:
	if _phase == Phase.CHAT_RECONNECT_CONNECTING:
		_set_status("Reconnected; resuming for tick %d." % _last_tick)
		if not _send({
			"type": "resume",
			"session_id": _session_token,
			"last_tick": _last_tick,
		}):
			return
		_phase = Phase.CHAT_RESUME_WELCOME_WAIT
		_arm_deadline(_timeout_ms)
		return
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
		"snapshot":
			_on_snapshot(message)
		"interest_view":
			_on_interest_view(message)
		"place_blueprint_result":
			_on_place_blueprint_result(message)
		"blueprint_lock_list":
			_on_blueprint_lock_list(message)
		"blueprint_lock_result":
			_on_blueprint_lock_result(message)
		"blueprint_unlock_result":
			_on_blueprint_unlock_result(message)
		"chat_sent":
			_on_chat_sent(message)
		"chat_log":
			_on_chat_log(message)
		"reject":
			_on_reject(message)
		"goodbye":
			_on_goodbye()
		_:
			_fail("unexpected_message_%s" % (tag if tag != "" else "missing_type"))

func _on_welcome(message: Dictionary) -> void:
	if _phase == Phase.CHAT_RESUME_WELCOME_WAIT:
		_on_chat_resume_welcome(message)
		return
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
	_session_token = str(message.get("session_token", ""))
	if bool(message.get("resumed", false)):
		_fail("unexpected_resumed_welcome")
		return
	var tick := int(message.get("tick", -1))
	if tick < 0:
		_fail("welcome_tick")
		return
	_last_tick = tick
	if _chat_enabled:
		_chat_panel.set_identity(0, _player_name)
	if _chat_listen:
		print("MP_CHAT_LISTEN status=ready player=%s" % _player_name)
		_phase = Phase.CHAT_LISTEN
		_arm_deadline(_timeout_ms)
		_set_status("Chat push listener ready; waiting for a pushed line.")
		return
	if _place_ui and not _ui_request_locks():
		return
	_set_status("Welcome %s; placing deterministic roads." % _session_id)
	_refresh_hud()
	_send_next_road()

## Send the next road in the deterministic plan, or the main set_interest once
## every road has been acknowledged.
func _send_next_road() -> void:
	if _road_index >= ROAD_PLAN.size():
		_set_status("Roads placed; requesting the %.0f m interest view." % _radius_m)
		if not _send({
			"type": "set_interest",
			"center_x": _center.x,
			"center_z": _center.y,
			"radius_m": _radius_m,
		}):
			return
		_phase = Phase.INTEREST_SENT
		_arm_deadline(_timeout_ms)
		return
	var plan: Dictionary = ROAD_PLAN[_road_index]
	var start: Array = plan.get("start", [0, 0])
	var end: Array = plan.get("end", [0, 0])
	if not _send({
		"type": "place_road",
		"start": [int(start[0]), int(start[1])],
		"end": [int(end[0]), int(end[1])],
	}):
		return
	_phase = Phase.ROAD_SENT
	_arm_deadline(_timeout_ms)

func _on_snapshot(message: Dictionary) -> void:
	var tick := int(message.get("tick", -1))
	var population := int(message.get("population", -1))
	var balance := int(message.get("balance", 0))
	var checksum := str(message.get("checksum", ""))
	if tick < 0 or population < 1 or not _is_hex16(checksum):
		_fail("snapshot_fields")
		return

	if _phase == Phase.CHAT_RESUME_SNAPSHOT_WAIT:
		if tick < _last_tick:
			_fail("chat_resume_snapshot_tick_%d" % tick)
			return
		_store_snapshot(tick, population, balance, checksum)
		# `after: 0` is deliberate: the chat seq is fresh and starts at 1, so
		# asking for everything avoids guessing whether the pre-resume line is
		# above or below a resume checkpoint. The handler asserts the returned
		# set holds exactly the one recorded pre-resume entry.
		if not _send({"type": "chat_since", "after": 0}):
			return
		_chat_awaiting = "log"
		_phase = Phase.CHAT_RECONNECT_CATCHUP_WAIT
		_set_status("Resumed at tick %d; catching up chat." % tick)
		_arm_deadline(_timeout_ms)
		return

	if _phase == Phase.ROAD_SENT:
		if tick != _last_tick + 1:
			_fail("road_tick_%d_after_%d" % [tick, _last_tick])
			return
		_store_snapshot(tick, population, balance, checksum)
		_road_index += 1
		_send_next_road()
		return

	if _phase == Phase.INTEREST_SENT:
		if not _check_view_snapshot(message, tick, checksum, _center, _radius_m):
			return
		_visible_roads = int(message.get("visible_roads", -1))
		if _visible_roads != ROAD_PLAN.size():
			_fail("interest_visible_roads_%d" % _visible_roads)
			return
		_store_snapshot(tick, population, balance, checksum)
		_set_status("Interest snapshot received; awaiting the detail view.")
		_refresh_hud()
		_phase = Phase.INTEREST_VIEW_WAIT
		_arm_deadline(_timeout_ms)
		return

	if _phase == Phase.ZERO_INTEREST_SENT:
		if not _check_view_snapshot(message, tick, checksum, ZERO_CENTER, ZERO_RADIUS_M):
			return
		_visible_roads = int(message.get("visible_roads", -1))
		if _visible_roads != 0:
			_fail("zero_visible_roads_%d" % _visible_roads)
			return
		_store_snapshot(tick, population, balance, checksum)
		_set_status("Zero-viewport snapshot received; awaiting the detail view.")
		_refresh_hud()
		_phase = Phase.ZERO_VIEW_WAIT
		_arm_deadline(_timeout_ms)
		return

	if _phase == Phase.UI_SNAPSHOT_SENT:
		if not _check_view_snapshot(message, tick, checksum, _ui_center, _radius_m):
			return
		_visible_roads = int(message.get("visible_roads", -1))
		if _visible_roads <= 0:
			_fail("ui_visible_roads_%d" % _visible_roads)
			return
		_store_snapshot(tick, population, balance, checksum)
		_set_status("Placement refresh snapshot received; awaiting the detail view.")
		_ui_checksum_stale = false
		_refresh_hud()
		_phase = Phase.UI_VIEW_WAIT
		_arm_deadline(_timeout_ms)
		return

	_fail("snapshot_in_%s" % _phase_name())

## Shared checks for a set_interest snapshot: the viewport is view-only, so the
## tick and checksum must not move and the echoed centre/radius must match.
func _check_view_snapshot(
	message: Dictionary, tick: int, checksum: String, center: Vector2, radius_m: float
) -> bool:
	if tick != _last_tick:
		_fail("interest_tick_%d_after_%d" % [tick, _last_tick])
		return false
	if checksum != _last_checksum:
		# Lock/unlock results carry only the lock-table checksum, so the world
		# checksum is unknown until the next view refresh.
		if not (_place_ui and _ui_checksum_stale and _phase == Phase.UI_SNAPSHOT_SENT):
			_fail("interest_checksum_changed")
			return false
	if not _is_pair_close(message.get("interest_center", null), center, 0.001):
		_fail("interest_center")
		return false
	var echoed_radius := float(message.get("interest_radius_m", -1.0))
	if absf(echoed_radius - radius_m) > 0.001:
		_fail("interest_radius_%.3f" % echoed_radius)
		return false
	return true

func _on_interest_view(message: Dictionary) -> void:
	if (
		_phase != Phase.INTEREST_VIEW_WAIT
		and _phase != Phase.ZERO_VIEW_WAIT
		and _phase != Phase.UI_VIEW_WAIT
	):
		_fail("interest_view_in_%s" % _phase_name())
		return
	var tick := int(message.get("tick", -1))
	var center_raw: Variant = message.get("center", null)
	var radius_m := float(message.get("radius_m", -1.0))
	if tick != _last_tick:
		_fail("interest_view_tick_%d" % tick)
		return
	if typeof(center_raw) != TYPE_ARRAY or center_raw.size() != 2:
		_fail("interest_view_center")
		return
	var center := Vector2(float(center_raw[0]), float(center_raw[1]))
	var is_zero := _phase == Phase.ZERO_VIEW_WAIT
	var expected_center := (
		ZERO_CENTER
		if is_zero
		else (_ui_center if _phase == Phase.UI_VIEW_WAIT else _center)
	)
	var expected_radius := ZERO_RADIUS_M if is_zero else _radius_m
	if not center.is_equal_approx(expected_center):
		_fail("interest_view_center_value")
		return
	if absf(radius_m - expected_radius) > 0.001:
		_fail("interest_view_radius_%.3f" % radius_m)
		return

	var roads_raw: Variant = message.get("roads", null)
	if typeof(roads_raw) != TYPE_ARRAY:
		_fail("interest_view_roads")
		return
	var roads: Array = InterestRender.parse_roads(roads_raw)
	if roads.size() != roads_raw.size():
		_fail("interest_view_road_parse_%d" % roads.size())
		return
	var truncated := bool(message.get("truncated", false))

	var buildings_raw: Variant = message.get("buildings", null)
	if buildings_raw != null and typeof(buildings_raw) != TYPE_ARRAY:
		_fail("interest_view_buildings")
		return
	var building_info := _parse_buildings(buildings_raw)
	var buildings: Array = building_info.get("buildings", [])
	var buildings_truncated := bool(
		message.get("buildings_truncated", false)
	) or bool(building_info.get("truncated", false))

	if not _build_scene(tick, center, radius_m, roads, truncated, buildings, buildings_truncated):
		return

	if _phase == Phase.UI_VIEW_WAIT:
		if not _assert_ui_view():
			return
		_set_status("Placement refresh rendered; placement mode active.")
		_phase = Phase.UI_ACTIVE
		_ui_next_action()
		return

	if _phase == Phase.INTEREST_VIEW_WAIT:
		if not _assert_main_view():
			return
		_main_marker = _marker_text("MP_WORLD_CLIENT", false)
		print(_main_marker)
		if _place_ui:
			_ui_begin()
			return
		if _chat_enabled:
			if _chat_reconnect:
				_chat_reconnect_begin()
			else:
				_chat_begin()
			return
		if _smoke:
			_set_status("Main view rendered; probing the zero-viewport case.")
			if not _send({
				"type": "set_interest",
				"center_x": ZERO_CENTER.x,
				"center_z": ZERO_CENTER.y,
				"radius_m": ZERO_RADIUS_M,
			}):
				return
			_phase = Phase.ZERO_INTEREST_SENT
			_arm_deadline(_timeout_ms)
			return
		_set_status("View rendered; disconnecting.")
		if not _send({"type": "disconnect"}):
			return
		_phase = Phase.DISCONNECT_SENT
		_arm_deadline(_timeout_ms)
		return

	if not _assert_zero_view():
		return
	_zero_marker = _marker_text("MP_WORLD_CLIENT_ZERO", true)
	print(_zero_marker)
	if not _place_hash.is_empty():
		_set_status("Zero viewport rendered; placing the authoritative blueprint.")
		if not _send({
			"type": "place_blueprint",
			"blueprint_hash": _place_hash,
			"x": _place_x,
			"z": _place_z,
			"rotation": _place_rotation,
		}):
			return
		_phase = Phase.PLACE_SENT
		_arm_deadline(_timeout_ms)
		return
	_set_status("Zero viewport rendered; disconnecting.")
	if not _send({"type": "disconnect"}):
		return
	_phase = Phase.DISCONNECT_SENT
	_arm_deadline(_timeout_ms)

## Accept the authoritative D-032 result, render its box, and close the session.
## Rejects are handled by `_on_reject` with the server code in the failure
## marker. This is the only place the client consumes `place_blueprint`.
func _on_place_blueprint_result(message: Dictionary) -> void:
	if _place_ui:
		_on_ui_place_result(message)
		return
	if _phase != Phase.PLACE_SENT:
		_fail("place_result_in_%s" % _phase_name())
		return
	var tick := int(message.get("tick", -1))
	if tick != _last_tick + 1:
		_fail("place_tick_%d_after_%d" % [tick, _last_tick])
		return
	var placement_id := int(message.get("placement_id", 0))
	if placement_id < 1:
		_fail("place_id_%d" % placement_id)
		return
	if str(message.get("blueprint_hash", "")) != _place_hash:
		_fail("place_hash_mismatch")
		return
	var placement_checksum := str(message.get("placement_checksum", ""))
	var world_checksum := str(message.get("world_checksum", ""))
	if not _is_hex16(placement_checksum) or not _is_hex16(world_checksum):
		_fail("place_checksums")
		return
	var view_raw: Variant = message.get("placement", null)
	if typeof(view_raw) != TYPE_DICTIONARY:
		_fail("place_view")
		return
	var view: Dictionary = view_raw
	if int(view.get("id", 0)) != placement_id:
		_fail("place_view_id")
		return
	if str(view.get("blueprint_hash", "")) != _place_hash:
		_fail("place_view_hash")
		return
	var center := _finite_array(view.get("center", null), 3)
	var footprint := _finite_array(view.get("footprint_m", null), 2)
	var bounds := _finite_array(view.get("bounds_m", null), 3)
	if center.is_empty() or footprint.is_empty() or bounds.is_empty():
		_fail("place_view_geometry")
		return
	if float(footprint[0]) <= 0.0 or float(footprint[1]) <= 0.0 or float(bounds[1]) <= 0.0:
		_fail("place_view_geometry_size")
		return

	_placement_id = placement_id
	_placement_hash = _place_hash
	_placement_checksum = placement_checksum
	_placement_world_checksum = world_checksum
	_placement_center = Vector2(float(center[0]), float(center[2]))
	_placement_footprint = Vector2(float(footprint[0]), float(footprint[1]))
	_placement_bounds = Vector3(float(bounds[0]), float(bounds[1]), float(bounds[2]))
	_has_placement = true
	# The result carries the new authoritative tick and world checksum, but not
	# a population/balance snapshot; keep the last zero-view values for the HUD.
	_store_snapshot(tick, _last_population, _last_balance, world_checksum)
	_refresh_hud()
	if not _render_placement():
		return
	if not _assert_placement():
		return
	_placement_marker = _placement_marker_text()
	print(_placement_marker)
	_set_status("Authoritative placement rendered; disconnecting.")
	if not _send({"type": "disconnect"}):
		return
	_phase = Phase.DISCONNECT_SENT
	_arm_deadline(_timeout_ms)

func _on_reject(message: Dictionary) -> void:
	if _place_ui and (
		_phase == Phase.UI_PLACE_WAIT
		or _phase == Phase.UI_LOCK_WAIT
		or _phase == Phase.UI_UNLOCK_WAIT
	):
		_on_ui_reject(message)
		return
	_fail("reject_%s_code=%s" % [_phase_name(), str(message.get("code", "missing"))])

func _on_goodbye() -> void:
	if _phase == Phase.CHAT_RECONNECT_WAIT_DISCONNECT:
		_start_chat_resume()
		return
	if _phase != Phase.DISCONNECT_SENT:
		_fail("goodbye_in_%s" % _phase_name())
		return
	if _chat_enabled and not _chat_listen:
		# The scripted active flow reads every line it consumes, so the owner
		# and the panel badge must both still be at 0 when it disconnects.
		var active_snapshot: Dictionary = _chat_panel.snapshot()
		if (
			_chat_unread != 0
			or int(active_snapshot.get("unread", -1)) != 0
			or str(_chat_panel.badge_text()) != ""
			or bool(_chat_panel.badge_visible())
		):
			_fail("chat_unread_in_active")
			return
		if _chat_reconnect:
			if (
				int(active_snapshot.get("line_count", 0)) != 1
				or int(active_snapshot.get("last_seq", 0))
				!= _chat_reconnect_first_seq
				or not _chat_reconnect_token_reused
			):
				_fail("chat_reconnect_incomplete")
				return
			_chat_marker = _chat_reconnect_marker_text()
		else:
			_chat_marker = _chat_marker_text()
		print(_chat_marker)
	_finish_ok()

# --- Opt-in chat (`--chat`) -------------------------------------------------

## Update the unread badge. Only shown while there is at least one line, so a
## quiet session keeps the HUD clean. `total` counts pushed lines this session
## ever, not the current badge value.
func _chat_unread_text() -> String:
	return "MP_CHAT_UNREAD unread=%d total=%d" % [_chat_unread, _chat_pushes]

func _set_chat_unread(value: int) -> void:
	var clamped := clampi(value, 0, CHAT_UNREAD_MAX)
	# Push every owner decision into the panel (push, reset, send, poll),
	# including 0; the panel never clears itself and owns only display state.
	if _chat_panel != null:
		_chat_panel.set_unread(clamped)
	if clamped == _chat_unread:
		return
	_chat_unread = clamped
	_refresh_chat_unread_hud()
	# Change-only marker: the sender's own response never moves the counter, so
	# it prints nothing; a passive listener prints 1 and then 0 on catch-up.
	if _chat_enabled:
		print(_chat_unread_text())
		_assert_chat_badge()

func _refresh_chat_unread_hud() -> void:
	if _chat_unread_label == null:
		return
	_chat_unread_label.text = "Chat: %d new" % _chat_unread
	_chat_unread_label.visible = _chat_unread > 0

## Prove the panel mirror kept up with the owner and print one deterministic
## line per transition. The standalone panel smoke owns the 0/N/99+ rendering
## rules; this only checks the pushed count, badge text, and visibility.
func _assert_chat_badge() -> void:
	if _chat_panel == null:
		return
	var panel_snapshot: Dictionary = _chat_panel.snapshot()
	var panel_unread := int(panel_snapshot.get("unread", -1))
	var text := str(_chat_panel.badge_text())
	var visible := bool(_chat_panel.badge_visible())
	var expected := ""
	if _chat_unread > 0:
		expected = "99+" if _chat_unread > 99 else str(_chat_unread)
	var ok := (
		panel_unread == _chat_unread
		and text == expected
		and visible == (_chat_unread > 0)
	)
	print("MP_CHAT_BADGE status=%s unread=%d panel=%d text=%s visible=%s" % [
		"ok" if ok else "fail",
		_chat_unread,
		panel_unread,
		text if text != "" else "-",
		"true" if visible else "false",
	])
	if not ok:
		_fail("chat_badge_mismatch")

## True when the mounted panel (or a child) owns keyboard focus, i.e. the player
## is reading/typing and a pushed line should not be counted. The smoke model is
## tree-less, so it is never focused.
func _chat_panel_focused() -> bool:
	if _chat_panel == null or not _chat_panel.is_inside_tree():
		return false
	var focused: Control = _chat_panel.get_viewport().gui_get_focus_owner()
	if focused == null:
		return false
	return _chat_panel == focused or _chat_panel.is_ancestor_of(focused)

## A pushed line counts only for a live chat session whose transcript is not on
## screen. `added` already deduplicates against `chat_log`/`chat_since`, and the
## sender's own scripted response is handled before this is reached.
func _chat_should_count_push(message: Dictionary) -> bool:
	if not _chat_enabled:
		return false
	if _phase == Phase.CHAT_ACTIVE and _chat_panel_focused():
		return false
	var entry_raw: Variant = message.get("entry", null)
	if typeof(entry_raw) == TYPE_DICTIONARY:
		# The server never pushes a line back to its sender, but an interactive
		# send can race the scripted exchange; never count our own name.
		if str(entry_raw.get("player", "")) == _player_name:
			return false
	return true

## Start the transcript exchange: ask for every retained line, then run the
## scripted actions. Chat never advances the authoritative tick or checksum.
func _chat_begin() -> void:
	if not _send({"type": "chat_since", "after": 0}):
		return
	_chat_awaiting = "log"
	_phase = Phase.CHAT_ACTIVE
	_set_chat_unread(0)
	_arm_deadline(_timeout_ms)
	_set_status("Chat connected; polling the transcript.")

## First connection of `--chat-reconnect`: send the one line that must survive
## the disconnect, then wait for its authoritative `chat_sent`.
func _chat_reconnect_begin() -> void:
	if not _send({"type": "chat", "text": _chat_reconnect_text}):
		return
	_chat_awaiting = "sent"
	_phase = Phase.CHAT_RECONNECT_WAIT_SENT
	_set_chat_unread(0)
	_arm_deadline(_timeout_ms)
	_set_status("Chat reconnect: sending the pre-resume line.")

## Open the resume socket after the first session closed cleanly. The local
## transcript is dropped on purpose: a resumed client cannot assume it still
## has the lines it missed, so `chat_since` must refill them from the server.
func _start_chat_resume() -> void:
	if (
		_session_token.is_empty()
		or _last_tick < 0
		or _chat_reconnect_first_seq < 1
	):
		_fail("chat_reconnect_state")
		return
	_chat_panel.set_log([])
	_set_chat_unread(0)
	if _peer != null:
		_peer.disconnect_from_host()
	_peer = StreamPeerTCP.new()
	_rx = PackedByteArray()
	var error := _peer.connect_to_host(_host, _port)
	if error != OK:
		_fail("chat_reconnect_start_%s" % error_string(error))
		return
	_phase = Phase.CHAT_RECONNECT_CONNECTING
	_arm_deadline(CONNECT_TIMEOUT_MS)
	_set_status("Reconnecting to resume session %s." % _session_id)

## The resumed welcome must carry the same session and token, so the marker's
## `token_reused=true` is checked against the token recorded before disconnect.
func _on_chat_resume_welcome(message: Dictionary) -> void:
	if not bool(message.get("resumed", false)):
		_fail("chat_resume_not_resumed")
		return
	var protocol_version := int(message.get("protocol_version", -1))
	if protocol_version != PROTOCOL_VERSION:
		_fail("chat_resume_protocol_version_%d" % protocol_version)
		return
	if str(message.get("session_id", "")) != _session_id:
		_fail("chat_resume_session_id")
		return
	if str(message.get("session_token", "")) != _session_token:
		_fail("chat_resume_session_token")
		return
	var tick := int(message.get("tick", -1))
	if tick < _last_tick:
		_fail("chat_resume_welcome_tick_%d" % tick)
		return
	_chat_reconnect_token_reused = true
	_phase = Phase.CHAT_RESUME_SNAPSHOT_WAIT
	_set_status("Session resumed at tick %d; awaiting the snapshot." % tick)
	_arm_deadline(_timeout_ms)

## The resumed session asks for the whole retained log (`after: 0`). On a fresh
## server the log holds exactly the one pre-resume line, so both the count and
## the seq are asserted here; a missing or repeated line fails before the marker.
func _on_chat_reconnect_log(message: Dictionary) -> void:
	if _chat_awaiting != "log":
		_fail("chat_log_in_%s" % _phase_name())
		return
	var entries_raw: Variant = message.get("entries", null)
	if typeof(entries_raw) != TYPE_ARRAY:
		_fail("chat_log_entries")
		return
	var entries: Array = entries_raw
	if entries.size() != 1:
		_fail("chat_reconnect_entries_%d" % entries.size())
		return
	var entry_raw: Variant = entries[0]
	if typeof(entry_raw) != TYPE_DICTIONARY:
		_fail("chat_reconnect_entry")
		return
	var entry: Dictionary = entry_raw
	if int(entry.get("seq", 0)) != _chat_reconnect_first_seq:
		_fail("chat_reconnect_seq_%d" % int(entry.get("seq", 0)))
		return
	if (
		str(entry.get("player", "")) != _player_name
		or str(entry.get("text", "")) != _chat_reconnect_text
	):
		_fail("chat_reconnect_entry_fields")
		return
	_chat_panel.apply_chat_log(message)
	if str(_chat_panel.snapshot().get("error_code", "")) != "":
		_fail("chat_reconnect_log_invalid")
		return
	_chat_awaiting = ""
	_set_chat_unread(0)
	_refresh_hud()
	_set_status("Chat catch-up merged; disconnecting.")
	if not _send({"type": "disconnect"}):
		return
	_phase = Phase.DISCONNECT_SENT
	_arm_deadline(_timeout_ms)

## Deterministic 16-hex digest over the caught-up transcript, same line format
## as `MP_CHAT`. `token_reused` is only true after the resumed welcome matched
## both the recorded session id and token.
func _chat_reconnect_marker_text() -> String:
	var snapshot: Dictionary = _chat_panel.snapshot()
	var lines: Array = snapshot.get("lines", [])
	var digest := "\n".join(lines).sha256_text().substr(0, 16)
	return "MP_CHAT_RECONNECT status=ok entries=%d last_seq=%d token_reused=%s checksum=%s" % [
		int(snapshot.get("line_count", 0)),
		int(snapshot.get("last_seq", 0)),
		"true" if _chat_reconnect_token_reused else "false",
		digest,
	]

## Run the next scripted action, or disconnect once the script is exhausted.
func _chat_advance() -> void:
	if _chat_action_index >= _chat_actions.size():
		_chat_awaiting = ""
		_set_status("Chat exchange complete; disconnecting.")
		if not _send({"type": "disconnect"}):
			return
		_phase = Phase.DISCONNECT_SENT
		_arm_deadline(_timeout_ms)
		return
	var action: Dictionary = _chat_actions[_chat_action_index]
	_chat_action_index += 1
	if str(action.get("type", "")) == "say":
		var text := str(action.get("text", ""))
		if not _send({"type": "chat", "text": text}):
			return
		_chat_awaiting = "sent"
		_set_chat_unread(0)
		_set_status("Chat: sent \"%s\"." % text)
	else:
		if not _send(_chat_panel.poll_command()):
			return
		_chat_awaiting = "log"
		_set_status("Chat: polling after #%d." % int(_chat_panel.last_seq))
	_arm_deadline(_timeout_ms)

func _on_chat_sent(message: Dictionary) -> void:
	if not _chat_enabled:
		_fail("unexpected_message_chat_sent")
		return
	if _chat_reconnect and _phase == Phase.CHAT_RECONNECT_WAIT_SENT:
		if _chat_awaiting != "sent":
			_fail("chat_sent_in_%s" % _phase_name())
			return
		if not _chat_panel.apply_chat_sent(message):
			_fail("chat_reconnect_sent")
			return
		var first_entry: Dictionary = message.get("entry", {})
		var first_seq := int(first_entry.get("seq", 0))
		if first_seq < 1:
			_fail("chat_reconnect_sent_seq_%d" % first_seq)
			return
		_chat_reconnect_first_seq = first_seq
		_chat_awaiting = ""
		_refresh_hud()
		_set_status("Pre-resume line #%d stored; disconnecting." % first_seq)
		if not _send({"type": "disconnect"}):
			return
		_phase = Phase.CHAT_RECONNECT_WAIT_DISCONNECT
		_arm_deadline(_timeout_ms)
		return
	var scripted := _phase == Phase.CHAT_ACTIVE and _chat_awaiting == "sent"
	var added: bool = _chat_panel.apply_chat_sent(message)
	if scripted:
		if not added:
			var error_code := str(_chat_panel.snapshot().get("error_code", ""))
			_fail("chat_sent_%s" % (error_code if error_code != "" else "duplicate"))
			return
		_chat_awaiting = ""
		_refresh_hud()
		_chat_advance()
		return
	# A line pushed from another session (or a duplicate) outside the scripted
	# exchange is transcript-only: never advance the script or fail on it.
	# `chat_log` remains the catch-up path after a reconnect.
	if not added:
		return
	if _chat_should_count_push(message):
		_chat_pushes += 1
		_set_chat_unread(_chat_unread + 1)
	_refresh_hud()
	if _phase == Phase.CHAT_LISTEN:
		_chat_push_match(message)

## Complete the passive listener once its expected pushed line arrives. The
## digest is SHA-256 over `seq|player|text`, first 16 hex, with no paths or
## clock values.
func _chat_push_match(message: Dictionary) -> void:
	var entry_raw: Variant = message.get("entry", null)
	if typeof(entry_raw) != TYPE_DICTIONARY:
		return
	var entry: Dictionary = entry_raw
	var seq := int(entry.get("seq", 0))
	var player := str(entry.get("player", ""))
	var text := str(entry.get("text", ""))
	if text != _chat_expect:
		return
	if seq < 1 or player.is_empty():
		_fail("chat_push_fields")
		return
	var digest := ("%d|%s|%s" % [seq, player, text]).sha256_text().substr(0, 16)
	print("MP_CHAT_PUSH status=ok player=%s seq=%d text=%s checksum=%s" % [
		player,
		seq,
		text,
		digest,
	])
	# Catch up through the normal poll path so the unread badge clears the same
	# way it would for a real client, and print the reset marker.
	if not _send(_chat_panel.poll_command()):
		return
	_chat_awaiting = "log"
	_arm_deadline(_timeout_ms)
	_set_status("Pushed line received; polling to clear unread.")

func _on_chat_log(message: Dictionary) -> void:
	if not _chat_enabled:
		_fail("unexpected_message_chat_log")
		return
	if _chat_reconnect and _phase == Phase.CHAT_RECONNECT_CATCHUP_WAIT:
		_on_chat_reconnect_log(message)
		return
	var listening := _chat_listen and _phase == Phase.CHAT_LISTEN and _chat_awaiting == "log"
	if not listening and (_phase != Phase.CHAT_ACTIVE or _chat_awaiting != "log"):
		_fail("chat_log_in_%s" % _phase_name())
		return
	if typeof(message.get("entries", null)) != TYPE_ARRAY:
		_fail("chat_log_entries")
		return
	_chat_panel.apply_chat_log(message)
	if str(_chat_panel.snapshot().get("error_code", "")) != "":
		_fail("chat_log_invalid")
		return
	_chat_awaiting = ""
	# A successful poll means the transcript is caught up: nothing is unread.
	_set_chat_unread(0)
	_refresh_hud()
	if listening:
		_set_status("Pushed line caught up; disconnecting.")
		if not _send({"type": "disconnect"}):
			return
		_phase = Phase.DISCONNECT_SENT
		_arm_deadline(_timeout_ms)
		return
	_chat_advance()

## Forward a panel-issued command while the chat exchange is live. Script and
## interactive sends share the same socket path.
func _on_chat_command(command: Dictionary) -> void:
	if not _chat_enabled or _phase != Phase.CHAT_ACTIVE:
		return
	if _send(command) and str(command.get("type", "")) == "chat":
		_set_chat_unread(0)

## Deterministic 16-hex digest over the panel's ascending transcript lines
## (`#seq player: text`), which encode exactly seq/player/text.
func _chat_marker_text() -> String:
	var snapshot: Dictionary = _chat_panel.snapshot()
	var lines: Array = snapshot.get("lines", [])
	var digest := "\n".join(lines).sha256_text().substr(0, 16)
	return "MP_CHAT status=ok entries=%d checksum=%s" % [
		int(snapshot.get("line_count", 0)),
		digest,
	]

## Release the tree-less smoke model; a mounted panel is owned by the scene.
func _free_chat_model() -> void:
	if _chat_panel != null and not _chat_panel.is_inside_tree():
		_chat_panel.free()
		_chat_panel = null

## Load the local catalog through the bridge and enter placement mode.
func _ui_begin() -> void:
	if not ClassDB.class_exists("TownsmithCore"):
		_fail("place_ui_bridge_missing")
		return
	_ui_core = ClassDB.instantiate("TownsmithCore")
	var catalog := PlacementUI.load_catalog(_ui_core, _library_root)
	if not bool(catalog.get("ok", false)):
		_fail("place_ui_catalog_%s" % str(catalog.get("error", "failed")))
		return
	_ui_entries = catalog.get("entries", [])
	if _ui_entries.is_empty():
		_fail("place_ui_empty")
		return
	_ui_catalog_checksum = str(catalog.get("checksum", ""))
	_ui_selected = 0
	_ui_rotation = PlacementUI.canonical_rotation(_place_rotation)
	if _ui_rotation < 0:
		_ui_rotation = 0
	_ui_render_ghost()
	var entry: Dictionary = _ui_entries[_ui_selected]
	var bounds: Array = entry.get("bounds_m", [0.0, 0.0, 0.0])
	var footprint: Array = entry.get("footprint_m", [0.0, 0.0])
	print((
		"MP_PLACE_UI status=ok mode=ui entries=%d selected=%d id=%s hash=%s"
		+ " bounds=%.1f,%.1f,%.1f footprint=%.1f,%.1f library_checksum=%s"
	) % [
			_ui_entries.size(),
			_ui_selected,
			str(entry.get("id", "")),
			str(entry.get("hash", "")),
			float(bounds[0]),
			float(bounds[1]),
			float(bounds[2]),
			float(footprint[0]),
			float(footprint[1]),
			_ui_catalog_checksum,
		])
	_phase = Phase.UI_ACTIVE
	_arm_deadline(_timeout_ms)
	_ui_next_action()

## Run the next scripted action, or finish. Interactive mode stays active.
func _ui_next_action() -> void:
	if _phase != Phase.UI_ACTIVE:
		return
	if _ui_action_index >= _ui_actions.size():
		if _ui_actions.is_empty() and _place_ui_script.is_empty() and not _smoke:
			_arm_deadline(_timeout_ms)
			return
		_ui_finish()
		return
	var action: Dictionary = _ui_actions[_ui_action_index]
	_ui_action_index += 1
	_ui_actions_run += 1
	match str(action.get("kind", "")):
		"select":
			_ui_select(int(action.get("index", 0)))
		"move":
			_ui_move(int(action.get("dx", 0)), int(action.get("dz", 0)))
		"rotate":
			_ui_rotate(int(action.get("rotation", 0)))
		"place":
			_ui_place()
		"lock":
			_ui_lock_action()
		"unlock":
			_ui_unlock_action()
		"cancel":
			_ui_finish()
		_:
			_fail("place_ui_action")

func _ui_select(index: int) -> void:
	if _ui_entries.is_empty():
		_fail("place_ui_empty")
		return
	var count := _ui_entries.size()
	_ui_selected = ((index % count) + count) % count
	_ui_render_ghost()
	_ui_next_action()

func _ui_move(dx: int, dz: int) -> void:
	_ui_center = Vector2(
		roundf(_ui_center.x + float(dx)),
		roundf(_ui_center.y + float(dz)),
	)
	_ui_center.x = clampf(_ui_center.x, -1000000.0, 1000000.0)
	_ui_center.y = clampf(_ui_center.y, -1000000.0, 1000000.0)
	_ui_render_ghost()
	_ui_next_action()

func _ui_rotate(rotation_deg: int) -> void:
	var rotation := PlacementUI.canonical_rotation(rotation_deg)
	if rotation < 0:
		_fail("place_ui_rotation")
		return
	_ui_rotation = rotation
	_ui_render_ghost()
	_ui_next_action()

func _ui_place() -> void:
	if _ui_entries.is_empty():
		_fail("place_ui_empty")
		return
	var entry: Dictionary = _ui_entries[_ui_selected]
	_ui_pending_hash = str(entry.get("hash", ""))
	_ui_pending_center = _ui_center
	_ui_pending_rotation = _ui_rotation
	_set_status("Placing %s at %.1f, %.1f." % [str(entry.get("id", "")), _ui_center.x, _ui_center.y])
	if not _send({
		"type": "place_blueprint",
		"blueprint_hash": _ui_pending_hash,
		"x": _ui_center.x,
		"z": _ui_center.y,
		"rotation": _ui_rotation,
	}):
		return
	_phase = Phase.UI_PLACE_WAIT
	_arm_deadline(_timeout_ms)

## Accept an authoritative placement, render its box, and refresh the view.
func _on_ui_place_result(message: Dictionary) -> void:
	if _phase != Phase.UI_PLACE_WAIT:
		_fail("place_result_in_%s" % _phase_name())
		return
	var tick := int(message.get("tick", -1))
	if tick != _last_tick + 1:
		_fail("place_ui_tick_%d_after_%d" % [tick, _last_tick])
		return
	var placement_id := int(message.get("placement_id", 0))
	if placement_id < 1:
		_fail("place_ui_id_%d" % placement_id)
		return
	if str(message.get("blueprint_hash", "")) != _ui_pending_hash:
		_fail("place_ui_hash_mismatch")
		return
	var placement_checksum := str(message.get("placement_checksum", ""))
	var world_checksum := str(message.get("world_checksum", ""))
	if not _is_hex16(placement_checksum) or not _is_hex16(world_checksum):
		_fail("place_ui_checksums")
		return
	var view_raw: Variant = message.get("placement", null)
	if typeof(view_raw) != TYPE_DICTIONARY:
		_fail("place_ui_view")
		return
	var view: Dictionary = view_raw
	var center := _finite_array(view.get("center", null), 3)
	var footprint := _finite_array(view.get("footprint_m", null), 2)
	var bounds := _finite_array(view.get("bounds_m", null), 3)
	if center.is_empty() or footprint.is_empty() or bounds.is_empty():
		_fail("place_ui_geometry")
		return
	var placed_center := Vector2(float(center[0]), float(center[2]))
	if not placed_center.is_equal_approx(_ui_pending_center):
		_fail("place_ui_center")
		return
	if int(view.get("rotation_deg", _ui_pending_rotation)) != _ui_pending_rotation:
		_fail("place_ui_rotation_echo")
		return
	if float(footprint[0]) <= 0.0 or float(footprint[1]) <= 0.0 or float(bounds[1]) <= 0.0:
		_fail("place_ui_geometry_size")
		return
	_ui_placements.append({
		"id": placement_id,
		"hash": _ui_pending_hash,
		"center": placed_center,
		"footprint": Vector2(float(footprint[0]), float(footprint[1])),
		"bounds": Vector3(float(bounds[0]), float(bounds[1]), float(bounds[2])),
		"checksum": placement_checksum,
		"world_checksum": world_checksum,
		"tick": tick,
	})
	_ui_accepted += 1
	_ui_last_error = ""
	print((
		"MP_PLACE_UI_PLACE status=ok index=%d id=%d hash=%s center=%.1f,%.1f"
		+ " rotation=%d footprint=%.1f,%.1f placement_checksum=%s"
		+ " world_checksum=%s tick=%d"
	) % [
			_ui_selected,
			placement_id,
			_ui_pending_hash,
			placed_center.x,
			placed_center.y,
			_ui_pending_rotation,
			float(footprint[0]),
			float(footprint[1]),
			placement_checksum,
			world_checksum,
			tick,
		])
	_store_snapshot(tick, _last_population, _last_balance, world_checksum)
	_ui_checksum_stale = false
	_ui_render_authoritative()
	_set_status("Placement accepted; refreshing the authoritative view.")
	if not _send({
		"type": "set_interest",
		"center_x": _ui_center.x,
		"center_z": _ui_center.y,
		"radius_m": _radius_m,
	}):
		return
	_phase = Phase.UI_SNAPSHOT_SENT
	_arm_deadline(_timeout_ms)

## Send `blueprint_locks` once after a successful welcome. The reply may arrive
## in any phase: it is read-only, never advances the tick, and does not change
## the road/interest/placement state machine.
func _ui_request_locks() -> bool:
	return _send({"type": "blueprint_locks"})

## Parse one wire lock view. Returns `{}` on a malformed record or a hash that
## is not already canonical, so neither the list nor a result can inject an
## unvalidated hash into the map.
func _ui_parse_lock(raw: Variant) -> Dictionary:
	if typeof(raw) != TYPE_DICTIONARY:
		return {}
	var entry: Dictionary = raw
	var hash := PlacementUI.canonical_hash(str(entry.get("hash", "")))
	if hash.is_empty() or hash != str(entry.get("hash", "")):
		return {}
	var player_id := int(entry.get("player_id", 0))
	if player_id < 1:
		return {}
	var player := str(entry.get("player", ""))
	if player.is_empty() or player.length() > 128:
		return {}
	var tick := int(entry.get("tick", -1))
	if tick < 0:
		return {}
	# Older servers omit the lease; 0 means "unknown", never "expired now".
	var expires_tick := int(entry.get("expires_tick", 0))
	if expires_tick < 0:
		expires_tick = 0
	return {
		"hash": hash,
		"player_id": player_id,
		"player": player,
		"tick": tick,
		"expires_tick": expires_tick,
	}

## Read-only lock table response. Allowed in any place-UI phase because the
## request is sent immediately after `welcome`, before the road plan.
func _on_blueprint_lock_list(message: Dictionary) -> void:
	if not _place_ui:
		_fail("unexpected_message_blueprint_lock_list")
		return
	var tick := int(message.get("tick", -1))
	var count := int(message.get("count", -1))
	var checksum := str(message.get("checksum", ""))
	var raw: Variant = message.get("locks", null)
	if tick < 0 or count < 0 or not _is_hex16(checksum) or typeof(raw) != TYPE_ARRAY:
		_fail("blueprint_lock_list_fields")
		return
	var locks_raw: Array = raw
	if count != locks_raw.size():
		_fail("blueprint_lock_list_count")
		return
	var locks := {}
	for entry in locks_raw:
		var lock := _ui_parse_lock(entry)
		if lock.is_empty():
			_fail("blueprint_lock_list_entry")
			return
		var hash := str(lock.get("hash", ""))
		if locks.has(hash):
			_fail("blueprint_lock_list_duplicate")
			return
		locks[hash] = lock
	_ui_locks = locks
	_ui_lock_checksum = checksum
	print(
		"MP_PLACE_UI_LOCKS status=ok count=%d checksum=%s tick=%d"
		% [locks.size(), checksum, tick]
	)
	_refresh_hud()

## Accepted `blueprint_lock` result: update the map, mark the hash as this
## session's, and continue the script or return to interactive placement.
func _on_blueprint_lock_result(message: Dictionary) -> void:
	if not _place_ui or _phase != Phase.UI_LOCK_WAIT:
		_fail("lock_result_in_%s" % _phase_name())
		return
	var tick := int(message.get("tick", -1))
	if tick != _last_tick + 1:
		_fail("lock_tick_%d_after_%d" % [tick, _last_tick])
		return
	var lock := _ui_parse_lock(message.get("lock", null))
	if lock.is_empty():
		_fail("blueprint_lock_result_view")
		return
	var hash := str(lock.get("hash", ""))
	if hash != _ui_lock_pending_hash:
		_fail("blueprint_lock_result_hash")
		return
	if int(lock.get("tick", -1)) != tick:
		_fail("blueprint_lock_result_tick")
		return
	var locks_count := int(message.get("locks_count", -1))
	var checksum := str(message.get("checksum", ""))
	if locks_count < 1 or not _is_hex16(checksum):
		_fail("blueprint_lock_result_fields")
		return
	_ui_locks[hash] = lock
	_ui_own_locks[hash] = true
	_ui_lock_checksum = checksum
	_ui_locks_taken += 1
	_ui_lock_pending_hash = ""
	_last_tick = tick
	_ui_checksum_stale = true
	print((
		"MP_PLACE_UI_LOCK status=ok selected=%d hash=%s player_id=%d"
		+ " count=%d checksum=%s tick=%d lock=%s lease=%s"
	) % [
			_ui_selected,
			hash,
			int(lock.get("player_id", 0)),
			locks_count,
			checksum,
			tick,
			_ui_lock_marker_state(hash),
			_ui_lock_lease_marker(hash),
		])
	_set_status("Blueprint lock held by this session.")
	_refresh_hud()
	_phase = Phase.UI_ACTIVE
	_arm_deadline(_timeout_ms)
	_ui_next_action()

## Accepted `blueprint_unlock` result: drop the map entry and continue.
func _on_blueprint_unlock_result(message: Dictionary) -> void:
	if not _place_ui or _phase != Phase.UI_UNLOCK_WAIT:
		_fail("unlock_result_in_%s" % _phase_name())
		return
	var tick := int(message.get("tick", -1))
	if tick != _last_tick + 1:
		_fail("unlock_tick_%d_after_%d" % [tick, _last_tick])
		return
	var hash := str(message.get("hash", ""))
	if hash != _ui_lock_pending_hash:
		_fail("blueprint_unlock_result_hash")
		return
	var locks_count := int(message.get("locks_count", -1))
	var checksum := str(message.get("checksum", ""))
	if locks_count < 0 or not _is_hex16(checksum):
		_fail("blueprint_unlock_result_fields")
		return
	_ui_locks.erase(hash)
	_ui_own_locks.erase(hash)
	_ui_lock_checksum = checksum
	_ui_locks_released += 1
	_ui_lock_pending_hash = ""
	_last_tick = tick
	_ui_checksum_stale = true
	print((
		"MP_PLACE_UI_UNLOCK status=ok selected=%d hash=%s count=%d"
		+ " checksum=%s tick=%d lock=%s"
	) % [
			_ui_selected,
			hash,
			locks_count,
			checksum,
			tick,
			_ui_lock_marker_state(hash),
		])
	_set_status("Blueprint unlocked.")
	_refresh_hud()
	_phase = Phase.UI_ACTIVE
	_arm_deadline(_timeout_ms)
	_ui_next_action()

## Remaining lease ticks for a lock view, or -1 when the payload carries no
## usable `expires_tick` (0 means an older server omitted the field) or no world
## tick has been observed yet. Callers render `expires=<tick>` instead.
func _ui_lock_remaining(lock: Dictionary) -> int:
	var expires_tick := int(lock.get("expires_tick", 0))
	if expires_tick <= 0 or _last_tick < 0:
		return -1
	return maxi(0, expires_tick - _last_tick)

## HUD suffix for the selected hash: empty when unlocked, `LOCKED you` for a
## lock this session took, otherwise `LOCKED #<player_id>`. The server lease is
## appended as remaining ticks, or as the raw `expires=<tick>` when the
## remaining count cannot be computed; a foreign holder never becomes "you".
func _ui_lock_suffix(hash: String) -> String:
	var lock: Dictionary = _ui_locks.get(hash, {})
	if lock.is_empty():
		return ""
	var suffix := (
		"LOCKED you" if _ui_own_locks.has(hash)
		else "LOCKED #%d" % int(lock.get("player_id", 0))
	)
	var remaining := _ui_lock_remaining(lock)
	if remaining < 0:
		return "%s · expires=%d" % [suffix, int(lock.get("expires_tick", 0))]
	return "%s · %dt" % [suffix, remaining]

## Path-free lease form for markers: remaining ticks (`299t`, `0t` at/after
## expiry) or the raw `expires=<tick>` when it cannot be computed.
func _ui_lock_lease_marker(hash: String) -> String:
	var lock: Dictionary = _ui_locks.get(hash, {})
	if lock.is_empty():
		return "unlocked"
	var remaining := _ui_lock_remaining(lock)
	if remaining < 0:
		return "expires=%d" % int(lock.get("expires_tick", 0))
	return "%dt" % remaining

## Path-free marker form of the same state (`LOCKED_you`, `LOCKED_<id>`,
## `unlocked`); the HUD keeps the spaced human text.
func _ui_lock_marker_state(hash: String) -> String:
	var lock: Dictionary = _ui_locks.get(hash, {})
	if lock.is_empty():
		return "unlocked"
	if _ui_own_locks.has(hash):
		return "LOCKED_you"
	return "LOCKED_%d" % int(lock.get("player_id", 0))

func _ui_selected_hash() -> String:
	if _ui_entries.is_empty():
		return ""
	return str(_ui_entries[_ui_selected].get("hash", ""))

## Scripted `lock`: always ask the server to lock (a re-lock refreshes), unless
## another holder is already visible, in which case it fails locally and the
## script continues without a wire command.
func _ui_lock_action() -> void:
	var hash := _ui_selected_hash()
	if hash.is_empty():
		_fail("place_ui_empty")
		return
	var lock: Dictionary = _ui_locks.get(hash, {})
	if not lock.is_empty() and not _ui_own_locks.has(hash):
		_ui_local_locked(hash, lock)
		_ui_next_action()
		return
	_ui_send_lock(hash)

## Scripted `unlock`: always send `blueprint_unlock`; an unknown/foreign lock
## comes back as a rejection, which is recorded and does not abort the script.
func _ui_unlock_action() -> void:
	var hash := _ui_selected_hash()
	if hash.is_empty():
		_fail("place_ui_empty")
		return
	_ui_send_unlock(hash)

## Key `L`: lock an unlocked hash, unlock a hash this session holds, and only
## report `blueprint.locked` locally when another holder owns it. The ghost is
## never blocked: the server stays authoritative on placement.
func _ui_toggle_lock() -> void:
	var hash := _ui_selected_hash()
	if hash.is_empty():
		return
	var lock: Dictionary = _ui_locks.get(hash, {})
	if lock.is_empty():
		_ui_send_lock(hash)
	elif _ui_own_locks.has(hash):
		_ui_send_unlock(hash)
	else:
		_ui_local_locked(hash, lock)

func _ui_local_locked(hash: String, lock: Dictionary) -> void:
	_ui_last_error = "blueprint.locked"
	_set_status("Blueprint locked by player #%d." % int(lock.get("player_id", 0)))
	print((
		"MP_PLACE_UI_LOCK_LOCAL status=ok selected=%d hash=%s player_id=%d"
		+ " code=blueprint.locked"
	) % [_ui_selected, hash, int(lock.get("player_id", 0))])
	_refresh_hud()

func _ui_send_lock(hash: String) -> bool:
	_ui_lock_pending_hash = hash
	_set_status("Requesting blueprint lock...")
	if not _send({"type": "blueprint_lock", "hash": hash}):
		return false
	_phase = Phase.UI_LOCK_WAIT
	_arm_deadline(_timeout_ms)
	return true

func _ui_send_unlock(hash: String) -> bool:
	_ui_lock_pending_hash = hash
	_set_status("Requesting blueprint unlock...")
	if not _send({"type": "blueprint_unlock", "hash": hash}):
		return false
	_phase = Phase.UI_UNLOCK_WAIT
	_arm_deadline(_timeout_ms)
	return true

func _on_ui_reject(message: Dictionary) -> void:
	var code := str(message.get("code", "missing"))
	_ui_rejected += 1
	_ui_last_error = code
	_ui_lock_pending_hash = ""
	print("MP_PLACE_UI_REJECT status=ok index=%d code=%s" % [_ui_selected, code])
	_set_status("Rejected: %s" % code)
	_phase = Phase.UI_ACTIVE
	_arm_deadline(_timeout_ms)
	_ui_next_action()

## Print the terminal marker, then leave the session cleanly.
func _ui_finish() -> void:
	if _finished:
		return
	print((
		"MP_PLACE_UI_DONE status=ok mode=ui actions=%d accepted=%d rejected=%d"
		+ " placements=%d selected=%d error=%s locks=%d unlocks=%d"
		+ " lock_checksum=%s"
	) % [
			_ui_actions_run,
			_ui_accepted,
			_ui_rejected,
			_ui_placements.size(),
			_ui_selected,
			_ui_last_error if not _ui_last_error.is_empty() else "-",
			_ui_locks_taken,
			_ui_locks_released,
			_ui_lock_checksum if not _ui_lock_checksum.is_empty() else "-",
		])
	_set_status("Placement UI complete; disconnecting.")
	if not _send({"type": "disconnect"}):
		return
	_phase = Phase.DISCONNECT_SENT
	_arm_deadline(_timeout_ms)

func _assert_ui_view() -> bool:
	if not _view_center.is_equal_approx(_ui_center):
		_fail("assert_ui_center")
		return false
	if absf(_view_radius_m - _radius_m) > 0.001:
		_fail("assert_ui_radius")
		return false
	if _visible_roads <= 0 or _roads.is_empty():
		_fail("assert_ui_roads")
		return false
	return true

func _ui_render_ghost() -> void:
	if _ui_root == null or _ui_entries.is_empty():
		return
	var entry: Dictionary = _ui_entries[_ui_selected]
	if _ui_ghost == null:
		var box := BoxMesh.new()
		box.size = Vector3.ONE
		var material := StandardMaterial3D.new()
		material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
		material.transparency = BaseMaterial3D.TRANSPARENCY_ALPHA
		material.albedo_color = COLOR_UI_GHOST
		box.material = material
		_ui_ghost = MeshInstance3D.new()
		_ui_ghost.name = "WorldClientPlacementGhost"
		_ui_ghost.mesh = box
		_ui_ghost.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
		_ui_root.add_child(_ui_ghost)
	_ui_ghost_transform = PlacementUI.ghost_transform(
		entry, _ui_center.x, _ui_center.y, _ui_rotation
	)
	_ui_ghost.transform = _ui_ghost_transform
	_ui_ghost.visible = true

## Render one box per accepted placement. These live under `_ui_root`, which is
## never cleared by `_build_scene`, so a view refresh keeps them.
func _ui_render_authoritative() -> void:
	if _ui_root == null:
		return
	for node in _ui_placement_transforms:
		if is_instance_valid(node):
			node.free()
	_ui_placement_transforms.clear()
	for placement in _ui_placements:
		var footprint: Vector2 = placement.get("footprint", Vector2.ONE)
		var bounds: Vector3 = placement.get("bounds", Vector3.ONE)
		var center: Vector2 = placement.get("center", Vector2.ZERO)
		var size := Vector3(footprint.x, bounds.y, footprint.y)
		var box := BoxMesh.new()
		box.size = Vector3.ONE
		var material := StandardMaterial3D.new()
		material.albedo_color = COLOR_UI_PLACEMENT
		material.roughness = 0.65
		box.material = material
		var node := MeshInstance3D.new()
		node.name = "WorldClientPlacementUI_%d" % int(placement.get("id", 0))
		node.mesh = box
		node.transform = Transform3D(
			Basis.from_scale(size),
			Vector3(center.x, size.y * 0.5, center.y),
		)
		node.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
		_ui_root.add_child(node)
		_ui_placement_transforms.append(node)

## Interactive controls. Scripted runs stay deterministic and ignore input.
func _unhandled_input(event: InputEvent) -> void:
	if not _place_ui or _phase != Phase.UI_ACTIVE or not _place_ui_script.is_empty():
		return
	if event is InputEventKey and event.pressed and not event.echo:
		match event.keycode:
			KEY_TAB:
				_ui_select(_ui_selected + (-1 if event.shift_pressed else 1))
			KEY_LEFT:
				_ui_move(-1, 0)
			KEY_RIGHT:
				_ui_move(1, 0)
			KEY_UP:
				_ui_move(0, -1)
			KEY_DOWN:
				_ui_move(0, 1)
			KEY_R:
				_ui_rotate(_ui_rotation + 90)
			KEY_L:
				_ui_toggle_lock()
			KEY_ENTER, KEY_KP_ENTER:
				_ui_place()
			KEY_ESCAPE:
				_ui_finish()
	elif event is InputEventMouseButton and event.pressed:
		if event.button_index == MOUSE_BUTTON_WHEEL_UP:
			_ui_select(_ui_selected - 1)
		elif event.button_index == MOUSE_BUTTON_WHEEL_DOWN:
			_ui_select(_ui_selected + 1)

func _finish_ok() -> void:
	if _finished:
		return
	_finished = true
	set_process(false)
	_set_status("3D world client exchange complete.")
	_refresh_hud()
	if _peer != null:
		_peer.disconnect_from_host()
	print(
		"MP_WORLD_CLIENT_DONE status=ok session=%s tick=%d frames_sent=%d frames_received=%d"
		% [_session_id, _last_tick, _frames_sent, _frames_received]
	)
	_free_chat_model()
	if _smoke:
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

func _fail(reason: String) -> void:
	if _finished:
		return
	_finished = true
	set_process(false)
	_set_status("3D world client failed: " + reason)
	if _peer != null:
		_peer.disconnect_from_host()
	print(
		"MP_WORLD_CLIENT status=fail error=%s session=%s frames_sent=%d frames_received=%d"
		% [reason, _session_id, _frames_sent, _frames_received]
	)
	_free_chat_model()
	if _smoke:
		get_tree().quit(1)

func _store_snapshot(tick: int, population: int, balance: int, checksum: String) -> void:
	_last_tick = tick
	_last_population = population
	_last_balance = balance
	_last_checksum = checksum

## Validate and build the scene from one interest view. Returns false after
## calling `_fail` on an invalid viewport or render setup.
func _build_scene(
	tick: int,
	center: Vector2,
	radius_m: float,
	roads: Array,
	truncated: bool,
	buildings: Array,
	buildings_truncated: bool
) -> bool:
	if not _is_valid_view(center, radius_m):
		_fail("render_invalid_view")
		return false
	_clear_scene()
	_view_tick = tick
	_view_center = center
	_view_radius_m = radius_m
	_roads = roads
	_buildings = buildings
	_truncated = truncated
	_buildings_truncated = buildings_truncated
	_road_checksum = InterestRender.checksum(_roads)
	_building_checksum = _checksum_buildings(_buildings)
	_classify_roads()
	_classify_buildings()

	_add_environment()
	_add_ground()
	_add_grid()
	_add_interest_ring()
	_add_center_marker()
	_add_roads()
	_add_buildings()
	_add_camera()
	_has_view = true
	_refresh_hud()
	return true

func _clear_scene() -> void:
	if _container == null:
		return
	for child in _container.get_children():
		_container.remove_child(child)
		child.free()
	_road_multimesh = null
	_building_multimesh = null
	_camera = null
	_roads.clear()
	_buildings.clear()
	_road_transforms.clear()
	_building_transforms.clear()
	_inside = 0
	_outside = 0
	_buildings_inside = 0
	_buildings_outside = 0
	_road_checksum = ""
	_building_checksum = ""
	_grid_lines = 0
	_has_view = false

func _classify_roads() -> void:
	_inside = 0
	_outside = 0
	for road in _roads:
		var midpoint := _road_midpoint(road)
		if midpoint.distance_to(_view_center) <= _view_radius_m + 0.0001:
			_inside += 1
		else:
			_outside += 1

func _classify_buildings() -> void:
	_buildings_inside = 0
	_buildings_outside = 0
	for building in _buildings:
		var center: Vector2 = building.get("center", Vector2.ZERO)
		if center.distance_to(_view_center) <= _view_radius_m + 0.0001:
			_buildings_inside += 1
		else:
			_buildings_outside += 1

func _road_midpoint(road: Dictionary) -> Vector2:
	var start: Vector2 = road.get("start", Vector2.ZERO)
	var end: Vector2 = road.get("end", Vector2.ZERO)
	return (start + end) * 0.5

## One flat box per road, sized from its width and centreline length. The
## per-instance colour marks a road the server should already have filtered.
func _road_transform(road: Dictionary) -> Transform3D:
	var start: Vector2 = road.get("start", Vector2.ZERO)
	var end: Vector2 = road.get("end", Vector2.ZERO)
	var width := clampf(float(road.get("width_m", 6.0)), MIN_WIDTH_M, MAX_WIDTH_M)
	var delta := end - start
	var length := maxf(delta.length(), MIN_ROAD_LENGTH_M)
	var yaw := atan2(delta.x, delta.y)
	var midpoint := (start + end) * 0.5
	var basis := Basis(Vector3.UP, yaw) * Basis.from_scale(
		Vector3(width, ROAD_HEIGHT_M, length)
	)
	return Transform3D(basis, Vector3(midpoint.x, ROAD_Y_M, midpoint.y))

## One box per building record at its centre. `radius_m` is a render hint, so
## the footprint is shown as a diameter with a height derived from it.
func _building_transform(building: Dictionary) -> Transform3D:
	var center: Vector2 = building.get("center", Vector2.ZERO)
	var radius_m := float(building.get("radius_m", DEFAULT_BUILDING_RADIUS_M))
	var footprint := radius_m * 2.0
	var height := clampf(
		radius_m * BUILDING_HEIGHT_RATIO, MIN_BUILDING_HEIGHT_M, MAX_BUILDING_HEIGHT_M
	)
	var basis := Basis.from_scale(Vector3(footprint, height, footprint))
	return Transform3D(basis, Vector3(center.x, height * 0.5, center.y))

func _add_environment() -> void:
	var env := Environment.new()
	env.background_mode = Environment.BG_COLOR
	env.background_color = Color(0.035, 0.045, 0.060)
	env.ambient_light_source = Environment.AMBIENT_SOURCE_COLOR
	env.ambient_light_color = Color(0.55, 0.62, 0.72)
	env.ambient_light_energy = 0.85
	var world_env := WorldEnvironment.new()
	world_env.name = "WorldClientEnvironment"
	world_env.environment = env
	_container.add_child(world_env)

	var light := DirectionalLight3D.new()
	light.name = "WorldClientLight"
	light.rotation_degrees = Vector3(-55.0, -40.0, 0.0)
	light.light_energy = 1.15
	light.shadow_enabled = false
	_container.add_child(light)

func _add_ground() -> void:
	var plane := PlaneMesh.new()
	var extent := maxf(_view_radius_m * GROUND_MARGIN, GRID_MIN_EXTENT_M)
	plane.size = Vector2(extent * 2.0, extent * 2.0)
	var material := StandardMaterial3D.new()
	material.albedo_color = COLOR_GROUND
	material.roughness = 1.0
	plane.material = material

	var ground := MeshInstance3D.new()
	ground.name = "WorldClientGround"
	ground.mesh = plane
	ground.position = Vector3(_view_center.x, -0.06, _view_center.y)
	ground.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	_container.add_child(ground)

## World-aligned 16 m grid plus X/Z/Y axis lines through the world origin when
## they intersect the framed extent.
func _add_grid() -> void:
	var extent := maxf(_view_radius_m * GROUND_MARGIN, GRID_MIN_EXTENT_M)
	var min_ix := floori((_view_center.x - extent) / GRID_STEP_M)
	var max_ix := ceili((_view_center.x + extent) / GRID_STEP_M)
	var min_iz := floori((_view_center.y - extent) / GRID_STEP_M)
	var max_iz := ceili((_view_center.y + extent) / GRID_STEP_M)
	var min_x := float(min_ix) * GRID_STEP_M
	var max_x := float(max_ix) * GRID_STEP_M
	var min_z := float(min_iz) * GRID_STEP_M
	var max_z := float(max_iz) * GRID_STEP_M
	var vertices: Array = []
	var colors: Array = []

	for ix in range(min_ix, max_ix + 1):
		var x := float(ix) * GRID_STEP_M
		_append_line(
			vertices,
			colors,
			Vector3(x, 0.02, min_z),
			Vector3(x, 0.02, max_z),
			COLOR_GRID
		)
	for iz in range(min_iz, max_iz + 1):
		var z := float(iz) * GRID_STEP_M
		_append_line(
			vertices,
			colors,
			Vector3(min_x, 0.02, z),
			Vector3(max_x, 0.02, z),
			COLOR_GRID
		)
	if min_z <= 0.0 and max_z >= 0.0:
		_append_line(
			vertices, colors, Vector3(0.0, 0.05, min_z), Vector3(0.0, 0.05, max_z), COLOR_AXIS_Z
		)
	if min_x <= 0.0 and max_x >= 0.0:
		_append_line(
			vertices, colors, Vector3(min_x, 0.05, 0.0), Vector3(max_x, 0.05, 0.0), COLOR_AXIS_X
		)
	if min_x <= 0.0 and max_x >= 0.0 and min_z <= 0.0 and max_z >= 0.0:
		_append_line(
			vertices, colors, Vector3(0.0, 0.0, 0.0), Vector3(0.0, 12.0, 0.0), COLOR_AXIS_Y
		)

	_grid_lines = vertices.size() / 2
	var arrays := []
	arrays.resize(Mesh.ARRAY_MAX)
	arrays[Mesh.ARRAY_VERTEX] = PackedVector3Array(vertices)
	arrays[Mesh.ARRAY_COLOR] = PackedColorArray(colors)
	var mesh := ArrayMesh.new()
	mesh.add_surface_from_arrays(Mesh.PRIMITIVE_LINES, arrays)
	var material := StandardMaterial3D.new()
	material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	material.vertex_color_use_as_albedo = true
	mesh.surface_set_material(0, material)

	var grid := MeshInstance3D.new()
	grid.name = "WorldClientGrid"
	grid.mesh = mesh
	grid.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	_container.add_child(grid)

func _append_line(
	vertices: Array, colors: Array, from: Vector3, to: Vector3, color: Color
) -> void:
	vertices.append(from)
	vertices.append(to)
	colors.append(color)
	colors.append(color)

func _add_interest_ring() -> void:
	if _view_radius_m <= 0.0:
		return
	var torus := TorusMesh.new()
	var tube := clampf(_view_radius_m * 0.012, 0.25, 1.0)
	torus.inner_radius = maxf(_view_radius_m - tube, 0.01)
	torus.outer_radius = _view_radius_m + tube
	torus.rings = RING_SEGMENTS
	torus.ring_segments = 8
	var material := StandardMaterial3D.new()
	material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	material.albedo_color = COLOR_RING
	torus.material = material

	var ring := MeshInstance3D.new()
	ring.name = "WorldClientInterestRing"
	ring.mesh = torus
	ring.position = Vector3(_view_center.x, 0.04, _view_center.y)
	ring.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	_container.add_child(ring)

func _add_center_marker() -> void:
	var box := BoxMesh.new()
	box.size = Vector3.ONE
	var material := StandardMaterial3D.new()
	material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	material.albedo_color = COLOR_CENTER
	box.material = material

	var marker := MeshInstance3D.new()
	marker.name = "WorldClientCenter"
	marker.mesh = box
	marker.position = Vector3(_view_center.x, 0.6, _view_center.y)
	marker.scale = Vector3(1.2, 1.2, 1.2)
	marker.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	_container.add_child(marker)

func _add_roads() -> void:
	if _roads.is_empty():
		return
	var box := BoxMesh.new()
	box.size = Vector3.ONE
	var material := StandardMaterial3D.new()
	material.vertex_color_use_as_albedo = true
	material.roughness = 0.9
	box.material = material

	_road_multimesh = MultiMesh.new()
	_road_multimesh.transform_format = MultiMesh.TRANSFORM_3D
	_road_multimesh.use_colors = true
	_road_multimesh.mesh = box
	_road_multimesh.instance_count = _roads.size()
	_road_transforms.clear()

	for index in range(_roads.size()):
		var road: Dictionary = _roads[index]
		var transform := _road_transform(road)
		_road_transforms.append(transform)
		_road_multimesh.set_instance_transform(index, transform)
		var color := (
			COLOR_OUTSIDE
			if _road_midpoint(road).distance_to(_view_center) > _view_radius_m + 0.0001
			else COLOR_INSIDE
		)
		_road_multimesh.set_instance_color(index, color)

	var node := MultiMeshInstance3D.new()
	node.name = "WorldClientRoads"
	node.multimesh = _road_multimesh
	node.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	_container.add_child(node)

func _add_buildings() -> void:
	if _buildings.is_empty():
		return
	var box := BoxMesh.new()
	box.size = Vector3.ONE
	var material := StandardMaterial3D.new()
	material.vertex_color_use_as_albedo = true
	material.roughness = 0.85
	box.material = material

	_building_multimesh = MultiMesh.new()
	_building_multimesh.transform_format = MultiMesh.TRANSFORM_3D
	_building_multimesh.use_colors = true
	_building_multimesh.mesh = box
	_building_multimesh.instance_count = _buildings.size()
	_building_transforms.clear()

	for index in range(_buildings.size()):
		var building: Dictionary = _buildings[index]
		var transform := _building_transform(building)
		_building_transforms.append(transform)
		_building_multimesh.set_instance_transform(index, transform)
		var center: Vector2 = building.get("center", Vector2.ZERO)
		var color := (
			COLOR_BUILDING_OUTSIDE
			if center.distance_to(_view_center) > _view_radius_m + 0.0001
			else COLOR_BUILDING_INSIDE
		)
		_building_multimesh.set_instance_color(index, color)

	var node := MultiMeshInstance3D.new()
	node.name = "WorldClientBuildings"
	node.multimesh = _building_multimesh
	node.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	_container.add_child(node)

## One authoritative placement box on the client ground. The server's placement
## Y (40 m placeholder ground) is not used; the box sits with its bottom at y=0.
## `_placement_transform` is the CPU-side mirror the smoke asserts.
func _render_placement() -> bool:
	if _container == null:
		_fail("placement_container_missing")
		return false
	_placement_size = Vector3(
		_placement_footprint.x, _placement_bounds.y, _placement_footprint.y
	)
	_placement_transform = Transform3D(
		Basis.from_scale(_placement_size),
		Vector3(_placement_center.x, _placement_size.y * 0.5, _placement_center.y)
	)

	var box := BoxMesh.new()
	box.size = Vector3.ONE
	var material := StandardMaterial3D.new()
	material.albedo_color = COLOR_PLACEMENT
	material.roughness = 0.65
	box.material = material

	var node := MeshInstance3D.new()
	node.name = "WorldClientPlacement"
	node.mesh = box
	node.transform = _placement_transform
	node.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	_container.add_child(node)
	return true

func _add_camera() -> void:
	var framed_radius := maxf(_view_radius_m, 8.0)
	_camera = Camera3D.new()
	_camera.name = "WorldClientCamera"
	_camera.fov = CAMERA_FOV
	_camera.near = 0.1
	_camera.far = maxf(framed_radius * 12.0, 256.0)
	_camera.position = (
		Vector3(_view_center.x, 0.0, _view_center.y)
		+ Vector3(framed_radius * 0.85, framed_radius * 1.15, framed_radius * 0.85)
	)
	_camera.current = true
	_container.add_child(_camera)
	_camera.look_at(Vector3(_view_center.x, 0.0, _view_center.y), Vector3.UP)

func _road_count() -> int:
	return _road_multimesh.instance_count if _road_multimesh != null else 0

func _building_count() -> int:
	return _building_multimesh.instance_count if _building_multimesh != null else 0

## Parse and bound the optional `interest_view.buildings` payload. Entries are
## `{id, center: Vector2, radius_m}` sorted by ascending id, capped at
## [`MAX_BUILDINGS`]; malformed entries are skipped.
func _parse_buildings(raw: Variant) -> Dictionary:
	var buildings: Array = []
	var raw_count := 0
	if typeof(raw) == TYPE_ARRAY:
		raw_count = raw.size()
		for entry in raw:
			if buildings.size() >= MAX_BUILDINGS:
				break
			if typeof(entry) != TYPE_DICTIONARY:
				continue
			var center_raw: Variant = entry.get("center", null)
			if not InterestRender.is_world_pair(center_raw):
				continue
			var radius_m := DEFAULT_BUILDING_RADIUS_M
			var radius_raw: Variant = entry.get("radius_m", DEFAULT_BUILDING_RADIUS_M)
			if typeof(radius_raw) == TYPE_INT or typeof(radius_raw) == TYPE_FLOAT:
				var candidate := float(radius_raw)
				if is_finite(candidate) and candidate > 0.0:
					radius_m = clampf(candidate, MIN_BUILDING_RADIUS_M, MAX_BUILDING_RADIUS_M)
			buildings.append(
				{
					"id": int(entry.get("id", 0)),
					"center": Vector2(float(center_raw[0]), float(center_raw[1])),
					"radius_m": radius_m,
				}
			)
	buildings.sort_custom(
		func(a: Dictionary, b: Dictionary) -> bool:
			var a_id := int(a.get("id", 0))
			var b_id := int(b.get("id", 0))
			if a_id != b_id:
				return a_id < b_id
			var a_center: Vector2 = a.get("center", Vector2.ZERO)
			var b_center: Vector2 = b.get("center", Vector2.ZERO)
			if a_center.x != b_center.x:
				return a_center.x < b_center.x
			if a_center.y != b_center.y:
				return a_center.y < b_center.y
			return float(a.get("radius_m", 0.0)) < float(b.get("radius_m", 0.0))
	)
	return {
		"buildings": buildings,
		"truncated": raw_count > MAX_BUILDINGS,
	}

## Deterministic 16-hex checksum over the bounded building ids, centres, and
## millimetre-quantized radii. Same two-lane FNV-1a shape as the road checksum.
func _checksum_buildings(buildings: Array) -> String:
	var low := InterestRender.FNV_OFFSET_BASIS
	var high := InterestRender.FNV_OFFSET_BASIS ^ InterestRender.FNV_PRIME
	low = _mix_low(low, buildings.size())
	high = _mix_high(high, buildings.size())
	for building in buildings:
		var id := int(building.get("id", 0))
		var center: Vector2 = building.get("center", Vector2.ZERO)
		var radius_mm := int(round(float(building.get("radius_m", 0.0)) * 1000.0))
		for field in [
			(id >> 32) & 0xFFFFFFFF,
			id & 0xFFFFFFFF,
			int(center.x) & 0xFFFFFFFF,
			int(center.y) & 0xFFFFFFFF,
			radius_mm & 0xFFFFFFFF,
		]:
			low = _mix_low(low, int(field))
			high = _mix_high(high, int(field))
	return "%08x%08x" % [low & 0xFFFFFFFF, high & 0xFFFFFFFF]

func _mix_low(state: int, value: int) -> int:
	return ((state ^ (value & 0xFFFFFFFF)) * InterestRender.FNV_PRIME) & 0xFFFFFFFF

func _mix_high(state: int, value: int) -> int:
	return ((state + (value & 0xFFFFFFFF) + 1) * InterestRender.FNV_PRIME) & 0xFFFFFFFF

## Deterministic smoke assertions on the CPU-side data, never on GPU-side
## `MultiMesh` transforms (headless Godot returns identity there).
func _assert_main_view() -> bool:
	if _roads.size() != ROAD_PLAN.size():
		_fail("assert_main_roads_%d" % _roads.size())
		return false
	if _truncated:
		_fail("assert_main_truncated")
		return false
	if _inside != ROAD_PLAN.size() or _outside != 0:
		_fail("assert_main_road_classify_%d_%d" % [_inside, _outside])
		return false
	if _road_count() != ROAD_PLAN.size() or _road_transforms.size() != ROAD_PLAN.size():
		_fail("assert_main_road_render_%d" % _road_count())
		return false
	if int(_roads[0].get("id", -1)) != 1:
		_fail("assert_main_road_order")
		return false
	var first_road: Transform3D = _road_transforms[0]
	if not first_road.origin.is_equal_approx(Vector3(0.0, ROAD_Y_M, 0.0)):
		_fail("assert_main_road_origin")
		return false
	if not first_road.basis.get_scale().is_equal_approx(Vector3(6.0, ROAD_HEIGHT_M, 128.0)):
		_fail("assert_main_road_scale")
		return false

	if _buildings.size() != 4:
		_fail("assert_main_buildings_%d" % _buildings.size())
		return false
	if _buildings_truncated:
		_fail("assert_main_buildings_truncated")
		return false
	if _buildings_inside != 4 or _buildings_outside != 0:
		_fail("assert_main_building_classify_%d_%d" % [_buildings_inside, _buildings_outside])
		return false
	if _building_count() != 4 or _building_transforms.size() != 4:
		_fail("assert_main_building_render_%d" % _building_count())
		return false
	if int(_buildings[0].get("id", -1)) != 1 or int(_buildings[-1].get("id", -1)) != 18:
		_fail("assert_main_building_ids")
		return false
	var first_building: Transform3D = _building_transforms[0]
	if not first_building.origin.is_equal_approx(Vector3(0.0, 2.0, 0.0)):
		_fail("assert_main_building_origin")
		return false
	if not first_building.basis.get_scale().is_equal_approx(Vector3(16.0, 4.0, 16.0)):
		_fail("assert_main_building_scale")
		return false

	if _camera == null or _grid_lines <= 0:
		_fail("assert_main_scene_nodes")
		return false
	if not _assert_hud_fields():
		return false
	return true

func _assert_zero_view() -> bool:
	if not _roads.is_empty() or not _buildings.is_empty():
		_fail("assert_zero_records_%d_%d" % [_roads.size(), _buildings.size()])
		return false
	if _truncated or _buildings_truncated:
		_fail("assert_zero_truncated")
		return false
	if _road_count() != 0 or _building_count() != 0:
		_fail("assert_zero_instances_%d_%d" % [_road_count(), _building_count()])
		return false
	if _road_multimesh != null or _building_multimesh != null:
		_fail("assert_zero_multimesh")
		return false
	if not _road_transforms.is_empty() or not _building_transforms.is_empty():
		_fail("assert_zero_transforms")
		return false
	if _camera == null or _grid_lines <= 0:
		_fail("assert_zero_scene_nodes")
		return false
	if not _hud_label.text.contains("roads=0") or not _hud_label.text.contains("buildings=0"):
		_fail("assert_zero_hud")
		return false
	return true

## Smoke assertions for the authoritative placement. Every check reads the
## CPU-side mirror; the `MeshInstance3D` node is verified by identity and mirror,
## never by a GPU readback.
func _assert_placement() -> bool:
	if not _has_placement:
		_fail("assert_placement_missing")
		return false
	if _placement_id < 1:
		_fail("assert_placement_id")
		return false
	if _placement_hash != _place_hash:
		_fail("assert_placement_hash")
		return false
	if not _is_hex16(_placement_checksum) or not _is_hex16(_placement_world_checksum):
		_fail("assert_placement_checksums")
		return false
	if not _placement_center.is_equal_approx(Vector2(_place_x, _place_z)):
		_fail("assert_placement_center")
		return false
	if not (_placement_footprint.x > 0.0 and _placement_footprint.y > 0.0):
		_fail("assert_placement_footprint")
		return false
	if not (_placement_bounds.x > 0.0 and _placement_bounds.y > 0.0 and _placement_bounds.z > 0.0):
		_fail("assert_placement_bounds")
		return false
	var expected_size := Vector3(
		_placement_footprint.x, _placement_bounds.y, _placement_footprint.y
	)
	if not _placement_size.is_equal_approx(expected_size):
		_fail("assert_placement_size")
		return false
	var expected_origin := Vector3(
		_placement_center.x, expected_size.y * 0.5, _placement_center.y
	)
	if not _placement_transform.origin.is_equal_approx(expected_origin):
		_fail("assert_placement_origin")
		return false
	if not _placement_transform.basis.get_scale().is_equal_approx(expected_size):
		_fail("assert_placement_scale")
		return false
	var node := _container.get_node_or_null("WorldClientPlacement") as MeshInstance3D
	if node == null:
		_fail("assert_placement_node")
		return false
	if not node.transform.is_equal_approx(_placement_transform):
		_fail("assert_placement_node_transform")
		return false
	if not _assert_placement_hud():
		return false
	return true

func _assert_placement_hud() -> bool:
	if _hud_label == null:
		_fail("assert_placement_hud_missing")
		return false
	for field in [
		"placement_id=%d" % _placement_id,
		"placement_checksum=%s" % _placement_checksum,
		"world_checksum=%s" % _placement_world_checksum,
	]:
		if not _hud_label.text.contains(field):
			_fail("assert_placement_hud_%s" % field)
			return false
	return true

func _assert_hud_fields() -> bool:
	if _hud_label == null:
		_fail("assert_hud_missing")
		return false
	var text := _hud_label.text
	for field in [
		"tick=%d" % _last_tick,
		"population=%d" % _last_population,
		"balance=%d" % _last_balance,
		"roads=%d" % _roads.size(),
		"buildings=%d" % _buildings.size(),
		"truncated=%s" % str(_truncated),
		"buildings_truncated=%s" % str(_buildings_truncated),
		"checksum=%s" % _last_checksum,
	]:
		if not text.contains(field):
			_fail("assert_hud_field_%s" % field)
			return false
	return true

func _marker_text(prefix: String, zero: bool) -> String:
	var rendered_roads := _road_count()
	var rendered_buildings := _building_count()
	if zero:
		return (
			"%s status=ok tick=%d population=%d balance=%d roads=%d buildings=%d"
			+ " truncated=%s buildings_truncated=%s checksum=%s"
			+ " rendered_roads=%d rendered_buildings=%d"
			+ " center=%.1f,%.1f radius=%.1f"
		) % [
			prefix,
			_last_tick,
			_last_population,
			_last_balance,
			_roads.size(),
			_buildings.size(),
			str(_truncated),
			str(_buildings_truncated),
			_last_checksum,
			rendered_roads,
			rendered_buildings,
			_view_center.x,
			_view_center.y,
			_view_radius_m,
		]
	return (
		"%s status=ok tick=%d population=%d balance=%d roads=%d buildings=%d"
		+ " truncated=%s buildings_truncated=%s checksum=%s"
		+ " road_checksum=%s building_checksum=%s"
		+ " rendered_roads=%d rendered_buildings=%d center=%.1f,%.1f radius=%.1f"
		+ " session=%s"
	) % [
		prefix,
		_last_tick,
		_last_population,
		_last_balance,
		_roads.size(),
		_buildings.size(),
		str(_truncated),
		str(_buildings_truncated),
		_last_checksum,
		_road_checksum,
		_building_checksum,
		rendered_roads,
		rendered_buildings,
		_view_center.x,
		_view_center.y,
		_view_radius_m,
		_session_id,
	]

func _placement_marker_text() -> String:
	return (
		"MP_PLACEMENT status=ok id=%d hash=%s placement_checksum=%s"
		+ " world_checksum=%s center=%.1f,%.1f footprint=%.1f,%.1f tick=%d"
	) % [
		_placement_id,
		_placement_hash,
		_placement_checksum,
		_placement_world_checksum,
		_placement_center.x,
		_placement_center.y,
		_placement_footprint.x,
		_placement_footprint.y,
		_last_tick,
	]

func _build_hud() -> void:
	_hud_layer = CanvasLayer.new()
	_hud_layer.name = "WorldClientHud"
	add_child(_hud_layer)

	var panel := PanelContainer.new()
	panel.position = Vector2(12.0, 12.0)
	var style := StyleBoxFlat.new()
	style.bg_color = Color(0.02, 0.03, 0.04, 0.78)
	style.border_color = Color(0.25, 0.32, 0.40)
	style.set_border_width_all(1)
	style.set_content_margin_all(10.0)
	panel.add_theme_stylebox_override("panel", style)
	_hud_layer.add_child(panel)

	_hud_label = Label.new()
	_hud_label.add_theme_color_override("font_color", Color(0.86, 0.90, 0.95))
	_hud_label.add_theme_font_size_override("font_size", 14)
	panel.add_child(_hud_label)

	# Unread chat badge. Separate from the status block above (and from the
	# placement/interest lines) so the markers and HUD text stay comparable.
	# Hidden at zero; this client is debug-only, so the label is English.
	_chat_unread_label = Label.new()
	_chat_unread_label.name = "WorldClientChatUnread"
	_chat_unread_label.visible = false
	_chat_unread_label.anchor_left = 1.0
	_chat_unread_label.anchor_right = 1.0
	_chat_unread_label.offset_left = -212.0
	_chat_unread_label.offset_right = -12.0
	_chat_unread_label.offset_top = 12.0
	_chat_unread_label.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT
	_chat_unread_label.add_theme_color_override("font_color", Color(1.0, 0.86, 0.45))
	_chat_unread_label.add_theme_font_size_override("font_size", 16)
	_hud_layer.add_child(_chat_unread_label)

	_refresh_chat_unread_hud()
	_refresh_hud()

func _refresh_hud() -> void:
	if _hud_label == null:
		return
	var tick_text := str(_last_tick) if _last_tick >= 0 else "-"
	var population_text := str(_last_population) if _last_population >= 0 else "-"
	var balance_text := str(_last_balance) if _last_tick >= 0 else "-"
	var roads_text := str(_roads.size()) if _has_view else "-"
	var buildings_text := str(_buildings.size()) if _has_view else "-"
	var truncated_text := str(_truncated) if _has_view else "-"
	var buildings_truncated_text := str(_buildings_truncated) if _has_view else "-"
	var checksum_text := _last_checksum if not _last_checksum.is_empty() else "-"
	var text := (
		"3D multiplayer world client (debug)\n"
		+ "status: %s\n"
		+ "session: %s\n"
		+ "tick=%s population=%s balance=%s\n"
		+ "interest: center=(%.1f, %.1f) radius=%.1f m\n"
		+ "roads=%s buildings=%s truncated=%s buildings_truncated=%s\n"
		+ "rendered: roads=%d buildings=%d grid_lines=%d camera=%s\n"
		+ "checksum=%s\n"
		+ "road_checksum=%s building_checksum=%s"
	) % [
		_status_text,
		_session_id if not _session_id.is_empty() else "-",
		tick_text,
		population_text,
		balance_text,
		_view_center.x,
		_view_center.y,
		_view_radius_m,
		roads_text,
		buildings_text,
		truncated_text,
		buildings_truncated_text,
		_road_count(),
		_building_count(),
		_grid_lines,
		"ok" if _camera != null else "-",
		checksum_text,
		_road_checksum if not _road_checksum.is_empty() else "-",
		_building_checksum if not _building_checksum.is_empty() else "-",
	]
	if _has_placement:
		text += (
			"\nplacement_id=%d hash=%s placement_checksum=%s world_checksum=%s"
			% [
				_placement_id,
				_placement_hash,
				_placement_checksum,
				_placement_world_checksum,
			]
		)
	if _place_ui:
		text += (
			(
				"\nplacement_ui: entries=%d selected=%d center=(%.1f, %.1f)"
				+ " rotation=%d accepted=%d rejected=%d error=%s"
			)
			% [
				_ui_entries.size(),
				_ui_selected,
				_ui_center.x,
				_ui_center.y,
				_ui_rotation,
				_ui_accepted,
				_ui_rejected,
				_ui_last_error if not _ui_last_error.is_empty() else "-",
			]
		)
		var lock_suffix := _ui_lock_suffix(_ui_selected_hash())
		if not lock_suffix.is_empty():
			text += " lock=%s" % lock_suffix
	_hud_label.text = text

func _set_status(text: String) -> void:
	_status_text = text
	_refresh_hud()

func _is_valid_view(center: Vector2, radius_m: float) -> bool:
	if not is_finite(center.x) or not is_finite(center.y):
		return false
	return is_finite(radius_m) and radius_m >= 0.0 and radius_m <= MAX_RADIUS_M

func _is_pair_close(value: Variant, expected: Vector2, epsilon: float) -> bool:
	if typeof(value) != TYPE_ARRAY or value.size() != 2:
		return false
	if typeof(value[0]) != TYPE_INT and typeof(value[0]) != TYPE_FLOAT:
		return false
	if typeof(value[1]) != TYPE_INT and typeof(value[1]) != TYPE_FLOAT:
		return false
	return (
		absf(float(value[0]) - expected.x) <= epsilon
		and absf(float(value[1]) - expected.y) <= epsilon
	)

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

## Return a finite numeric array of exactly `size` entries, or `[]` when the
## payload is missing, the wrong length, or carries a non-numeric/non-finite
## entry. Placement geometry is validated before it reaches the renderer.
func _finite_array(value: Variant, size: int) -> Array:
	if typeof(value) != TYPE_ARRAY or value.size() != size:
		return []
	for item in value:
		if typeof(item) != TYPE_INT and typeof(item) != TYPE_FLOAT:
			return []
		if not is_finite(float(item)):
			return []
	return value

## Canonicalize `--place-hash`: accept `blake3:` + 64 lowercase hex or bare 64
## lowercase hex. Anything else returns an empty string.
func _canonical_place_hash(value: String) -> String:
	if value.is_empty():
		return ""
	var hex := value
	if hex.begins_with(BLUEPRINT_HASH_PREFIX):
		hex = hex.substr(BLUEPRINT_HASH_PREFIX.length())
	if hex.length() != 64:
		return ""
	for index in range(hex.length()):
		var character := hex[index]
		if not (
			(character >= "0" and character <= "9")
			or (character >= "a" and character <= "f")
		):
			return ""
	return BLUEPRINT_HASH_PREFIX + hex

func _phase_name() -> String:
	match _phase:
		Phase.CONNECTING:
			return "connecting"
		Phase.HELLO_SENT:
			return "hello_sent"
		Phase.ROAD_SENT:
			return "road_sent"
		Phase.INTEREST_SENT:
			return "interest_sent"
		Phase.INTEREST_VIEW_WAIT:
			return "interest_view_wait"
		Phase.ZERO_INTEREST_SENT:
			return "zero_interest_sent"
		Phase.ZERO_VIEW_WAIT:
			return "zero_view_wait"
		Phase.UI_SNAPSHOT_SENT:
			return "ui_snapshot_sent"
		Phase.UI_VIEW_WAIT:
			return "ui_view_wait"
		Phase.UI_PLACE_WAIT:
			return "ui_place_wait"
		Phase.UI_ACTIVE:
			return "ui_active"
		Phase.PLACE_SENT:
			return "place_sent"
		Phase.CHAT_ACTIVE:
			return "chat_active"
		Phase.CHAT_LISTEN:
			return "chat_listen"
		Phase.CHAT_RECONNECT_WAIT_SENT:
			return "chat_reconnect_wait_sent"
		Phase.CHAT_RECONNECT_WAIT_DISCONNECT:
			return "chat_reconnect_wait_disconnect"
		Phase.CHAT_RECONNECT_CONNECTING:
			return "chat_reconnect_connecting"
		Phase.CHAT_RESUME_WELCOME_WAIT:
			return "chat_resume_welcome_wait"
		Phase.CHAT_RESUME_SNAPSHOT_WAIT:
			return "chat_resume_snapshot_wait"
		Phase.CHAT_RECONNECT_CATCHUP_WAIT:
			return "chat_reconnect_catchup_wait"
		Phase.DISCONNECT_SENT:
			return "disconnect_sent"
		_:
			return "unknown"

func _arm_deadline(duration_ms: int) -> void:
	_deadline_ms = Time.get_ticks_msec() + duration_ms

func _arg(args: PackedStringArray, name: String, fallback: String) -> String:
	var index := args.find(name)
	if index >= 0 and index + 1 < args.size():
		return args[index + 1]
	return fallback

func _float_arg(args: PackedStringArray, name: String, fallback: float) -> float:
	var text := _arg(args, name, "")
	if text.is_empty() or not text.is_valid_float():
		return fallback
	var value := text.to_float()
	return value if is_finite(value) else fallback

func _int_arg(args: PackedStringArray, name: String, fallback: int) -> int:
	var text := _arg(args, name, "")
	if text.is_empty() or not text.is_valid_int():
		return fallback
	return text.to_int()

func _positive_int(value: String, fallback: int) -> int:
	if not value.is_valid_int():
		return fallback
	var number := value.to_int()
	return number if number > 0 else fallback

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
