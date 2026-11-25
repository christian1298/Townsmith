extends Node3D

## Streamed-world placement preview.
##
## Fly over the procedural terrain, aim the ghost 24 m ahead, validate the
## rotated footprint, place a saved blueprint, and save/load the placement
## layer. The bridge owns validation and persistence; this node owns the ghost,
## placed meshes, HUD, and input.

const TerrainStreamer = preload("res://terrain_streamer.gd")
const EditorCamera = preload("res://editor_camera.gd")
const MayorCamera = preload("res://mayor_camera.gd")
const WorldI18n = preload("res://world_i18n.gd")
const ContentI18n = preload("res://content_i18n.gd")

const SEED := 2026
const FIXTURE_X := 169.0
const FIXTURE_Z := 125.0
const DEFAULT_BLUEPRINT := "acceptance_house.tsbp"
const DEFAULT_SAVE := "world.json"
const STARTING_TREASURY := 100000.0
const LOAN_AMOUNT := 50000.0
# Each dashboard rebuild asks the bridge for the advisor plus city, economy,
# industry, utilities, services, construction, traffic, progression, and
# research summaries; traffic_state rebuilds the road graph. That is too much
# work per HUD update, so the visible panel is refreshed on this cadence.
const DASHBOARD_REFRESH_SECONDS := 0.5
# A construction tick runs at 4 Hz and ends with a full HUD rebuild (blueprint,
# selected placement/owner, advisor, city, economy, industry, utilities,
# services, construction, traffic, progression, and research bridge queries).
# Rebuilding that whole panel four times a second, on top of the input-driven
# refreshes, dominated the tick cost, so the tick-driven refresh is throttled
# through a dirty flag; direct input handlers still call `_update_hud()` now.
const HUD_REFRESH_SECONDS := 0.5
const CONSTRUCTION_STEP_SECONDS := 0.25
# Maximum construction steps consumed by one frame. A stalled frame's backlog
# is clamped to `CONSTRUCTION_STEP_SECONDS * CONSTRUCTION_MAX_STEPS` (2 s)
# explicitly; the clamp is the only dropped-time path.
const CONSTRUCTION_MAX_STEPS := 8
# The dashboard's District section queries one circular district around the
# selected building (F6 selection focus) or the Mayor camera target; this
# matches the 512 m aggregate district cell scale in
# docs/07 and stays well inside the bridge's 0..=1,000,000 m radius guard.
const DISTRICT_RADIUS_M := 256.0
# F7 district heatmap overlay: one read-only query per refresh around the same
# focus as the materialization layer. 512 m / 32 m rounds up to a 32 x 32 grid,
# which is exactly the 1,024-cell cap the renderer accepts. Cells whose active
# metric is zero are omitted (zero cells stay hidden), and each visible cell is
# a flat pooled box lifted 0.15 m above the sampled terrain height.
const HEATMAP_REFRESH_SECONDS := 0.5
const HEATMAP_RADIUS_M := 512.0
const HEATMAP_CELL_M := 32.0
const HEATMAP_MAX_CELLS := 1024
const HEATMAP_LIFT_M := 0.15
# Colorblind-safe, perceptually ordered ramp for the F7 overlay. The six stops
# run dark purple -> blue -> teal -> green -> yellow, and sRGB luminance
# (0.2126 R + 0.7152 G + 0.0722 B) rises monotonically from the first stop to
# the last, so low/high magnitude survives red-green deficiency, blue-yellow
# deficiency, and monochrome viewing instead of relying on a red/green hue
# pair. Alpha rises 0.62 -> 0.74 but stays below 0.75 so terrain and
# buildings remain readable under the overlay. The legend names Low -> High
# and the active metric, so the scale is also explicit in text.
const HEATMAP_RAMP_STOPS: Array[Color] = [
	Color(0.267, 0.004, 0.329, 0.62),
	Color(0.255, 0.267, 0.529, 0.644),
	Color(0.165, 0.471, 0.557, 0.668),
	Color(0.133, 0.659, 0.518, 0.692),
	Color(0.478, 0.820, 0.318, 0.716),
	Color(0.992, 0.906, 0.145, 0.74),
]
# F7 opens on population; Shift+F7 cycles these in order while the overlay is
# visible. Jobs and population normalise against their own integer peak;
# happiness uses the bridge's 0..1 fraction and normalises against the happiest
# positive cell, then displays its peak as a percentage.
const HEATMAP_METRICS := ["population", "jobs", "happiness"]
const HEATMAP_METRIC_KEYS := {
	"population": "base.ui.heatmap.metric.population",
	"jobs": "base.ui.heatmap.metric.jobs",
	"happiness": "base.ui.heatmap.metric.happiness",
}
# Materialization is a derived, read-only selection around the Mayor focus or
# selected building. Rebuild on toggle/selection change and at the same 0.5 s
# cadence as the dashboard so a panning camera follows without per-frame nodes.
# The core keeps the previous selection and applies 80 m in / 120 m out
# hysteresis, so the overlay is a stateful scheduler rather than a fresh query.
const MATERIALIZATION_REFRESH_SECONDS := 0.5
const MATERIALIZATION_INNER_RADIUS_M := 250.0
const MATERIALIZATION_OUTER_RADIUS_M := 350.0
const MATERIALIZATION_DETAILED_RADIUS_M := 120.0
# Added and updated markers fade in over this window; removed markers fade out
# over it before their node is freed. It is shorter than the refresh cadence so
# a slow camera leaves only a brief cross-fade between two selections.
const MATERIALIZATION_FADE_SECONDS := 0.25
# Safety deadline for a removed node: even if its alpha never reaches zero, it
# is freed after this many seconds.
const MATERIALIZATION_FADE_MAX_SECONDS := 1.0
# The bridge returns at most this many diff entries per step. The overlay
# applies at most this many node changes per refresh, including any fallback
# reconciliation left over from a truncated diff.
const MATERIALIZATION_CHANGE_BUDGET := 64
const MOVEMENT_REFRESH_SECONDS := 0.1
# Crowd LOD for the F4 movement markers. Routed citizens inside this radius use
# the full-size near box; farther citizens share a second pooled MultiMesh with
# a smaller box, stride-sampled from the id-sorted far set to a fixed budget.
# The split is visual only and never removes simulation state.
const MOVEMENT_NEAR_RADIUS_M := 120.0
const MOVEMENT_FAR_BUDGET := 500
const MOVEMENT_NEAR_HEIGHT := 1.7
const MOVEMENT_FAR_HEIGHT := 0.9
const MOVEMENT_NEAR_SIZE := Vector3(0.32, 1.7, 0.32)
const MOVEMENT_FAR_SIZE := Vector3(0.12, 0.9, 0.12)
# Follow pulls the Mayor target toward the selected home with an exponential
# ease. The epsilon is the arrival radius and also the pan detector: a larger
# target move between follow steps means the player panned and takes over.
const FOLLOW_SHARPNESS := 6.0
const FOLLOW_ARRIVE_EPS_M := 0.05
# Named-citizen lifecycle clock: one game day per 60 real seconds. Whole days
# accumulate in `_process` and are applied through `named_lifecycle_tick` only
# while a household population store is open. Headless runs (including the
# scene smoke) keep the clock paused so save/load checksums and benchmark
# baselines stay fixed; a script may drive `_tick_named_lifecycle(days)`.
const LIFECYCLE_SECONDS_PER_DAY := 60.0
# `named_lifecycle_tick` accepts at most 36,500 days (100 years) per call.
const LIFECYCLE_MAX_DAYS_PER_TICK := 36500
const TAX_POLICIES := [
	{"name": "low", "resident": 4.0, "business": 2.0},
	{"name": "standard", "resident": 8.0, "business": 4.0},
	{"name": "high", "resident": 20.0, "business": 8.0},
]
# Compact dashboard policy editing (docs/57 §5.3). The dashboard edits only the
# smallest-id policy containing the district centre, through the existing
# bridge CRUD, and always re-sends the full record read from
# `district_policies_state`. Defaults mirror the standalone policy editor so a
# default create is deterministic and comparable across both consumers.
const POLICY_DEFAULT_RADIUS_M := 64.0
const POLICY_DEFAULT_TAX := 1.0
const POLICY_DEFAULT_FUNDING := 1.0
const POLICY_TAX_STEP := 0.25
const POLICY_FUNDING_STEP := 0.25
const POLICY_MIN_TAX := 0.25
const POLICY_MAX_TAX := 4.0
const POLICY_MAX_FUNDING := 2.0
const POLICY_STEP_SNAP := 0.01

var core
var i18n
var streamer
var camera: Camera3D
var mayor_camera
var architect_camera
var mode := "mayor"
var placed_root: Node3D
var road_root: Node3D
var road_nodes: Dictionary = {}
var road_material: StandardMaterial3D
var road_mode := false
var road_start := Vector2i(-999999, -999999)
var traffic_root: Node3D
var traffic_cars: Dictionary = {}
var traffic_materials: Array[StandardMaterial3D] = []
var traffic_stats: Dictionary = {}
var lot_root: Node3D
var lot_nodes: Dictionary = {}
var lot_materials: Dictionary = {}
var lot_mode := false
var lot_start := Vector2i(-999999, -999999)
var settlement_root: Node3D
var settlement_nodes: Dictionary = {}
var settlement_material: StandardMaterial3D
var materialization_root: Node3D
var materialization_active_mesh: BoxMesh
var materialization_detailed_mesh: BoxMesh
var materialization_active_material: StandardMaterial3D
var materialization_detailed_material: StandardMaterial3D
var materialization_visible := false
var materialization_refresh_accum := 0.0
var materialization_selected_key := -1
var materialization_counts := Vector2i.ZERO
# Live markers keyed by citizen id. Retained citizens keep their node across
# refreshes; only the bounded diff touches nodes.
var materialization_nodes: Dictionary = {}
# Removed markers finish their fade outside the id map, then are freed.
var materialization_fading: Array[MeshInstance3D] = []
var movement_root: Node3D
var movement_mesh: BoxMesh
var movement_material: StandardMaterial3D
var movement_far_mesh: BoxMesh
var movement_far_material: StandardMaterial3D
# One pooled MultiMesh holds every routed citizen marker; the transform buffer
# is rewritten in place each refresh instead of creating a node per citizen.
# A container keeps the near/far pools as one child of `CitizenMovement`, so
# the route mesh plus this container stay the only two direct children.
var movement_pool_root: Node3D
var movement_multimesh_instance: MultiMeshInstance3D
var movement_multimesh: MultiMesh
var movement_far_multimesh_instance: MultiMeshInstance3D
var movement_far_multimesh: MultiMesh
# Cached terrain height keyed by citizen id. Entries are dropped when the
# citizen leaves the drawn near/far set so the cache stays bounded by
# (near count + far budget).
var movement_ground: Dictionary = {}
# Citizen ids in MultiMesh instance order for the most recent refresh. Near is
# every routed citizen inside MOVEMENT_NEAR_RADIUS_M; far is the deterministic
# stride sample of the id-sorted set beyond it, capped at MOVEMENT_FAR_BUDGET.
var movement_ids: Array[int] = []
var movement_far_ids: Array[int] = []
var movement_far_checksum := 0
var movement_lod_counts := Vector2i.ZERO
# One merged line mesh for every routed household, rebuilt only when the
# bridge movement checksum changes.
var movement_route_lines: MeshInstance3D
var movement_route_material: StandardMaterial3D
var movement_route_key := ""
var movement_accum := 0.0
var movement_time := 0.0
var last_movement_state: Dictionary = {}
# F7 district heatmap overlay. One MultiMesh holds up to HEATMAP_MAX_CELLS flat
# boxes; instance_count only grows, while visible_instance_count tracks the
# current nonzero-metric cell count. The ground cache is keyed by rounded
# cell centre and invalidated by the terrain-delta checksum. The rebuild key
# includes the active metric so Shift+F7 forces an immediate redraw.
var heatmap_root: Node3D
var heatmap_mesh: BoxMesh
var heatmap_material: StandardMaterial3D
var heatmap_multimesh_instance: MultiMeshInstance3D
var heatmap_multimesh: MultiMesh
var heatmap_visible := false
var heatmap_metric := "population"
var heatmap_refresh_accum := 0.0
var heatmap_ground: Dictionary = {}
var heatmap_terrain_key := ""
var heatmap_rebuild_key := ""
var heatmap_last_state: Dictionary = {}
var heatmap_cells := 0
# Colours written to the pooled instances; headless MultiMesh reads back black,
# so smoke checks this CPU-side mirror instead.
var heatmap_colors: Array[Color] = []
var heatmap_peak := 0.0
var heatmap_error := ""
var industry_root: Node3D
var industry_nodes: Dictionary = {}
# Per-frame animation state for the industry spinners, rebuilt with the nodes
# (F-070). Reading `base_y`/`spinner` back through `get_meta` and `get_child(0)`
# every frame was per-node Dictionary churn for values that never change; the
# cached list is the same data with direct references.
var industry_animated: Array[Dictionary] = []
var industry_kinds: Dictionary = {}
var industry_time := 0.0
var utility_root: Node3D
var utility_nodes: Dictionary = {}
var utility_kinds: Dictionary = {}
var utility_time := 0.0
var service_root: Node3D
var service_nodes: Dictionary = {}
var service_kinds: Dictionary = {}
var service_time := 0.0
var lot_zone := "residential"
var selection_box: MeshInstance3D
var selected_id := -1
# District focus follows the selected building when true (F6); the default is
# the Mayor camera target. Clearing the selection resets this to camera focus.
var district_focus_selection := false
# Dashboard settlement selector. A non-negative id is an explicit override:
# the 256 m District query, the Policies containment point, and the F7 heatmap
# use that settlement's centre ahead of F6 building focus and the camera.
# PageUp/PageDown cycle the id-sorted list through a leading "none" entry, so
# the override is always clearable. The id is runtime UI state: it is not saved,
# survives dashboard rebuilds, and is revalidated after a world load (a removed
# or foreign id falls back to no settlement and the camera/selection focus).
var selected_settlement_id := -1
var follow_active := false
var follow_target := Vector3.ZERO
var follow_prev_target := Vector3.ZERO

var ghost: Node3D
var ghost_mesh: MeshInstance3D
var ghost_foundation: MeshInstance3D
var ghost_material: StandardMaterial3D
var placed_material: StandardMaterial3D
var foundation_material: StandardMaterial3D
var construction_material: StandardMaterial3D

var hud: Label
var status_label: Label
var modal_label: Label
var dashboard_panel: PanelContainer
var dashboard_text: RichTextLabel
var dashboard_visible := false
var dashboard_title: Label
var dashboard_hint: Label
var dashboard_policy_hint: Label
var dashboard_refresh_accum := 0.0
var citizen_panel: PanelContainer
var citizen_text: RichTextLabel
var citizen_visible := false
var citizen_refresh_accum := 0.0
# Lifecycle clock state. The last result is kept so the status block can show
# the most recent tick without re-running it; the totals are presentation only.
var lifecycle_clock_enabled := true
var lifecycle_accum_days := 0.0
var lifecycle_days_total := 0
var lifecycle_deaths_total := 0
var lifecycle_last_error := ""
var lifecycle_last_result: Dictionary = {}

# Opt-in construction clock (D-029, Proposed). The bridge store is the source
# of truth; these fields cache the last successful read. `campaign_clock_days`
# is presentation only: it accumulates construction-tick seconds divided by
# the campaign rate and resets when campaign mode is entered.
const CAMPAIGN_SECONDS_PER_DAY := 60.0
var construction_clock_mode := "developer_seconds"
var construction_clock_seconds_per_day := 0.0
var construction_clock_checksum := ""
var campaign_clock_days := 0.0
var construction_clock_last_error := ""

var blueprint_path := ""
var save_path := ""
var starter_index := -1
var starter_ids: Array = []
var blueprint_mesh: ArrayMesh
var blueprint_info: Dictionary = {}
var blueprint_info_cache: Dictionary = {}
var mesh_cache: Dictionary = {}
var placed_nodes: Dictionary = {}
var construction_nodes: Dictionary = {}
var construction_tick_accum := 0.0
var hud_refresh_accum := 0.0
var hud_dirty := false
var first_click_guard := true

var rotation_deg := 0
var target_x := FIXTURE_X
var target_z := FIXTURE_Z
var last_cell := Vector2i(-999999, -999999)
var preview_validation: Dictionary = {}
var preview_valid := false
var preview_error := ""

func _ready() -> void:
	lifecycle_clock_enabled = not OS.get_cmdline_user_args().has("--scene-smoke") and DisplayServer.get_name() != "headless"
	i18n = WorldI18n.new()
	core = TownsmithCore.new()
	_refresh_construction_clock_state()
	_resolve_paths()
	_setup_world()
	_setup_ui()
	_setup_ghost()
	_setup_selection()
	_setup_roads()
	_setup_traffic()
	_setup_lots()
	_setup_settlements()
	_setup_industry()
	_setup_utilities()
	_setup_services()
	_setup_materialization()
	_setup_heatmap()
	if _ensure_blueprint():
		_refresh_preview(true)
	else:
		_set_modal(i18n.t("base.ui.modal.blueprint_failed", {"path": blueprint_path}), true)
	_update_hud()
	if OS.get_cmdline_user_args().has("--scene-smoke"):
		_run_scene_smoke()
		return

	if DisplayServer.get_name() != "headless":
		Input.mouse_mode = Input.MOUSE_MODE_VISIBLE

func _process(delta: float) -> void:
	if dashboard_visible:
		dashboard_refresh_accum += delta
		if dashboard_refresh_accum >= DASHBOARD_REFRESH_SECONDS:
			dashboard_refresh_accum = 0.0
			_update_dashboard()
	else:
		dashboard_refresh_accum = 0.0
	if materialization_visible:
		if selected_id != materialization_selected_key:
			_rebuild_materialization()
		materialization_refresh_accum += delta
		if materialization_refresh_accum >= MATERIALIZATION_REFRESH_SECONDS:
			materialization_refresh_accum = 0.0
			_step_materialization()
		movement_accum += delta
		movement_time += delta
		if movement_accum >= MOVEMENT_REFRESH_SECONDS:
			movement_accum = 0.0
			_update_movement_markers()
		_advance_materialization_fades(delta)
	else:
		materialization_refresh_accum = 0.0
		movement_accum = 0.0
	if citizen_visible:
		citizen_refresh_accum += delta
		if citizen_refresh_accum >= DASHBOARD_REFRESH_SECONDS:
			citizen_refresh_accum = 0.0
			_update_citizen_panel()
	else:
		citizen_refresh_accum = 0.0
	if heatmap_visible:
		heatmap_refresh_accum += delta
		if heatmap_refresh_accum >= HEATMAP_REFRESH_SECONDS:
			heatmap_refresh_accum = 0.0
			_refresh_heatmap()
	else:
		heatmap_refresh_accum = 0.0
	if lifecycle_clock_enabled:
		_advance_lifecycle_clock(delta)
	construction_tick_accum += delta
	# A long frame stall must not silently vanish into `fmod`. Clamp the backlog
	# to exactly what one frame can consume, then step it; the remainder of a
	# pathological stall is dropped by this explicit clamp, not by truncation.
	if construction_tick_accum > CONSTRUCTION_STEP_SECONDS * CONSTRUCTION_MAX_STEPS:
		construction_tick_accum = CONSTRUCTION_STEP_SECONDS * CONSTRUCTION_MAX_STEPS
	var steps := 0
	while construction_tick_accum >= CONSTRUCTION_STEP_SECONDS and steps < CONSTRUCTION_MAX_STEPS:
		_advance_construction(CONSTRUCTION_STEP_SECONDS, true)
		construction_tick_accum -= CONSTRUCTION_STEP_SECONDS
		steps += 1
	hud_refresh_accum = minf(hud_refresh_accum + delta, HUD_REFRESH_SECONDS)
	if hud_dirty and hud_refresh_accum >= HUD_REFRESH_SECONDS:
		_update_hud()
	industry_time += delta
	var industry_bob := sin(industry_time * 2.0) * 0.15
	var worker_bob := 0.8 + absf(sin(industry_time * 3.0)) * 0.35
	for entry in industry_animated:
		var pivot: Node3D = entry.get("pivot")
		if not is_instance_valid(pivot):
			continue
		pivot.position.y = float(entry.get("base_y", 0.0)) + industry_bob
		var spinner: Node3D = entry.get("spinner")
		if is_instance_valid(spinner):
			spinner.rotation.y += delta * 2.4
			var worker: Node3D = entry.get("worker")
			if is_instance_valid(worker):
				worker.position.y = worker_bob
	utility_time += delta
	for node in utility_nodes.values():
		if is_instance_valid(node):
			var base_y := float(node.get_meta("base_y", 0.0))
			node.position.y = base_y + sin(utility_time * 1.6) * 0.08
	service_time += delta
	for node in service_nodes.values():
		if is_instance_valid(node):
			var base_y := float(node.get_meta("base_y", 0.0))
			node.position.y = base_y + sin(service_time * 1.4) * 0.06
	_update_traffic(delta)
	if mode == "mayor" and mayor_camera != null:
		_update_follow(delta)
		var ground: float = core.world_height_at(
			SEED, mayor_camera.target.x, mayor_camera.target.z
		)
		if ground >= 0.0:
			mayor_camera.target.y = ground
		mayor_camera.refresh()
	if blueprint_mesh == null or not is_instance_valid(camera):
		return
	if mode == "architect":
		return
	var target: Vector3 = mayor_camera.target
	var cell := Vector2i(floori(target.x), floori(target.z))
	if cell == last_cell:
		return
	last_cell = cell
	target_x = float(cell.x)
	target_z = float(cell.y)
	_refresh_preview(false)

func _unhandled_input(event: InputEvent) -> void:
	if event is InputEventKey and event.pressed and not event.echo:
		if event.physical_keycode == KEY_M and mode == "architect":
			_return_to_mayor()
			return
		if event.physical_keycode == KEY_ESCAPE:
			if follow_active:
				_stop_follow(i18n.t("base.ui.citizen.follow_stopped"))
			elif citizen_visible:
				_close_citizen_panel()
			elif dashboard_visible:
				_toggle_dashboard()
			elif mode == "architect":
				_return_to_mayor()
			elif lot_mode:
				if lot_start.x != -999999:
					lot_start = Vector2i(-999999, -999999)
					_set_modal(i18n.t("base.ui.modal.lot_corner_cleared"), false)
				else:
					lot_mode = false
					_set_modal("", false)
				_update_hud()
			elif road_mode:
				road_mode = false
				road_start = Vector2i(-999999, -999999)
				_set_modal("", false)
				_update_hud()
			else:
				_set_modal("", false)
				Input.mouse_mode = Input.MOUSE_MODE_VISIBLE
				_update_hud()
			return
		if mode == "mayor":
			match event.physical_keycode:
				KEY_R:
					rotation_deg = (rotation_deg + 90) % 360
					_refresh_preview(true)
				KEY_B:
					_cycle_starter()
				KEY_X:
					_edit_target(1)
				KEY_Z:
					_edit_target(-1)
				KEY_C:
					_toggle_crew()
				KEY_I:
					_cycle_industry()
				KEY_U:
					_cycle_utility()
				KEY_K:
					_cycle_service()
				KEY_F1:
					_toggle_dashboard()
				KEY_F2:
					_toggle_citizen_panel()
				KEY_F3:
					_toggle_follow()
				KEY_F4:
					_toggle_materialization()
				KEY_F6:
					_toggle_district_focus()
				KEY_F7:
					if event.shift_pressed:
						_cycle_heatmap_metric()
					else:
						_toggle_heatmap()
				KEY_F8:
					_toggle_construction_clock()
				KEY_P:
					if dashboard_visible:
						if event.shift_pressed:
							_policy_remove_selected()
						else:
							_policy_create_default()
				KEY_J:
					if dashboard_visible:
						_policy_toggle_construction()
				KEY_BRACKETLEFT:
					if dashboard_visible:
						_policy_adjust_tax(-POLICY_TAX_STEP)
				KEY_BRACKETRIGHT:
					if dashboard_visible:
						_policy_adjust_tax(POLICY_TAX_STEP)
				KEY_COMMA:
					if dashboard_visible:
						_policy_adjust_funding(-POLICY_FUNDING_STEP)
				KEY_PERIOD:
					if dashboard_visible:
						_policy_adjust_funding(POLICY_FUNDING_STEP)
				KEY_PAGEUP:
					if dashboard_visible:
						_cycle_settlement(-1)
				KEY_PAGEDOWN:
					if dashboard_visible:
						_cycle_settlement(1)
				KEY_T:
					if event.shift_pressed:
						if dashboard_visible and selected_settlement_id >= 0:
							_cycle_selected_settlement_tax()
					else:
						_cycle_tax_policy()
				KEY_L:
					_borrow()
				KEY_Y:
					_research_next()
				KEY_V:
					core.construction_add_materials(10000)
					_update_hud()
				KEY_DELETE:
					_clear_placements()
				KEY_G:
					_toggle_road_mode()
				KEY_H:
					_toggle_lot_mode()
				KEY_N:
					if event.shift_pressed:
						_clear_settlements()
					else:
						_found_settlement()
				KEY_1:
					if lot_mode:
						_set_lot_zone("residential")
				KEY_2:
					if lot_mode:
						_set_lot_zone("commercial")
				KEY_3:
					if lot_mode:
						_set_lot_zone("industrial")
				KEY_BACKSPACE:
					if lot_mode:
						core.lot_clear()
						_rebuild_lots()
					else:
						core.road_clear()
						_rebuild_roads()
					_update_hud()
				KEY_ENTER, KEY_KP_ENTER:
					_enter_architect()
				KEY_F5:
					_save_world()
				KEY_F9:
					_load_world()
		else:
			match event.physical_keycode:
				KEY_F5:
					_save_world()
				KEY_F9:
					_load_world()
	elif event is InputEventMouseButton and event.pressed:
		if mode == "mayor" and event.button_index == MOUSE_BUTTON_RIGHT:
			_select_at_screen(event.position)
		elif mode == "mayor" and event.button_index == MOUSE_BUTTON_LEFT:
			if first_click_guard:
				first_click_guard = false
				return
			if lot_mode:
				_lot_click(event.position)
			elif road_mode:
				_road_click(event.position)
			else:
				_place_at_target()
		elif mode == "architect" and event.button_index == MOUSE_BUTTON_LEFT:
			if Input.mouse_mode != Input.MOUSE_MODE_CAPTURED:
				Input.mouse_mode = Input.MOUSE_MODE_CAPTURED

func _resolve_paths() -> void:
	var user_dir := OS.get_user_data_dir().path_join("world_placement")
	if OS.get_cmdline_user_args().has("--scene-smoke"):
		user_dir = OS.get_user_data_dir().path_join("world_scene_smoke")
	DirAccess.make_dir_recursive_absolute(user_dir)
	if OS.get_cmdline_user_args().has("--scene-smoke"):
		# Keep the smoke deterministic across local reruns.
		for name in [DEFAULT_BLUEPRINT, DEFAULT_SAVE]:
			var stale := user_dir.path_join(name)
			if FileAccess.file_exists(stale):
				DirAccess.remove_absolute(stale)
	save_path = _arg_value("--world-save")
	if save_path == "":
		save_path = user_dir.path_join(DEFAULT_SAVE)
	else:
		save_path = _absolute_arg(save_path)
	var save_dir := save_path.get_base_dir()
	if save_dir != "":
		DirAccess.make_dir_recursive_absolute(save_dir)
	blueprint_path = _arg_value("--blueprint")
	if blueprint_path == "":
		blueprint_path = user_dir.path_join(DEFAULT_BLUEPRINT)
	else:
		blueprint_path = _absolute_arg(blueprint_path)

func _arg_value(name: String) -> String:
	var args := OS.get_cmdline_user_args()
	for index in range(args.size()):
		if args[index] == name and index + 1 < args.size():
			return args[index + 1]
		var prefix := name + "="
		if args[index].begins_with(prefix):
			return args[index].substr(prefix.length())
	return ""

func _absolute_arg(value: String) -> String:
	if value.begins_with("res://") or value.begins_with("user://"):
		return ProjectSettings.globalize_path(value)
	return value

func _setup_world() -> void:
	streamer = TerrainStreamer.new()
	streamer.seed = SEED
	streamer.view_radius = 1
	streamer.keep_radius = 2
	streamer.builds_per_frame = 2
	streamer.core = core
	add_child(streamer)

	mayor_camera = MayorCamera.new()
	add_child(mayor_camera)
	mayor_camera.focus(Vector3(FIXTURE_X, 41.0, FIXTURE_Z), 90.0)
	mayor_camera.current = true
	camera = mayor_camera

	architect_camera = EditorCamera.new()
	architect_camera.position = Vector3(FIXTURE_X, 43.0, FIXTURE_Z + 12.0)
	architect_camera.far = 1500.0
	architect_camera.move_speed_mps = 8.0
	add_child(architect_camera)
	architect_camera.look_at(Vector3(FIXTURE_X, 42.0, FIXTURE_Z))
	architect_camera.sync_look_from_rotation()
	architect_camera.current = false
	streamer.bind_camera(camera)

	var light := DirectionalLight3D.new()
	light.rotation_degrees = Vector3(-50.0, -30.0, 0.0)
	light.light_energy = 1.2
	add_child(light)

	var world_environment := WorldEnvironment.new()
	var environment := Environment.new()
	environment.background_mode = Environment.BG_COLOR
	environment.background_color = Color(0.55, 0.72, 0.9)
	environment.ambient_light_source = Environment.AMBIENT_SOURCE_COLOR
	environment.ambient_light_color = Color(0.75, 0.8, 0.9)
	environment.ambient_light_energy = 0.55
	world_environment.environment = environment
	add_child(world_environment)

	placed_root = Node3D.new()
	placed_root.name = "Placed"
	add_child(placed_root)

func _setup_ui() -> void:
	var layer := CanvasLayer.new()
	add_child(layer)
	var panel := PanelContainer.new()
	panel.position = Vector2(16.0, 16.0)
	panel.custom_minimum_size = Vector2(620.0, 0.0)
	layer.add_child(panel)
	var box := VBoxContainer.new()
	box.add_theme_constant_override("separation", 4)
	panel.add_child(box)
	hud = Label.new()
	hud.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	box.add_child(hud)
	status_label = Label.new()
	status_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	box.add_child(status_label)
	modal_label = Label.new()
	modal_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	modal_label.add_theme_color_override("font_color", Color(1.0, 0.55, 0.35))
	modal_label.visible = false
	box.add_child(modal_label)

	dashboard_panel = PanelContainer.new()
	dashboard_panel.name = "Dashboard"
	dashboard_panel.anchor_left = 1.0
	dashboard_panel.anchor_right = 1.0
	dashboard_panel.anchor_top = 0.0
	dashboard_panel.anchor_bottom = 1.0
	dashboard_panel.offset_left = -470.0
	dashboard_panel.offset_right = -16.0
	dashboard_panel.offset_top = 16.0
	dashboard_panel.offset_bottom = -16.0
	dashboard_panel.visible = false
	layer.add_child(dashboard_panel)
	var dashboard_box := VBoxContainer.new()
	dashboard_box.add_theme_constant_override("separation", 6)
	dashboard_panel.add_child(dashboard_box)
	dashboard_title = Label.new()
	dashboard_title.text = i18n.t("base.ui.dashboard.title")
	dashboard_title.add_theme_font_size_override("font_size", 20)
	dashboard_box.add_child(dashboard_title)
	dashboard_text = RichTextLabel.new()
	dashboard_text.name = "DashboardText"
	dashboard_text.bbcode_enabled = true
	dashboard_text.fit_content = false
	dashboard_text.scroll_active = true
	dashboard_text.size_flags_vertical = Control.SIZE_EXPAND_FILL
	dashboard_text.custom_minimum_size = Vector2(0.0, 420.0)
	dashboard_box.add_child(dashboard_text)
	dashboard_hint = Label.new()
	dashboard_hint.text = i18n.t("base.ui.dashboard.close_hint")
	dashboard_hint.add_theme_color_override("font_color", Color(0.7, 0.7, 0.7))
	dashboard_box.add_child(dashboard_hint)
	dashboard_policy_hint = Label.new()
	dashboard_policy_hint.text = i18n.t("base.ui.dashboard.policy_hint")
	dashboard_policy_hint.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	dashboard_policy_hint.add_theme_color_override("font_color", Color(0.62, 0.78, 0.66))
	dashboard_box.add_child(dashboard_policy_hint)

	citizen_panel = PanelContainer.new()
	citizen_panel.name = "CitizenPanel"
	citizen_panel.anchor_left = 0.0
	citizen_panel.anchor_right = 0.0
	citizen_panel.anchor_top = 1.0
	citizen_panel.anchor_bottom = 1.0
	citizen_panel.offset_left = 16.0
	citizen_panel.offset_right = 436.0
	citizen_panel.offset_top = -320.0
	citizen_panel.offset_bottom = -16.0
	citizen_panel.visible = false
	layer.add_child(citizen_panel)
	citizen_text = RichTextLabel.new()
	citizen_text.name = "CitizenText"
	citizen_text.bbcode_enabled = true
	citizen_text.fit_content = false
	citizen_text.scroll_active = true
	citizen_text.size_flags_vertical = Control.SIZE_EXPAND_FILL
	citizen_text.custom_minimum_size = Vector2(380.0, 280.0)
	citizen_panel.add_child(citizen_text)

func _setup_ghost() -> void:
	ghost = Node3D.new()
	ghost.name = "PlacementGhost"
	ghost.visible = false
	add_child(ghost)

	ghost_material = StandardMaterial3D.new()
	ghost_material.transparency = BaseMaterial3D.TRANSPARENCY_ALPHA
	ghost_material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	ghost_material.cull_mode = BaseMaterial3D.CULL_DISABLED
	ghost_material.albedo_color = Color(0.2, 0.9, 0.4, 0.35)

	ghost_mesh = MeshInstance3D.new()
	ghost_mesh.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	ghost_mesh.material_override = ghost_material
	ghost.add_child(ghost_mesh)

	ghost_foundation = MeshInstance3D.new()
	ghost_foundation.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	ghost_foundation.visible = false
	var ghost_foundation_material := StandardMaterial3D.new()
	ghost_foundation_material.transparency = BaseMaterial3D.TRANSPARENCY_ALPHA
	ghost_foundation_material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	ghost_foundation_material.albedo_color = Color(0.3, 0.55, 0.3, 0.25)
	ghost_foundation.material_override = ghost_foundation_material
	ghost.add_child(ghost_foundation)

	placed_material = StandardMaterial3D.new()
	placed_material.albedo_color = Color(1.0, 1.0, 1.0)
	placed_material.vertex_color_use_as_albedo = true
	placed_material.roughness = 1.0
	placed_material.metallic = 0.0
	foundation_material = StandardMaterial3D.new()
	foundation_material.albedo_color = Color(0.45, 0.45, 0.42)
	foundation_material.roughness = 1.0
	foundation_material.metallic = 0.0
	construction_material = StandardMaterial3D.new()
	construction_material.transparency = BaseMaterial3D.TRANSPARENCY_ALPHA
	construction_material.albedo_color = Color(0.95, 0.62, 0.25, 0.55)
	construction_material.roughness = 1.0
	construction_material.cull_mode = BaseMaterial3D.CULL_DISABLED

func _setup_selection() -> void:
	selection_box = MeshInstance3D.new()
	selection_box.name = "Selection"
	selection_box.visible = false
	selection_box.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	var material := StandardMaterial3D.new()
	material.transparency = BaseMaterial3D.TRANSPARENCY_ALPHA
	material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	material.cull_mode = BaseMaterial3D.CULL_DISABLED
	material.albedo_color = Color(1.0, 0.82, 0.2, 0.22)
	selection_box.material_override = material
	add_child(selection_box)

func _setup_roads() -> void:
	road_root = Node3D.new()
	road_root.name = "Roads"
	add_child(road_root)
	road_material = StandardMaterial3D.new()
	road_material.albedo_color = Color(0.22, 0.23, 0.24)
	road_material.roughness = 1.0
	road_material.metallic = 0.0

func _setup_traffic() -> void:
	traffic_root = Node3D.new()
	traffic_root.name = "Traffic"
	add_child(traffic_root)
	for color in [Color(0.85, 0.2, 0.2), Color(0.2, 0.5, 0.85), Color(0.95, 0.8, 0.2), Color(0.9, 0.9, 0.9)]:
		var material := StandardMaterial3D.new()
		material.albedo_color = color
		material.roughness = 0.6
		material.metallic = 0.1
		traffic_materials.append(material)

func _rebuild_traffic() -> void:
	for node in traffic_cars.values():
		if is_instance_valid(node):
			node.free()
	traffic_cars.clear()
	traffic_stats = core.traffic_state(SEED)
	var roads: Dictionary = core.roads()
	for road in roads.get("roads", []):
		_add_traffic_car(road)

func _add_traffic_car(road: Dictionary) -> void:
	var id := int(road.get("id", -1))
	if id < 0 or traffic_cars.has(id):
		return
	var start: PackedInt32Array = road.get("start", PackedInt32Array())
	var end: PackedInt32Array = road.get("end", PackedInt32Array())
	var center: PackedFloat32Array = road.get("center", PackedFloat32Array())
	var length := float(road.get("length_m", 0))
	var width := float(road.get("width_m", 8))
	if start.size() != 2 or end.size() != 2 or center.size() != 2 or length <= 0.0:
		return
	var ground: float = core.world_height_at(SEED, center[0], center[1])
	if ground < 0.0:
		return
	var along_x := bool(road.get("runs_along_x", false))
	var body := BoxMesh.new()
	body.size = Vector3(1.5, 0.55, 0.8)
	var car := MeshInstance3D.new()
	car.mesh = body
	car.material_override = traffic_materials[id % traffic_materials.size()]
	car.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	var lane := float((id % 3) - 1) * minf(width * 0.25, 2.0)
	car.set_meta("start", Vector2(float(start[0]), float(start[1])))
	car.set_meta("end", Vector2(float(end[0]), float(end[1])))
	car.set_meta("length", length)
	car.set_meta("speed", 6.0 + float(id % 5) * 1.3)
	car.set_meta("phase", float((id * 37) % 100) / 100.0)
	car.set_meta("reverse", id % 2 == 1)
	car.set_meta("lane", lane)
	car.set_meta("along_x", along_x)
	car.set_meta("ground", ground)
	car.set_meta("t", float(car.get_meta("phase")))
	car.rotation.y = 0.0 if along_x else PI * 0.5
	traffic_root.add_child(car)
	traffic_cars[id] = car
	_place_car(car)

func _place_car(car: MeshInstance3D) -> void:
	var start: Vector2 = car.get_meta("start")
	var end: Vector2 = car.get_meta("end")
	var t := float(car.get_meta("t"))
	var along := 1.0 - t if bool(car.get_meta("reverse")) else t
	var lane := float(car.get_meta("lane"))
	var point := start.lerp(end, along)
	if bool(car.get_meta("along_x")):
		point.y += lane
	else:
		point.x += lane
	car.position = Vector3(point.x, float(car.get_meta("ground")) + 0.5, point.y)

func _update_traffic(delta: float) -> void:
	if delta <= 0.0:
		return
	for car in traffic_cars.values():
		if not is_instance_valid(car):
			continue
		var length := float(car.get_meta("length", 1.0))
		if length <= 0.0:
			continue
		var speed := float(car.get_meta("speed", 6.0))
		var t := fmod(float(car.get_meta("t", 0.0)) + speed * delta / length, 1.0)
		car.set_meta("t", t)
		_place_car(car)

func _setup_lots() -> void:
	lot_root = Node3D.new()
	lot_root.name = "Lots"
	add_child(lot_root)
	var colors := {
		"residential": Color(0.25, 0.75, 0.35, 0.3),
		"commercial": Color(0.25, 0.5, 0.95, 0.3),
		"industrial": Color(0.95, 0.55, 0.2, 0.3),
	}
	for zone in colors:
		var material := StandardMaterial3D.new()
		material.transparency = BaseMaterial3D.TRANSPARENCY_ALPHA
		material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
		material.cull_mode = BaseMaterial3D.CULL_DISABLED
		material.albedo_color = colors[zone]
		lot_materials[zone] = material

func _toggle_lot_mode() -> void:
	lot_mode = not lot_mode
	lot_start = Vector2i(-999999, -999999)
	if lot_mode:
		road_mode = false
		road_start = Vector2i(-999999, -999999)
		_set_modal(i18n.t("base.ui.modal.lot_mode_help"), false)
	else:
		_set_modal("", false)
	_update_hud()

func _set_lot_zone(zone: String) -> void:
	if not lot_materials.has(zone):
		return
	lot_zone = zone
	_set_modal(i18n.t("base.ui.modal.lot_zone", {"zone": zone}), false)
	_update_hud()

func _lot_click(screen: Vector2) -> bool:
	var point := _road_ground_point(screen)
	var cell := Vector2i(roundi(point.x / 4.0) * 4, roundi(point.y / 4.0) * 4)
	if lot_start.x == -999999:
		lot_start = cell
		_set_modal(i18n.t("base.ui.modal.lot_corner", {"x": cell.x, "z": cell.y}), false)
		return true
	var min_x := mini(lot_start.x, cell.x)
	var min_z := mini(lot_start.y, cell.y)
	var max_x := maxi(lot_start.x, cell.x)
	var max_z := maxi(lot_start.y, cell.y)
	if max_x - min_x < 4 or max_z - min_z < 4:
		_set_modal(i18n.t("base.ui.modal.lot_min_size"), true)
		return false
	var ok := _add_lot(min_x, min_z, max_x, max_z, lot_zone)
	if ok:
		lot_start = Vector2i(-999999, -999999)
	return ok

func _add_lot(min_x: int, min_z: int, max_x: int, max_z: int, zone: String) -> bool:
	var result: Dictionary = core.lot_add(
		SEED, min_x, min_z, max_x, max_z, zone
	)
	if not bool(result.get("ok", false)):
		_set_modal(i18n.t("base.ui.modal.lot_rejected", {
			"code": str(result.get("error_code", "unknown")),
		}), true)
		return false
	_add_lot_node(result.get("lot", {}))
	_update_hud()
	return true

func _rebuild_lots() -> void:
	_clear_lot_nodes()
	var state: Dictionary = core.lots()
	for lot in state.get("lots", []):
		_add_lot_node(lot)

func _clear_lot_nodes() -> void:
	for node in lot_nodes.values():
		if is_instance_valid(node):
			node.free()
	lot_nodes.clear()

func _add_lot_node(lot: Dictionary) -> void:
	var id := int(lot.get("id", -1))
	if id < 0 or lot_nodes.has(id):
		return
	var bounds: PackedInt32Array = lot.get("bounds", PackedInt32Array())
	var center: PackedFloat32Array = lot.get("center", PackedFloat32Array())
	if bounds.size() != 4 or center.size() != 2:
		return
	var ground: float = core.world_height_at(SEED, center[0], center[1])
	if ground < 0.0:
		return
	var box := BoxMesh.new()
	box.size = Vector3(
		float(bounds[2] - bounds[0]), 0.1, float(bounds[3] - bounds[1])
	)
	var node := MeshInstance3D.new()
	node.mesh = box
	node.material_override = lot_materials.get(str(lot.get("zone", "residential")))
	node.position = Vector3(center[0], ground + 0.06, center[1])
	node.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	lot_root.add_child(node)
	lot_nodes[id] = node

func _lot_node_matches(lot: Dictionary) -> bool:
	var id := int(lot.get("id", -1))
	var node: MeshInstance3D = lot_nodes.get(id)
	if not is_instance_valid(node) or not (node.mesh is BoxMesh):
		return false
	var box := node.mesh as BoxMesh
	var bounds: PackedInt32Array = lot.get("bounds", PackedInt32Array())
	var center: PackedFloat32Array = lot.get("center", PackedFloat32Array())
	if bounds.size() != 4 or center.size() != 2:
		return false
	var zone := str(lot.get("zone", "residential"))
	return (
		absf(box.size.x - float(bounds[2] - bounds[0])) < 0.001
		and absf(box.size.z - float(bounds[3] - bounds[1])) < 0.001
		and absf(node.position.x - center[0]) < 0.001
		and absf(node.position.z - center[1]) < 0.001
		and node.material_override == lot_materials.get(zone)
	)

func _setup_settlements() -> void:
	settlement_root = Node3D.new()
	settlement_root.name = "Settlements"
	add_child(settlement_root)
	settlement_material = StandardMaterial3D.new()
	settlement_material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	settlement_material.cull_mode = BaseMaterial3D.CULL_DISABLED
	settlement_material.albedo_color = Color(0.95, 0.78, 0.2, 0.95)

func _setup_industry() -> void:
	industry_root = Node3D.new()
	industry_root.name = "Industry"
	add_child(industry_root)

func _setup_materialization() -> void:
	materialization_root = Node3D.new()
	materialization_root.name = "Materialization"
	add_child(materialization_root)
	materialization_active_mesh = BoxMesh.new()
	materialization_active_mesh.size = Vector3(0.45, 2.2, 0.45)
	materialization_detailed_mesh = BoxMesh.new()
	materialization_detailed_mesh.size = Vector3(0.9, 3.0, 0.9)
	materialization_active_material = StandardMaterial3D.new()
	materialization_active_material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	materialization_active_material.albedo_color = Color(0.25, 0.55, 1.0)
	materialization_detailed_material = StandardMaterial3D.new()
	materialization_detailed_material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	materialization_detailed_material.albedo_color = Color(1.0, 0.64, 0.18)
	movement_root = Node3D.new()
	movement_root.name = "CitizenMovement"
	add_child(movement_root)
	movement_mesh = BoxMesh.new()
	movement_mesh.size = MOVEMENT_NEAR_SIZE
	movement_material = StandardMaterial3D.new()
	movement_material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	movement_material.albedo_color = Color(0.18, 0.95, 0.72)
	# Far markers are a smaller box, so the pooled sample reads as a crowd
	# without paying for a full-height agent at 120 m+.
	movement_far_mesh = BoxMesh.new()
	movement_far_mesh.size = MOVEMENT_FAR_SIZE
	movement_far_material = StandardMaterial3D.new()
	movement_far_material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	movement_far_material.albedo_color = Color(0.14, 0.66, 0.5)
	movement_route_material = StandardMaterial3D.new()
	movement_route_material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	movement_route_material.albedo_color = Color(0.16, 0.62, 0.58, 0.75)
	movement_route_material.transparency = BaseMaterial3D.TRANSPARENCY_ALPHA

func _setup_movement_pool() -> void:
	# Created lazily on the first marker refresh so a hidden overlay owns no
	# child nodes under `CitizenMovement`. The container keeps both pools behind
	# one direct child, alongside the merged route mesh.
	if movement_root == null:
		return
	if movement_multimesh_instance != null and is_instance_valid(movement_multimesh_instance):
		if movement_far_multimesh_instance != null and is_instance_valid(movement_far_multimesh_instance):
			return
	if movement_pool_root == null or not is_instance_valid(movement_pool_root):
		movement_pool_root = Node3D.new()
		movement_pool_root.name = "CitizenMovementPools"
		movement_root.add_child(movement_pool_root)
	if movement_multimesh_instance == null or not is_instance_valid(movement_multimesh_instance):
		movement_multimesh = MultiMesh.new()
		movement_multimesh.mesh = movement_mesh
		# transform_format and use_colors must be set before instance_count; Godot
		# rejects both once instances exist.
		movement_multimesh.transform_format = MultiMesh.TRANSFORM_3D
		movement_multimesh.use_colors = false
		movement_multimesh.instance_count = 0
		movement_multimesh.visible_instance_count = 0
		movement_multimesh_instance = MultiMeshInstance3D.new()
		movement_multimesh_instance.name = "CitizenMovementNear"
		movement_multimesh_instance.multimesh = movement_multimesh
		movement_multimesh_instance.material_override = movement_material
		movement_multimesh_instance.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
		movement_pool_root.add_child(movement_multimesh_instance)
	if movement_far_multimesh_instance == null or not is_instance_valid(movement_far_multimesh_instance):
		movement_far_multimesh = MultiMesh.new()
		movement_far_multimesh.mesh = movement_far_mesh
		movement_far_multimesh.transform_format = MultiMesh.TRANSFORM_3D
		movement_far_multimesh.use_colors = false
		movement_far_multimesh.instance_count = 0
		movement_far_multimesh.visible_instance_count = 0
		movement_far_multimesh_instance = MultiMeshInstance3D.new()
		movement_far_multimesh_instance.name = "CitizenMovementFar"
		movement_far_multimesh_instance.multimesh = movement_far_multimesh
		movement_far_multimesh_instance.material_override = movement_far_material
		movement_far_multimesh_instance.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
		movement_pool_root.add_child(movement_far_multimesh_instance)

func _setup_heatmap() -> void:
	# The root exists even while the overlay is off so F7 can create the pool
	# lazily; a hidden overlay owns no child nodes or instance buffers.
	heatmap_root = Node3D.new()
	heatmap_root.name = "DistrictHeatmap"
	add_child(heatmap_root)
	heatmap_mesh = BoxMesh.new()
	heatmap_mesh.size = Vector3(HEATMAP_CELL_M * 0.94, 0.05, HEATMAP_CELL_M * 0.94)
	heatmap_material = StandardMaterial3D.new()
	heatmap_material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	heatmap_material.transparency = BaseMaterial3D.TRANSPARENCY_ALPHA
	heatmap_material.vertex_color_use_as_albedo = true
	heatmap_material.cull_mode = BaseMaterial3D.CULL_DISABLED

func _ensure_heatmap_pool() -> void:
	if heatmap_root == null:
		return
	if heatmap_multimesh_instance != null and is_instance_valid(heatmap_multimesh_instance):
		return
	heatmap_multimesh = MultiMesh.new()
	heatmap_multimesh.mesh = heatmap_mesh
	# transform_format and use_colors must be set before instance_count; Godot
	# rejects both once instances exist.
	heatmap_multimesh.transform_format = MultiMesh.TRANSFORM_3D
	heatmap_multimesh.use_colors = true
	heatmap_multimesh.instance_count = 0
	heatmap_multimesh.visible_instance_count = 0
	heatmap_multimesh_instance = MultiMeshInstance3D.new()
	heatmap_multimesh_instance.name = "DistrictHeatmapPool"
	heatmap_multimesh_instance.multimesh = heatmap_multimesh
	heatmap_multimesh_instance.material_override = heatmap_material
	heatmap_multimesh_instance.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	heatmap_root.add_child(heatmap_multimesh_instance)

func _heatmap_color(ratio: float) -> Color:
	# Interpolate the ordered ramp by position, never by hue alone: dark purple
	# is the low end, bright yellow is the high end, and each stop adds light.
	# The peak cell is therefore the brightest, not merely the "reddest".
	var value := clampf(ratio, 0.0, 1.0)
	var last := HEATMAP_RAMP_STOPS.size() - 1
	var scaled := value * float(last)
	var index := clampi(floori(scaled), 0, last - 1)
	return HEATMAP_RAMP_STOPS[index].lerp(
		HEATMAP_RAMP_STOPS[index + 1], scaled - float(index))

## sRGB luminance of a ramp colour. The stops are chosen so this value never
## decreases as `ratio` rises, which is what keeps the ramp readable without
## red/green hue discrimination.
func _heatmap_ramp_luminance(color: Color) -> float:
	return 0.2126 * color.r + 0.7152 * color.g + 0.0722 * color.b

func _heatmap_metric_value(cell: Dictionary) -> float:
	if heatmap_metric == "jobs":
		return float(int(cell.get("jobs", 0)))
	if heatmap_metric == "happiness":
		var happiness := float(cell.get("happiness", 0.0))
		if not is_finite(happiness):
			return 0.0
		return clampf(happiness, 0.0, 1.0)
	return float(int(cell.get("population", 0)))

func _heatmap_peak_value(cells: Array) -> float:
	var peak := 0.0
	for cell in cells:
		peak = maxf(peak, _heatmap_metric_value(cell))
	return peak

func _heatmap_peak_text() -> String:
	# Happiness is a 0..1 fraction, so its legend shows a percentage. An
	# empty metric renders the shared not-available dash instead of a
	# misleading zero.
	if heatmap_peak <= 0.0:
		return i18n.NOT_AVAILABLE if heatmap_metric == "happiness" else i18n.format_int(0)
	if heatmap_metric == "happiness":
		return i18n.format_float(heatmap_peak * 100.0, 0) + "%"
	return i18n.format_int(roundi(heatmap_peak))

func _cycle_heatmap_metric() -> String:
	# Shift+F7 only cycles while the overlay is visible; a hidden overlay
	# keeps its selected metric for the next F7 open.
	if not heatmap_visible:
		return heatmap_metric
	var index := HEATMAP_METRICS.find(heatmap_metric)
	heatmap_metric = HEATMAP_METRICS[(index + 1) % HEATMAP_METRICS.size()]
	heatmap_refresh_accum = 0.0
	_refresh_heatmap()
	_update_hud()
	return heatmap_metric

func _heatmap_ground_at(x: float, z: float) -> float:
	var key := Vector2i(roundi(x), roundi(z))
	if heatmap_ground.has(key):
		return float(heatmap_ground[key])
	# F-070: this dictionary is the heatmap's only per-cell allocation and it is
	# already bounded: at most 2 * HEATMAP_MAX_CELLS rounded cell centres, each
	# one Vector2i -> float entry, cleared wholesale when the terrain changes.
	# The overlay itself is a single MultiMesh, so no per-frame node churn.
	if heatmap_ground.size() >= HEATMAP_MAX_CELLS * 2:
		heatmap_ground.clear()
	var ground: float = core.world_height_at(SEED, x, z)
	if ground >= 0.0:
		heatmap_ground[key] = ground
	return ground

func _clear_heatmap() -> void:
	if heatmap_root != null:
		for child in heatmap_root.get_children():
			if is_instance_valid(child):
				child.free()
	heatmap_multimesh_instance = null
	heatmap_multimesh = null
	heatmap_ground.clear()
	heatmap_terrain_key = ""
	heatmap_rebuild_key = ""
	heatmap_last_state = {}
	heatmap_cells = 0
	heatmap_colors.clear()
	heatmap_peak = 0.0
	heatmap_error = ""

func _refresh_heatmap() -> void:
	if not heatmap_visible or core == null or heatmap_root == null:
		return
	var center := _heatmap_center_xz()
	var state: Dictionary = core.district_heatmap(
		SEED, center.x, center.y, HEATMAP_RADIUS_M, HEATMAP_CELL_M
	)
	heatmap_last_state = state
	heatmap_cells = 0
	heatmap_colors.clear()
	heatmap_peak = 0.0
	heatmap_error = ""
	# The key includes the active metric (plus the query centre), so a mode
	# change is a new layer rather than a stale redraw of the old colours.
	heatmap_rebuild_key = "%s|%.1f|%.1f" % [heatmap_metric, center.x, center.y]
	if not bool(state.get("ok", false)):
		heatmap_error = str(state.get("error_code", "unavailable"))
		_ensure_heatmap_pool()
		if heatmap_multimesh != null:
			heatmap_multimesh.visible_instance_count = 0
			heatmap_multimesh.custom_aabb = AABB()
		return
	var cells: Array = state.get("cells", [])
	var peak := _heatmap_peak_value(cells)
	heatmap_peak = peak
	# A terrain edit changes the sampled heights, so keep the cache honest
	# without querying the height field for every cell on every refresh.
	var terrain_key := str(core.terrain_delta_checksum())
	if terrain_key != heatmap_terrain_key:
		heatmap_terrain_key = terrain_key
		heatmap_ground.clear()
	var transforms: Array[Transform3D] = []
	var colors: Array[Color] = []
	var min_x := INF
	var max_x := -INF
	var min_z := INF
	var max_z := -INF
	var min_y := INF
	var max_y := -INF
	for cell in cells:
		var value := _heatmap_metric_value(cell)
		if value <= 0.0:
			continue
		var x := float(cell.get("x", 0.0))
		var z := float(cell.get("z", 0.0))
		var ground := _heatmap_ground_at(x, z)
		if ground < 0.0:
			continue
		var y := ground + HEATMAP_LIFT_M
		var ratio := value / peak if peak > 0.0 else 0.0
		transforms.append(Transform3D(Basis.IDENTITY, Vector3(x, y, z)))
		colors.append(_heatmap_color(ratio))
		min_x = minf(min_x, x)
		max_x = maxf(max_x, x)
		min_z = minf(min_z, z)
		max_z = maxf(max_z, z)
		min_y = minf(min_y, y)
		max_y = maxf(max_y, y)
	heatmap_cells = transforms.size()
	_ensure_heatmap_pool()
	if heatmap_multimesh == null:
		return
	# Grow the buffer once, then hide the unused tail with visible_instance_count
	# so a camera pan reuses the same MultiMesh instead of reallocating it.
	var count := transforms.size()
	if count > HEATMAP_MAX_CELLS:
		count = HEATMAP_MAX_CELLS
	if heatmap_multimesh.instance_count < count:
		heatmap_multimesh.instance_count = count
	heatmap_multimesh.visible_instance_count = count
	heatmap_colors = colors
	for index in count:
		heatmap_multimesh.set_instance_transform(index, transforms[index])
		heatmap_multimesh.set_instance_color(index, colors[index])
	if count == 0:
		heatmap_multimesh.custom_aabb = AABB()
	else:
		var half_x := HEATMAP_CELL_M * 0.47
		var half_z := HEATMAP_CELL_M * 0.47
		heatmap_multimesh.custom_aabb = AABB(
			Vector3(min_x - half_x, min_y - 0.025, min_z - half_z),
			Vector3(
				max_x - min_x + half_x * 2.0,
				max_y - min_y + 0.05,
				max_z - min_z + half_z * 2.0
			))

func _toggle_heatmap() -> bool:
	heatmap_visible = not heatmap_visible
	heatmap_refresh_accum = 0.0
	if heatmap_visible:
		_refresh_heatmap()
	else:
		_clear_heatmap()
	_update_hud()
	return heatmap_visible

func _materialization_center() -> Vector3:
	if selected_id >= 0:
		var selected := _selected_placement()
		var center: PackedFloat32Array = selected.get("center", PackedFloat32Array())
		if center.size() == 3:
			return Vector3(center[0], center[1], center[2])
	if mayor_camera != null:
		return mayor_camera.target
	return Vector3(target_x, 0.0, target_z)

## F7 query centre. An explicit settlement selection wins; otherwise the
## materialization centre (selected building, then Mayor camera target).
func _heatmap_center_xz() -> Vector2:
	var settlement_center := _settlement_focus_center()
	if is_finite(settlement_center.x) and is_finite(settlement_center.y):
		return settlement_center
	var center := _materialization_center()
	return Vector2(center.x, center.z)

func _materialization_pin() -> int:
	# The bridge treats 0 as "no pin"; selected_id is -1 when nothing is
	# selected and a negative id would be rejected.
	return maxi(selected_id, 0)

func _toggle_materialization() -> bool:
	materialization_visible = not materialization_visible
	materialization_refresh_accum = 0.0
	movement_accum = 0.0
	if not materialization_visible:
		_clear_movement_nodes()
	_rebuild_materialization()
	_update_hud()
	return materialization_visible

func _clear_materialization_nodes() -> void:
	if materialization_root == null:
		return
	for node in materialization_nodes.values():
		if is_instance_valid(node):
			node.free()
	materialization_nodes.clear()
	for node in materialization_fading:
		if is_instance_valid(node):
			node.free()
	materialization_fading.clear()
	for child in materialization_root.get_children():
		if is_instance_valid(child):
			child.free()
	materialization_counts = Vector2i.ZERO

func _materialization_marker_material(detailed_here: bool, alpha: float) -> StandardMaterial3D:
	var base: StandardMaterial3D = materialization_detailed_material if detailed_here else materialization_active_material
	var material := base.duplicate() as StandardMaterial3D
	material.transparency = BaseMaterial3D.TRANSPARENCY_ALPHA
	var color := base.albedo_color
	color.a = clampf(alpha, 0.0, 1.0)
	material.albedo_color = color
	return material

func _set_materialization_marker_alpha(node: MeshInstance3D, alpha: float) -> void:
	var material := node.material_override as StandardMaterial3D
	if material == null:
		return
	var clamped := clampf(alpha, 0.0, 1.0)
	var color := material.albedo_color
	color.a = clamped
	material.albedo_color = color
	node.set_meta("materialization_alpha", clamped)

func _style_materialization_marker(node: MeshInstance3D, detailed_here: bool) -> void:
	node.mesh = materialization_detailed_mesh if detailed_here else materialization_active_mesh
	var base: StandardMaterial3D = materialization_detailed_material if detailed_here else materialization_active_material
	var material := node.material_override as StandardMaterial3D
	if material != null:
		var alpha := material.albedo_color.a
		material.albedo_color = Color(
			base.albedo_color.r, base.albedo_color.g, base.albedo_color.b, alpha
		)
	node.set_meta("materialization_detailed", detailed_here)

func _place_materialization_marker(node: MeshInstance3D, home: PackedFloat32Array) -> void:
	if home.size() != 2:
		return
	var ground: float = core.world_height_at(SEED, home[0], home[1])
	if ground < 0.0:
		return
	var mesh: BoxMesh = node.mesh as BoxMesh
	var height := mesh.size.y if mesh != null else 0.0
	node.position = Vector3(home[0], ground + height * 0.5, home[1])

func _create_materialization_marker(citizen: Dictionary, detailed_here: bool, fade_in: bool) -> MeshInstance3D:
	var home: PackedFloat32Array = citizen.get("home", PackedFloat32Array())
	if home.size() != 2:
		return null
	var ground: float = core.world_height_at(SEED, home[0], home[1])
	if ground < 0.0:
		return null
	var mesh: BoxMesh = materialization_detailed_mesh if detailed_here else materialization_active_mesh
	var alpha := 0.0 if fade_in else 1.0
	var node := MeshInstance3D.new()
	node.mesh = mesh
	node.material_override = _materialization_marker_material(detailed_here, alpha)
	node.position = Vector3(home[0], ground + mesh.size.y * 0.5, home[1])
	node.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	node.set_meta("materialization_id", int(citizen.get("id", -1)))
	node.set_meta("materialization_detailed", detailed_here)
	node.set_meta("materialization_alpha", alpha)
	materialization_root.add_child(node)
	return node

func _reclaim_fading_marker(id: int) -> void:
	# A citizen that is re-added before its fade-out finishes replaces the old
	# marker instead of leaving two nodes for one id.
	for index in range(materialization_fading.size() - 1, -1, -1):
		var node: MeshInstance3D = materialization_fading[index]
		if not is_instance_valid(node):
			materialization_fading.remove_at(index)
		elif int(node.get_meta("materialization_id", -1)) == id:
			materialization_fading.remove_at(index)
			node.free()

func _upsert_materialization_marker(
	id: int, citizen: Dictionary, detailed_here: bool, fade_in: bool
) -> bool:
	var node: MeshInstance3D = materialization_nodes.get(id)
	if is_instance_valid(node):
		var home: PackedFloat32Array = citizen.get("home", PackedFloat32Array())
		_style_materialization_marker(node, detailed_here)
		_place_materialization_marker(node, home)
		if fade_in:
			_set_materialization_marker_alpha(node, 0.0)
		return true
	_reclaim_fading_marker(id)
	node = _create_materialization_marker(citizen, detailed_here, fade_in)
	if node == null:
		return false
	materialization_nodes[id] = node
	return true

func _retire_materialization_marker(id: int) -> void:
	var node: MeshInstance3D = materialization_nodes.get(id)
	if not is_instance_valid(node):
		materialization_nodes.erase(id)
		return
	materialization_nodes.erase(id)
	var alpha := float(node.get_meta("materialization_alpha", 1.0))
	if alpha <= 0.001:
		node.free()
		return
	node.set_meta("materialization_fade_from", alpha)
	node.set_meta("materialization_fade_out", 0.0)
	materialization_fading.append(node)

func _advance_materialization_fades(delta: float) -> void:
	if delta <= 0.0 or materialization_root == null:
		return
	var fade_step := delta / MATERIALIZATION_FADE_SECONDS
	for id in materialization_nodes.keys():
		var node: MeshInstance3D = materialization_nodes[id]
		if not is_instance_valid(node):
			materialization_nodes.erase(id)
			continue
		var alpha := float(node.get_meta("materialization_alpha", 1.0))
		if alpha < 1.0:
			_set_materialization_marker_alpha(node, minf(1.0, alpha + fade_step))
	var index := materialization_fading.size() - 1
	while index >= 0:
		var node := materialization_fading[index]
		if not is_instance_valid(node):
			materialization_fading.remove_at(index)
		else:
			var elapsed := float(node.get_meta("materialization_fade_out", 0.0)) + delta
			node.set_meta("materialization_fade_out", elapsed)
			var fade_from := float(node.get_meta("materialization_fade_from", 1.0))
			var remaining := clampf(1.0 - elapsed / MATERIALIZATION_FADE_SECONDS, 0.0, 1.0)
			_set_materialization_marker_alpha(node, fade_from * remaining)
			if remaining <= 0.0 or elapsed >= MATERIALIZATION_FADE_MAX_SECONDS:
				materialization_fading.remove_at(index)
				node.free()
		index -= 1

func _reconcile_materialization_markers(
	active_by_id: Dictionary, detailed_ids: Dictionary, budget: int
) -> int:
	# The bridge caps a diff at 64 changes, but it has already stored the full
	# new selection, so entries deferred by a truncated diff are never sent
	# again. Reconcile only the budget the diff left unused, so a truncated
	# step converges over later refreshes without exceeding 64 node changes.
	var changed := 0
	for id in materialization_nodes.keys():
		if changed >= budget:
			return changed
		if not active_by_id.has(id):
			_retire_materialization_marker(id)
			changed += 1
	for id in active_by_id.keys():
		if changed >= budget:
			return changed
		if not materialization_nodes.has(id):
			var citizen: Dictionary = active_by_id[id]
			if _upsert_materialization_marker(id, citizen, detailed_ids.has(id), true):
				changed += 1
	return changed

func _apply_materialization_state(state: Dictionary) -> void:
	var counts: Dictionary = state.get("counts", {})
	var active_count := int(counts.get("active", 0))
	var detailed_count := int(counts.get("detailed", 0))
	var active: Array = state.get("active", [])
	var detailed: Array = state.get("detailed", [])
	var detailed_ids := {}
	for index in mini(detailed_count, detailed.size()):
		var detailed_id := int(detailed[index].get("id", -1))
		if detailed_id >= 0:
			detailed_ids[detailed_id] = true
	var active_by_id := {}
	for index in mini(active_count, active.size()):
		var citizen: Dictionary = active[index]
		var id := int(citizen.get("id", -1))
		if id >= 0:
			active_by_id[id] = citizen
	var changed: Array = state.get("changed", [])
	var applied := 0
	for entry in changed:
		if applied >= MATERIALIZATION_CHANGE_BUDGET:
			break
		var id := int(entry.get("id", -1))
		if id < 0:
			continue
		match str(entry.get("action", "")):
			"removed":
				_retire_materialization_marker(id)
			"added", "updated":
				_upsert_materialization_marker(id, entry, detailed_ids.has(id), true)
			_:
				continue
		applied += 1
	if applied < MATERIALIZATION_CHANGE_BUDGET:
		applied += _reconcile_materialization_markers(
			active_by_id, detailed_ids, MATERIALIZATION_CHANGE_BUDGET - applied
		)
	# Detailed membership is not an identity/home change, so refresh retained
	# markers without touching their nodes or fade timers.
	for id in materialization_nodes.keys():
		var node: MeshInstance3D = materialization_nodes.get(id)
		if not is_instance_valid(node):
			continue
		var detailed_here := detailed_ids.has(id)
		if bool(node.get_meta("materialization_detailed", false)) != detailed_here:
			_style_materialization_marker(node, detailed_here)
			var citizen: Dictionary = active_by_id.get(id, {})
			if not citizen.is_empty():
				_place_materialization_marker(node, citizen.get("home", PackedFloat32Array()))

func _step_materialization() -> void:
	if not materialization_visible or core == null:
		return
	var previous_counts := materialization_counts
	var center := _materialization_center()
	var state: Dictionary = core.materialization_step(
		SEED, center.x, center.z,
		MATERIALIZATION_INNER_RADIUS_M, MATERIALIZATION_OUTER_RADIUS_M,
		MATERIALIZATION_DETAILED_RADIUS_M, _materialization_pin()
	)
	if not bool(state.get("ok", false)):
		return
	_apply_materialization_state(state)
	var counts: Dictionary = state.get("counts", {})
	materialization_counts = Vector2i(int(counts.get("active", 0)), int(counts.get("detailed", 0)))
	if materialization_counts != previous_counts:
		_update_hud()

func _rebuild_materialization() -> void:
	var previous_counts := materialization_counts
	_clear_materialization_nodes()
	materialization_selected_key = selected_id
	if not materialization_visible or core == null:
		return
	var center := _materialization_center()
	var state: Dictionary = core.materialization_step(
		SEED, center.x, center.z,
		MATERIALIZATION_INNER_RADIUS_M, MATERIALIZATION_OUTER_RADIUS_M,
		MATERIALIZATION_DETAILED_RADIUS_M, _materialization_pin()
	)
	if not bool(state.get("ok", false)):
		return
	var counts: Dictionary = state.get("counts", {})
	var active_count := int(counts.get("active", 0))
	var detailed_count := int(counts.get("detailed", 0))
	var active: Array = state.get("active", [])
	var detailed: Array = state.get("detailed", [])
	var detailed_ids := {}
	for index in mini(detailed_count, detailed.size()):
		var detailed_id := int(detailed[index].get("id", -1))
		if detailed_id >= 0:
			detailed_ids[detailed_id] = true
	# A selection/state reset recreates the full selection once. Steady-state
	# refreshes go through _step_materialization and stay within the budget.
	for index in mini(active_count, active.size()):
		var citizen: Dictionary = active[index]
		var id := int(citizen.get("id", -1))
		if id < 0:
			continue
		_upsert_materialization_marker(id, citizen, detailed_ids.has(id), true)
	materialization_counts = Vector2i(active_count, detailed_count)
	if materialization_counts != previous_counts:
		_update_hud()

func _clear_movement_nodes() -> void:
	if movement_root != null:
		for child in movement_root.get_children():
			if is_instance_valid(child):
				child.free()
	movement_multimesh_instance = null
	movement_multimesh = null
	movement_far_multimesh_instance = null
	movement_far_multimesh = null
	movement_pool_root = null
	movement_route_lines = null
	movement_route_key = ""
	movement_ground.clear()
	movement_ids.clear()
	movement_far_ids.clear()
	movement_far_checksum = 0
	movement_lod_counts = Vector2i.ZERO
	last_movement_state = {}


func _refresh_route_lines(state: Dictionary) -> void:
	if movement_root == null or core == null:
		return
	var key := str(state.get("checksum", ""))
	if movement_route_lines != null and is_instance_valid(movement_route_lines) and movement_route_key == key:
		return
	var surface := SurfaceTool.new()
	surface.begin(Mesh.PRIMITIVE_LINES)
	var vertex_count := 0
	for route in state.get("route_polylines", []):
		var points: PackedVector2Array = route.get("points", PackedVector2Array())
		if points.size() < 2:
			continue
		for index in points.size() - 1:
			var start := points[index]
			var end := points[index + 1]
			var start_y: float = core.world_height_at(SEED, start.x, start.y)
			var end_y: float = core.world_height_at(SEED, end.x, end.y)
			if start_y < 0.0 or end_y < 0.0:
				continue
			surface.add_vertex(Vector3(start.x, start_y + 0.35, start.y))
			surface.add_vertex(Vector3(end.x, end_y + 0.35, end.y))
			vertex_count += 2
	var mesh: ArrayMesh = surface.commit() if vertex_count > 0 else null
	if movement_route_lines == null or not is_instance_valid(movement_route_lines):
		movement_route_lines = MeshInstance3D.new()
		movement_route_lines.name = "CitizenRoutes"
		movement_route_lines.material_override = movement_route_material
		movement_route_lines.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
		movement_root.add_child(movement_route_lines)
		movement_root.move_child(movement_route_lines, 0)
	movement_route_lines.mesh = mesh
	movement_route_lines.visible = vertex_count > 0
	movement_route_key = key


func _movement_lod_split(samples: Array, center: Vector3) -> Dictionary:
	# Deterministic split by citizen id: near keeps every routed sample inside
	# MOVEMENT_NEAR_RADIUS_M, far stride-samples the id-sorted remainder to
	# MOVEMENT_FAR_BUDGET. Sorting ids (not relying on the bridge's position
	# order) keeps the far selection and checksum stable across refreshes.
	var by_id := {}
	var ids: Array[int] = []
	for sample in samples:
		var id := int(sample.get("id", -1))
		var point: PackedFloat32Array = sample.get("position", PackedFloat32Array())
		if id < 0 or point.size() != 2 or by_id.has(id):
			continue
		var dx := float(point[0]) - center.x
		var dz := float(point[1]) - center.z
		by_id[id] = {"id": id, "position": point, "distance_sq": dx * dx + dz * dz}
		ids.append(id)
	ids.sort()
	var near_radius_sq := MOVEMENT_NEAR_RADIUS_M * MOVEMENT_NEAR_RADIUS_M
	var near: Array = []
	var far_all: Array = []
	for id in ids:
		var entry: Dictionary = by_id[id]
		if float(entry.get("distance_sq", INF)) <= near_radius_sq:
			near.append(entry)
		else:
			far_all.append(entry)
	var far: Array = []
	if far_all.size() <= MOVEMENT_FAR_BUDGET:
		far = far_all
	else:
		# Uniform stride over sorted ids. Bounded by the budget; a far set that
		# is not a multiple of the stride draws fewer than the cap.
		var stride := maxi(1, int(ceil(float(far_all.size()) / float(MOVEMENT_FAR_BUDGET))))
		var index := 0
		while index < far_all.size() and far.size() < MOVEMENT_FAR_BUDGET:
			far.append(far_all[index])
			index += stride
	return {"near": near, "far": far}


func _movement_id_checksum(ids: Array[int]) -> int:
	# FNV-1a (32-bit) over the id order. The smoke compares this across repeats;
	# it is presentation-only and never enters save or network state.
	var digest := 2166136261
	for id in ids:
		digest = ((digest ^ (id & 0xFFFFFFFF)) * 16777619) & 0xFFFFFFFF
	return digest


func _movement_draw_set(entries: Array, height: float, seen: Dictionary) -> Dictionary:
	# Resolves cached ground height for drawn markers only, so the height cache
	# stays bounded by near + far rather than every routed citizen.
	var ids: Array[int] = []
	var transforms: Array[Transform3D] = []
	for entry in entries:
		var id := int(entry.get("id", -1))
		var point: PackedFloat32Array = entry.get("position", PackedFloat32Array())
		if id < 0 or point.size() != 2:
			continue
		if not movement_ground.has(id):
			var ground: float = core.world_height_at(SEED, point[0], point[1])
			if ground < 0.0:
				continue
			movement_ground[id] = ground
		ids.append(id)
		seen[id] = true
		var y := float(movement_ground[id]) + height
		transforms.append(Transform3D(Basis.IDENTITY, Vector3(point[0], y, point[1])))
	return {"ids": ids, "transforms": transforms}


func _write_movement_pool(multimesh: MultiMesh, transforms: Array, size: Vector3) -> void:
	var count := transforms.size()
	# Capacity grows only; `visible_instance_count` hides spare slots instead of
	# shrinking and reallocating the buffer when the near set changes size.
	if multimesh.instance_count < count:
		multimesh.instance_count = count
	multimesh.visible_instance_count = count
	for index in count:
		multimesh.set_instance_transform(index, transforms[index])
	if count == 0:
		multimesh.custom_aabb = AABB()
		return
	var min_point := Vector3(INF, INF, INF)
	var max_point := Vector3(-INF, -INF, -INF)
	for transform in transforms:
		var origin: Vector3 = transform.origin
		min_point.x = minf(min_point.x, origin.x)
		min_point.y = minf(min_point.y, origin.y)
		min_point.z = minf(min_point.z, origin.z)
		max_point.x = maxf(max_point.x, origin.x)
		max_point.y = maxf(max_point.y, origin.y)
		max_point.z = maxf(max_point.z, origin.z)
	var half_x := size.x * 0.5
	var half_z := size.z * 0.5
	var half_y := size.y * 0.5
	multimesh.custom_aabb = AABB(
		Vector3(min_point.x - half_x, min_point.y - half_y, min_point.z - half_z),
		Vector3(
			max_point.x - min_point.x + size.x,
			max_point.y - min_point.y + size.y,
			max_point.z - min_point.z + size.z
		))


func _update_movement_markers() -> void:
	if not materialization_visible or core == null or movement_root == null:
		return
	var center := _materialization_center()
	var state: Dictionary = core.citizen_movement(
		SEED, center.x, center.z,
		MATERIALIZATION_OUTER_RADIUS_M, MATERIALIZATION_DETAILED_RADIUS_M, movement_time
	)
	last_movement_state = state
	if not bool(state.get("ok", false)):
		return
	_setup_movement_pool()
	if movement_multimesh == null or movement_far_multimesh == null:
		return
	_refresh_route_lines(state)
	var lod := _movement_lod_split(state.get("positions", []), center)
	var seen := {}
	var near: Dictionary = _movement_draw_set(lod.get("near", []), MOVEMENT_NEAR_HEIGHT * 0.5, seen)
	var far: Dictionary = _movement_draw_set(lod.get("far", []), MOVEMENT_FAR_HEIGHT * 0.5, seen)
	# Drop cached heights for citizens that left the drawn near/far set.
	for id in movement_ground.keys():
		if not seen.has(id):
			movement_ground.erase(id)
	var previous_lod := movement_lod_counts
	movement_ids.clear()
	for id in near.get("ids", []):
		movement_ids.append(int(id))
	movement_far_ids.clear()
	for id in far.get("ids", []):
		movement_far_ids.append(int(id))
	movement_lod_counts = Vector2i(movement_ids.size(), movement_far_ids.size())
	movement_far_checksum = _movement_id_checksum(movement_far_ids)
	_write_movement_pool(movement_multimesh, near.get("transforms", []), MOVEMENT_NEAR_SIZE)
	_write_movement_pool(movement_far_multimesh, far.get("transforms", []), MOVEMENT_FAR_SIZE)
	if movement_lod_counts != previous_lod:
		_update_hud()


func _industry_color(kind: String) -> Color:
	match kind:
		"farm":
			return Color(0.85, 0.75, 0.2)
		"hunt":
			return Color(0.7, 0.35, 0.2)
		_:
			return Color(0.45, 0.3, 0.15)

func _refresh_industry_visuals() -> void:
	if industry_root == null:
		return
	for node in industry_nodes.values():
		if is_instance_valid(node):
			node.free()
	industry_nodes.clear()
	industry_animated.clear()
	industry_kinds.clear()
	if core == null:
		return
	var state: Dictionary = core.industry_state()
	for site in state.get("sites", []):
		var id := int(site.get("placement_id", -1))
		if id < 0:
			continue
		var kind := str(site.get("kind", ""))
		industry_kinds[id] = kind
		var pivot := Node3D.new()
		pivot.name = "Industry%d" % id
		var base_y := float(site.get("center_y", 40.0))
		pivot.position = Vector3(
			float(site.get("center_x", 0.0)),
			base_y,
			float(site.get("center_z", 0.0))
		)
		# The metas stay as the node-structure contract the scene smoke checks;
		# the per-frame loop reads the `industry_animated` cache instead.
		pivot.set_meta("base_y", base_y)
		var mesh_instance := MeshInstance3D.new()
		var box := BoxMesh.new()
		box.size = Vector3(0.8, 3.0, 0.8)
		mesh_instance.mesh = box
		mesh_instance.position.y = 1.7
		var material := StandardMaterial3D.new()
		material.albedo_color = _industry_color(kind)
		material.emission_enabled = true
		material.emission = _industry_color(kind) * 0.35
		mesh_instance.material_override = material
		pivot.add_child(mesh_instance)
		var spinner := Node3D.new()
		spinner.name = "WorkerLoop"
		var worker := MeshInstance3D.new()
		var worker_box := BoxMesh.new()
		worker_box.size = Vector3(0.45, 0.9, 0.45)
		worker.mesh = worker_box
		worker.position = Vector3(2.0, 0.8, 0.0)
		var worker_material := StandardMaterial3D.new()
		worker_material.albedo_color = Color(0.9, 0.9, 0.85)
		worker.material_override = worker_material
		spinner.add_child(worker)
		pivot.add_child(spinner)
		pivot.set_meta("spinner", spinner)
		var label := Label3D.new()
		label.text = _content_display("base.content.industry.%s" % kind, kind.capitalize())
		label.font_size = 40
		label.billboard = BaseMaterial3D.BILLBOARD_ENABLED
		label.position.y = 3.9
		label.modulate = _industry_color(kind)
		pivot.add_child(label)
		industry_root.add_child(pivot)
		industry_nodes[id] = pivot
		industry_animated.append({
			"pivot": pivot,
			"base_y": base_y,
			"spinner": spinner,
			"worker": worker,
		})

func _cycle_starter() -> void:
	var catalog: Dictionary = core.starter_library(SEED)
	if not bool(catalog.get("ok", false)):
		_set_modal(i18n.t("base.ui.modal.starter_catalog_failed", {
			"code": str(catalog.get("error_code", "unknown")),
		}), true)
		return
	starter_ids.clear()
	for item in catalog.get("items", []):
		if bool(item.get("available", false)):
			starter_ids.append(str(item.get("id", "")))
	if starter_ids.is_empty():
		_set_modal(i18n.t("base.ui.modal.starter_none"), true)
		return
	starter_index = (starter_index + 1) % starter_ids.size()
	var selected_id := str(starter_ids[starter_index])
	var result: Dictionary = core.starter_blueprint(SEED, selected_id)
	if not bool(result.get("ok", false)):
		_set_modal(
			i18n.t("base.ui.modal.starter_failed", {
				"code": str(result.get("error_code", result.get("error", ""))),
			}),
			true
		)
		return
	var path := str(result.get("path", ""))
	if path == "":
		_set_modal(i18n.t("base.ui.modal.starter_no_path"), true)
		return
	blueprint_path = path
	if not _ensure_blueprint():
		_set_modal(i18n.t("base.ui.modal.starter_package_rejected"), true)
		return
	_refresh_preview(true)
	_set_modal(
		i18n.t("base.ui.modal.starter_selected", {
			"name": _starter_display(str(result.get("id", selected_id)), str(result.get("name", selected_id))),
			"index": starter_index + 1,
			"total": starter_ids.size(),
		}),
		false
	)
	_update_hud()

func _cycle_industry() -> void:
	if selected_id < 0:
		_set_modal(i18n.t("base.ui.modal.select_building"), true)
		return
	var selected := _selected_placement()
	if int(selected.get("job_slots", 0)) <= 0:
		_set_modal(i18n.t("base.ui.modal.no_job_slots"), true)
		return
	var current := str(industry_kinds.get(selected_id, ""))
	var next := ""
	match current:
		"":
			next = "lumberjack"
		"lumberjack":
			next = "farm"
		"farm":
			next = "hunt"
		"hunt":
			next = ""
	if next == "":
		core.industry_remove_site(selected_id)
		_set_modal(i18n.t("base.ui.modal.industry_removed"), false)
	else:
		var result: Dictionary = core.industry_add_site(SEED, selected_id, next)
		if not bool(result.get("ok", false)):
			_set_modal(
				i18n.t("base.ui.modal.industry_failed", {
					"code": str(result.get("error_code", result.get("error", ""))),
				}),
				true
			)
			return
		_set_modal(i18n.t("base.ui.modal.industry_selected", {"kind": _content_display("base.content.industry.%s" % next, next)}), false)
	_refresh_industry_visuals()
	_update_hud()

func _setup_utilities() -> void:
	utility_root = Node3D.new()
	utility_root.name = "Utilities"
	add_child(utility_root)

func _setup_services() -> void:
	service_root = Node3D.new()
	service_root.name = "CivicServices"
	add_child(service_root)

func _service_color(kind: String) -> Color:
	match kind:
		"school":
			return Color(0.35, 0.65, 0.95)
		"clinic":
			return Color(0.35, 0.85, 0.55)
		"fire":
			return Color(0.95, 0.35, 0.25)
		"police":
			return Color(0.35, 0.45, 0.85)
		_:
			return Color(0.8, 0.8, 0.8)

func _service_offset(kind: String) -> Vector2:
	match kind:
		"school":
			return Vector2(-1.8, -1.8)
		"clinic":
			return Vector2(1.8, -1.8)
		"fire":
			return Vector2(-1.8, 1.8)
		"police":
			return Vector2(1.8, 1.8)
		_:
			return Vector2.ZERO

func _refresh_service_visuals() -> void:
	if service_root == null:
		return
	for node in service_nodes.values():
		if is_instance_valid(node):
			node.free()
	service_nodes.clear()
	service_kinds.clear()
	if core == null:
		return
	var state: Dictionary = core.services_state()
	for site in state.get("sites", []):
		var id := int(site.get("placement_id", -1))
		if id < 0:
			continue
		var kind := str(site.get("kind", ""))
		if not ["school", "clinic", "fire", "police"].has(kind):
			continue
		service_kinds[id] = kind
		var pivot := Node3D.new()
		pivot.name = "%s%d" % [kind.capitalize(), id]
		var base_y := float(site.get("center_y", 40.0))
		var offset := _service_offset(kind)
		pivot.position = Vector3(
			float(site.get("center_x", 0.0)) + offset.x,
			base_y,
			float(site.get("center_z", 0.0)) + offset.y
		)
		pivot.set_meta("base_y", base_y)
		var color := _service_color(kind)
		var base := MeshInstance3D.new()
		var base_box := BoxMesh.new()
		base_box.size = Vector3(1.8, 0.24, 1.8)
		base.mesh = base_box
		base.position.y = 0.12
		var base_material := StandardMaterial3D.new()
		base_material.albedo_color = color
		base_material.emission_enabled = true
		base_material.emission = color * 0.25
		base.material_override = base_material
		pivot.add_child(base)
		var column := MeshInstance3D.new()
		var column_box := BoxMesh.new()
		column_box.size = Vector3(0.9, 2.2, 0.9)
		column.mesh = column_box
		column.position.y = 1.35
		var column_material := StandardMaterial3D.new()
		column_material.albedo_color = color
		column.material_override = column_material
		pivot.add_child(column)
		var label := Label3D.new()
		label.text = _content_display("base.content.service.%s" % kind, kind.capitalize())
		label.font_size = 34
		label.billboard = BaseMaterial3D.BILLBOARD_ENABLED
		label.position.y = 3.1
		label.modulate = color
		pivot.add_child(label)
		service_root.add_child(pivot)
		service_nodes["%s:%d" % [kind, id]] = pivot

func _cycle_service() -> void:
	if selected_id < 0:
		_set_modal(i18n.t("base.ui.modal.select_building"), true)
		return
	var selected := _selected_placement()
	if int(selected.get("job_slots", 0)) <= 0:
		_set_modal(i18n.t("base.ui.modal.no_job_slots"), true)
		return
	var current := str(service_kinds.get(selected_id, ""))
	var next := ""
	match current:
		"":
			next = "school"
		"school":
			next = "clinic"
		"clinic":
			next = "fire"
		"fire":
			next = "police"
		"police":
			next = ""
	if next == "":
		core.service_remove_site(SEED, selected_id)
		_set_modal(i18n.t("base.ui.modal.service_removed"), false)
	else:
		var result: Dictionary = core.service_add_site(SEED, selected_id, next)
		if not bool(result.get("ok", false)):
			_set_modal(
				i18n.t("base.ui.modal.service_failed", {
					"code": str(result.get("error_code", result.get("error", ""))),
				}),
				true
			)
			return
		_set_modal(i18n.t("base.ui.modal.service_selected", {"kind": _content_display("base.content.service.%s" % next, next)}), false)
	_refresh_service_visuals()
	_update_hud()

func _utility_color(kind: String) -> Color:
	if kind == "power":
		return Color(0.95, 0.8, 0.25)
	return Color(0.25, 0.65, 0.95)

func _refresh_utility_visuals() -> void:
	if utility_root == null:
		return
	for node in utility_nodes.values():
		if is_instance_valid(node):
			node.free()
	utility_nodes.clear()
	utility_kinds.clear()
	if core == null:
		return
	var state: Dictionary = core.utilities_state()
	for site in state.get("sites", []):
		var id := int(site.get("placement_id", -1))
		if id < 0:
			continue
		var kind := str(site.get("kind", ""))
		if kind != "water" and kind != "power":
			continue
		if not utility_kinds.has(id):
			utility_kinds[id] = []
		utility_kinds[id].append(kind)
		var pivot := Node3D.new()
		pivot.name = "%s%d" % [kind.capitalize(), id]
		var base_y := float(site.get("center_y", 40.0))
		var offset := -1.4 if kind == "water" else 1.4
		pivot.position = Vector3(
			float(site.get("center_x", 0.0)) + offset,
			base_y,
			float(site.get("center_z", 0.0))
		)
		pivot.set_meta("base_y", base_y)
		var color := _utility_color(kind)
		var base := MeshInstance3D.new()
		var base_box := BoxMesh.new()
		base_box.size = Vector3(2.4, 0.22, 2.4)
		base.mesh = base_box
		base.position.y = 0.11
		var base_material := StandardMaterial3D.new()
		base_material.albedo_color = color
		base_material.emission_enabled = true
		base_material.emission = color * 0.25
		base.material_override = base_material
		pivot.add_child(base)
		var pipe := MeshInstance3D.new()
		var pipe_box := BoxMesh.new()
		pipe_box.size = Vector3(0.42, 2.6, 0.42)
		pipe.mesh = pipe_box
		pipe.position.y = 1.4
		var pipe_material := StandardMaterial3D.new()
		pipe_material.albedo_color = color
		pipe.material_override = pipe_material
		pivot.add_child(pipe)
		var label := Label3D.new()
		label.text = _utility_display(kind)
		label.font_size = 36
		label.billboard = BaseMaterial3D.BILLBOARD_ENABLED
		label.position.y = 3.2
		label.modulate = color
		pivot.add_child(label)
		utility_root.add_child(pivot)
		utility_nodes["%s:%d" % [kind, id]] = pivot

func _cycle_utility() -> void:
	if selected_id < 0:
		_set_modal(i18n.t("base.ui.modal.select_building"), true)
		return
	var selected := _selected_placement()
	if int(selected.get("job_slots", 0)) <= 0:
		_set_modal(i18n.t("base.ui.modal.no_job_slots"), true)
		return
	var kinds: Array = utility_kinds.get(selected_id, [])
	var has_water := kinds.has("water")
	var has_power := kinds.has("power")
	if not has_water and not has_power:
		var result: Dictionary = core.utility_add_site(SEED, selected_id, "water")
		if not bool(result.get("ok", false)):
			_set_modal(i18n.t("base.ui.modal.utility_failed", {
				"code": str(result.get("error_code", "")),
			}), true)
			return
		_set_modal(i18n.t("base.ui.modal.utility_water"), false)
	elif has_water and not has_power:
		var result: Dictionary = core.utility_add_site(SEED, selected_id, "power")
		if not bool(result.get("ok", false)):
			_set_modal(i18n.t("base.ui.modal.utility_failed", {
				"code": str(result.get("error_code", "")),
			}), true)
			return
		_set_modal(i18n.t("base.ui.modal.utility_water_power"), false)
	elif has_power and not has_water:
		var result: Dictionary = core.utility_add_site(SEED, selected_id, "water")
		if not bool(result.get("ok", false)):
			_set_modal(i18n.t("base.ui.modal.utility_failed", {
				"code": str(result.get("error_code", "")),
			}), true)
			return
		_set_modal(i18n.t("base.ui.modal.utility_water_power"), false)
	else:
		core.utility_remove_site(SEED, selected_id, "water")
		core.utility_remove_site(SEED, selected_id, "power")
		_set_modal(i18n.t("base.ui.modal.utility_removed"), false)
	_refresh_utility_visuals()
	_update_hud()

func _cycle_tax_policy() -> void:
	var state: Dictionary = core.economy_state()
	var resident := float(state.get("tax_per_resident_per_day", 8.0))
	var business := float(state.get("business_tax_per_job_per_day", 4.0))
	var index := -1
	for i in TAX_POLICIES.size():
		var policy: Dictionary = TAX_POLICIES[i]
		if absf(float(policy["resident"]) - resident) < 0.01 and absf(float(policy["business"]) - business) < 0.01:
			index = i
			break
	var next: Dictionary = TAX_POLICIES[(index + 1) % TAX_POLICIES.size()]
	var result: Dictionary = core.economy_set_taxes(
		SEED, float(next["resident"]), float(next["business"])
	)
	if not bool(result.get("ok", false)):
		_set_modal(i18n.t("base.ui.modal.tax_failed", {
			"code": str(result.get("error_code", "")),
		}), true)
		return
	_set_modal(i18n.t("base.ui.modal.tax_policy", {"name": str(next["name"])}), false)
	_update_hud()

func _borrow() -> void:
	var result: Dictionary = core.economy_borrow(SEED, LOAN_AMOUNT)
	if not bool(result.get("ok", false)):
		_set_modal(i18n.t("base.ui.modal.loan_rejected", {
			"code": str(result.get("error_code", "")),
		}), true)
		_update_hud()
		return
	_set_modal(i18n.t("base.ui.modal.loan_taken", {
		"amount": "%.0f" % LOAN_AMOUNT,
		"debt": "%.0f" % float(result.get("debt", 0.0)),
	}), false)
	_update_hud()

func _found_settlement() -> bool:
	var state: Dictionary = core.settlements()
	var used: Dictionary = {}
	for settlement in state.get("settlements", []):
		used[str(settlement.get("name", "")).to_lower()] = true
	var index: int = core.settlement_count() + 1
	var name := "Settlement %d" % index
	while used.has(name.to_lower()):
		index += 1
		name = "Settlement %d" % index
	var result: Dictionary = core.settlement_found(
		SEED,
		name,
		"player",
		roundi(target_x),
		roundi(target_z),
		64
	)
	if not bool(result.get("ok", false)):
		_set_modal(i18n.t("base.ui.modal.settlement_rejected", {
			"code": str(result.get("error_code", "unknown")),
		}), true)
		_update_hud()
		return false
	_add_settlement_node(result.get("settlement", {}))
	_set_modal(i18n.t("base.ui.modal.settlement_founded", {
		"name": str(result.get("settlement", {}).get("name", "settlement")),
	}), false)
	_update_hud()
	if dashboard_visible:
		_update_dashboard()
	return true

func _clear_settlements() -> void:
	core.settlement_clear()
	_clear_settlement_nodes()
	selected_settlement_id = -1
	if heatmap_visible:
		_refresh_heatmap()
	_set_modal(i18n.t("base.ui.modal.settlements_cleared"), false)
	_update_hud()
	if dashboard_visible:
		_update_dashboard()

func _rebuild_settlements() -> void:
	_clear_settlement_nodes()
	var state: Dictionary = core.settlements()
	for settlement in state.get("settlements", []):
		_add_settlement_node(settlement)
	# A load may replace the layer with different ids; keep a still-valid
	# selection and drop a removed or foreign one.
	_validate_settlement_selection(state)

func _clear_settlement_nodes() -> void:
	for node in settlement_nodes.values():
		if is_instance_valid(node):
			node.free()
	settlement_nodes.clear()

func _add_settlement_node(settlement: Dictionary) -> void:
	var id := int(settlement.get("id", -1))
	if id < 0 or settlement_nodes.has(id):
		return
	var center: PackedInt32Array = settlement.get("center", PackedInt32Array())
	if center.size() != 2:
		return
	var ground: float = core.world_height_at(SEED, center[0], center[1])
	if ground < 0.0:
		return
	var radius := float(settlement.get("radius_m", 0))
	if radius < 1.0:
		return
	var root := Node3D.new()
	root.position = Vector3(center[0], ground, center[1])
	root.set_meta("radius_m", radius)
	root.set_meta("owner", str(settlement.get("owner", "player")))

	var ring_mesh := ImmediateMesh.new()
	ring_mesh.surface_begin(Mesh.PRIMITIVE_LINES, settlement_material)
	var segments := 64
	for index in range(segments):
		var a0 := TAU * float(index) / float(segments)
		var a1 := TAU * float(index + 1) / float(segments)
		var p0 := Vector3(cos(a0) * radius, 0.0, sin(a0) * radius)
		var p1 := Vector3(cos(a1) * radius, 0.0, sin(a1) * radius)
		var h0: float = core.world_height_at(SEED, root.position.x + p0.x, root.position.z + p0.z)
		var h1: float = core.world_height_at(SEED, root.position.x + p1.x, root.position.z + p1.z)
		p0.y = (h0 - ground) + 0.5 if h0 >= 0.0 else 0.5
		p1.y = (h1 - ground) + 0.5 if h1 >= 0.0 else 0.5
		ring_mesh.surface_add_vertex(p0)
		ring_mesh.surface_add_vertex(p1)
	ring_mesh.surface_end()
	var ring := MeshInstance3D.new()
	ring.mesh = ring_mesh
	ring.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	root.add_child(ring)

	var label := Label3D.new()
	label.text = "%s\n%s" % [
		str(settlement.get("name", "Settlement")),
		str(settlement.get("owner", "player")),
	]
	label.position = Vector3(0.0, 5.0, 0.0)
	label.font_size = 64
	label.pixel_size = 0.02
	label.billboard = BaseMaterial3D.BILLBOARD_ENABLED
	label.no_depth_test = true
	label.modulate = Color(1.0, 0.9, 0.5)
	root.add_child(label)

	settlement_root.add_child(root)
	settlement_nodes[id] = root

func _settlement_node_matches(settlement: Dictionary) -> bool:
	var id := int(settlement.get("id", -1))
	var root: Node3D = settlement_nodes.get(id) as Node3D
	if not is_instance_valid(root) or root.get_child_count() < 2:
		return false
	var center: PackedInt32Array = settlement.get("center", PackedInt32Array())
	if center.size() != 2:
		return false
	var ring := root.get_child(0) as MeshInstance3D
	var label := root.get_child(1) as Label3D
	if ring == null or not (ring.mesh is ImmediateMesh) or label == null:
		return false
	var radius := float(settlement.get("radius_m", -1))
	var owner := str(settlement.get("owner", ""))
	var arrays: Array = ring.mesh.surface_get_arrays(0)
	var vertices: PackedVector3Array = arrays[Mesh.ARRAY_VERTEX]
	return (
		ring.mesh.get_surface_count() == 1
		and vertices.size() == 128
		and label.text.begins_with(str(settlement.get("name", "")))
		and label.text.contains(owner)
		and absf(float(root.get_meta("radius_m", -1)) - radius) < 0.001
		and str(root.get_meta("owner", "")) == owner
		and absf(root.position.x - center[0]) < 0.001
		and absf(root.position.z - center[1]) < 0.001
	)

## Settlement records in stable ascending-id order (a bridge read by default).
func _settlement_records(state: Dictionary = {}) -> Array:
	var source: Dictionary = state
	if source.is_empty():
		source = core.settlements()
	var value = source.get("settlements", [])
	if not (value is Array):
		return []
	var records: Array = value.duplicate()
	records.sort_custom(_settlement_id_less)
	return records

func _settlement_id_less(a, b) -> bool:
	if not (a is Dictionary) or not (b is Dictionary):
		return false
	return int(a.get("id", 0)) < int(b.get("id", 0))

func _settlement_by_id(id: int, state: Dictionary = {}) -> Dictionary:
	if id < 0:
		return {}
	var records := _settlement_records(state)
	for record in records:
		if int(record.get("id", -1)) == id:
			return record
	return {}

## Read an `[x, z]` centre from a plain or packed array; NAN when malformed.
func _settlement_center(settlement: Dictionary) -> Vector2:
	var value = settlement.get("center", null)
	if not (value is Array or value is PackedInt32Array or value is PackedFloat32Array):
		return Vector2(NAN, NAN)
	if value.size() < 2:
		return Vector2(NAN, NAN)
	return Vector2(float(value[0]), float(value[1]))

func _selected_settlement() -> Dictionary:
	if selected_settlement_id < 0:
		return {}
	return _settlement_by_id(selected_settlement_id)

## Current settlement override centre, or NAN when there is no valid override.
## A missing/malformed selection is cleared so callers fall back cleanly.
func _settlement_focus_center() -> Vector2:
	if selected_settlement_id < 0:
		return Vector2(NAN, NAN)
	var settlement := _settlement_by_id(selected_settlement_id)
	var center := _settlement_center(settlement)
	if not settlement.is_empty() and is_finite(center.x) and is_finite(center.y):
		return center
	selected_settlement_id = -1
	return Vector2(NAN, NAN)

## Drop a selection whose id disappeared (load, clear, foreign id).
func _validate_settlement_selection(state: Dictionary = {}) -> bool:
	if selected_settlement_id < 0:
		return true
	var settlement := _settlement_by_id(selected_settlement_id, state)
	var center := _settlement_center(settlement)
	if not settlement.is_empty() and is_finite(center.x) and is_finite(center.y):
		return true
	selected_settlement_id = -1
	return false

## Apply a settlement id (or -1 for none) to every dependent query.
func _apply_settlement_selection(id: int) -> int:
	var next_id := -1
	if id >= 0:
		var settlement := _settlement_by_id(id)
		var center := _settlement_center(settlement)
		if not settlement.is_empty() and is_finite(center.x) and is_finite(center.y):
			next_id = id
	selected_settlement_id = next_id
	dashboard_refresh_accum = 0.0
	if heatmap_visible:
		_refresh_heatmap()
	if dashboard_visible:
		_update_dashboard()
	_update_hud()
	return selected_settlement_id

func _select_settlement(id: int) -> bool:
	var applied := _apply_settlement_selection(id)
	if id < 0:
		return applied < 0
	return applied == id

## Step through `none, id1, id2, ...` in ascending id order, wrapping.
func _cycle_settlement(direction: int) -> int:
	var cycle: Array[int] = [-1]
	for record in _settlement_records():
		var id := int(record.get("id", -1))
		if id >= 0:
			cycle.append(id)
	var index := cycle.find(selected_settlement_id)
	if index < 0:
		index = 0
	var step := -1 if direction < 0 else 1
	var next_index := (index + step + cycle.size()) % cycle.size()
	return _apply_settlement_selection(cycle[next_index])

func _ensure_blueprint() -> bool:
	if not FileAccess.file_exists(blueprint_path):
		if not core.create_editor_volume(0, 0, 0):
			return false
		if not core.editor_build_acceptance_house():
			return false
		var saved: Dictionary = core.editor_save_blueprint(
			blueprint_path,
			"local:acceptance_house",
			"Acceptance House"
		)
		if not bool(saved.get("ok", false)):
			return false

	var info: Dictionary = core.world_blueprint_info(blueprint_path)
	if not bool(info.get("ok", false)):
		_set_modal(i18n.t("base.ui.modal.blueprint_rejected", {
			"code": str(info.get("error_code", "unknown")),
		}), true)
		return false
	blueprint_info = info
	blueprint_info_cache[blueprint_path] = info
	var mesh: ArrayMesh = core.build_blueprint_mesh(blueprint_path)
	if mesh.get_surface_count() == 0:
		_set_modal(i18n.t("base.ui.modal.blueprint_mesh_empty"), true)
		return false
	blueprint_mesh = mesh
	mesh_cache[blueprint_path] = mesh
	var bounds: PackedFloat32Array = info.get("bounds", PackedFloat32Array())
	if bounds.size() != 3:
		_set_modal(i18n.t("base.ui.modal.blueprint_bounds_invalid"), true)
		return false
	ghost_mesh.mesh = mesh
	ghost_mesh.position = Vector3(-bounds[0] * 0.5, 0.0, -bounds[2] * 0.5)
	ghost.visible = true
	return true

func _refresh_preview(force: bool) -> void:
	if blueprint_mesh == null:
		return
	if force:
		last_cell = Vector2i(floori(target_x), floori(target_z))
	preview_validation = core.world_validate_placement(
		SEED, blueprint_path, target_x, target_z, rotation_deg
	)
	preview_valid = bool(preview_validation.get("ok", false))
	preview_error = str(preview_validation.get(
		"error_code", preview_validation.get("error", "")
	))

	var ground: float = core.world_height_at(SEED, target_x, target_z)
	if ground < 0.0:
		ground = camera.global_position.y - 8.0
	var y := ground
	var foundation := 0.0
	var footprint := Vector2(1.0, 1.0)
	var local_bounds: PackedFloat32Array = blueprint_info.get("bounds", PackedFloat32Array())
	if local_bounds.size() == 3:
		# The ghost pivot is rotated, so the foundation box must use the
		# unrotated blueprint footprint; `footprint_m` is already world-axis.
		footprint = Vector2(local_bounds[0], local_bounds[2])
	if preview_valid:
		var center: PackedFloat32Array = preview_validation.get("center", PackedFloat32Array())
		if center.size() == 3:
			y = center[1]
		foundation = float(preview_validation.get("foundation_m", 0.0))

	ghost.position = Vector3(target_x, y, target_z)
	ghost.rotation_degrees = Vector3(0.0, float(rotation_deg), 0.0)
	ghost_foundation.visible = preview_valid and foundation > 0.001
	if ghost_foundation.visible:
		var box := BoxMesh.new()
		box.size = Vector3(footprint.x, foundation, footprint.y)
		ghost_foundation.mesh = box
		ghost_foundation.position = Vector3(0.0, -foundation * 0.5, 0.0)
	_update_ghost_color()
	_update_hud()

func _update_ghost_color() -> void:
	if ghost_material == null:
		return
	ghost_material.albedo_color = (
		Color(0.2, 0.9, 0.4, 0.35) if preview_valid else Color(0.9, 0.2, 0.15, 0.35)
	)

func _content_display(key: String, fallback: String) -> String:
	var localized := ContentI18n.display(key, i18n.locale())
	return fallback if localized == key else localized


func _starter_display(id: String, fallback: String) -> String:
	if not id.begins_with("base:"):
		return fallback
	var key := "base.content.starter.%s" % id.get_slice(":", 1)
	var localized := ContentI18n.display(key, i18n.locale())
	return fallback if localized == key else localized


func _utility_display(kind: String) -> String:
	return _content_display("base.content.utility.%s" % kind, kind)


func _tutorial_display(step: String, fallback: String) -> String:
	if step == "":
		return fallback
	return _content_display("base.content.tutorial.%s" % step, fallback)


func _update_hud() -> void:
	if hud == null:
		return
	# Any rebuild satisfies a pending tick-driven refresh, so the dirty flag and
	# its cadence window restart here regardless of the caller.
	hud_dirty = false
	hud_refresh_accum = 0.0
	var blueprint_name := _starter_display(str(blueprint_info.get("id", "")), str(blueprint_info.get("name", "none")))
	var rooms := int(blueprint_info.get("rooms", 0))
	var capacity := int(blueprint_info.get("resident_capacity", 0))
	var structure: Dictionary = blueprint_info.get("structure", {})
	var structure_text: String = i18n.t("base.ui.hud.structure")
	if not structure.is_empty() and bool(structure.get("present", false)):
		if bool(structure.get("valid", false)):
			structure_text = i18n.t("base.ui.hud.structure_valid", {
				"system": str(structure.get("system_name", "?")),
				"stories": i18n.format_int(int(structure.get("stories", 0))),
				"max_stories": i18n.format_int(int(structure.get("effective_max_stories", 0))),
				"span": i18n.format_float(float(structure.get("max_span_m", 0.0)), 1),
				"max_span": i18n.format_float(float(structure.get("effective_max_span_m", 0.0)), 1),
				"foundation": i18n.format_float(float(structure.get("foundation_depth_m", 0.0)), 1),
				"required": i18n.format_float(float(structure.get("required_foundation_m", 0.0)), 1),
			})
		else:
			structure_text = i18n.t("base.ui.hud.structure_invalid", {
				"system": str(structure.get("system_name", "?")),
				"error": str(structure.get("error", "")),
				"code": str(structure.get("error_code", "")),
			})
	var selected := _selected_placement()
	var selected_line: String = i18n.t("base.ui.hud.selected_none")
	if not selected.is_empty():
		var selected_info := _info_for_path(_placement_blueprint_path(selected))
		var footprint: PackedFloat32Array = selected.get("footprint", PackedFloat32Array())
		var footprint_text := "? × ? m"
		if footprint.size() == 2:
			footprint_text = "%s × %s m" % [
				i18n.format_float(footprint[0]),
				i18n.format_float(footprint[1]),
			]
		selected_line = i18n.t("base.ui.hud.selected", {
			"id": selected_id,
			"name": _starter_display(str(selected.get("blueprint_id", "")), str(selected.get("name", "?"))),
			"rooms": i18n.format_int(int(selected_info.get("rooms", 0))),
			"capacity": i18n.format_int(int(selected.get("resident_capacity", 0))),
			"jobs": i18n.format_int(int(selected.get("job_slots", 0))),
			"footprint": footprint_text,
		})
		var center: PackedFloat32Array = selected.get("center", PackedFloat32Array())
		if center.size() == 3:
			var owner_info: Dictionary = core.settlement_at(roundi(center[0]), roundi(center[2]))
			var owner_name: String = i18n.t("base.ui.hud.wilderness")
			if bool(owner_info.get("found", false)):
				var settlement: Dictionary = owner_info.get("settlement", {})
				owner_name = str(settlement.get("name", owner_name))
			selected_line += i18n.t("base.ui.hud.selected_owner", {"owner": owner_name})
		var industry_kind := str(industry_kinds.get(selected_id, ""))
		if industry_kind != "":
			selected_line += i18n.t("base.ui.hud.selected_industry", {
				"kind": _content_display("base.content.industry.%s" % industry_kind, industry_kind),
			})
		var utility_kinds_here: Array = utility_kinds.get(selected_id, [])
		if utility_kinds_here.size() > 0:
			var utility_names: Array[String] = []
			for utility_kind in utility_kinds_here:
				utility_names.append(_utility_display(str(utility_kind)))
			selected_line += i18n.t("base.ui.hud.selected_utility", {
				"kinds": ", ".join(utility_names),
			})
		var service_kind := str(service_kinds.get(selected_id, ""))
		if service_kind != "":
			selected_line += i18n.t("base.ui.hud.selected_service", {
				"kind": _content_display("base.content.service.%s" % service_kind, service_kind),
			})
		var selected_structure: Dictionary = selected.get("structure", {})
		if bool(selected_structure.get("present", false)):
			selected_line += i18n.t("base.ui.hud.selected_stories", {
				"system": str(selected_structure.get("system_name", "?")),
				"stories": i18n.format_int(int(selected_structure.get("stories", 0))),
			})
		var named: Dictionary = core.named_citizens_for(SEED, selected_id)
		if int(named.get("count", 0)) > 0:
			var names: Array = []
			for citizen in named.get("citizens", []):
				names.append(str(citizen.get("name", "?")))
			selected_line += i18n.t("base.ui.hud.selected_residents", {
				"names": ", ".join(names),
			})
	if mode == "architect":
		hud.text = "%s\n%s\n%s\n%s" % [
			i18n.t("base.ui.hud.architect.title", {"seed": SEED}),
			i18n.t("base.ui.hud.blueprint", {"name": blueprint_name}),
			structure_text,
			i18n.t("base.ui.help.architect"),
		]
	else:
		hud.text = "%s\n%s\n%s\n%s\n%s\n%s" % [
			i18n.t("base.ui.hud.mayor.title", {"seed": SEED}),
			i18n.t("base.ui.hud.blueprint_mayor", {
				"name": blueprint_name,
				"rooms": i18n.format_int(rooms),
				"capacity": i18n.format_int(capacity),
			}),
			structure_text,
			i18n.t("base.ui.help.move"),
			i18n.t("base.ui.help.edit"),
			i18n.t("base.ui.help.tools", {
				"follow": i18n.t("base.ui.help.follow"),
				"clock": i18n.t("base.ui.help.clock"),
			}),
		]
	var state: String = (
		i18n.t("base.ui.hud.valid")
		if preview_valid
		else i18n.t("base.ui.hud.invalid", {"error": preview_error})
	)
	var construction: Dictionary = core.construction_summary()
	var city: Dictionary = core.city_summary()
	var progression: Dictionary = core.progression_state(SEED)
	var age_text := "—"
	if bool(progression.get("ok", false)):
		var next_age := str(progression.get("next_age", ""))
		if next_age == "":
			age_text = i18n.t("base.ui.hud.age_final", {
				"age": str(progression.get("age", "?")),
			})
		else:
			age_text = i18n.t("base.ui.hud.age_next", {
				"age": str(progression.get("age", "?")),
				"next": next_age,
				"threshold": i18n.format_int(int(progression.get("next_threshold", 0))),
				"progress": i18n.format_int(roundi(float(progression.get("progress", 0.0)) * 100.0)),
			})
	var research: Dictionary = core.research_state(SEED)
	var research_text := "—"
	if bool(research.get("open", false)):
		research_text = i18n.t("base.ui.hud.research", {
			"points": i18n.format_int(roundi(float(research.get("points", 0.0)))),
			"completed": i18n.format_int(int(research.get("completed_count", 0))),
			"total": i18n.format_int(int(research.get("node_count", 0))),
			"rate": i18n.format_float(float(research.get("rate_per_day", 0.0)), 1),
		})
	var tutorial: Dictionary = core.tutorial_state(SEED)
	var tutorial_text := "—"
	if bool(tutorial.get("ok", false)):
		if bool(tutorial.get("complete", false)):
			tutorial_text = i18n.t("base.ui.hud.tutorial_complete", {
				"completed": i18n.format_int(int(tutorial.get("completed", 0))),
				"total": i18n.format_int(int(tutorial.get("total", 0))),
			})
		else:
			tutorial_text = i18n.t("base.ui.hud.tutorial_next", {
				"completed": i18n.format_int(int(tutorial.get("completed", 0))),
				"total": i18n.format_int(int(tutorial.get("total", 0))),
				"title": _tutorial_display(str(tutorial.get("current", "")), str(tutorial.get("current_title", "?"))),
			})
	var economy: Dictionary = core.economy_state()
	var treasury_text := "—"
	if bool(economy.get("open", false)):
		var net := float(economy.get("daily_net", 0.0))
		var net_text: String = ("+" if net >= 0.0 else "") + i18n.format_float(net, 0)
		treasury_text = i18n.t("base.ui.hud.treasury_value", {
			"balance": i18n.format_float(float(economy.get("balance", 0.0)), 0),
			"net": net_text,
			"tax": i18n.format_float(float(economy.get("tax_per_resident_per_day", 0.0)), 0),
			"business": i18n.format_float(float(economy.get("business_tax_per_job_per_day", 0.0)), 0),
		})
		if float(economy.get("debt", 0.0)) > 0.001:
			treasury_text = i18n.t("base.ui.hud.treasury_debt", {
				"value": treasury_text,
				"debt": i18n.format_float(float(economy.get("debt", 0.0)), 0),
			})
		if bool(economy.get("bankrupt", false)):
			treasury_text = i18n.t("base.ui.hud.treasury_bankrupt", {"value": treasury_text})
	var industry: Dictionary = core.industry_state()
	var industry_text := "—"
	if bool(industry.get("open", false)):
		industry_text = i18n.t("base.ui.hud.industry_value", {
			"wood": i18n.format_float(float(industry.get("wood", 0.0)), 0),
			"food": i18n.format_float(float(industry.get("food", 0.0)), 0),
			"hides": i18n.format_float(float(industry.get("hides", 0.0)), 0),
			"supply": i18n.format_float(float(industry.get("food_supply", 0.0)) * 100.0, 0),
		})
	var utilities: Dictionary = core.utilities_state()
	var utilities_text := "—"
	if bool(utilities.get("open", false)):
		utilities_text = i18n.t("base.ui.hud.utilities_value", {
			"water": i18n.format_float(float(utilities.get("water_supply", 0.0)) * 100.0, 0),
			"water_sites": i18n.format_int(int(utilities.get("water_sites", 0))),
			"power": i18n.format_float(float(utilities.get("power_supply", 0.0)) * 100.0, 0),
			"power_sites": i18n.format_int(int(utilities.get("power_sites", 0))),
		})
	var services: Dictionary = core.services_state()
	var service_text := "—"
	if bool(services.get("open", false)):
		service_text = i18n.t("base.ui.hud.civic_value", {
			"school": i18n.format_float(float(services.get("school_supply", 0.0)) * 100.0, 0),
			"clinic": i18n.format_float(float(services.get("clinic_supply", 0.0)) * 100.0, 0),
			"fire": i18n.format_float(float(services.get("fire_supply", 0.0)) * 100.0, 0),
			"police": i18n.format_float(float(services.get("police_supply", 0.0)) * 100.0, 0),
			"research": i18n.format_float(float(services.get("research_per_day", 0.0)), 1),
		})
	var traffic: Dictionary = traffic_stats
	if traffic.is_empty():
		traffic = core.traffic_state(SEED)
	var traffic_text := "—"
	if bool(traffic.get("open", false)):
		traffic_text = i18n.t("base.ui.hud.traffic_value", {
			"nodes": i18n.format_int(int(traffic.get("nodes", 0))),
			"edges": i18n.format_int(int(traffic.get("edges", 0))),
			"length": i18n.format_float(float(traffic.get("total_length_m", 0.0)), 0),
			"cars": i18n.format_int(traffic_cars.size()),
		})
	var happiness_text := "—"
	var needs_text := "—"
	if int(city.get("population", 0)) > 0:
		happiness_text = i18n.format_float(float(city.get("happiness", 0.0)) * 100.0, 0) + "%"
		needs_text = i18n.t("base.ui.hud.needs_value", {
			"food": i18n.format_float(float(city.get("food", 0.0)) * 100.0, 0),
			"water": i18n.format_float(float(city.get("water", 0.0)) * 100.0, 0),
			"rest": i18n.format_float(float(city.get("rest", 0.0)) * 100.0, 0),
		})
	var status_lines: Array[String] = [
		selected_line,
		i18n.t("base.ui.hud.target", {
			"x": int(target_x),
			"z": int(target_z),
			"rotation": rotation_deg,
			"state": state,
		}),
		i18n.t("base.ui.hud.placements", {
			"placements": i18n.format_int(core.world_placement_count()),
			"roads": i18n.format_int(core.road_count()),
			"lots": i18n.format_int(core.lot_count()),
			"settlements": i18n.format_int(core.settlement_count()),
			"edits": i18n.format_int(core.terrain_delta_count()),
		}),
		i18n.t("base.ui.hud.city", {
			"population": i18n.format_int(int(city.get("population", 0))),
			"housing": i18n.format_int(int(city.get("housing_capacity", 0))),
			"vacant": i18n.format_int(int(city.get("vacancies", 0))),
			"jobs": i18n.format_int(int(city.get("jobs", 0))),
			"employed": i18n.format_int(int(city.get("employed", 0))),
			"happiness": happiness_text,
		}),
		i18n.t("base.ui.hud.age", {"value": age_text}),
		i18n.t("base.ui.hud.research_line", {"value": research_text}),
		i18n.t("base.ui.hud.tutorial_line", {"value": tutorial_text}),
		i18n.t("base.ui.hud.commute", {
			"commute": i18n.format_float(float(city.get("average_commute_m", 0.0)), 0),
			"wages": i18n.format_float(float(city.get("daily_wages", 0.0)), 0),
		}),
		i18n.t("base.ui.hud.needs", {"needs": needs_text}),
		i18n.t("base.ui.hud.treasury", {"treasury": treasury_text}),
		i18n.t("base.ui.hud.industry", {"value": industry_text}),
		i18n.t("base.ui.hud.utilities", {"value": utilities_text}),
		i18n.t("base.ui.hud.civic", {"value": service_text}),
		i18n.t("base.ui.hud.traffic", {"value": traffic_text}),
		i18n.t("base.ui.hud.construction", {
			"active": i18n.format_int(int(construction.get("active", 0))),
			"stalled": i18n.format_int(int(construction.get("stalled", 0))),
			"crew": i18n.format_int(int(construction.get("effective_crew", 0))),
			"capacity": i18n.format_int(int(construction.get("crew_capacity", 0))),
			"trades": i18n.format_int(int(construction.get("trades", []).size())),
			"materials": i18n.format_int(int(construction.get("materials_stock", 0))),
			"save": save_path,
			"clock": _construction_clock_text(),
		}),
	]
	if not lifecycle_last_result.is_empty():
		status_lines.append(i18n.t("base.ui.hud.lifecycle", {
			"day": i18n.format_int(lifecycle_days_total),
			"aged": i18n.format_int(int(lifecycle_last_result.get("aged", 0))),
			"died": i18n.format_int(lifecycle_deaths_total),
		}))
	status_label.text = "\n".join(status_lines)
	if materialization_visible:
		status_label.text += "\n" + i18n.t("base.ui.hud.citizens_near", {
			"active": i18n.format_int(materialization_counts.x),
			"detailed": i18n.format_int(materialization_counts.y),
			"near": i18n.format_int(movement_lod_counts.x),
			"far": i18n.format_int(movement_lod_counts.y),
		})
	var heatmap_state: String = i18n.t("base.ui.heatmap.on") if heatmap_visible else i18n.t("base.ui.heatmap.off")
	status_label.text += "\n" + i18n.t("base.ui.heatmap.status", {
		"state": heatmap_state,
		"cells": i18n.format_int(heatmap_cells),
		"peak": _heatmap_peak_text(),
	})
	if heatmap_visible:
		status_label.text += "\n" + i18n.t("base.ui.heatmap.metric_line", {
			"metric": i18n.t(HEATMAP_METRIC_KEYS.get(heatmap_metric, "base.ui.heatmap.metric.population")),
			"peak": _heatmap_peak_text(),
		})
		status_label.text += "\n" + i18n.t("base.ui.heatmap.legend", {
			"metric": i18n.t(HEATMAP_METRIC_KEYS.get(heatmap_metric, "base.ui.heatmap.metric.population")),
		})
		if heatmap_error != "":
			status_label.text += "\n" + i18n.t("base.ui.heatmap.unavailable", {"code": heatmap_error})
		elif heatmap_peak <= 0.0:
			status_label.text += "\n" + i18n.t("base.ui.heatmap.empty")

func _toggle_citizen_panel() -> bool:
	citizen_visible = not citizen_visible
	if citizen_panel != null:
		citizen_panel.visible = citizen_visible
	citizen_refresh_accum = 0.0
	if citizen_visible:
		_update_citizen_panel()
	return citizen_visible

func _close_citizen_panel() -> void:
	citizen_visible = false
	if citizen_panel != null:
		citizen_panel.visible = false
	citizen_refresh_accum = 0.0

func _toggle_follow() -> bool:
	if follow_active:
		_stop_follow(i18n.t("base.ui.citizen.follow_stopped"))
		return false
	return _start_follow()

func _start_follow() -> bool:
	if mode != "mayor" or mayor_camera == null:
		return false
	var selected := _selected_placement()
	if selected.is_empty():
		_set_modal(i18n.t("base.ui.citizen.follow_select_first"), true)
		return false
	var center: PackedFloat32Array = selected.get("center", PackedFloat32Array())
	if center.size() != 3:
		_set_modal(i18n.t("base.ui.citizen.follow_select_first"), true)
		return false
	follow_target = Vector3(center[0], center[1], center[2])
	follow_prev_target = mayor_camera.target
	follow_active = true
	_set_modal(i18n.t("base.ui.citizen.follow_started", {"id": selected_id}), false)
	_update_hud()
	if citizen_visible:
		_update_citizen_panel()
	return true

func _stop_follow(message: String = "") -> void:
	if not follow_active:
		return
	follow_active = false
	if message != "":
		_set_modal(message, false)
	_update_hud()
	if citizen_visible:
		_update_citizen_panel()

func _update_follow(delta: float) -> void:
	if not follow_active or mayor_camera == null:
		return
	if mode != "mayor" or selected_id < 0:
		_stop_follow()
		return
	var current: Vector3 = mayor_camera.target
	var drift := Vector2(
		current.x - follow_prev_target.x, current.z - follow_prev_target.z
	).length()
	if drift > FOLLOW_ARRIVE_EPS_M:
		# The camera target moved outside the follow step: the player panned,
		# so hand control back instead of fighting the existing controls.
		_stop_follow()
		return
	var weight := 1.0 - exp(-FOLLOW_SHARPNESS * maxf(delta, 0.0))
	var next: Vector3 = current.lerp(follow_target, weight)
	if Vector2(
		next.x - follow_target.x, next.z - follow_target.z
	).length() <= FOLLOW_ARRIVE_EPS_M:
		next = follow_target
	mayor_camera.target = next
	follow_prev_target = next

func _household_for(placement_id: int) -> Dictionary:
	var state: Dictionary = core.population_state()
	for household in state.get("households", []):
		if int(household.get("placement_id", -1)) == placement_id:
			return household
	return {}

func _update_citizen_panel() -> void:
	if citizen_text == null:
		return
	var lines: Array[String] = []
	lines.append("[b]%s[/b]  (%s)" % [
		i18n.t("base.ui.citizen.title"),
		i18n.t("base.ui.citizen.close_hint"),
	])
	if selected_id < 0:
		lines.append("")
		lines.append(i18n.t("base.ui.citizen.no_selection"))
		lines.append(i18n.t("base.ui.citizen.no_selection_hint"))
		citizen_text.text = "\n".join(lines)
		return
	var selected := _selected_placement()
	if selected.is_empty():
		lines.append("")
		lines.append(i18n.t("base.ui.citizen.selection_unavailable"))
		citizen_text.text = "\n".join(lines)
		return
	lines.append("")
	lines.append("[b]#%d  %s[/b]" % [selected_id, _starter_display(str(selected.get("blueprint_id", "")), str(selected.get("name", "?")))])
	lines.append(i18n.t("base.ui.citizen.follow_hint"))
	if follow_active:
		lines.append(i18n.t("base.ui.citizen.following", {"id": selected_id}))
	var named: Dictionary = core.named_citizens_for(SEED, selected_id)
	var citizens: Array = named.get("citizens", [])
	if citizens.is_empty():
		lines.append(i18n.t("base.ui.citizen.no_named"))
	else:
		for citizen in citizens:
			lines.append(i18n.t("base.ui.citizen.resident_line", {
				"name": str(citizen.get("name", "?")),
				"slot": int(citizen.get("slot", 0)) + 1,
				"age": int(citizen.get("age_years", 0)),
				"employment": i18n.t("base.ui.citizen.employed") if bool(citizen.get("employed", false)) else i18n.t("base.ui.citizen.unemployed"),
			}))
	var household := _household_for(selected_id)
	if household.is_empty():
		lines.append("")
		lines.append(i18n.t("base.ui.citizen.household_unavailable"))
	else:
		lines.append("")
		lines.append("[b]%s[/b]" % i18n.t("base.ui.citizen.household"))
		lines.append(i18n.t("base.ui.citizen.household_line", {
			"residents": int(household.get("residents", 0)),
			"capacity": int(household.get("capacity", 0)),
			"employed": int(household.get("workers", 0)),
		}))
		lines.append(i18n.t("base.ui.citizen.commute_income", {
			"commute": "%.0f" % float(household.get("commute_m", 0.0)),
			"income": "%.0f" % float(household.get("income_per_day", 0.0)),
		}))
		lines.append(i18n.t("base.ui.citizen.needs", {
			"food": "%.0f" % (float(household.get("food", 0.0)) * 100.0),
			"water": "%.0f" % (float(household.get("water", 0.0)) * 100.0),
			"rest": "%.0f" % (float(household.get("rest", 0.0)) * 100.0),
		}))
	citizen_text.text = "\n".join(lines)

func _toggle_dashboard() -> bool:
	dashboard_visible = not dashboard_visible
	if dashboard_panel != null:
		dashboard_panel.visible = dashboard_visible
	dashboard_refresh_accum = 0.0
	if dashboard_visible:
		_update_dashboard()
	return dashboard_visible

func _update_dashboard() -> void:
	if dashboard_text == null:
		return
	if dashboard_title != null:
		dashboard_title.text = i18n.t("base.ui.dashboard.title")
	if dashboard_hint != null:
		dashboard_hint.text = i18n.t("base.ui.dashboard.close_hint")
	if dashboard_policy_hint != null:
		dashboard_policy_hint.text = i18n.t("base.ui.dashboard.policy_hint")
	var city: Dictionary = core.city_summary()
	var economy: Dictionary = core.economy_state()
	var industry: Dictionary = core.industry_state()
	var utilities: Dictionary = core.utilities_state()
	var services: Dictionary = core.services_state()
	var construction: Dictionary = core.construction_summary()
	var traffic: Dictionary = core.traffic_state(SEED)
	var progression: Dictionary = core.progression_state(SEED)
	var research: Dictionary = core.research_state(SEED)
	var advice: Dictionary = core.city_advisor(SEED)
	var settlement_state: Dictionary = core.settlements()
	_validate_settlement_selection(settlement_state)
	var district_focus := not _district_focus_placement().is_empty()
	var district_center: Vector2 = _district_center_xz()
	var district: Dictionary = core.district_summary(
		SEED, district_center.x, district_center.y, DISTRICT_RADIUS_M
	)
	var policy_state: Dictionary = core.district_policies_state(SEED)
	var population := int(city.get("population", 0))
	var happiness := "—"
	var needs := "—"
	if population > 0:
		happiness = i18n.format_float(float(city.get("happiness", 0.0)) * 100.0, 0) + "%"
		needs = i18n.t("base.ui.dashboard.body.needs_values", {
			"food": i18n.format_float(float(city.get("food", 0.0)) * 100.0, 0),
			"water": i18n.format_float(float(city.get("water", 0.0)) * 100.0, 0),
			"rest": i18n.format_float(float(city.get("rest", 0.0)) * 100.0, 0),
		})
	var treasury: String = i18n.t("base.ui.dashboard.body.treasury_closed")
	if bool(economy.get("open", false)):
		var net := float(economy.get("daily_net", 0.0))
		treasury = i18n.t("base.ui.dashboard.body.treasury", {
			"balance": i18n.format_float(float(economy.get("balance", 0.0)), 0),
			"net": ("+" if net >= 0.0 else "") + i18n.format_float(net, 0),
			"debt": i18n.format_float(float(economy.get("debt", 0.0)), 0),
		})
		if bool(economy.get("bankrupt", false)):
			treasury = i18n.t("base.ui.dashboard.body.treasury_bankrupt", {
				"treasury": treasury,
			})
	var utilities_line: String = i18n.t("base.ui.dashboard.body.utilities_closed")
	if bool(industry.get("open", false)) or bool(utilities.get("open", false)):
		utilities_line = i18n.t("base.ui.dashboard.body.utilities", {
			"food": i18n.format_float(float(industry.get("food_supply", 0.0)) * 100.0, 0),
			"water": i18n.format_float(float(utilities.get("water_supply", 0.0)) * 100.0, 0),
			"power": i18n.format_float(float(utilities.get("power_supply", 0.0)) * 100.0, 0),
		})
	var civic_line: String = i18n.t("base.ui.dashboard.body.civic_closed")
	if bool(services.get("open", false)):
		civic_line = i18n.t("base.ui.dashboard.body.civic", {
			"school": i18n.format_float(float(services.get("school_supply", 0.0)) * 100.0, 0),
			"clinic": i18n.format_float(float(services.get("clinic_supply", 0.0)) * 100.0, 0),
			"fire": i18n.format_float(float(services.get("fire_supply", 0.0)) * 100.0, 0),
			"police": i18n.format_float(float(services.get("police_supply", 0.0)) * 100.0, 0),
			"research": i18n.format_float(float(services.get("research_per_day", 0.0)), 1),
		})
	var roads: String = i18n.t("base.ui.dashboard.body.roads_closed")
	if bool(traffic.get("open", false)):
		roads = i18n.t("base.ui.dashboard.body.roads", {
			"nodes": i18n.format_int(int(traffic.get("nodes", 0))),
			"edges": i18n.format_int(int(traffic.get("edges", 0))),
			"length": i18n.format_float(float(traffic.get("total_length_m", 0.0)), 0),
			"components": i18n.format_int(int(traffic.get("components", 0))),
		})
	var text := "[b]%s[/b]\n%s\n" % [
		i18n.t("base.ui.dashboard.section.population"),
		i18n.t("base.ui.dashboard.body.population", {
			"residents": i18n.format_int(population),
			"occupied": i18n.format_int(population),
			"housing": i18n.format_int(int(city.get("housing_capacity", 0))),
			"vacant": i18n.format_int(int(city.get("vacancies", 0))),
		}),
	]
	if materialization_visible:
		text += i18n.t("base.ui.dashboard.body.citizens_near", {
			"active": i18n.format_int(materialization_counts.x),
			"detailed": i18n.format_int(materialization_counts.y),
		}) + "\n"
	text += _district_section(district, district_focus)
	text += _settlements_section(settlement_state, district, policy_state)
	text += _policies_section(policy_state)
	var age_line := "—"
	if bool(progression.get("ok", false)):
		var next_age := str(progression.get("next_age", ""))
		if next_age == "":
			age_line = i18n.t("base.ui.dashboard.body.age_final", {
				"age": str(progression.get("age", "?")),
			})
		else:
			age_line = i18n.t("base.ui.dashboard.body.age_next", {
				"age": str(progression.get("age", "?")),
				"next": next_age,
				"threshold": i18n.format_int(int(progression.get("next_threshold", 0))),
				"progress": i18n.format_int(roundi(float(progression.get("progress", 0.0)) * 100.0)),
			})
	text += "[b]%s[/b]\n%s\n" % [i18n.t("base.ui.dashboard.section.age"), age_line]
	var research_line := "—"
	if bool(research.get("open", false)):
		research_line = i18n.t("base.ui.dashboard.body.research", {
			"points": i18n.format_int(roundi(float(research.get("points", 0.0)))),
			"rate": i18n.format_float(float(research.get("rate_per_day", 0.0)), 1),
			"completed": i18n.format_int(int(research.get("completed_count", 0))),
			"total": i18n.format_int(int(research.get("node_count", 0))),
		})
	text += "[b]%s[/b]\n%s\n" % [i18n.t("base.ui.dashboard.section.research"), research_line]
	var tutorial: Dictionary = core.tutorial_state(SEED)
	var tutorial_line := "—"
	if bool(tutorial.get("ok", false)):
		if bool(tutorial.get("complete", false)):
			tutorial_line = i18n.t("base.ui.dashboard.body.tutorial_complete", {
				"completed": i18n.format_int(int(tutorial.get("completed", 0))),
				"total": i18n.format_int(int(tutorial.get("total", 0))),
			})
		else:
			tutorial_line = i18n.t("base.ui.dashboard.body.tutorial_next", {
				"completed": i18n.format_int(int(tutorial.get("completed", 0))),
				"total": i18n.format_int(int(tutorial.get("total", 0))),
				"title": _tutorial_display(str(tutorial.get("current", "")), str(tutorial.get("current_title", "?"))),
			})
	text += "[b]%s[/b]\n%s\n" % [i18n.t("base.ui.dashboard.section.tutorial"), tutorial_line]
	text += "[b]%s[/b]\n%s\n" % [
		i18n.t("base.ui.dashboard.section.work"),
		i18n.t("base.ui.dashboard.body.work", {
			"filled": i18n.format_int(int(city.get("employed", 0))),
			"jobs": i18n.format_int(int(city.get("jobs", 0))),
			"unemployed": i18n.format_int(int(city.get("unemployed", 0))),
			"commute": i18n.format_float(float(city.get("average_commute_m", 0.0)), 0),
		}),
	]
	text += "[b]%s[/b]\n%s\n%s\n" % [
		i18n.t("base.ui.dashboard.section.wellbeing"),
		i18n.t("base.ui.dashboard.body.happiness", {"value": happiness}),
		i18n.t("base.ui.dashboard.body.needs", {"needs": needs}),
	]
	text += "[b]%s[/b]\n%s\n[b]%s[/b]\n%s\n[b]%s[/b]\n%s\n[b]%s[/b]\n%s\n" % [
		i18n.t("base.ui.dashboard.section.utilities"),
		utilities_line,
		i18n.t("base.ui.dashboard.section.civic"),
		civic_line,
		i18n.t("base.ui.dashboard.section.treasury"),
		treasury,
		i18n.t("base.ui.dashboard.section.industry"),
		i18n.t("base.ui.dashboard.body.industry", {
			"wood": i18n.format_float(float(industry.get("wood", 0.0)), 0),
			"food": i18n.format_float(float(industry.get("food", 0.0)), 0),
			"hides": i18n.format_float(float(industry.get("hides", 0.0)), 0),
		}),
	]
	text += "[b]%s[/b]\n%s\n[b]%s[/b]\n%s\n" % [
		i18n.t("base.ui.dashboard.section.roads"),
		roads,
		i18n.t("base.ui.dashboard.section.construction"),
		i18n.t("base.ui.dashboard.body.construction", {
			"active": i18n.format_int(int(construction.get("active", 0))),
			"stalled": i18n.format_int(int(construction.get("stalled", 0))),
			"completed": i18n.format_int(int(city.get("completed_buildings", 0))),
		}),
	]
	text += "\n[b]%s[/b]\n" % i18n.t("base.ui.dashboard.section.advisor")
	var messages: Array = advice.get("messages", [])
	if messages.is_empty():
		text += i18n.t("base.ui.dashboard.body.no_advice") + "\n"
	else:
		var advisor_fallback: String = i18n.t("base.ui.dashboard.body.advisor_fallback")
		for message in messages:
			var severity := str(message.get("severity", "info"))
			var color := "#9fd3ff"
			if severity == "critical":
				color = "#ff6b6b"
			elif severity == "warning":
				color = "#ffb454"
			elif severity == "info":
				color = "#9fe6a0"
			var title: String = i18n.advisor_title(
				str(message.get("code", "")),
				str(message.get("title", advisor_fallback)),
			)
			var detail: String = i18n.advisor_detail(
				str(message.get("code", "")),
				str(message.get("detail", "")),
				message.get("params", {}),
			)
			text += "[color=%s]■[/color] [b]%s[/b]\n%s\n" % [
				color,
				title,
				detail,
			]
	dashboard_text.text = text

func _district_focus_placement() -> Dictionary:
	if not district_focus_selection or selected_id < 0:
		return {}
	return _selected_placement()

func _district_center_xz() -> Vector2:
	# Precedence: an explicit settlement selection wins over F6 building focus,
	# which in turn wins over the camera target. A selected settlement id that is
	# missing, malformed, or foreign is cleared by `_settlement_focus_center` so
	# the section falls back to the normal camera/selection path instead of
	# querying a stale centre.
	var settlement_center := _settlement_focus_center()
	if is_finite(settlement_center.x) and is_finite(settlement_center.y):
		return settlement_center
	var focus := _district_focus_placement()
	if not focus.is_empty():
		var center: PackedFloat32Array = focus.get("center", PackedFloat32Array())
		if center.size() == 3:
			return Vector2(center[0], center[2])
		# The selection vanished or its placement is malformed: fall back to
		# the camera rather than querying a stale centre.
		district_focus_selection = false
	if mayor_camera != null:
		return Vector2(mayor_camera.target.x, mayor_camera.target.z)
	return Vector2(target_x, target_z)

func _toggle_district_focus() -> bool:
	if _selected_placement().is_empty():
		district_focus_selection = false
		_set_modal(i18n.t("base.ui.modal.select_building_full"), false)
		return false
	district_focus_selection = not district_focus_selection
	dashboard_refresh_accum = 0.0
	if dashboard_visible:
		_update_dashboard()
	return district_focus_selection

func _district_section(district: Dictionary, focus_selected := false) -> String:
	var section := "[b]%s[/b]\n" % i18n.t("base.ui.dashboard.section.district")
	var action_key := "base.ui.dashboard.body.district_focus_action_camera" if focus_selected else "base.ui.dashboard.body.district_focus_action_selection"
	var settlement := _selected_settlement()
	var settlement_focus := not settlement.is_empty()
	if settlement_focus:
		section += i18n.t("base.ui.dashboard.body.settlement_focus", {
			"name": str(settlement.get("name", "")),
		}) + "\n"
	elif focus_selected:
		section += i18n.t("base.ui.dashboard.body.district_focus") + "\n"
	if not bool(district.get("ok", false)):
		section += i18n.t("base.ui.dashboard.body.district_unavailable") + "\n"
		if selected_id >= 0 and not settlement_focus:
			section += i18n.t(action_key) + "\n"
		return section
	var population := int(district.get("population", 0))
	var happiness: String = i18n.NOT_AVAILABLE
	var needs: String = i18n.NOT_AVAILABLE
	if population > 0:
		happiness = i18n.format_float(float(district.get("happiness", 0.0)) * 100.0, 0) + "%"
		needs = i18n.t("base.ui.dashboard.body.needs_values", {
			"food": i18n.format_float(float(district.get("food", 0.0)) * 100.0, 0),
			"water": i18n.format_float(float(district.get("water", 0.0)) * 100.0, 0),
			"rest": i18n.format_float(float(district.get("rest", 0.0)) * 100.0, 0),
		})
	section += i18n.t("base.ui.dashboard.body.district", {
		"residents": i18n.format_int(population),
		"housing": i18n.format_int(int(district.get("housing_capacity", 0))),
		"vacant": i18n.format_int(int(district.get("vacancies", 0))),
	}) + "\n"
	section += i18n.t("base.ui.dashboard.body.district_jobs", {
		"jobs": i18n.format_int(int(district.get("jobs", 0))),
		"employed": i18n.format_int(int(district.get("employed", 0))),
	}) + "\n"
	section += i18n.t("base.ui.dashboard.body.district_needs", {
		"happiness": happiness,
		"needs": needs,
		"commute": i18n.format_float(float(district.get("average_commute_m", 0.0)), 0),
	}) + "\n"
	if settlement_focus:
		section += i18n.t("base.ui.dashboard.body.settlement_focus_hint", {
			"radius": i18n.format_float(DISTRICT_RADIUS_M, 0),
		}) + "\n"
	else:
		section += i18n.t(
			"base.ui.dashboard.body.district_focus_hint" if focus_selected else "base.ui.dashboard.body.district_hint",
			{"radius": i18n.format_float(DISTRICT_RADIUS_M, 0)},
		) + "\n"
	if selected_id >= 0 and not settlement_focus:
		section += i18n.t(action_key) + "\n"
	return section

## Dashboard settlement list. Ids are stable selectors; PageUp/PageDown cycle
## the same id order with a leading "none" entry (see `_cycle_settlement`).
## When a settlement is selected, a read-only detail block follows the list:
## it reuses the dashboard's existing 256 m `district_summary` payload and the
## `district_policies_state` result, so it never adds a bridge call or save field.
func _settlements_section(state: Dictionary, district: Dictionary = {}, policy_state: Dictionary = {}) -> String:
	var section := "[b]%s[/b]\n" % i18n.t("base.ui.dashboard.section.settlements")
	if not bool(state.get("ok", true)):
		section += i18n.t("base.ui.dashboard.body.settlements_unavailable") + "\n"
		return section
	_validate_settlement_selection(state)
	var records := _settlement_records(state)
	if records.is_empty():
		section += i18n.t("base.ui.dashboard.body.settlements_none") + "\n"
		return section
	var selected_name: String = i18n.t("base.ui.dashboard.body.settlement_none")
	var selected := _settlement_by_id(selected_settlement_id, state)
	if not selected.is_empty():
		selected_name = str(selected.get("name", ""))
	section += i18n.t("base.ui.dashboard.body.settlements_count", {
		"count": i18n.format_int(records.size()),
		"selected": selected_name,
	}) + "\n"
	for record in records:
		var id := int(record.get("id", -1))
		var center := _settlement_center(record)
		var params := {
			"id": id,
			"name": str(record.get("name", "")),
			"owner": str(record.get("owner", "")),
			"x": int(center.x),
			"z": int(center.y),
			"radius": i18n.format_float(float(record.get("radius_m", 0.0)), 0),
		}
		if id == selected_settlement_id:
			section += "[b]%s[/b]\n" % i18n.t("base.ui.dashboard.body.settlement_row_selected", params)
		else:
			section += i18n.t("base.ui.dashboard.body.settlement_row", params) + "\n"
	section += i18n.t("base.ui.dashboard.body.settlement_hint") + "\n"
	if not selected.is_empty():
		section += _settlement_detail_section(selected, district, policy_state)
	return section

## Read-only per-settlement detail block under the Settlements list. The caller
## passes the payloads the dashboard already queried this rebuild, so this adds
## no bridge call. Missing, malformed, foreign-seed, or `ok=false` district data
## clears every derived value and falls back to the existing unavailable line.
func _settlement_detail_section(settlement: Dictionary, district: Dictionary, policy_state: Dictionary) -> String:
	var center := _settlement_center(settlement)
	if settlement.is_empty() or not is_finite(center.x) or not is_finite(center.y):
		return ""
	var block := "[b]%s[/b]\n" % i18n.t("base.ui.dashboard.body.settlement_detail_title")
	block += i18n.t("base.ui.dashboard.body.settlement_detail", {
		"name": str(settlement.get("name", "")),
		"owner": str(settlement.get("owner", "")),
		"id": int(settlement.get("id", 0)),
		"x": int(center.x),
		"z": int(center.y),
		"radius": i18n.format_float(float(settlement.get("radius_m", 0.0)), 0),
	}) + "\n"
	block += _settlement_membership_line(settlement) + "\n"
	var district_center := _xz_from_pair(district.get("center", null))
	if not bool(district.get("ok", false)) or not is_finite(district_center.x) \
		or not is_finite(district_center.y) or district_center.distance_to(center) >= 0.001:
		block += i18n.t("base.ui.dashboard.body.district_unavailable") + "\n"
		return block
	var population := int(district.get("population", 0))
	var happiness: String = i18n.NOT_AVAILABLE
	var needs: String = i18n.NOT_AVAILABLE
	if population > 0:
		happiness = i18n.format_float(float(district.get("happiness", 0.0)) * 100.0, 0) + "%"
		needs = i18n.t("base.ui.dashboard.body.needs_values", {
			"food": i18n.format_float(float(district.get("food", 0.0)) * 100.0, 0),
			"water": i18n.format_float(float(district.get("water", 0.0)) * 100.0, 0),
			"rest": i18n.format_float(float(district.get("rest", 0.0)) * 100.0, 0),
		})
	block += i18n.t("base.ui.dashboard.body.settlement_detail_district", {
		"radius": i18n.format_float(DISTRICT_RADIUS_M, 0),
		"residents": i18n.format_int(population),
		"housing": i18n.format_int(int(district.get("housing_capacity", 0))),
		"vacant": i18n.format_int(int(district.get("vacancies", 0))),
	}) + "\n"
	block += i18n.t("base.ui.dashboard.body.settlement_detail_jobs", {
		"jobs": i18n.format_int(int(district.get("jobs", 0))),
		"employed": i18n.format_int(int(district.get("employed", 0))),
		"unemployed": i18n.format_int(int(district.get("unemployed", 0))),
	}) + "\n"
	block += i18n.t("base.ui.dashboard.body.district_needs", {
		"happiness": happiness,
		"needs": needs,
		"commute": i18n.format_float(float(district.get("average_commute_m", 0.0)), 0),
	}) + "\n"
	# `district_summary` always reports both counts; if a payload ever omits
	# them, drop the line instead of inventing zeros.
	if district.has("completed_buildings") and district.has("construction_sites"):
		block += i18n.t("base.ui.dashboard.body.settlement_detail_buildings", {
			"completed": i18n.format_int(int(district.get("completed_buildings", 0))),
			"sites": i18n.format_int(int(district.get("construction_sites", 0))),
		}) + "\n"
	if not bool(policy_state.get("ok", false)):
		block += i18n.t("base.ui.dashboard.body.policies_unavailable") + "\n"
		return block
	var policies_value = policy_state.get("policies", [])
	var policies: Array = policies_value if policies_value is Array else []
	var policy := _containing_policy(policies, center)
	if policy.is_empty():
		block += i18n.t("base.ui.dashboard.body.settlement_detail_policy_none") + "\n"
		return block
	block += i18n.t("base.ui.dashboard.body.settlement_detail_policy", {
		"name": str(policy.get("name", "")),
		"tax": i18n.format_float(float(policy.get("tax_multiplier", 0.0)), 2),
		"funding": i18n.format_float(float(policy.get("service_funding", 0.0)), 2),
	}) + "\n"
	block += i18n.t(
		"base.ui.dashboard.body.policy_construction_allowed" if bool(policy.get("construction_allowed", false)) else "base.ui.dashboard.body.policy_construction_blocked"
	) + "\n"
	var allowlist_count := _policy_allowlist_count(policy.get("blueprint_allowlist", null))
	block += i18n.t("base.ui.dashboard.body.policy_allowlist", {
		"count": i18n.format_int(allowlist_count) if allowlist_count > 0 else i18n.NOT_AVAILABLE,
	}) + "\n"
	return block

## One compact D-033 membership line for the settlement detail block. It reads
## only the `settlements()` record the list already rendered: `owned`,
## `owner_id`, `members`, and `membership_revision`. Role counts are sorted by
## role name so the rebuilt dashboard text stays identical, and numbers use
## `WorldI18n.format_int`. An unowned record has no member/owner count to show.
## Next settlement tax step. The store accepts 0..=100; out-of-range input is
## clamped into that window first, and 100 wraps to 0 so the control never
## sends an invalid rate. Pure so the smoke can pin the whole cycle.
func _next_settlement_tax(current: int) -> int:
	var clamped := clampi(current, 0, 100)
	if clamped >= 100:
		return 0
	return clamped + 5

## Shift+T: step the selected settlement's tax by 5% through the bridge.
## Only the Settlements section writes here. The key is gated by the dashboard
## and a valid selection; an unowned record is rejected locally without a
## bridge call or a store mutation. Success refreshes the list/detail in place
## (same selected id) and reports the returned rate and store checksum.
func _cycle_selected_settlement_tax() -> bool:
	var settlement := _selected_settlement()
	if settlement.is_empty():
		return false
	if not bool(settlement.get("owned", false)):
		_set_modal("settlement.unowned", true)
		_update_hud()
		return false
	var id := int(settlement.get("id", -1))
	if id < 0:
		return false
	var next_rate := _next_settlement_tax(int(settlement.get("tax_rate", 0)))
	var result: Dictionary = core.settlement_set_tax(id, next_rate)
	if not bool(result.get("ok", false)):
		_set_modal("%s: %s" % [
			str(result.get("error_code", "unknown")),
			str(result.get("error", "")),
		], true)
		_update_hud()
		return false
	var updated: Dictionary = result.get("settlement", {})
	var rate := int(updated.get("tax_rate", next_rate))
	_set_modal("Settlement tax: %d%% · checksum %s" % [
		rate,
		str(result.get("checksum", "")),
	], false)
	dashboard_refresh_accum = 0.0
	_update_dashboard()
	_update_hud()
	return true

func _settlement_membership_line(settlement: Dictionary) -> String:
	var revision: String = i18n.format_int(int(settlement.get("membership_revision", 0)))
	if not bool(settlement.get("owned", false)):
		return "Members %s · revision %s" % [i18n.NOT_AVAILABLE, revision]
	var owner_value = settlement.get("owner_id", 0)
	var owner_id := int(owner_value) if owner_value != null else 0
	var rows_value = settlement.get("members", [])
	var rows: Array = rows_value if rows_value is Array else []
	var counts := {}
	for member in rows:
		if member is Dictionary:
			var role := str(member.get("role", "member"))
			counts[role] = int(counts.get(role, 0)) + 1
	var roles: Array = counts.keys()
	roles.sort()
	var suffix := ""
	for role in roles:
		suffix += " · %s %s" % [str(role), i18n.format_int(int(counts[role]))]
	return "Owner #%s · %s members · revision %s%s · tax %s%%" % [
		i18n.format_int(owner_id),
		i18n.format_int(rows.size()),
		revision,
		suffix,
		i18n.format_int(int(settlement.get("tax_rate", 0))),
	]

## Read an `[x, z]` pair from a plain or packed array; NAN when malformed.
## A detail-only helper: it never converts missing values into a fake zero.
func _xz_from_pair(value) -> Vector2:
	if not (value is Array or value is PackedInt32Array or value is PackedFloat32Array):
		return Vector2(NAN, NAN)
	if value.size() < 2:
		return Vector2(NAN, NAN)
	return Vector2(float(value[0]), float(value[1]))

func _policies_section(state: Dictionary) -> String:
	var section := "[b]%s[/b]\n" % i18n.t("base.ui.dashboard.section.policies")
	if not bool(state.get("ok", false)):
		section += i18n.t("base.ui.dashboard.body.policies_unavailable") + "\n"
		return section
	var policies_value = state.get("policies", [])
	var policies: Array = policies_value if policies_value is Array else []
	section += i18n.t("base.ui.dashboard.body.policies_count", {
		"count": i18n.format_int(policies.size()),
	}) + "\n"
	var policy := _containing_policy(policies, _district_center_xz())
	if policy.is_empty():
		section += i18n.t("base.ui.dashboard.body.policy_none") + "\n"
		return section
	section += i18n.t("base.ui.dashboard.body.policy", {
		"name": str(policy.get("name", "")),
		"tax": i18n.format_float(float(policy.get("tax_multiplier", 0.0)), 2),
		"funding": i18n.format_float(float(policy.get("service_funding", 0.0)), 2),
	}) + "\n"
	section += i18n.t(
		"base.ui.dashboard.body.policy_construction_allowed" if bool(policy.get("construction_allowed", false)) else "base.ui.dashboard.body.policy_construction_blocked"
	) + "\n"
	var allowlist_count := _policy_allowlist_count(policy.get("blueprint_allowlist", null))
	section += i18n.t("base.ui.dashboard.body.policy_allowlist", {
		"count": i18n.format_int(allowlist_count) if allowlist_count > 0 else i18n.NOT_AVAILABLE,
	}) + "\n"
	return section

## Smallest-id policy covering the dashboard district centre, or an error.
## `no_policy` is a local target state, never a bridge error code.
func _policy_edit_target(seed: int) -> Dictionary:
	var state: Dictionary = core.district_policies_state(seed)
	if not bool(state.get("ok", false)):
		return {"ok": false, "error_code": _policy_error_code(state)}
	var policy := _containing_policy(_policy_array(state), _district_center_xz())
	if policy.is_empty():
		return {"ok": false, "error_code": "no_policy"}
	return {"ok": true, "policy": policy}

## Resolve the editable policy and report a missing/failed target once.
func _policy_target_or_report(seed: int) -> Dictionary:
	var target := _policy_edit_target(seed)
	if bool(target.get("ok", false)):
		return target
	var code := str(target.get("error_code", "failed"))
	if code == "no_policy":
		_set_modal(i18n.t("base.ui.policies.no_target"), true)
	else:
		_set_modal(i18n.t("base.ui.policies.failed", {"code": code}), true)
	return target

## Create the deterministic default policy at the district centre: radius 64 m,
## tax 1.0, funding 1.0, construction allowed, empty allowlist, named
## `District {n}` for n = live count + 1. The store assigns the id.
func _policy_create_default(seed: int = SEED) -> Dictionary:
	var state: Dictionary = core.district_policies_state(seed)
	if not bool(state.get("ok", false)):
		return _policy_failure(_policy_error_code(state))
	var index := _policy_array(state).size() + 1
	var center: Vector2 = _district_center_xz()
	var result: Dictionary = core.district_policy_create(
		seed,
		"District %d" % index,
		center.x,
		center.y,
		POLICY_DEFAULT_RADIUS_M,
		POLICY_DEFAULT_TAX,
		POLICY_DEFAULT_FUNDING,
		true,
		PackedStringArray()
	)
	if bool(result.get("ok", false)):
		var policy: Dictionary = result.get("policy", {})
		_set_modal(i18n.t("base.ui.policies.created", {
			"id": i18n.format_int(int(policy.get("id", 0))),
			"name": str(policy.get("name", "")),
		}), false)
		_policy_refresh_dashboard()
	else:
		_policy_failure(_policy_error_code(result))
	return result

## Remove the containing policy by id.
func _policy_remove_selected(seed: int = SEED) -> Dictionary:
	var target := _policy_target_or_report(seed)
	if not bool(target.get("ok", false)):
		return target
	var policy: Dictionary = target.get("policy", {})
	var id := int(policy.get("id", 0))
	if id <= 0:
		return _policy_failure("invalid_record")
	var result: Dictionary = core.district_policy_remove(seed, id)
	if bool(result.get("ok", false)):
		_set_modal(i18n.t("base.ui.policies.removed", {
			"id": i18n.format_int(id),
		}), false)
		_policy_refresh_dashboard()
	else:
		_policy_failure(_policy_error_code(result))
	return result

## Flip `construction_allowed` on the containing policy.
func _policy_toggle_construction(seed: int = SEED) -> Dictionary:
	var target := _policy_target_or_report(seed)
	if not bool(target.get("ok", false)):
		return target
	var policy: Dictionary = target.get("policy", {})
	var current = policy.get("construction_allowed", null)
	if not (current is bool):
		return _policy_failure("invalid_record")
	return _policy_apply_update(seed, policy, {"construction_allowed": not bool(current)})

func _policy_adjust_tax(delta: float, seed: int = SEED) -> Dictionary:
	return _policy_adjust_field(seed, "tax_multiplier", delta, POLICY_MIN_TAX, POLICY_MAX_TAX)

func _policy_adjust_funding(delta: float, seed: int = SEED) -> Dictionary:
	return _policy_adjust_field(seed, "service_funding", delta, 0.0, POLICY_MAX_FUNDING)

## Step one numeric field by `delta`, clamp to the bridge bounds, and send the
## complete record. A step that clamps back to the current value sends nothing.
func _policy_adjust_field(
	seed: int, field: String, delta: float, minimum: float, maximum: float
) -> Dictionary:
	var target := _policy_target_or_report(seed)
	if not bool(target.get("ok", false)):
		return target
	var policy: Dictionary = target.get("policy", {})
	var value = policy.get(field, null)
	if not _policy_is_number(value):
		return _policy_failure("invalid_record")
	var current := float(value)
	var next := clampf(snappedf(current + delta, POLICY_STEP_SNAP), minimum, maximum)
	if is_equal_approx(next, current):
		return {"ok": true, "unchanged": true, "policy": policy}
	return _policy_apply_update(seed, policy, {field: next})

## Send the full record through `district_policy_update`. Fields not named in
## `overrides` come from the state payload unchanged; a missing or mistyped
## field rejects locally as `invalid_record` instead of inventing a value.
func _policy_apply_update(seed: int, policy: Dictionary, overrides: Dictionary) -> Dictionary:
	var request := _policy_update_request(policy, overrides)
	if not bool(request.get("ok", false)):
		return _policy_failure(str(request.get("error_code", "invalid_record")))
	var values: Dictionary = request["request"]
	var result: Dictionary = core.district_policy_update(
		seed,
		int(values["id"]),
		str(values["name"]),
		float(values["center_x"]),
		float(values["center_z"]),
		float(values["radius_m"]),
		float(values["tax_multiplier"]),
		float(values["service_funding"]),
		bool(values["construction_allowed"]),
		values["blueprint_allowlist"]
	)
	if bool(result.get("ok", false)):
		_set_modal(i18n.t("base.ui.policies.updated", {
			"id": i18n.format_int(int(values["id"])),
		}), false)
		_policy_refresh_dashboard()
	else:
		_policy_failure(_policy_error_code(result))
	return result

## Build the exact update request from the state record plus explicit
## overrides. Every required field must exist with the expected type; the
## dashboard never substitutes a default for a missing field.
func _policy_update_request(policy: Dictionary, overrides: Dictionary) -> Dictionary:
	var center := _policy_center(policy.get("center", null))
	var allowlist = policy.get("blueprint_allowlist", null)
	var id := int(policy.get("id", 0))
	var policy_name = policy.get("name", null)
	var radius = policy.get("radius_m", null)
	var tax = policy.get("tax_multiplier", null)
	var funding = policy.get("service_funding", null)
	var construction = policy.get("construction_allowed", null)
	var valid := id > 0 and policy_name is String \
		and is_finite(center.x) and is_finite(center.y) \
		and _policy_is_number(radius) and _policy_is_number(tax) \
		and _policy_is_number(funding) and construction is bool \
		and (allowlist is Array or allowlist is PackedStringArray)
	if not valid:
		return {"ok": false, "error_code": "invalid_record"}
	return {"ok": true, "request": {
		"id": id,
		"name": policy_name,
		"center_x": center.x,
		"center_z": center.y,
		"radius_m": float(radius),
		"tax_multiplier": float(overrides.get("tax_multiplier", tax)),
		"service_funding": float(overrides.get("service_funding", funding)),
		"construction_allowed": bool(overrides.get("construction_allowed", construction)),
		"blueprint_allowlist": _policy_allowlist_copy(allowlist),
	}}

## Copy the allowlist without changing entries or order.
func _policy_allowlist_copy(value) -> PackedStringArray:
	var copy := PackedStringArray()
	if value is PackedStringArray:
		for entry in value:
			copy.append(entry)
	elif value is Array:
		for entry in value:
			copy.append(str(entry))
	return copy

func _policy_is_number(value) -> bool:
	return value is int or value is float

func _policy_array(state: Dictionary) -> Array:
	var value = state.get("policies", [])
	return value if value is Array else []

func _policy_error_code(result: Dictionary) -> String:
	var code := str(result.get("error_code", ""))
	return code if not code.is_empty() else "failed"

func _policy_failure(code: String) -> Dictionary:
	_set_modal(i18n.t("base.ui.policies.failed", {"code": code}), true)
	return {"ok": false, "error_code": code}

func _policy_refresh_dashboard() -> void:
	dashboard_refresh_accum = 0.0
	if dashboard_visible:
		_update_dashboard()

## Smallest-id policy whose inclusive XZ circle contains `point`.
func _containing_policy(policies: Array, point: Vector2) -> Dictionary:
	var best: Dictionary = {}
	for value in policies:
		if not (value is Dictionary):
			continue
		var policy: Dictionary = value
		var center := _policy_center(policy.get("center", null))
		var radius := float(policy.get("radius_m", 0.0))
		if not is_finite(center.x) or not is_finite(center.y):
			continue
		if not is_finite(radius) or radius < 0.0:
			continue
		if point.distance_to(center) > radius:
			continue
		var id := int(policy.get("id", 0))
		if best.is_empty() or id < int(best.get("id", 0)):
			best = policy
	return best

## Read an `[x, z]` centre from a plain or packed array; NAN when malformed.
func _policy_center(value) -> Vector2:
	if not (value is Array or value is PackedFloat32Array):
		return Vector2(NAN, NAN)
	if value.size() < 2:
		return Vector2(NAN, NAN)
	return Vector2(float(value[0]), float(value[1]))

## Allowlist length for a plain or packed string array.
func _policy_allowlist_count(value) -> int:
	if value is Array or value is PackedStringArray:
		return value.size()
	return 0

func _info_for_path(path: String) -> Dictionary:
	if path == "":
		return {}
	if blueprint_info_cache.has(path):
		return blueprint_info_cache[path]
	var info: Dictionary = core.world_blueprint_info(path)
	blueprint_info_cache[path] = info
	return info

## Package path a placement should render/query from.
##
## A world loaded with `world_load_with_library` can carry a bridge-only
## `resolved_blueprint_path` when the recorded package file is gone. The
## recorded `blueprint_path` stays authoritative for saves.
func _placement_blueprint_path(placement: Dictionary, fallback := "") -> String:
	var path := str(placement.get("blueprint_path", fallback))
	var resolved := str(placement.get("resolved_blueprint_path", ""))
	return resolved if resolved != "" else path

func _selected_placement() -> Dictionary:
	if selected_id < 0:
		return {}
	var state: Dictionary = core.world_placements()
	for placement in state.get("placements", []):
		if int(placement.get("id", -1)) == selected_id:
			return placement
	return {}

func _select_at_screen(screen: Vector2) -> bool:
	var id := _pick_placement_at(screen)
	if id < 0:
		_clear_selection()
		return false
	return _select_placement(id)

func _select_placement(id: int) -> bool:
	if id != selected_id:
		_stop_follow()
	selected_id = id
	var placement := _selected_placement()
	if placement.is_empty():
		_clear_selection()
		return false
	var center: PackedFloat32Array = placement.get("center", PackedFloat32Array())
	var footprint: PackedFloat32Array = placement.get("footprint", PackedFloat32Array())
	var bounds: PackedFloat32Array = placement.get("bounds", PackedFloat32Array())
	if center.size() != 3 or footprint.size() != 2 or bounds.size() != 3:
		_clear_selection()
		return false
	var box := BoxMesh.new()
	box.size = Vector3(footprint[0] + 0.4, bounds[1] + 0.4, footprint[1] + 0.4)
	selection_box.mesh = box
	selection_box.position = Vector3(
		center[0], center[1] + bounds[1] * 0.5, center[2]
	)
	selection_box.visible = true
	_update_hud()
	if citizen_visible:
		_update_citizen_panel()
	return true

func _clear_selection() -> void:
	_stop_follow()
	selected_id = -1
	district_focus_selection = false
	if selection_box != null:
		selection_box.visible = false
	_update_hud()
	if citizen_visible:
		_update_citizen_panel()

func _pick_placement_at(screen: Vector2) -> int:
	if camera == null:
		return -1
	var origin := camera.project_ray_origin(screen)
	var direction := camera.project_ray_normal(screen).normalized()
	var state: Dictionary = core.world_placements()
	var best := INF
	var best_id := -1
	for placement in state.get("placements", []):
		var center: PackedFloat32Array = placement.get("center", PackedFloat32Array())
		var footprint: PackedFloat32Array = placement.get("footprint", PackedFloat32Array())
		var bounds: PackedFloat32Array = placement.get("bounds", PackedFloat32Array())
		if center.size() != 3 or footprint.size() != 2 or bounds.size() != 3:
			continue
		var min_point := Vector3(
			center[0] - footprint[0] * 0.5,
			center[1],
			center[2] - footprint[1] * 0.5
		)
		var max_point := Vector3(
			center[0] + footprint[0] * 0.5,
			center[1] + bounds[1],
			center[2] + footprint[1] * 0.5
		)
		var hit := _ray_aabb(origin, direction, min_point, max_point)
		if hit >= 0.0 and hit < best:
			best = hit
			best_id = int(placement.get("id", -1))
	return best_id

func _ray_aabb(
	origin: Vector3, direction: Vector3, min_point: Vector3, max_point: Vector3
) -> float:
	var t_min := -INF
	var t_max := INF
	for axis in range(3):
		if absf(direction[axis]) < 0.000001:
			if origin[axis] < min_point[axis] or origin[axis] > max_point[axis]:
				return -1.0
			continue
		var inverse := 1.0 / direction[axis]
		var t1 := (min_point[axis] - origin[axis]) * inverse
		var t2 := (max_point[axis] - origin[axis]) * inverse
		if t1 > t2:
			var swap := t1
			t1 = t2
			t2 = swap
		t_min = maxf(t_min, t1)
		t_max = minf(t_max, t2)
		if t_min > t_max:
			return -1.0
	if t_min >= 0.0:
		return t_min
	return t_max if t_max >= 0.0 else -1.0

func _door_position(placement: Dictionary) -> Vector3:
	var center: PackedFloat32Array = placement.get("center", PackedFloat32Array())
	var footprint: PackedFloat32Array = placement.get("footprint", PackedFloat32Array())
	if center.size() != 3 or footprint.size() != 2:
		return Vector3.ZERO
	var bounds: PackedFloat32Array = placement.get("bounds", PackedFloat32Array())
	if bounds.size() != 3:
		return Vector3.ZERO
	var rotation := deg_to_rad(float(placement.get("rotation_deg", 0)))
	var front := Vector3(0.0, 0.0, 1.0).rotated(Vector3.UP, rotation)
	var distance := bounds[2] * 0.5 + 2.0
	return Vector3(center[0], center[1], center[2]) + front * distance

func _enter_architect() -> bool:
	if mode != "mayor":
		return false
	_stop_follow()
	if citizen_visible:
		_close_citizen_panel()
	if dashboard_visible:
		_toggle_dashboard()
	var placement := _selected_placement()
	if placement.is_empty():
		_set_modal(i18n.t("base.ui.modal.select_building_full"), true)
		return false
	var door := _door_position(placement)
	var center: PackedFloat32Array = placement.get("center", PackedFloat32Array())
	var ground: float = core.world_height_at(SEED, door.x, door.z)
	var base := center[1] if center.size() == 3 else 0.0
	if ground < 0.0:
		ground = base
	ground = maxf(ground, base)
	architect_camera.global_position = Vector3(door.x, ground + 1.7, door.z)
	if center.size() == 3:
		architect_camera.look_at(
			Vector3(center[0], center[1] + 1.5, center[2]), Vector3.UP
		)
	architect_camera.sync_look_from_rotation()
	architect_camera.current = true
	mayor_camera.current = false
	camera = architect_camera
	mode = "architect"
	streamer.bind_camera(architect_camera)
	ghost.visible = false
	selection_box.visible = false
	if DisplayServer.get_name() != "headless":
		Input.mouse_mode = Input.MOUSE_MODE_CAPTURED
	_set_modal(i18n.t("base.ui.modal.architect_view"), false)
	_update_hud()
	return true

func _return_to_mayor() -> bool:
	if mode != "architect":
		return false
	mode = "mayor"
	camera = mayor_camera
	mayor_camera.current = true
	architect_camera.current = false
	streamer.bind_camera(mayor_camera)
	ghost.visible = true
	selection_box.visible = selected_id >= 0
	if DisplayServer.get_name() != "headless":
		Input.mouse_mode = Input.MOUSE_MODE_VISIBLE
	_set_modal("", false)
	_refresh_preview(true)
	return true

func _edit_target(delta: int) -> bool:
	var current: float = core.world_height_at(SEED, target_x, target_z)
	if current < 0.0:
		_set_modal(i18n.t("base.ui.modal.no_terrain"), true)
		return false
	var height := clampi(roundi(current) + delta, 0, 255)
	var result: Dictionary = core.terrain_edit_height(
		SEED, int(target_x), int(target_z), height
	)
	if not bool(result.get("ok", false)):
		_set_modal(i18n.t("base.ui.modal.terrain_edit_failed", {
			"code": str(result.get("error_code", "unknown")),
		}), true)
		_update_hud()
		return false
	var chunk := Vector2i(
		int(result.get("chunk_x", 0)), int(result.get("chunk_z", 0))
	)
	var local_x := int(result.get("local_x", 0))
	var local_z := int(result.get("local_z", 0))
	for coord in _touched_chunks(chunk, local_x, local_z):
		streamer.invalidate_chunk(coord)
	_set_modal(i18n.t("base.ui.modal.terrain_edit", {
		"height": height,
		"x": int(target_x),
		"z": int(target_z),
	}), false)
	_refresh_preview(true)
	return true

func _touched_chunks(chunk: Vector2i, local_x: int, local_z: int) -> Array[Vector2i]:
	# The far edge comes from the streamer, never a literal 255: a copied
	# CHUNK_CELLS would silently stop invalidating the neighbour chunk (and
	# leave a stale border) if the chunk resolution ever changed.
	var cells: int = streamer.chunk_cell_count() if streamer != null else 1
	var last := maxi(cells, 1) - 1
	var touched: Array[Vector2i] = [chunk]
	if local_x == 0:
		touched.append(chunk + Vector2i(-1, 0))
	elif local_x >= last:
		touched.append(chunk + Vector2i(1, 0))
	if local_z == 0:
		touched.append(chunk + Vector2i(0, -1))
	elif local_z >= last:
		touched.append(chunk + Vector2i(0, 1))
	return touched

func _toggle_road_mode() -> void:
	road_mode = not road_mode
	road_start = Vector2i(-999999, -999999)
	if road_mode:
		lot_mode = false
		lot_start = Vector2i(-999999, -999999)
		_set_modal(i18n.t("base.ui.modal.road_mode_help"), false)
	else:
		_set_modal("", false)
	_update_hud()

func _road_click(screen: Vector2) -> bool:
	var point := _road_ground_point(screen)
	var cell := Vector2i(roundi(point.x), roundi(point.y))
	if road_start.x == -999999:
		road_start = cell
		_set_modal(i18n.t("base.ui.modal.road_start", {"x": cell.x, "z": cell.y}), false)
		return true
	var end := cell
	if absi(end.x - road_start.x) >= absi(end.y - road_start.y):
		end.y = road_start.y
	else:
		end.x = road_start.x
	if end == road_start:
		_set_modal(i18n.t("base.ui.modal.road_zero_length"), true)
		return false
	var ok := _add_road(road_start.x, road_start.y, end.x, end.y)
	if ok:
		_set_modal(i18n.t("base.ui.modal.road_added", {
			"length": absi(end.x - road_start.x) + absi(end.y - road_start.y),
			"x1": road_start.x,
			"z1": road_start.y,
			"x2": end.x,
			"z2": end.y,
		}), false)
		road_start = Vector2i(-999999, -999999)
	return ok

func _add_road(x1: int, z1: int, x2: int, z2: int) -> bool:
	var result: Dictionary = core.road_add(SEED, x1, z1, x2, z2, 8)
	if not bool(result.get("ok", false)):
		_set_modal(i18n.t("base.ui.modal.road_rejected", {
			"code": str(result.get("error_code", "unknown")),
		}), true)
		return false
	_rebuild_roads()
	_update_hud()
	return true

func _rebuild_roads() -> void:
	for node in road_nodes.values():
		if is_instance_valid(node):
			node.free()
	road_nodes.clear()
	var state: Dictionary = core.roads()
	for road in state.get("roads", []):
		_add_road_node(road)
	_rebuild_traffic()

func _add_road_node(road: Dictionary) -> void:
	var id := int(road.get("id", -1))
	if id < 0 or road_nodes.has(id):
		return
	var center: PackedFloat32Array = road.get("center", PackedFloat32Array())
	var start: PackedInt32Array = road.get("start", PackedInt32Array())
	var end: PackedInt32Array = road.get("end", PackedInt32Array())
	var width := float(road.get("width_m", 8))
	var length := float(road.get("length_m", 0))
	if center.size() != 2 or start.size() != 2 or end.size() != 2 or length <= 0.0:
		return
	var ground: float = core.world_height_at(SEED, center[0], center[1])
	if ground < 0.0:
		return
	var box := BoxMesh.new()
	if bool(road.get("runs_along_x", false)):
		box.size = Vector3(length + 0.2, 0.2, width)
	else:
		box.size = Vector3(width, 0.2, length + 0.2)
	var node := MeshInstance3D.new()
	node.mesh = box
	node.material_override = road_material
	node.position = Vector3(center[0], ground + 0.12, center[1])
	node.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	road_root.add_child(node)
	road_nodes[id] = node

func _road_ground_point(screen: Vector2) -> Vector2:
	var origin := camera.project_ray_origin(screen)
	var direction := camera.project_ray_normal(screen).normalized()
	for distance in range(0, 600, 2):
		var point := origin + direction * float(distance)
		var height: float = core.world_height_at(SEED, point.x, point.z)
		if height >= 0.0 and point.y <= height + 0.5:
			return Vector2(point.x, point.z)
	return Vector2(mayor_camera.target.x, mayor_camera.target.z)

func _road_node_matches(road: Dictionary) -> bool:
	var id := int(road.get("id", -1))
	var node: MeshInstance3D = road_nodes.get(id)
	if not is_instance_valid(node) or not (node.mesh is BoxMesh):
		return false
	var box := node.mesh as BoxMesh
	var width := float(road.get("width_m", 8))
	var length := float(road.get("length_m", 0))
	if bool(road.get("runs_along_x", false)):
		return absf(box.size.x - (length + 0.2)) < 0.001 and absf(box.size.z - width) < 0.001
	return absf(box.size.x - width) < 0.001 and absf(box.size.z - (length + 0.2)) < 0.001

func _place_at_target() -> bool:
	if not preview_valid or blueprint_mesh == null:
		_set_modal(i18n.t("base.ui.modal.cannot_place", {"error": preview_error}), true)
		return false
	var result: Dictionary = core.world_place_blueprint(
		SEED, blueprint_path, target_x, target_z, rotation_deg
	)
	if not bool(result.get("ok", false)):
		preview_error = str(result.get("error_code", result.get("error", "place_failed")))
		_set_modal(i18n.t("base.ui.modal.placement_failed", {"error": preview_error}), true)
		_update_hud()
		return false
	_add_placement_visual(result.get("placement", {}))
	_set_modal(i18n.t("base.ui.modal.construction_site"), false)
	_refresh_preview(true)
	return true

func _add_placement_visual(placement: Dictionary, site_override: Dictionary = {}) -> void:
	var site := site_override
	if site.is_empty():
		site = _construction_site_for(int(placement.get("id", -1)))
	if site.is_empty() or bool(site.get("complete", false)):
		_add_placement_node(placement)
	else:
		_add_construction_node(placement, site)

func _construction_index() -> Dictionary:
	var index: Dictionary = {}
	var state: Dictionary = core.construction_sites()
	for site in state.get("sites", []):
		index[int(site.get("id", -1))] = site
	return index

func _construction_site_for(id: int) -> Dictionary:
	if id < 0:
		return {}
	var state: Dictionary = core.construction_sites()
	for site in state.get("sites", []):
		if int(site.get("id", -1)) == id:
			return site
	return {}

func _create_placement_pivot(
	placement: Dictionary, material: StandardMaterial3D, y_scale: float
) -> Node3D:
	var id := int(placement.get("id", -1))
	if id < 0 or placed_nodes.has(id):
		return null
	var path := _placement_blueprint_path(placement, blueprint_path)
	var mesh: ArrayMesh = mesh_cache.get(path)
	if mesh == null:
		mesh = core.build_blueprint_mesh(path)
		if mesh.get_surface_count() == 0:
			return null
		mesh_cache[path] = mesh
	var bounds: PackedFloat32Array = placement.get("bounds", PackedFloat32Array())
	var center: PackedFloat32Array = placement.get("center", PackedFloat32Array())
	if bounds.size() != 3 or center.size() != 3:
		return null
	var pivot := Node3D.new()
	pivot.position = Vector3(center[0], center[1], center[2])
	pivot.rotation_degrees = Vector3(0.0, float(placement.get("rotation_deg", 0)), 0.0)
	var mesh_node := MeshInstance3D.new()
	mesh_node.mesh = mesh
	mesh_node.position = Vector3(-bounds[0] * 0.5, 0.0, -bounds[2] * 0.5)
	mesh_node.scale.y = maxf(y_scale, 0.01)
	mesh_node.material_override = material
	pivot.add_child(mesh_node)
	var foundation := float(placement.get("foundation_m", 0.0))
	if foundation > 0.001:
		# Keep the foundation top above the sampled terrain plane. A top that
		# is exactly coplanar with the terrain z-fights across the whole lot;
		# the 2 cm plinth is hidden at the building edge and removes the fight.
		var foundation_height := foundation + 0.02
		var box := BoxMesh.new()
		box.size = Vector3(bounds[0], foundation_height, bounds[2])
		var foundation_node := MeshInstance3D.new()
		foundation_node.name = "Foundation"
		foundation_node.mesh = box
		foundation_node.position = Vector3(0.0, 0.01 - foundation * 0.5, 0.0)
		foundation_node.material_override = foundation_material
		pivot.add_child(foundation_node)
	return pivot

func _add_placement_node(placement: Dictionary) -> void:
	var pivot := _create_placement_pivot(placement, placed_material, 1.0)
	if pivot == null:
		return
	placed_root.add_child(pivot)
	placed_nodes[int(placement.get("id", -1))] = pivot

func _add_construction_node(placement: Dictionary, site: Dictionary) -> void:
	var id := int(placement.get("id", -1))
	var progress := clampf(float(site.get("progress", 0.0)), 0.02, 0.98)
	var pivot := _create_placement_pivot(placement, construction_material, progress)
	if pivot == null:
		return
	var bounds: PackedFloat32Array = placement.get("bounds", PackedFloat32Array())
	var label := Label3D.new()
	label.billboard = BaseMaterial3D.BILLBOARD_ENABLED
	label.no_depth_test = true
	label.font_size = 28
	label.position = Vector3(0.0, (bounds[1] if bounds.size() == 3 else 6.0) + 1.2, 0.0)
	label.text = _construction_label(site)
	pivot.add_child(label)
	placed_root.add_child(pivot)
	placed_nodes[id] = pivot
	construction_nodes[id] = {
		"pivot": pivot,
		"mesh": pivot.get_child(0),
		"label": label,
		"placement": placement,
	}

func _construction_label(site: Dictionary) -> String:
	var phase := str(site.get("phase_label", site.get("phase", "Construction")))
	var trade := str(site.get("trade_name", ""))
	var percent := roundi(float(site.get("progress", 0.0)) * 100.0)
	var stall := str(site.get("stall_reason", ""))
	var trade_text := "" if trade == "" else " · " + trade
	if stall != "":
		return "%s%s %d%% · stalled: %s" % [phase, trade_text, percent, stall]
	return "%s%s %d%%" % [phase, trade_text, percent]

func _refresh_construction_visuals() -> void:
	if construction_nodes.is_empty():
		return
	var state: Dictionary = core.construction_sites()
	var sites: Dictionary = {}
	for site in state.get("sites", []):
		sites[int(site.get("id", -1))] = site
	for id in construction_nodes.keys().duplicate():
		var entry: Dictionary = construction_nodes[id]
		var site: Dictionary = sites.get(id, {})
		if site.is_empty() or bool(site.get("complete", false)):
			var pivot: Node3D = entry.get("pivot")
			if is_instance_valid(pivot):
				pivot.free()
			placed_nodes.erase(id)
			construction_nodes.erase(id)
			_add_placement_node(entry.get("placement", {}))
			continue
		var mesh: MeshInstance3D = entry.get("mesh")
		var label: Label3D = entry.get("label")
		if is_instance_valid(mesh):
			mesh.scale.y = clampf(float(site.get("progress", 0.0)), 0.02, 0.98)
		if is_instance_valid(label):
			label.text = _construction_label(site)

func _research_next() -> void:
	var state: Dictionary = core.research_state(SEED)
	var nodes: Array = state.get("nodes", [])
	var best := ""
	var best_cost := INF
	for node in nodes:
		if bool(node.get("available", false)) and float(node.get("cost", INF)) < best_cost:
			best = str(node.get("id", ""))
			best_cost = float(node.get("cost", INF))
	if best == "":
		_set_modal(i18n.t("base.ui.modal.research_none"), true)
		return
	var result: Dictionary = core.research_unlock(SEED, best)
	if bool(result.get("ok", false)):
		_set_modal(i18n.t("base.ui.modal.researched", {
			"name": str(result.get("name", best)),
		}), false)
	else:
		_set_modal(i18n.t("base.ui.modal.research_failed", {
			"error": str(result.get("error", "unknown")),
		}), true)
	_update_hud()

func _advance_lifecycle_clock(delta: float) -> void:
	if not lifecycle_clock_enabled or not is_finite(delta) or delta <= 0.0:
		return
	lifecycle_accum_days += delta / LIFECYCLE_SECONDS_PER_DAY
	var days := floori(lifecycle_accum_days)
	if days < 1:
		return
	var population: Dictionary = core.population_state()
	if not bool(population.get("open", false)):
		# Keep at most one day banked so a later population store catches up
		# without replaying the whole paused clock.
		lifecycle_accum_days = minf(lifecycle_accum_days, 1.0)
		return
	var days_to_tick := float(mini(days, LIFECYCLE_MAX_DAYS_PER_TICK))
	var result := _tick_named_lifecycle(days_to_tick)
	if bool(result.get("ok", false)):
		lifecycle_accum_days = maxf(0.0, lifecycle_accum_days - days_to_tick)
	else:
		lifecycle_accum_days = 0.0


func _tick_named_lifecycle(days: float) -> Dictionary:
	if core == null or not is_finite(days) or days < 0.0:
		return {"ok": false, "error_code": "invalid_days"}
	var population: Dictionary = core.population_state()
	if not bool(population.get("open", false)):
		return {"ok": false, "error_code": "no_population"}
	var result: Dictionary = core.named_lifecycle_tick(SEED, days)
	if not bool(result.get("ok", false)):
		var code := str(result.get("error_code", result.get("error", "lifecycle_failed")))
		if code != lifecycle_last_error:
			lifecycle_last_error = code
			push_warning("Named lifecycle tick failed: " + code)
		return result
	lifecycle_last_error = ""
	lifecycle_last_result = result
	lifecycle_days_total += int(floor(days))
	lifecycle_deaths_total += int(result.get("died", 0))
	_update_hud()
	if citizen_visible:
		_update_citizen_panel()
	if materialization_visible:
		_step_materialization()
	return result


func _advance_construction(seconds: float, defer_hud := false) -> Dictionary:
	if seconds <= 0.0:
		return {}
	core.construction_set_trades(SEED)
	if construction_clock_mode == "campaign_days" and construction_clock_seconds_per_day > 0.0:
		campaign_clock_days += seconds / construction_clock_seconds_per_day
	var report: Dictionary = core.construction_tick(seconds)
	core.utilities_refresh(SEED)
	core.services_refresh(SEED)
	core.industry_tick(SEED, seconds)
	core.population_tick(SEED, seconds)
	core.research_tick(SEED, seconds)
	core.economy_tick(SEED, seconds)
	_refresh_construction_visuals()
	# The `_process` loop runs up to eight ticks per frame and marks the HUD
	# dirty instead of rebuilding it per tick; every other caller (input, smoke,
	# save/load) still wants the panel updated immediately.
	if defer_hud:
		hud_dirty = true
	else:
		_update_hud()
	return report


func _refresh_construction_clock_state() -> Dictionary:
	var state: Dictionary = core.construction_time_scale_state()
	if bool(state.get("ok", false)):
		construction_clock_mode = str(state.get("mode", "developer_seconds"))
		construction_clock_seconds_per_day = float(state.get("seconds_per_game_day", 0.0))
		construction_clock_checksum = str(state.get("checksum", ""))
	else:
		var code := str(state.get("error_code", "clock_state_failed"))
		if code != construction_clock_last_error:
			construction_clock_last_error = code
			push_warning("Construction clock state failed: " + code)
	return state


func _construction_clock_text() -> String:
	if construction_clock_mode == "campaign_days":
		return i18n.t("base.ui.hud.clock_campaign", {
			"seconds": i18n.format_float(construction_clock_seconds_per_day, 0),
			"day": i18n.format_int(floori(campaign_clock_days)),
		})
	return i18n.t("base.ui.hud.clock_developer")


func _toggle_construction_clock() -> bool:
	var next_mode := "developer" if construction_clock_mode == "campaign_days" else "campaign"
	var seconds := 0.0 if next_mode == "developer" else CAMPAIGN_SECONDS_PER_DAY
	var result: Dictionary = core.set_construction_time_scale(next_mode, seconds)
	if not bool(result.get("ok", false)):
		var code := str(result.get("error_code", result.get("error", "clock_failed")))
		if code != construction_clock_last_error:
			construction_clock_last_error = code
			push_warning("Construction clock change failed: " + code)
		return false
	construction_clock_last_error = ""
	construction_clock_mode = str(result.get("mode", "developer_seconds"))
	construction_clock_seconds_per_day = float(result.get("seconds_per_game_day", 0.0))
	construction_clock_checksum = str(result.get("checksum", ""))
	if construction_clock_mode == "campaign_days":
		campaign_clock_days = 0.0
	_update_hud()
	return true

func _toggle_crew() -> void:
	var summary: Dictionary = core.construction_summary()
	var capacity := int(summary.get("crew_capacity", 4))
	var next := 0 if capacity > 0 else 4
	if core.construction_set_crew(next):
		_set_modal(i18n.t("base.ui.modal.crew", {"count": next}), false)
	_update_hud()

func _clear_placed_nodes() -> void:
	for node in placed_nodes.values():
		if is_instance_valid(node):
			node.free()
	placed_nodes.clear()
	construction_nodes.clear()

func _clear_placements() -> void:
	_clear_selection()
	_clear_placed_nodes()
	core.world_clear_placements()
	_refresh_industry_visuals()
	_refresh_utility_visuals()
	_refresh_service_visuals()
	_set_modal("", false)
	_refresh_preview(true)
	_update_hud()

func _save_world() -> bool:
	var result: Dictionary = core.world_save(save_path)
	var ok := bool(result.get("ok", false))
	if ok:
		_set_modal(i18n.t("base.ui.modal.saved", {
			"placements": core.world_placement_count(),
			"edits": core.terrain_delta_count(),
			"sites": int(result.get("construction_sites", 0)),
			"roads": core.road_count(),
			"lots": core.lot_count(),
			"settlements": core.settlement_count(),
			"path": save_path,
		}), false)
	else:
		_set_modal(i18n.t("base.ui.modal.save_failed", {
			"error": str(result.get("error_code", result.get("error", ""))),
		}), true)
	_update_hud()
	return ok

func _load_world() -> bool:
	var result: Dictionary = core.world_load(save_path, SEED)
	if not bool(result.get("ok", false)):
		_set_modal(i18n.t("base.ui.modal.load_failed", {
			"error": str(result.get("error_code", result.get("error", ""))),
		}), true)
		_update_hud()
		return false
	_clear_selection()
	_clear_placed_nodes()
	_rebuild_roads()
	_rebuild_lots()
	_rebuild_settlements()
	_refresh_industry_visuals()
	_refresh_utility_visuals()
	_refresh_service_visuals()
	streamer.invalidate_all()
	var state: Dictionary = core.world_placements()
	var construction_index := _construction_index()
	for placement in state.get("placements", []):
		_add_placement_visual(
			placement, construction_index.get(int(placement.get("id", -1)), {})
		)
	if placed_nodes.size() != int(state.get("count", -1)):
		_set_modal(
			i18n.t("base.ui.modal.loaded_mismatch", {
				"placements": core.world_placement_count(),
				"rebuilt": placed_nodes.size(),
			}),
			true
		)
		_update_hud()
		return false
	_set_modal(i18n.t("base.ui.modal.loaded", {
		"placements": core.world_placement_count(),
		"path": save_path,
	}), false)
	_refresh_preview(true)
	return true

func _set_modal(message: String, is_error: bool) -> void:
	if modal_label == null:
		return
	modal_label.text = message
	modal_label.visible = message != ""
	modal_label.add_theme_color_override(
		"font_color", Color(1.0, 0.55, 0.35) if is_error else Color(0.75, 1.0, 0.75)
	)

func _foundation_matches_bounds(node: Node) -> bool:
	if node.get_child_count() < 2:
		return false
	var foundation_node := node.get_child(1) as MeshInstance3D
	if foundation_node == null or not (foundation_node.mesh is BoxMesh):
		return false
	var box := foundation_node.mesh as BoxMesh
	var bounds: PackedFloat32Array = blueprint_info.get("bounds", PackedFloat32Array())
	if bounds.size() != 3:
		return false
	# The foundation top must sit above the placement base; a coplanar top
	# z-fights with the terrain across the whole footprint.
	var foundation_top := foundation_node.position.y + box.size.y * 0.5
	return absf(box.size.x - bounds[0]) < 0.001 and absf(box.size.z - bounds[2]) < 0.001 and foundation_top > 0.001

## Smoke-only: dispatch one synthetic key through the real input handler so the
## dashboard policy bindings are exercised, not just their action helpers.
func _smoke_press_key(code: int, shift := false) -> void:
	var event := InputEventKey.new()
	event.physical_keycode = code
	event.shift_pressed = shift
	event.pressed = true
	_unhandled_input(event)

## Smoke-only contract for the compact editor: a value edit must preserve the
## name, centre, radius, and allowlist it never edits.
func _smoke_policy_preserved(policy: Dictionary, center: Vector2, policy_name: String) -> bool:
	if policy.is_empty():
		return false
	if str(policy.get("name", "")) != policy_name:
		return false
	if absf(float(policy.get("radius_m", 0.0)) - POLICY_DEFAULT_RADIUS_M) > 0.001:
		return false
	if _policy_center(policy.get("center", null)).distance_to(center) > 0.001:
		return false
	var allowlist = policy.get("blueprint_allowlist", null)
	return _policy_allowlist_count(allowlist) == 1 and str(allowlist[0]) == "base:workshop"

func _run_scene_smoke() -> void:
	var status := "ok"
	var preview_ok := false
	var water_ok := false
	var rotation_ok := false
	var edit_ok := false
	var chunk_cells_ok := false
	var place_ok := false
	var road_ok := false
	var traffic_ok := false
	var lot_ok := false
	var settlement_ok := false
	var select_ok := false
	var enter_ok := false
	var return_ok := false
	var build_ok := false
	var build_done_ok := false
	var trades_ok := false
	var named_ok := false
	var citizens_ok := false
	var citizen_rows_ok := false
	var citizen_i18n_ok := false
	var content_i18n_ok := false
	var content_ui_ok := false
	var materialization_ok := false
	var materialization_step_ok := false
	var materialization_fade_ok := false
	var movement_scene_ok := false
	var movement_pool_ok := false
	var movement_routes_ok := false
	var movement_lod_ok := false
	var follow_ok := false
	var render_ok := false
	var structure_ok := false
	var age_ok := false
	var research_ok := false
	var occupancy_before_ok := false
	var occupancy_after_ok := false
	var industry_ok := false
	var utility_ok := false
	var service_ok := false
	var tutorial_ok := false
	var starter_ok := false
	var economy_ok := false
	var dashboard_ok := false
	var district_ui_ok := false
	var policy_ui_ok := false
	var policy_edit_ok := false
	var district_select_ok := false
	var settlement_select_ok := false
	var settlement_detail_ok := false
	var dashboard_tax_ok := false
	var heatmap_ui_ok := false
	var heatmap_modes_ok := false
	var advisor_i18n_ok := false
	var advisor_detail_i18n_ok := false
	var hud_i18n_ok := false
	var modal_i18n_ok := false
	var number_i18n_ok := false
	var hud_body_i18n_ok := false
	var hud_status_i18n_ok := false
	var dashboard_body_i18n_ok := false
	var lifecycle_scene_ok := false
	var campaign_hud_ok := false
	var save_ok := false
	var clear_ok := false
	var load_ok := false
	var rooms := int(blueprint_info.get("rooms", -1))
	var smoke_locale: String = i18n.locale()
	# Keep the English assertions below deterministic on a German desktop;
	# the citizen check switches to `de` explicitly and restores this locale.
	i18n.set_locale("en")

	# Empty world: no settlement store, no rows, no selection. Reading and
	# cycling must not create a store, and the section must render the
	# localized "none" text (plus the unavailable state on a failed read).
	var empty_settlements: Dictionary = core.settlements()
	settlement_select_ok = not bool(empty_settlements.get("open", true))
	settlement_select_ok = settlement_select_ok and int(empty_settlements.get("count", -1)) == 0
	settlement_select_ok = settlement_select_ok and _settlement_records(empty_settlements).is_empty()
	settlement_select_ok = settlement_select_ok and not _select_settlement(1)
	settlement_select_ok = settlement_select_ok and selected_settlement_id == -1
	settlement_select_ok = settlement_select_ok and _cycle_settlement(1) == -1
	settlement_select_ok = settlement_select_ok and not bool(core.settlements().get("open", true))
	var empty_settlement_section := _settlements_section(empty_settlements)
	settlement_select_ok = settlement_select_ok and empty_settlement_section.contains(i18n.t("base.ui.dashboard.body.settlements_none"))
	settlement_select_ok = settlement_select_ok and not empty_settlement_section.contains("!base.ui.")
	settlement_select_ok = settlement_select_ok and _settlements_section({"ok": false}).contains(i18n.t("base.ui.dashboard.body.settlements_unavailable"))
	settlement_select_ok = settlement_select_ok and not _settlements_section({"ok": false}).contains("!base.ui.")
	# Detail block: zero settlements means no block at all; an unavailable read
	# keeps the existing unavailable line and never invents a record.
	settlement_detail_ok = not empty_settlement_section.contains(i18n.t("base.ui.dashboard.body.settlement_detail_title"))
	settlement_detail_ok = settlement_detail_ok and not empty_settlement_section.contains(i18n.t("base.ui.dashboard.body.settlement_detail_policy_none"))
	settlement_detail_ok = settlement_detail_ok and _settlement_detail_section({}, {}, {}).is_empty()
	settlement_detail_ok = settlement_detail_ok and not _settlements_section({"ok": false}, {}, {}).contains(i18n.t("base.ui.dashboard.body.settlement_detail_title"))
	settlement_detail_ok = settlement_detail_ok and not _settlements_section({"ok": false}, {}, {}).contains("!base.ui.")

	target_x = FIXTURE_X
	target_z = FIXTURE_Z
	rotation_deg = 0
	_refresh_preview(true)
	preview_ok = preview_valid and ghost_material.albedo_color.g > ghost_material.albedo_color.r and absf(ghost.position.y - 41.0) < 0.001
	if not preview_ok:
		status = "preview_failed:" + preview_error

	target_x = 128.0
	target_z = 128.0
	_refresh_preview(true)
	water_ok = not preview_valid and preview_error == "water" and ghost_material.albedo_color.r > ghost_material.albedo_color.g
	var water_place: Dictionary = core.world_place_blueprint(SEED, blueprint_path, 128.0, 128.0, 0)
	var economy_after_reject: Dictionary = core.economy_state()
	water_ok = water_ok and not bool(water_place.get("ok", true))
	water_ok = water_ok and not bool(economy_after_reject.get("open", true))
	if not water_ok and status == "ok":
		status = "water_failed:" + preview_error

	rotation_deg = 90
	target_x = FIXTURE_X
	target_z = FIXTURE_Z
	_refresh_preview(true)
	rotation_ok = preview_valid and absf(ghost.rotation_degrees.y - 90.0) < 0.001
	if not rotation_ok and status == "ok":
		status = "rotation_failed:" + preview_error

	if status == "ok":
		var bad_road: Dictionary = core.road_add(SEED, 120, 128, 136, 128, 8)
		var empty_after_reject := not bool(core.world_placements().get("open", true))
		var road_added := _add_road(180, 125, 200, 125)
		var roads_state: Dictionary = core.roads()
		var road_list: Array = roads_state.get("roads", [])
		road_ok = not bool(bad_road.get("ok", true)) and empty_after_reject
		road_ok = road_ok and road_added and core.road_count() == 1
		road_ok = road_ok and road_nodes.size() == 1
		road_ok = road_ok and road_list.size() == 1 and _road_node_matches(road_list[0])
		road_ok = road_ok and bool(core.world_placements().get("open", false))
		road_ok = road_ok and core.world_placement_count() == 0
		if not road_ok:
			status = "road_failed:" + str(bad_road.get("error_code", ""))

		var traffic: Dictionary = core.traffic_state(SEED)
		traffic_ok = bool(traffic.get("open", false))
		traffic_ok = traffic_ok and int(traffic.get("nodes", -1)) == 2
		traffic_ok = traffic_ok and int(traffic.get("edges", -1)) == 1
		traffic_ok = traffic_ok and int(traffic.get("components", -1)) == 1
		traffic_ok = traffic_ok and bool(traffic.get("connected", false))
		traffic_ok = traffic_ok and absf(float(traffic.get("total_length_m", 0.0)) - 20.0) < 0.01
		traffic_ok = traffic_ok and traffic_cars.size() == 1
		if traffic_cars.size() == 1:
			var car: MeshInstance3D = traffic_cars.values()[0]
			var before_car := car.position
			_update_traffic(1.0)
			traffic_ok = traffic_ok and car.position.distance_to(before_car) > 0.5
		if not traffic_ok:
			status = "traffic_failed"

		var bad_lot: Dictionary = core.lot_add(SEED, 120, 128, 136, 140, "residential")
		var lot_added := _add_lot(184, 128, 198, 140, "residential")
		var lots_state: Dictionary = core.lots()
		var lot_list: Array = lots_state.get("lots", [])
		lot_ok = not bool(bad_lot.get("ok", true)) and str(bad_lot.get("error_code", "")) == "no_road_frontage"
		lot_ok = lot_ok and lot_added and core.lot_count() == 1 and lot_nodes.size() == 1
		lot_ok = lot_ok and lot_list.size() == 1 and _lot_node_matches(lot_list[0])
		if not lot_ok:
			status = "lot_failed:" + str(bad_lot.get("error_code", ""))

		var settlement_found := _found_settlement()
		var settlement_state: Dictionary = core.settlements()
		var settlement_list: Array = settlement_state.get("settlements", [])
		var owner_state: Dictionary = core.settlement_at(int(FIXTURE_X), int(FIXTURE_Z))
		settlement_ok = settlement_found and core.settlement_count() == 1
		settlement_ok = settlement_ok and settlement_list.size() == 1
		settlement_ok = settlement_ok and _settlement_node_matches(settlement_list[0])
		settlement_ok = settlement_ok and bool(owner_state.get("found", false))
		var owner_settlement: Dictionary = owner_state.get("settlement", {})
		settlement_ok = settlement_ok and str(owner_settlement.get("owner", "")) == "player"
		if not settlement_ok:
			status = "settlement_failed"

	edit_ok = _edit_target(1) and core.terrain_delta_count() == 1 and absf(core.world_height_at(SEED, FIXTURE_X, FIXTURE_Z) - 42.0) < 0.001 and streamer.dirty_count() > 0
	var border_touched := _touched_chunks(Vector2i(0, 0), 0, 255)
	edit_ok = edit_ok and border_touched.size() == 3 and border_touched.has(Vector2i(-1, 0)) and border_touched.has(Vector2i(0, 1))
	# The border test must follow the streamer's cell count, not a copied 255.
	var chunk_cells: int = streamer.chunk_cell_count()
	var last_cell := chunk_cells - 1
	var far_corner := _touched_chunks(Vector2i(0, 0), last_cell, last_cell)
	chunk_cells_ok = chunk_cells == 256
	chunk_cells_ok = chunk_cells_ok and far_corner.size() == 3
	chunk_cells_ok = chunk_cells_ok and far_corner.has(Vector2i(1, 0))
	chunk_cells_ok = chunk_cells_ok and far_corner.has(Vector2i(0, 1))
	chunk_cells_ok = chunk_cells_ok and _touched_chunks(Vector2i(0, 0), 1, 1).size() == 1
	if not chunk_cells_ok and status == "ok":
		status = "chunk_cells_failed"
	# A chunk that is still on its first build must also be invalidated by load.
	streamer.in_flight[9999] = {"coord": Vector2i(1, 1), "step": 1, "generation": 0}
	streamer.invalidate_all()
	edit_ok = edit_ok and streamer.dirty.has(Vector2i(1, 1))
	streamer.in_flight.erase(9999)
	if not edit_ok and status == "ok":
		status = "edit_failed:" + preview_error

	place_ok = _place_at_target() and core.world_placement_count() == 1 and placed_root.get_child_count() == 1
	if place_ok:
		var node := placed_root.get_child(0)
		place_ok = absf(node.rotation_degrees.y - 90.0) < 0.001 and _foundation_matches_bounds(node)
		var economy_after_place: Dictionary = core.economy_state()
		var construction_spent := float(economy_after_place.get("construction_spent", 0.0))
		var material_cost := float(blueprint_info.get("material_cost", -1.0))
		var expected_cost := snappedf(material_cost * 0.03, 0.01)
		place_ok = place_ok and bool(economy_after_place.get("open", false))
		place_ok = place_ok and material_cost > 0.0
		place_ok = place_ok and absf(construction_spent - expected_cost) < 0.01
		place_ok = place_ok and absf(float(economy_after_place.get("balance", 0.0)) - (STARTING_TREASURY - expected_cost)) < 0.01
	if not place_ok and status == "ok":
		status = "place_failed:" + preview_error

	if status == "ok":
		var site := _construction_site_for(1)
		var construction_summary: Dictionary = core.construction_summary()
		build_ok = not site.is_empty() and construction_summary.sites == 1
		build_ok = build_ok and construction_summary.has("payment_due") and construction_summary.has("payment_stalled")
		build_ok = build_ok and construction_nodes.has(1) and int(site.get("crew_required", 0)) >= 1
		var before_progress := float(site.get("progress", 0.0))
		_advance_construction(2.0)
		var after_site := _construction_site_for(1)
		build_ok = build_ok and float(after_site.get("progress", 0.0)) > before_progress
		var trade_summary: Dictionary = core.construction_summary()
		trades_ok = int(trade_summary.get("trades", []).size()) > 0
		trades_ok = trades_ok and int(trade_summary.get("effective_crew", 0)) >= 1
		var trade_site := _construction_site_for(1)
		trades_ok = trades_ok and str(trade_site.get("trade", "")) != ""
		var city_before: Dictionary = core.city_summary()
		occupancy_before_ok = int(city_before.get("population", -1)) == 0
		occupancy_before_ok = occupancy_before_ok and int(city_before.get("housing_capacity", -1)) == 0
		occupancy_before_ok = occupancy_before_ok and int(city_before.get("jobs", -1)) == 0
		occupancy_before_ok = occupancy_before_ok and int(city_before.get("completed_buildings", -1)) == 0
		occupancy_before_ok = occupancy_before_ok and int(city_before.get("construction_sites", -1)) == 1
		if not build_ok:
			status = "construction_failed"
		elif not trades_ok:
			status = "trades_failed"

	if status == "ok":
		mayor_camera.focus(Vector3(FIXTURE_X, 42.0, FIXTURE_Z), 60.0)
		mayor_camera.current = true
		architect_camera.current = false
		camera = mayor_camera
		var screen := camera.unproject_position(Vector3(FIXTURE_X, 44.0, FIXTURE_Z))
		var miss_ok := not _select_at_screen(screen + Vector2(2000.0, 0.0))
		select_ok = miss_ok and _select_at_screen(screen) and selected_id >= 0 and selection_box.visible
		if select_ok:
			var selected := _selected_placement()
			var selected_info := _info_for_path(_placement_blueprint_path(selected))
			select_ok = int(selected_info.get("rooms", -1)) == 4
		if select_ok:
			var selected := _selected_placement()
			var home_center: PackedFloat32Array = selected.get("center", PackedFloat32Array())
			follow_ok = home_center.size() == 3 and _start_follow()
			for _step in 40:
				_update_follow(0.05)
			follow_ok = follow_ok and mayor_camera.target.distance_to(Vector3(home_center[0], home_center[1], home_center[2])) < 0.5
			# Selection changes and Architect entry cancel the follow.
			_clear_selection()
			follow_ok = follow_ok and not follow_active
			follow_ok = follow_ok and _select_placement(1)
			follow_ok = follow_ok and _start_follow()
			follow_ok = follow_ok and _enter_architect()
			follow_ok = follow_ok and not follow_active
			follow_ok = follow_ok and _return_to_mayor()
		if select_ok:
			var mayor_target: Vector3 = mayor_camera.target
			enter_ok = _enter_architect() and mode == "architect" and camera == architect_camera
			if enter_ok:
				var selected := _selected_placement()
				var center: PackedFloat32Array = selected.get("center", PackedFloat32Array())
				enter_ok = architect_camera.current and streamer.camera == architect_camera
				enter_ok = enter_ok and not ghost.visible and not selection_box.visible
				if center.size() == 3:
					enter_ok = enter_ok and architect_camera.global_position.y >= center[1] + 1.7 - 0.001
					var forward: Vector3 = -architect_camera.global_transform.basis.z
					var to_center: Vector3 = Vector3(center[0], center[1] + 1.5, center[2]) - architect_camera.global_position
					enter_ok = enter_ok and forward.dot(to_center.normalized()) > 0.9
				return_ok = _return_to_mayor() and mode == "mayor" and camera == mayor_camera
				return_ok = return_ok and mayor_camera.current and streamer.camera == mayor_camera
				return_ok = return_ok and ghost.visible and selection_box.visible
				return_ok = return_ok and mayor_camera.target.distance_to(mayor_target) < 0.001
		if not select_ok:
			status = "select_failed"
		elif not enter_ok:
			status = "enter_failed"
		elif not return_ok:
			status = "return_failed"

	var saved_site_progress := 0.0
	if status == "ok":
		saved_site_progress = float(_construction_site_for(1).get("progress", 0.0))
		save_ok = _save_world()
		if not save_ok:
			status = "save_failed"

	var road_checksum := ""
	var traffic_checksum := ""
	var lot_checksum := ""
	var settlement_checksum := ""
	if status == "ok":
		var edit_checksum := str(core.terrain_delta_checksum())
		road_checksum = str(core.road_checksum())
		traffic_checksum = str(core.traffic_state(SEED).get("checksum", ""))
		lot_checksum = str(core.lot_checksum())
		settlement_checksum = str(core.settlement_checksum())
		_clear_placements()
		clear_ok = core.world_placement_count() == 0 and placed_root.get_child_count() == 0
		clear_ok = clear_ok and core.construction_summary().sites == 0
		if not clear_ok:
			status = "clear_failed"
		else:
			core.terrain_clear_deltas()
			core.road_clear()
			_rebuild_roads()
			core.lot_clear()
			_rebuild_lots()
			_clear_settlements()
			if core.terrain_delta_count() != 0 or core.road_count() != 0 or road_nodes.size() != 0:
				status = "delta_clear_failed"
			if core.lot_count() != 0 or lot_nodes.size() != 0:
				status = "lot_clear_failed"
			if core.settlement_count() != 0 or settlement_nodes.size() != 0:
				status = "settlement_clear_failed"
			load_ok = _load_world() and core.world_placement_count() == 1 and placed_root.get_child_count() == 1
			if load_ok:
				var loaded_placements: Dictionary = core.world_placements()
				var loaded_placement_list: Array = loaded_placements.get("placements", [])
				var loaded_structure: Dictionary = {}
				if loaded_placement_list.size() == 1:
					loaded_structure = loaded_placement_list[0].get("structure", {})
				load_ok = bool(loaded_structure.get("present", false))
				load_ok = load_ok and str(loaded_structure.get("system", "")) == "timber_frame"
				load_ok = load_ok and int(loaded_structure.get("stories", 0)) == 2
				var node := placed_root.get_child(0)
				load_ok = absf(node.rotation_degrees.y - 90.0) < 0.001 and _foundation_matches_bounds(node)
				load_ok = load_ok and core.terrain_delta_count() == 1
				load_ok = load_ok and absf(core.world_height_at(SEED, FIXTURE_X, FIXTURE_Z) - 42.0) < 0.001
				load_ok = load_ok and str(core.terrain_delta_checksum()) == edit_checksum
				load_ok = load_ok and core.terrain_deltas_seed() == SEED
				load_ok = load_ok and core.road_count() == 1
				load_ok = load_ok and str(core.road_checksum()) == road_checksum
				load_ok = load_ok and road_nodes.size() == 1
				load_ok = load_ok and traffic_cars.size() == 1
				load_ok = load_ok and str(core.traffic_state(SEED).get("checksum", "")) == traffic_checksum
				load_ok = load_ok and core.lot_count() == 1
				load_ok = load_ok and str(core.lot_checksum()) == lot_checksum
				load_ok = load_ok and lot_nodes.size() == 1
				load_ok = load_ok and core.settlement_count() == 1
				load_ok = load_ok and str(core.settlement_checksum()) == settlement_checksum
				load_ok = load_ok and settlement_nodes.size() == 1
				if core.road_count() == 1:
					var roads_state: Dictionary = core.roads()
					var road_list: Array = roads_state.get("roads", [])
					load_ok = load_ok and road_list.size() == 1 and _road_node_matches(road_list[0])
				if core.lot_count() == 1:
					var lots_state: Dictionary = core.lots()
					var lot_list: Array = lots_state.get("lots", [])
					load_ok = load_ok and lot_list.size() == 1 and _lot_node_matches(lot_list[0])
				if core.settlement_count() == 1:
					var settlements_state: Dictionary = core.settlements()
					var settlement_list: Array = settlements_state.get("settlements", [])
					load_ok = load_ok and settlement_list.size() == 1 and _settlement_node_matches(settlement_list[0])
					var owner_after: Dictionary = core.settlement_at(int(FIXTURE_X), int(FIXTURE_Z))
					var owner_record: Dictionary = owner_after.get("settlement", {})
					load_ok = load_ok and bool(owner_after.get("found", false))
					load_ok = load_ok and str(owner_record.get("owner", "")) == "player"
				load_ok = load_ok and construction_nodes.has(1) and not placed_nodes.is_empty()
				var loaded_site := _construction_site_for(1)
				load_ok = load_ok and absf(float(loaded_site.get("progress", -1.0)) - saved_site_progress) < 0.001
				var loaded_entry: Dictionary = construction_nodes.get(1, {})
				var loaded_mesh: MeshInstance3D = loaded_entry.get("mesh")
				var loaded_label: Label3D = loaded_entry.get("label")
				load_ok = load_ok and is_instance_valid(loaded_mesh)
				load_ok = load_ok and absf(loaded_mesh.scale.y - saved_site_progress) < 0.01
				load_ok = load_ok and is_instance_valid(loaded_label) and loaded_label.text.contains("%")
			if load_ok:
				_advance_construction(1000.0)
				var done_site := _construction_site_for(1)
				build_done_ok = bool(done_site.get("complete", false)) and construction_nodes.is_empty()
				build_done_ok = build_done_ok and placed_nodes.has(1) and placed_root.get_child_count() == 1
				build_done_ok = build_done_ok and _foundation_matches_bounds(placed_root.get_child(0))
				var finished_mesh := placed_root.get_child(0).get_child(0) as MeshInstance3D
				build_done_ok = build_done_ok and finished_mesh != null and finished_mesh.material_override == placed_material
				build_done_ok = build_done_ok and absf(finished_mesh.scale.y - 1.0) < 0.001
				if finished_mesh != null and finished_mesh.mesh != null:
					var arrays: Array = finished_mesh.mesh.surface_get_arrays(0)
					render_ok = placed_material.vertex_color_use_as_albedo
					render_ok = render_ok and arrays.size() > Mesh.ARRAY_COLOR
					if render_ok:
						render_ok = (arrays[Mesh.ARRAY_COLOR] as PackedColorArray).size() > 0
				var blueprint_structure: Dictionary = blueprint_info.get("structure", {})
				structure_ok = bool(blueprint_structure.get("valid", false))
				structure_ok = structure_ok and str(blueprint_structure.get("system", "")) == "timber_frame"
				structure_ok = structure_ok and int(blueprint_structure.get("stories", 0)) == 2
				structure_ok = structure_ok and int(blueprint_structure.get("effective_max_stories", 0)) == 3
				var placement_state: Dictionary = core.world_placements()
				var placement_list: Array = placement_state.get("placements", [])
				if placement_list.size() == 1:
					var placement_structure: Dictionary = placement_list[0].get("structure", {})
					structure_ok = structure_ok and bool(placement_structure.get("present", false))
					structure_ok = structure_ok and str(placement_structure.get("system", "")) == "timber_frame"
					structure_ok = structure_ok and int(placement_structure.get("stories", 0)) == 2
				else:
					structure_ok = false
				var pacing_ok: bool = core.population_set_rate(1.0)
				core.population_clear()
				_advance_construction(0.25)
				pacing_ok = pacing_ok and int(core.city_summary().get("population", -1)) == 0
				_advance_construction(2.0)
				pacing_ok = pacing_ok and int(core.city_summary().get("population", -1)) == 2
				var city_after: Dictionary = core.city_summary()
				occupancy_after_ok = pacing_ok
				occupancy_after_ok = occupancy_after_ok and int(city_after.get("population", -1)) == 2
				occupancy_after_ok = occupancy_after_ok and int(city_after.get("housing_capacity", -1)) == 2
				occupancy_after_ok = occupancy_after_ok and int(city_after.get("jobs", -1)) == 2
				occupancy_after_ok = occupancy_after_ok and int(city_after.get("employed", -1)) == 2
				occupancy_after_ok = occupancy_after_ok and int(city_after.get("completed_buildings", -1)) == 1
				occupancy_after_ok = occupancy_after_ok and int(city_after.get("construction_sites", -1)) == 0
				occupancy_after_ok = occupancy_after_ok and int(city_after.get("vacancies", -1)) == 0
				occupancy_after_ok = occupancy_after_ok and float(city_after.get("happiness", 0.0)) > 0.5
				occupancy_after_ok = occupancy_after_ok and absf(float(city_after.get("average_commute_m", -1.0))) < 0.001
				occupancy_after_ok = occupancy_after_ok and absf(float(city_after.get("daily_wages", 0.0)) - 40.0) < 0.01
				occupancy_after_ok = occupancy_after_ok and float(city_after.get("food", 0.0)) > 0.0
				occupancy_after_ok = occupancy_after_ok and float(city_after.get("water", 0.0)) > 0.0
				occupancy_after_ok = occupancy_after_ok and float(city_after.get("rest", 0.0)) > 0.0
				var named_home: Dictionary = core.named_citizens_for(SEED, 1)
				named_ok = int(named_home.get("count", 0)) == 2
				var named_ids: Array = []
				for citizen in named_home.get("citizens", []):
					named_ids.append(int(citizen.get("id", 0)))
					named_ok = named_ok and str(citizen.get("name", "")) != ""
				var named_again: Dictionary = core.named_citizens_for(SEED, 1)
				var again_ids: Array = []
				for citizen in named_again.get("citizens", []):
					again_ids.append(int(citizen.get("id", 0)))
				named_ok = named_ok and named_ids == again_ids
				var known_name := ""
				if not named_home.get("citizens", []).is_empty():
					known_name = str(named_home.get("citizens", [])[0].get("name", ""))
				_clear_selection()
				_toggle_citizen_panel()
				citizens_ok = citizen_panel.visible
				citizens_ok = citizens_ok and citizen_text.text.contains("No home selected")
				_close_citizen_panel()
				citizens_ok = citizens_ok and not citizen_panel.visible
				citizens_ok = citizens_ok and _select_placement(1)
				_toggle_citizen_panel()
				citizens_ok = citizens_ok and citizen_panel.visible
				citizens_ok = citizens_ok and known_name != "" and citizen_text.text.contains(known_name)
				citizens_ok = citizens_ok and citizen_text.text.contains("2 / 2 residents")
				citizens_ok = citizens_ok and citizen_text.text.contains("40/day")
				citizen_rows_ok = citizen_text.text.contains("age ") and (citizen_text.text.contains("employed") or citizen_text.text.contains("unemployed"))
				content_i18n_ok = ContentI18n.display("base.content.service.school", "en") == "School"
				content_i18n_ok = content_i18n_ok and ContentI18n.display("base.content.service.school", "de") == "Schule"
				var saved_content_locale: String = i18n.locale()
				i18n.set_locale("de")
				content_ui_ok = _starter_display("base:cottage", "Cottage") == "Kate"
				content_ui_ok = content_ui_ok and _utility_display("water") == "Wasser"
				content_ui_ok = content_ui_ok and _tutorial_display("house", "House") == "Haus"
				i18n.set_locale("en")
				content_ui_ok = content_ui_ok and _starter_display("base:cottage", "Cottage") == "Cottage"
				content_ui_ok = content_ui_ok and _utility_display("water") == "Water"
				content_ui_ok = content_ui_ok and _tutorial_display("house", "House") == "House"
				i18n.set_locale(saved_content_locale)
				_close_citizen_panel()
				citizens_ok = citizens_ok and not citizen_panel.visible
				# The same card must render in German and English.
				_select_placement(1)
				i18n.set_locale("de")
				_toggle_citizen_panel()
				citizen_i18n_ok = citizen_text.text.contains("Bürger")
				citizen_i18n_ok = citizen_i18n_ok and known_name != "" and citizen_text.text.contains(known_name)
				i18n.set_locale("en")
				_update_citizen_panel()
				citizen_i18n_ok = citizen_i18n_ok and citizen_text.text.contains("Citizens")
				_close_citizen_panel()
				i18n.set_locale(smoke_locale)
				var materialization_center := _materialization_center()
				var materialization_query: Dictionary = core.materialization_step(
					SEED, materialization_center.x, materialization_center.z,
					MATERIALIZATION_INNER_RADIUS_M, MATERIALIZATION_OUTER_RADIUS_M,
					MATERIALIZATION_DETAILED_RADIUS_M, _materialization_pin()
				)
				var materialization_query_counts: Dictionary = materialization_query.get("counts", {})
				var expected_active := int(materialization_query_counts.get("active", 0))
				var expected_detailed := int(materialization_query_counts.get("detailed", 0))
				materialization_ok = bool(materialization_query.get("ok", false))
				materialization_ok = materialization_ok and expected_active > 0
				materialization_ok = materialization_ok and expected_detailed > 0
				var step_diff: Dictionary = materialization_query.get("diff", {})
				var step_changed: Array = materialization_query.get("changed", [])
				materialization_step_ok = materialization_ok
				materialization_step_ok = materialization_step_ok and str(materialization_query.get("checksum", "")).length() == 16
				for key in ["added", "removed", "updated", "retained", "changed", "applied", "truncated"]:
					materialization_step_ok = materialization_step_ok and step_diff.has(key)
				materialization_step_ok = materialization_step_ok and step_changed.size() <= 64
				materialization_step_ok = materialization_step_ok and step_changed.size() == int(step_diff.get("applied", -1))
				materialization_step_ok = materialization_step_ok and int(step_diff.get("applied", 65)) <= 64
				materialization_step_ok = materialization_step_ok and int(step_diff.get("changed", 0)) >= step_changed.size()
				materialization_ok = materialization_ok and materialization_step_ok
				materialization_ok = materialization_ok and _toggle_materialization()
				materialization_ok = materialization_ok and materialization_root.get_child_count() > 0
				materialization_ok = materialization_ok and materialization_root.get_child_count() <= expected_active
				materialization_ok = materialization_ok and materialization_counts == Vector2i(expected_active, expected_detailed)
				materialization_ok = materialization_ok and hud.text.contains("F4 materialization")
				materialization_ok = materialization_ok and status_label.text.contains(
					"Citizens near: %d active · %d detailed" % [expected_active, expected_detailed]
				)
				# Added markers fade in: a clear resets the bridge selection so
				# the next step is an all-added diff against the same node map.
				_advance_materialization_fades(MATERIALIZATION_FADE_SECONDS + 0.05)
				var settled_alpha_ok := materialization_root.get_child_count() > 0
				for child in materialization_root.get_children():
					if is_instance_valid(child) and child.material_override is StandardMaterial3D:
						settled_alpha_ok = settled_alpha_ok and (
							(child.material_override as StandardMaterial3D).albedo_color.a >= 0.999
						)
				materialization_fade_ok = settled_alpha_ok
				core.materialization_clear()
				var marker_count := materialization_root.get_child_count()
				_step_materialization()
				var added_fading_ok := false
				for child in materialization_root.get_children():
					if is_instance_valid(child) and child.material_override is StandardMaterial3D:
						if (child.material_override as StandardMaterial3D).albedo_color.a < 0.999:
							added_fading_ok = true
				materialization_fade_ok = materialization_fade_ok and added_fading_ok
				materialization_fade_ok = materialization_fade_ok and materialization_root.get_child_count() == marker_count
				movement_time = 0.0
				_update_movement_markers()
				movement_scene_ok = bool(last_movement_state.get("ok", false))
				var movement_positions: Array = last_movement_state.get("positions", [])
				movement_scene_ok = movement_scene_ok and movement_positions.size() > 0
				var movement_pool: MultiMeshInstance3D = movement_multimesh_instance
				var movement_pool_live := movement_pool != null and is_instance_valid(movement_pool)
				# One pooled MultiMesh instead of one node per citizen.
				movement_pool_ok = movement_pool_live and movement_multimesh != null
				movement_pool_ok = movement_pool_ok and movement_root.get_child_count() == 2
				movement_pool_ok = movement_pool_ok and movement_multimesh.instance_count == movement_positions.size()
				movement_pool_ok = movement_pool_ok and movement_ids.size() == movement_positions.size()
				movement_pool_ok = movement_pool_ok and movement_ground.size() == movement_positions.size()
				var route_vertices := 0
				var route_count := 0
				for route in last_movement_state.get("route_polylines", []):
					var route_points: PackedVector2Array = route.get("points", PackedVector2Array())
					if route_points.size() < 2:
						continue
					route_count += 1
					route_vertices += (route_points.size() - 1) * 2
				movement_routes_ok = route_count > 0 and route_vertices > 0
				movement_routes_ok = movement_routes_ok and movement_route_lines != null and is_instance_valid(movement_route_lines)
				var route_mesh: ArrayMesh = null
				if movement_route_lines != null:
					route_mesh = movement_route_lines.mesh as ArrayMesh
				movement_routes_ok = movement_routes_ok and route_mesh != null
				movement_routes_ok = movement_routes_ok and route_mesh.get_surface_count() == 1
				movement_routes_ok = movement_routes_ok and route_mesh.surface_get_array_len(0) == route_vertices
				movement_routes_ok = movement_routes_ok and movement_route_key.length() > 0
				var route_mesh_ref := movement_route_lines
				movement_lod_ok = movement_lod_counts.x + movement_lod_counts.y == movement_positions.size()
				movement_lod_ok = movement_lod_ok and movement_lod_counts.y <= MOVEMENT_FAR_BUDGET
				movement_lod_ok = movement_lod_ok and movement_ids.size() == movement_lod_counts.x
				movement_lod_ok = movement_lod_ok and movement_far_ids.size() == movement_lod_counts.y
				var first_far_ids: Array[int] = movement_far_ids.duplicate()
				var first_far_checksum := movement_far_checksum
				movement_scene_ok = movement_scene_ok and movement_pool_ok and movement_routes_ok
				var movement_cache: Dictionary = core.movement_cache_state()
				movement_scene_ok = movement_scene_ok and bool(movement_cache.get("open", false))
				movement_scene_ok = movement_scene_ok and int(movement_cache.get("citizens", 0)) == movement_positions.size()
				movement_time = 10.0
				_update_movement_markers()
				var moved_positions: Array = last_movement_state.get("positions", [])
				movement_scene_ok = movement_scene_ok and bool(last_movement_state.get("ok", false))
				# The pool is reused across ticks; only count and transforms change.
				movement_scene_ok = movement_scene_ok and movement_pool_live and movement_multimesh_instance == movement_pool
				movement_scene_ok = movement_scene_ok and movement_root.get_child_count() == 2
				movement_scene_ok = movement_scene_ok and movement_multimesh.instance_count >= movement_lod_counts.x
				movement_pool_ok = movement_pool_ok and movement_ids.size() == moved_positions.size()
				movement_lod_ok = movement_lod_ok and movement_far_ids == first_far_ids
				movement_lod_ok = movement_lod_ok and movement_far_checksum == first_far_checksum
				movement_routes_ok = movement_routes_ok and movement_route_lines == route_mesh_ref
				movement_scene_ok = movement_scene_ok and movement_routes_ok
				_advance_materialization_fades(MATERIALIZATION_FADE_SECONDS + 0.05)
				var added_settled_ok := materialization_root.get_child_count() > 0
				for child in materialization_root.get_children():
					if is_instance_valid(child) and child.material_override is StandardMaterial3D:
						added_settled_ok = added_settled_ok and (
							(child.material_override as StandardMaterial3D).albedo_color.a >= 0.999
						)
				materialization_fade_ok = materialization_fade_ok and added_settled_ok
				# Removed markers stay mounted while their alpha falls, then are
				# freed at the fade deadline. Move the fallback camera target far
				# enough that hysteresis drops the whole selection.
				_clear_selection()
				var materialization_target: Vector3 = mayor_camera.target
				mayor_camera.target = materialization_target + Vector3(5000.0, 0.0, 0.0)
				var fade_marker_count := materialization_root.get_child_count()
				_step_materialization()
				materialization_fade_ok = materialization_fade_ok and materialization_nodes.is_empty()
				materialization_fade_ok = materialization_fade_ok and materialization_root.get_child_count() == fade_marker_count
				_advance_materialization_fades(0.05)
				var removed_fading_ok := false
				for child in materialization_root.get_children():
					if is_instance_valid(child) and child.material_override is StandardMaterial3D:
						if (child.material_override as StandardMaterial3D).albedo_color.a < 0.999:
							removed_fading_ok = true
				materialization_fade_ok = materialization_fade_ok and removed_fading_ok
				_advance_materialization_fades(MATERIALIZATION_FADE_SECONDS + 0.05)
				materialization_fade_ok = materialization_fade_ok and materialization_root.get_child_count() == 0
				mayor_camera.target = materialization_target
				materialization_ok = materialization_ok and _select_placement(1)
				_rebuild_materialization()
				_advance_materialization_fades(MATERIALIZATION_FADE_SECONDS + 0.05)
				materialization_ok = materialization_ok and materialization_counts == Vector2i(expected_active, expected_detailed)
				materialization_ok = materialization_ok and not _toggle_materialization()
				materialization_ok = materialization_ok and materialization_root.get_child_count() == 0
				# F4 off frees the pooled markers and clears the height/id caches.
				movement_scene_ok = movement_scene_ok and movement_root.get_child_count() == 0
				movement_scene_ok = movement_scene_ok and movement_multimesh_instance == null
				movement_scene_ok = movement_scene_ok and movement_route_lines == null
				movement_pool_ok = movement_pool_ok and movement_root.get_child_count() == 0
				movement_pool_ok = movement_pool_ok and movement_multimesh == null
				movement_pool_ok = movement_pool_ok and movement_ground.is_empty()
				movement_pool_ok = movement_pool_ok and movement_ids.is_empty()
				# A small camera move must run through the stored selection and
				# return a bounded diff with a checksum.
				var moved_step: Dictionary = core.materialization_step(
					SEED, materialization_center.x + 1.0, materialization_center.z + 1.0,
					MATERIALIZATION_INNER_RADIUS_M, MATERIALIZATION_OUTER_RADIUS_M,
					MATERIALIZATION_DETAILED_RADIUS_M, _materialization_pin()
				)
				var moved_diff: Dictionary = moved_step.get("diff", {})
				var moved_changed: Array = moved_step.get("changed", [])
				materialization_step_ok = materialization_step_ok and bool(moved_step.get("ok", false))
				materialization_step_ok = materialization_step_ok and str(moved_step.get("checksum", "")).length() == 16
				for moved_key in ["added", "removed", "updated", "retained", "changed", "applied", "truncated"]:
					materialization_step_ok = materialization_step_ok and moved_diff.has(moved_key)
				materialization_step_ok = materialization_step_ok and moved_changed.size() <= 64
				materialization_step_ok = materialization_step_ok and moved_changed.size() == int(moved_diff.get("applied", -1))
				materialization_step_ok = materialization_step_ok and int(moved_diff.get("truncated", 0)) <= 1
				materialization_ok = materialization_ok and materialization_step_ok
				var progression_after: Dictionary = core.progression_state(SEED)
				age_ok = bool(progression_after.get("ok", false))
				age_ok = age_ok and str(progression_after.get("age", "")) == "wood"
				age_ok = age_ok and str(progression_after.get("next_age", "")) == "masonry"
				age_ok = age_ok and int(progression_after.get("next_threshold", 0)) == 250
				age_ok = age_ok and float(progression_after.get("progress", -1.0)) >= 0.0
				core.population_tick(SEED, 1000.0)
				var utility_before: Dictionary = core.city_summary()
				var before_water := float(utility_before.get("water", 0.0))
				var before_rest := float(utility_before.get("rest", 0.0))
				var water_added: Dictionary = core.utility_add_site(SEED, 1, "water")
				var power_added: Dictionary = core.utility_add_site(SEED, 1, "power")
				_refresh_utility_visuals()
				var utility_report: Dictionary = core.utilities_refresh(SEED)
				utility_ok = bool(water_added.get("ok", false)) and bool(power_added.get("ok", false))
				utility_ok = utility_ok and bool(utility_report.get("open", false))
				utility_ok = utility_ok and int(utility_report.get("water_sites", 0)) == 1
				utility_ok = utility_ok and int(utility_report.get("power_sites", 0)) == 1
				utility_ok = utility_ok and float(utility_report.get("water_supply", 0.0)) > 0.99
				utility_ok = utility_ok and float(utility_report.get("power_supply", 0.0)) > 0.99
				utility_ok = utility_ok and utility_nodes.size() == 2
				utility_ok = utility_ok and before_water < 0.8
				core.population_tick(SEED, 300.0)
				var utility_after: Dictionary = core.city_summary()
				var after_water := float(utility_after.get("water", 0.0))
				var after_rest := float(utility_after.get("rest", 0.0))
				utility_ok = utility_ok and after_water > before_water
				utility_ok = utility_ok and after_rest >= before_rest
				industry_ok = true
				for kind in ["lumberjack", "farm", "hunt"]:
					var added: Dictionary = core.industry_add_site(SEED, 1, kind)
					industry_ok = industry_ok and bool(added.get("ok", false))
					_refresh_industry_visuals()
					industry_ok = industry_ok and industry_nodes.size() == 1
					industry_ok = industry_ok and str(industry_kinds.get(1, "")) == kind
					var kind_tick: Dictionary = core.industry_tick(SEED, 300.0)
					industry_ok = industry_ok and int(kind_tick.get("workers", 0)) == 2
					match kind:
						"lumberjack":
							industry_ok = industry_ok and float(kind_tick.get("stock_wood", 0.0)) > 0.0
						"farm":
							industry_ok = industry_ok and float(kind_tick.get("stock_food", 0.0)) > 0.0
						"hunt":
							industry_ok = industry_ok and float(kind_tick.get("stock_food", 0.0)) > 0.0
							industry_ok = industry_ok and float(kind_tick.get("stock_hides", 0.0)) > 0.0
				var industry_added: Dictionary = core.industry_add_site(SEED, 1, "farm")
				industry_ok = industry_ok and bool(industry_added.get("ok", false))
				_refresh_industry_visuals()
				var beacon = industry_nodes.get(1, null)
				industry_ok = industry_ok and beacon != null
				industry_ok = industry_ok and beacon.has_meta("spinner")
				var industry_tick: Dictionary = core.industry_tick(SEED, 300.0)
				industry_ok = industry_ok and bool(industry_tick.get("open", false))
				industry_ok = industry_ok and int(industry_tick.get("workers", 0)) == 2
				industry_ok = industry_ok and float(industry_tick.get("food", 0.0)) > 0.0
				industry_ok = industry_ok and float(industry_tick.get("stock_food", 0.0)) > 0.0
				industry_ok = industry_ok and float(industry_tick.get("food_supply", 0.0)) > 0.99
				core.population_tick(SEED, 300.0)
				industry_ok = industry_ok and float(core.city_summary().get("food", 0.0)) > 0.9
				var industry_after: Dictionary = core.industry_state()
				industry_ok = industry_ok and int(industry_after.get("site_count", 0)) == 1
				industry_ok = industry_ok and float(industry_after.get("food", 0.0)) > 0.0
				occupancy_after_ok = occupancy_after_ok and industry_ok
				service_ok = true
				for service_kind in ["school", "clinic", "fire", "police"]:
					var service_added: Dictionary = core.service_add_site(SEED, 1, service_kind)
					service_ok = service_ok and bool(service_added.get("ok", false))
					service_ok = service_ok and str(service_added.get("tier", "")) == "basic"
					_refresh_service_visuals()
					service_ok = service_ok and service_nodes.size() == 1
					service_ok = service_ok and str(service_kinds.get(1, "")) == service_kind
					var service_report: Dictionary = core.services_refresh(SEED)
					service_ok = service_ok and bool(service_report.get("open", false))
					service_ok = service_ok and int(service_report.get("site_count", 0)) == 1
					match service_kind:
						"school":
							service_ok = service_ok and float(service_report.get("school_supply", 0.0)) > 0.99
							service_ok = service_ok and float(service_report.get("research_per_day", 0.0)) > 7.9
						"clinic":
							service_ok = service_ok and float(service_report.get("clinic_supply", 0.0)) > 0.99
						"fire":
							service_ok = service_ok and float(service_report.get("fire_supply", 0.0)) > 0.99
						"police":
							service_ok = service_ok and float(service_report.get("police_supply", 0.0)) > 0.99
				var final_school: Dictionary = core.service_add_site(SEED, 1, "school")
				service_ok = service_ok and bool(final_school.get("ok", false))
				_refresh_service_visuals()
				var final_service: Dictionary = core.services_refresh(SEED)
				service_ok = service_ok and float(final_service.get("research_per_day", 0.0)) > 7.9
				service_ok = service_ok and float(core.research_state(SEED).get("rate_per_day", 0.0)) > 8.0
				occupancy_after_ok = occupancy_after_ok and service_ok
				var tutorial: Dictionary = core.tutorial_state(SEED)
				tutorial_ok = bool(tutorial.get("ok", false))
				tutorial_ok = tutorial_ok and int(tutorial.get("total", 0)) == 5
				tutorial_ok = tutorial_ok and int(tutorial.get("completed", 0)) >= 4
				tutorial_ok = tutorial_ok and str(tutorial.get("current", "")) == "lumber_camp"
				tutorial_ok = tutorial_ok and tutorial.get("steps", []).size() == 5
				occupancy_after_ok = occupancy_after_ok and tutorial_ok
				var economy_after: Dictionary = core.economy_state()
				occupancy_after_ok = occupancy_after_ok and bool(economy_after.get("open", false))
				occupancy_after_ok = occupancy_after_ok and float(economy_after.get("balance", 100000.0)) < 100000.0
				occupancy_after_ok = occupancy_after_ok and float(economy_after.get("daily_net", 0.0)) < 0.0
				occupancy_after_ok = occupancy_after_ok and not bool(economy_after.get("bankrupt", true))
				var advice: Dictionary = core.city_advisor(SEED)
				var advice_codes: Array = []
				for advice_message in advice.get("messages", []):
					advice_codes.append(str(advice_message.get("code", "")))
				dashboard_ok = bool(advice.get("ok", false))
				dashboard_ok = dashboard_ok and dashboard_panel != null and not dashboard_panel.visible
				dashboard_ok = dashboard_ok and int(advice.get("warnings", 0)) >= 1
				dashboard_ok = dashboard_ok and advice_codes.has("city.budget_deficit")
				dashboard_ok = dashboard_ok and not bool(advice.get("healthy", true))
				_toggle_dashboard()
				dashboard_ok = dashboard_ok and dashboard_panel.visible
				dashboard_ok = dashboard_ok and dashboard_text != null
				dashboard_ok = dashboard_ok and dashboard_text.text.contains("Advisor")
				dashboard_ok = dashboard_ok and dashboard_text.text.contains("deficit")
				var district_world_checksum := str(core.world_placement_checksum())
				var district_population_checksum := str(core.population_checksum())
				var district_construction_checksum := str(core.construction_checksum())
				var district_center: Vector2 = _district_center_xz()
				var district: Dictionary = core.district_summary(
					SEED, district_center.x, district_center.y, DISTRICT_RADIUS_M
				)
				district_ui_ok = bool(district.get("ok", false))
				district_ui_ok = district_ui_ok and absf(float(district.get("radius_m", -1.0)) - DISTRICT_RADIUS_M) < 0.001
				district_ui_ok = district_ui_ok and int(district.get("population", -1)) == 2
				district_ui_ok = district_ui_ok and int(district.get("jobs", -1)) == 2
				for district_field in ["population", "housing_capacity", "vacancies", "jobs", "employed", "unemployed", "happiness", "food", "water", "rest", "average_commute_m"]:
					var district_value := float(district.get(district_field, -1.0))
					district_ui_ok = district_ui_ok and is_finite(district_value) and district_value >= 0.0
				district_ui_ok = district_ui_ok and dashboard_text.text.contains("District")
				district_ui_ok = district_ui_ok and dashboard_text.text.contains("Within 256 m of the camera target")
				district_ui_ok = district_ui_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.district", {
					"residents": i18n.format_int(int(district.get("population", 0))),
					"housing": i18n.format_int(int(district.get("housing_capacity", 0))),
					"vacant": i18n.format_int(int(district.get("vacancies", 0))),
				}))
				district_ui_ok = district_ui_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.district_jobs", {
					"jobs": i18n.format_int(int(district.get("jobs", 0))),
					"employed": i18n.format_int(int(district.get("employed", 0))),
				}))
				district_ui_ok = district_ui_ok and _district_section({}).contains(i18n.t("base.ui.dashboard.body.district_unavailable"))
				district_ui_ok = district_ui_ok and not _district_section({}).contains("!base.ui.")
				district_ui_ok = district_ui_ok and str(core.world_placement_checksum()) == district_world_checksum
				district_ui_ok = district_ui_ok and str(core.population_checksum()) == district_population_checksum
				district_ui_ok = district_ui_ok and str(core.construction_checksum()) == district_construction_checksum
				# District focus: F6 moves the district query to the selected
				# building and F6 returns it to the camera. Clearing the
				# selection also returns it to the camera.
				var focus_placement := _selected_placement()
				var focus_center: PackedFloat32Array = focus_placement.get("center", PackedFloat32Array())
				var camera_district_center: Vector2 = _district_center_xz()
				district_select_ok = not district_focus_selection and focus_center.size() == 3
				district_select_ok = district_select_ok and not dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.district_focus"))
				if focus_center.size() == 3:
					var focus_on := _toggle_district_focus()
					district_select_ok = district_select_ok and focus_on and district_focus_selection
					var district_focus_center: Vector2 = _district_center_xz()
					district_select_ok = district_select_ok and absf(district_focus_center.x - focus_center[0]) < 0.001
					district_select_ok = district_select_ok and absf(district_focus_center.y - focus_center[2]) < 0.001
					var focused_district: Dictionary = core.district_summary(
						SEED, district_focus_center.x, district_focus_center.y, DISTRICT_RADIUS_M
					)
					district_select_ok = district_select_ok and bool(focused_district.get("ok", false))
					var focused_center: PackedFloat32Array = focused_district.get("center", PackedFloat32Array())
					district_select_ok = district_select_ok and focused_center.size() == 2
					if focused_center.size() == 2:
						district_select_ok = district_select_ok and absf(focused_center[0] - focus_center[0]) < 0.001
						district_select_ok = district_select_ok and absf(focused_center[1] - focus_center[2]) < 0.001
					district_select_ok = district_select_ok and int(focused_district.get("population", -1)) == 2
					district_select_ok = district_select_ok and int(focused_district.get("jobs", -1)) == 2
					_update_dashboard()
					district_select_ok = district_select_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.district_focus"))
					district_select_ok = district_select_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.district_focus_hint", {
						"radius": i18n.format_float(DISTRICT_RADIUS_M, 0),
					}))
					district_select_ok = district_select_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.district_focus_action_camera"))
					i18n.set_locale("de")
					_update_dashboard()
					district_select_ok = district_select_ok and dashboard_text.text.contains("Stadtteilfokus: ausgewähltes Gebäude")
					district_select_ok = district_select_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.district_focus_action_camera"))
					district_select_ok = district_select_ok and dashboard_text.text.contains("Im Umkreis von 256 m um das ausgewählte Gebäude")
					i18n.set_locale("en")
					_update_dashboard()
					var focus_off := not _toggle_district_focus()
					district_select_ok = district_select_ok and focus_off and not district_focus_selection
					var camera_again: Vector2 = _district_center_xz()
					district_select_ok = district_select_ok and camera_again.distance_to(camera_district_center) < 0.001
					_update_dashboard()
					district_select_ok = district_select_ok and not dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.district_focus"))
					district_select_ok = district_select_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.district_hint", {
						"radius": i18n.format_float(DISTRICT_RADIUS_M, 0),
					}))
					district_select_ok = district_select_ok and _toggle_district_focus()
					_clear_selection()
					district_select_ok = district_select_ok and not district_focus_selection
					var cleared_center: Vector2 = _district_center_xz()
					district_select_ok = district_select_ok and cleared_center.distance_to(camera_district_center) < 0.001
					district_select_ok = district_select_ok and _select_placement(1)
					_update_dashboard()
					district_select_ok = district_select_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.district_focus_action_selection"))
					district_select_ok = district_select_ok and str(core.world_placement_checksum()) == district_world_checksum
					district_select_ok = district_select_ok and str(core.population_checksum()) == district_population_checksum
					district_select_ok = district_select_ok and str(core.construction_checksum()) == district_construction_checksum
				# Compact dashboard policy editing: the same bridge CRUD as the
				# standalone editor, driven by the footer keys. The dashboard
				# edits only the smallest-id containing policy and re-sends the
				# full record read from `district_policies_state`.
				policy_edit_ok = dashboard_visible and dashboard_policy_hint != null
				policy_edit_ok = policy_edit_ok and dashboard_policy_hint.text == i18n.t("base.ui.dashboard.policy_hint")
				policy_edit_ok = policy_edit_ok and not dashboard_policy_hint.text.contains("!base.ui.")
				var edit_fresh: Dictionary = core.district_policies_state(SEED)
				policy_edit_ok = policy_edit_ok and bool(edit_fresh.get("ok", false))
				policy_edit_ok = policy_edit_ok and int(edit_fresh.get("count", -1)) == 0
				policy_edit_ok = policy_edit_ok and str(edit_fresh.get("checksum", "")) == "0000000000000000"
				var edit_center: Vector2 = _district_center_xz()
				var edit_world_checksum := str(core.world_placement_checksum())
				var edit_population_checksum := str(core.population_checksum())
				var edit_construction_checksum := str(core.construction_checksum())
				var edit_created: Dictionary = _policy_create_default()
				var edit_state: Dictionary = core.district_policies_state(SEED)
				var edit_target: Dictionary = _policy_edit_target(SEED)
				var edit_policy: Dictionary = edit_target.get("policy", {})
				var edit_id := int(edit_policy.get("id", 0))
				policy_edit_ok = policy_edit_ok and bool(edit_created.get("ok", false))
				policy_edit_ok = policy_edit_ok and bool(edit_target.get("ok", false))
				policy_edit_ok = policy_edit_ok and int(edit_state.get("count", -1)) == 1
				policy_edit_ok = policy_edit_ok and _policy_array(edit_state).size() == 1
				policy_edit_ok = policy_edit_ok and edit_id == 1
				policy_edit_ok = policy_edit_ok and str(edit_policy.get("name", "")) == "District 1"
				policy_edit_ok = policy_edit_ok and absf(float(edit_policy.get("radius_m", 0.0)) - POLICY_DEFAULT_RADIUS_M) < 0.001
				policy_edit_ok = policy_edit_ok and _policy_center(edit_policy.get("center", null)).distance_to(edit_center) < 0.001
				policy_edit_ok = policy_edit_ok and absf(float(edit_policy.get("tax_multiplier", 0.0)) - POLICY_DEFAULT_TAX) < 0.001
				policy_edit_ok = policy_edit_ok and absf(float(edit_policy.get("service_funding", 0.0)) - POLICY_DEFAULT_FUNDING) < 0.001
				policy_edit_ok = policy_edit_ok and bool(edit_policy.get("construction_allowed", false))
				policy_edit_ok = policy_edit_ok and _policy_allowlist_count(edit_policy.get("blueprint_allowlist", null)) == 0
				policy_edit_ok = policy_edit_ok and modal_label.text == i18n.t("base.ui.policies.created", {
					"id": i18n.format_int(1),
					"name": "District 1",
				})
				_update_dashboard()
				policy_edit_ok = policy_edit_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.policies_count", {
					"count": i18n.format_int(1),
				}))
				policy_edit_ok = policy_edit_ok and not dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.policy_none"))
				# Give the record a non-default name and allowlist so every
				# compact edit proves it preserved both.
				var edit_seeded: Dictionary = core.district_policy_update(
					SEED,
					edit_id,
					"Compact Test",
					edit_center.x,
					edit_center.y,
					POLICY_DEFAULT_RADIUS_M,
					POLICY_DEFAULT_TAX,
					POLICY_DEFAULT_FUNDING,
					true,
					PackedStringArray(["base:workshop"])
				)
				policy_edit_ok = policy_edit_ok and bool(edit_seeded.get("ok", false))
				_smoke_press_key(KEY_J)
				edit_policy = _policy_edit_target(SEED).get("policy", {})
				policy_edit_ok = policy_edit_ok and not bool(edit_policy.get("construction_allowed", true))
				policy_edit_ok = policy_edit_ok and _smoke_policy_preserved(edit_policy, edit_center, "Compact Test")
				for index in 13:
					_smoke_press_key(KEY_BRACKETRIGHT)
				edit_policy = _policy_edit_target(SEED).get("policy", {})
				policy_edit_ok = policy_edit_ok and absf(float(edit_policy.get("tax_multiplier", 0.0)) - POLICY_MAX_TAX) < 0.001
				for index in 15:
					_smoke_press_key(KEY_BRACKETLEFT)
				_smoke_press_key(KEY_BRACKETLEFT)
				edit_policy = _policy_edit_target(SEED).get("policy", {})
				policy_edit_ok = policy_edit_ok and absf(float(edit_policy.get("tax_multiplier", 0.0)) - POLICY_MIN_TAX) < 0.001
				for index in 5:
					_smoke_press_key(KEY_PERIOD)
				edit_policy = _policy_edit_target(SEED).get("policy", {})
				policy_edit_ok = policy_edit_ok and absf(float(edit_policy.get("service_funding", 0.0)) - POLICY_MAX_FUNDING) < 0.001
				for index in 9:
					_smoke_press_key(KEY_COMMA)
				edit_policy = _policy_edit_target(SEED).get("policy", {})
				policy_edit_ok = policy_edit_ok and absf(float(edit_policy.get("service_funding", 0.0))) < 0.001
				policy_edit_ok = policy_edit_ok and _smoke_policy_preserved(edit_policy, edit_center, "Compact Test")
				# Remove through the real key path; the section falls back to
				# the no-policy line and the count drops to zero.
				_smoke_press_key(KEY_P, true)
				edit_state = core.district_policies_state(SEED)
				policy_edit_ok = policy_edit_ok and int(edit_state.get("count", -1)) == 0
				policy_edit_ok = policy_edit_ok and _policy_array(edit_state).is_empty()
				policy_edit_ok = policy_edit_ok and modal_label.text == i18n.t("base.ui.policies.removed", {"id": i18n.format_int(1)})
				_update_dashboard()
				policy_edit_ok = policy_edit_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.policies_count", {
					"count": i18n.format_int(0),
				}))
				policy_edit_ok = policy_edit_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.policy_none"))
				# A foreign seed and a rejected update must leave the registry,
				# the world checksums, and the UI state unchanged.
				var reject_created: Dictionary = _policy_create_default()
				var reject_before: Dictionary = core.district_policies_state(SEED)
				var reject_policy: Dictionary = _policy_edit_target(SEED).get("policy", {})
				var reject_result: Dictionary = _policy_apply_update(SEED + 1, reject_policy, {"construction_allowed": false})
				var reject_after: Dictionary = core.district_policies_state(SEED)
				var reject_record: Dictionary = _policy_edit_target(SEED).get("policy", {})
				policy_edit_ok = policy_edit_ok and bool(reject_created.get("ok", false))
				policy_edit_ok = policy_edit_ok and not bool(reject_result.get("ok", true))
				policy_edit_ok = policy_edit_ok and str(reject_result.get("error_code", "")) == "seed_mismatch"
				policy_edit_ok = policy_edit_ok and modal_label.text == i18n.t("base.ui.policies.failed", {"code": "seed_mismatch"})
				policy_edit_ok = policy_edit_ok and not modal_label.text.contains("!base.ui.")
				policy_edit_ok = policy_edit_ok and bool(reject_record.get("construction_allowed", false))
				policy_edit_ok = policy_edit_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.policies_count", {
					"count": i18n.format_int(1),
				}))
				policy_edit_ok = policy_edit_ok and str(reject_after.get("checksum", "")) == str(reject_before.get("checksum", ""))
				policy_edit_ok = policy_edit_ok and int(reject_after.get("count", -1)) == int(reject_before.get("count", -2))
				policy_edit_ok = policy_edit_ok and str(core.world_placement_checksum()) == edit_world_checksum
				policy_edit_ok = policy_edit_ok and str(core.population_checksum()) == edit_population_checksum
				policy_edit_ok = policy_edit_ok and str(core.construction_checksum()) == edit_construction_checksum
				_smoke_press_key(KEY_P, true)
				edit_state = core.district_policies_state(SEED)
				policy_edit_ok = policy_edit_ok and int(edit_state.get("count", -1)) == 0
				var edit_empty_checksum := str(edit_state.get("checksum", ""))
				# The footer and no-target message are keyed in EN/DE.
				i18n.set_locale("de")
				_update_dashboard()
				policy_edit_ok = policy_edit_ok and dashboard_policy_hint.text == i18n.t("base.ui.dashboard.policy_hint")
				policy_edit_ok = policy_edit_ok and dashboard_policy_hint.text.contains("Umschalt+P")
				_policy_toggle_construction(SEED)
				policy_edit_ok = policy_edit_ok and modal_label.text == i18n.t("base.ui.policies.no_target")
				policy_edit_ok = policy_edit_ok and not modal_label.text.contains("!base.ui.")
				i18n.set_locale("en")
				_set_modal("", false)
				_update_dashboard()
				# Policies: one read-only section driven by one
				# `district_policies_state` call per dashboard rebuild. The
				# bridge has no `policy_at`, so GDScript scans the inclusive
				# XZ circle at the district centre and takes the smallest id.
				var policy_probe: Vector2 = Vector2.ZERO
				var plain_policies: Array = [
					{"id": 2, "center": [64.0, 0.0], "radius_m": 64.0},
					{"id": 1, "center": [10.0, 0.0], "radius_m": 50.0},
				]
				policy_ui_ok = int(_containing_policy(plain_policies, policy_probe).get("id", 0)) == 1
				policy_ui_ok = policy_ui_ok and int(_containing_policy(plain_policies, Vector2(128.0, 0.0)).get("id", 0)) == 2
				var packed_policies: Array = [
					{"id": 3, "center": PackedFloat32Array([0.0, 0.0]), "radius_m": 16.0},
				]
				policy_ui_ok = policy_ui_ok and int(_containing_policy(packed_policies, Vector2(16.0, 0.0)).get("id", 0)) == 3
				var invalid_policies: Array = [
					{"id": 4, "center": [NAN, 0.0], "radius_m": 64.0},
					{"id": 5, "center": [0.0, 0.0], "radius_m": INF},
				]
				policy_ui_ok = policy_ui_ok and _containing_policy(invalid_policies, policy_probe).is_empty()
				policy_ui_ok = policy_ui_ok and _policies_section({"ok": false}).contains(i18n.t("base.ui.dashboard.body.policies_unavailable"))
				policy_ui_ok = policy_ui_ok and not _policies_section({"ok": false}).contains("!base.ui.")
				var policy_state: Dictionary = core.district_policies_state(SEED)
				policy_ui_ok = policy_ui_ok and bool(policy_state.get("ok", false))
				policy_ui_ok = policy_ui_ok and int(policy_state.get("count", -1)) == 0
				policy_ui_ok = policy_ui_ok and str(policy_state.get("checksum", "")) == edit_empty_checksum
				_update_dashboard()
				policy_ui_ok = policy_ui_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.section.policies"))
				policy_ui_ok = policy_ui_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.policies_count", {
					"count": i18n.format_int(0),
				}))
				policy_ui_ok = policy_ui_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.policy_none"))
				var policy_world_checksum := str(core.world_placement_checksum())
				var policy_population_checksum := str(core.population_checksum())
				var policy_construction_checksum := str(core.construction_checksum())
				var policy_center: Vector2 = _district_center_xz()
				var policy_created: Dictionary = core.district_policy_create(
					SEED,
					"Old Town",
					policy_center.x,
					policy_center.y,
					64.0,
					1.25,
					1.1,
					true,
					PackedStringArray(["base:workshop"])
				)
				var created_policy: Dictionary = policy_created.get("policy", {})
				var policy_id := int(created_policy.get("id", 0))
				policy_ui_ok = policy_ui_ok and bool(policy_created.get("ok", false)) and policy_id > 0
				_update_dashboard()
				policy_ui_ok = policy_ui_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.policies_count", {
					"count": i18n.format_int(1),
				}))
				policy_ui_ok = policy_ui_ok and dashboard_text.text.contains("Old Town")
				policy_ui_ok = policy_ui_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.policy", {
					"name": "Old Town",
					"tax": i18n.format_float(1.25, 2),
					"funding": i18n.format_float(1.1, 2),
				}))
				policy_ui_ok = policy_ui_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.policy_construction_allowed"))
				policy_ui_ok = policy_ui_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.policy_allowlist", {
					"count": i18n.format_int(1),
				}))
				policy_ui_ok = policy_ui_ok and not dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.policy_none"))
				i18n.set_locale("de")
				_update_dashboard()
				policy_ui_ok = policy_ui_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.section.policies"))
				policy_ui_ok = policy_ui_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.policies_count", {
					"count": i18n.format_int(1),
				}))
				policy_ui_ok = policy_ui_ok and dashboard_text.text.contains("Old Town")
				policy_ui_ok = policy_ui_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.policy", {
					"name": "Old Town",
					"tax": i18n.format_float(1.25, 2),
					"funding": i18n.format_float(1.1, 2),
				}))
				policy_ui_ok = policy_ui_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.policy_construction_allowed"))
				policy_ui_ok = policy_ui_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.policy_allowlist", {
					"count": i18n.format_int(1),
				}))
				policy_ui_ok = policy_ui_ok and not dashboard_text.text.contains("!base.ui.")
				i18n.set_locale("en")
				var policy_updated: Dictionary = core.district_policy_update(
					SEED,
					policy_id,
					"Old Town",
					policy_center.x,
					policy_center.y,
					64.0,
					1.25,
					1.1,
					false,
					PackedStringArray(["base:workshop"])
				)
				policy_ui_ok = policy_ui_ok and bool(policy_updated.get("ok", false))
				_update_dashboard()
				policy_ui_ok = policy_ui_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.policy_construction_blocked"))
				policy_ui_ok = policy_ui_ok and not dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.policy_construction_allowed"))
				i18n.set_locale("de")
				_update_dashboard()
				policy_ui_ok = policy_ui_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.policy_construction_blocked"))
				policy_ui_ok = policy_ui_ok and not dashboard_text.text.contains("!base.ui.")
				i18n.set_locale("en")
				var policy_removed: Dictionary = core.district_policy_remove(SEED, policy_id)
				policy_ui_ok = policy_ui_ok and bool(policy_removed.get("ok", false))
				policy_ui_ok = policy_ui_ok and int(policy_removed.get("count", -1)) == 0
				_update_dashboard()
				policy_ui_ok = policy_ui_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.policies_count", {
					"count": i18n.format_int(0),
				}))
				policy_ui_ok = policy_ui_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.policy_none"))
				policy_ui_ok = policy_ui_ok and not dashboard_text.text.contains("Old Town")
				policy_ui_ok = policy_ui_ok and str(core.world_placement_checksum()) == policy_world_checksum
				policy_ui_ok = policy_ui_ok and str(core.population_checksum()) == policy_population_checksum
				policy_ui_ok = policy_ui_ok and str(core.construction_checksum()) == policy_construction_checksum
				# Settlements: a second record makes the selector observable.
				# The id order is stable, PageUp/PageDown cycle none -> id1 ->
				# id2 -> none, the selected row is marked, and an explicit
				# settlement override wins over F6 building focus for the
				# district query. A removed/foreign id falls back cleanly.
				var second_found: Dictionary = core.settlement_found(
					SEED, "Smoke Harbour", "player", 600, 125, 64
				)
				var second_record: Dictionary = second_found.get("settlement", {})
				var second_settlement_id := int(second_record.get("id", -1))
				var second_settlement_center := _settlement_center(second_record)
				var settlement_records: Array = _settlement_records()
				var first_record: Dictionary = settlement_records[0] if settlement_records.size() > 0 else {}
				var first_settlement_id := int(first_record.get("id", -1))
				var first_settlement_center := _settlement_center(first_record)
				settlement_select_ok = settlement_select_ok and bool(second_found.get("ok", false))
				settlement_select_ok = settlement_select_ok and settlement_records.size() == 2
				settlement_select_ok = settlement_select_ok and second_settlement_id > first_settlement_id
				settlement_select_ok = settlement_select_ok and selected_settlement_id == -1
				settlement_select_ok = settlement_select_ok and _cycle_settlement(1) == first_settlement_id
				settlement_select_ok = settlement_select_ok and selected_settlement_id == first_settlement_id
				settlement_select_ok = settlement_select_ok and _cycle_settlement(1) == second_settlement_id
				settlement_select_ok = settlement_select_ok and selected_settlement_id == second_settlement_id
				settlement_select_ok = settlement_select_ok and _cycle_settlement(1) == -1
				settlement_select_ok = settlement_select_ok and _cycle_settlement(-1) == second_settlement_id
				settlement_select_ok = settlement_select_ok and _cycle_settlement(-1) == first_settlement_id
				settlement_select_ok = settlement_select_ok and _select_settlement(second_settlement_id)
				var settlement_district_center: Vector2 = _district_center_xz()
				settlement_select_ok = settlement_select_ok and settlement_district_center.distance_to(second_settlement_center) < 0.001
				var settlement_district: Dictionary = core.district_summary(
					SEED, settlement_district_center.x, settlement_district_center.y, DISTRICT_RADIUS_M
				)
				settlement_select_ok = settlement_select_ok and bool(settlement_district.get("ok", false))
				settlement_select_ok = settlement_select_ok and int(settlement_district.get("population", -1)) == 0
				settlement_select_ok = settlement_select_ok and int(settlement_district.get("jobs", -1)) == 0
				_update_dashboard()
				var first_row_params := {
					"id": first_settlement_id,
					"name": str(first_record.get("name", "")),
					"owner": str(first_record.get("owner", "")),
					"x": int(first_settlement_center.x),
					"z": int(first_settlement_center.y),
					"radius": i18n.format_float(float(first_record.get("radius_m", 0.0)), 0),
				}
				var second_row_params := {
					"id": second_settlement_id,
					"name": "Smoke Harbour",
					"owner": "player",
					"x": int(second_settlement_center.x),
					"z": int(second_settlement_center.y),
					"radius": i18n.format_float(float(second_record.get("radius_m", 0.0)), 0),
				}
				var settlement_section: String = _settlements_section(core.settlements())
				settlement_select_ok = settlement_select_ok and settlement_section.contains(i18n.t("base.ui.dashboard.body.settlements_count", {
					"count": i18n.format_int(2),
					"selected": "Smoke Harbour",
				}))
				settlement_select_ok = settlement_select_ok and settlement_section.contains(i18n.t("base.ui.dashboard.body.settlement_row_selected", second_row_params))
				settlement_select_ok = settlement_select_ok and settlement_section.contains(i18n.t("base.ui.dashboard.body.settlement_row", first_row_params))
				settlement_select_ok = settlement_select_ok and settlement_section.contains(i18n.t("base.ui.dashboard.body.settlement_hint"))
				settlement_select_ok = settlement_select_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.settlement_focus", {"name": "Smoke Harbour"}))
				settlement_select_ok = settlement_select_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.settlement_focus_hint", {
					"radius": i18n.format_float(DISTRICT_RADIUS_M, 0),
				}))
				settlement_select_ok = settlement_select_ok and not dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.district_focus"))
				print("SETTLEMENT_UI settlements=%d selected=%d centre=(%d,%d) district_population=%d" % [
					settlement_records.size(),
					selected_settlement_id,
					int(second_settlement_center.x),
					int(second_settlement_center.y),
					int(settlement_district.get("population", 0)),
				])
				# Per-settlement detail block. It reuses the same 256 m district
				# payload and policy state the dashboard already queried. The two
				# centres are 431 m apart, so population, jobs, and the containing
				# policy must differ and must never bleed between records.
				var detail_policy: Dictionary = core.district_policy_create(
					SEED,
					"Smoke Detail",
					first_settlement_center.x,
					first_settlement_center.y,
					96.0,
					1.5,
					1.2,
					false,
					PackedStringArray(["base:cottage"])
				)
				var detail_policy_id := int(detail_policy.get("policy", {}).get("id", -1))
				settlement_detail_ok = bool(detail_policy.get("ok", false)) and detail_policy_id >= 0
				var detail_policy_outer: Dictionary = core.district_policy_create(
					SEED,
					"Smoke Detail Outer",
					first_settlement_center.x + 10.0,
					first_settlement_center.y,
					200.0,
					0.5,
					0.5,
					true,
					PackedStringArray()
				)
				var detail_outer_id := int(detail_policy_outer.get("policy", {}).get("id", -1))
				settlement_detail_ok = settlement_detail_ok and bool(detail_policy_outer.get("ok", false))
				settlement_detail_ok = settlement_detail_ok and detail_outer_id > detail_policy_id
				var first_district: Dictionary = core.district_summary(
					SEED, first_settlement_center.x, first_settlement_center.y, DISTRICT_RADIUS_M
				)
				var second_district: Dictionary = core.district_summary(
					SEED, second_settlement_center.x, second_settlement_center.y, DISTRICT_RADIUS_M
				)
				settlement_detail_ok = settlement_detail_ok and bool(first_district.get("ok", false))
				settlement_detail_ok = settlement_detail_ok and bool(second_district.get("ok", false))
				settlement_detail_ok = settlement_detail_ok and first_district.has("completed_buildings")
				settlement_detail_ok = settlement_detail_ok and first_district.has("construction_sites")
				var first_detail_params := {
					"name": str(first_record.get("name", "")),
					"owner": str(first_record.get("owner", "")),
					"id": first_settlement_id,
					"x": int(first_settlement_center.x),
					"z": int(first_settlement_center.y),
					"radius": i18n.format_float(float(first_record.get("radius_m", 0.0)), 0),
				}
				var second_detail_params := {
					"name": "Smoke Harbour",
					"owner": "player",
					"id": second_settlement_id,
					"x": int(second_settlement_center.x),
					"z": int(second_settlement_center.y),
					"radius": i18n.format_float(float(second_record.get("radius_m", 0.0)), 0),
				}
				# `settlement_found` creates an unowned v1 record: revision 0,
				# no members. The fixture therefore exercises the unowned
				# membership line; the helper check covers the owned format.
				var unowned_membership_line := "Members %s · revision %s" % [
					i18n.NOT_AVAILABLE,
					i18n.format_int(0),
				]
				settlement_detail_ok = settlement_detail_ok and _settlement_membership_line({
					"owned": true,
					"owner_id": 42,
					"membership_revision": 7,
					"tax_rate": 25,
					"members": [
						{"player": 3, "role": "member"},
						{"player": 4, "role": "builder"},
						{"player": 5, "role": "manager"},
						{"player": 6, "role": "builder"},
					],
				}) == "Owner #42 · 4 members · revision 7 · builder 2 · manager 1 · member 1 · tax 25%"
				settlement_detail_ok = settlement_detail_ok and _settlement_membership_line({}) == unowned_membership_line
				settlement_detail_ok = settlement_detail_ok and _select_settlement(first_settlement_id)
				_update_dashboard()
				var first_detail_section: String = _settlements_section(
					core.settlements(), first_district, core.district_policies_state(SEED)
				)
				settlement_detail_ok = settlement_detail_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.settlement_detail_title"))
				settlement_detail_ok = settlement_detail_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.settlement_detail", first_detail_params))
				settlement_detail_ok = settlement_detail_ok and first_detail_section.contains(i18n.t("base.ui.dashboard.body.settlement_detail", first_detail_params))
				settlement_detail_ok = settlement_detail_ok and first_detail_section.contains(unowned_membership_line)
				settlement_detail_ok = settlement_detail_ok and first_detail_section.contains(i18n.t("base.ui.dashboard.body.settlement_detail_district", {
					"radius": i18n.format_float(DISTRICT_RADIUS_M, 0),
					"residents": i18n.format_int(int(first_district.get("population", 0))),
					"housing": i18n.format_int(int(first_district.get("housing_capacity", 0))),
					"vacant": i18n.format_int(int(first_district.get("vacancies", 0))),
				}))
				settlement_detail_ok = settlement_detail_ok and first_detail_section.contains(i18n.t("base.ui.dashboard.body.settlement_detail_jobs", {
					"jobs": i18n.format_int(int(first_district.get("jobs", 0))),
					"employed": i18n.format_int(int(first_district.get("employed", 0))),
					"unemployed": i18n.format_int(int(first_district.get("unemployed", 0))),
				}))
				settlement_detail_ok = settlement_detail_ok and first_detail_section.contains(i18n.t("base.ui.dashboard.body.settlement_detail_buildings", {
					"completed": i18n.format_int(int(first_district.get("completed_buildings", 0))),
					"sites": i18n.format_int(int(first_district.get("construction_sites", 0))),
				}))
				settlement_detail_ok = settlement_detail_ok and first_detail_section.contains(i18n.t("base.ui.dashboard.body.district_needs", {
					"happiness": i18n.format_float(float(first_district.get("happiness", 0.0)) * 100.0, 0) + "%",
					"needs": i18n.t("base.ui.dashboard.body.needs_values", {
						"food": i18n.format_float(float(first_district.get("food", 0.0)) * 100.0, 0),
						"water": i18n.format_float(float(first_district.get("water", 0.0)) * 100.0, 0),
						"rest": i18n.format_float(float(first_district.get("rest", 0.0)) * 100.0, 0),
					}),
					"commute": i18n.format_float(float(first_district.get("average_commute_m", 0.0)), 0),
				}))
				settlement_detail_ok = settlement_detail_ok and first_detail_section.contains(i18n.t("base.ui.dashboard.body.settlement_detail_policy", {
					"name": "Smoke Detail",
					"tax": i18n.format_float(1.5, 2),
					"funding": i18n.format_float(1.2, 2),
				}))
				settlement_detail_ok = settlement_detail_ok and first_detail_section.contains(i18n.t("base.ui.dashboard.body.policy_construction_blocked"))
				settlement_detail_ok = settlement_detail_ok and first_detail_section.contains(i18n.t("base.ui.dashboard.body.policy_allowlist", {
					"count": i18n.format_int(1),
				}))
				# Smallest id wins even though the outer policy also contains
				# the settlement centre.
				settlement_detail_ok = settlement_detail_ok and first_detail_section.contains("Smoke Detail")
				settlement_detail_ok = settlement_detail_ok and not first_detail_section.contains("Smoke Detail Outer")
				settlement_detail_ok = settlement_detail_ok and not first_detail_section.contains(i18n.t("base.ui.dashboard.body.settlement_detail", second_detail_params))
				settlement_detail_ok = settlement_detail_ok and not first_detail_section.contains("!base.ui.")
				settlement_detail_ok = settlement_detail_ok and int(first_district.get("population", -1)) == 2
				settlement_detail_ok = settlement_detail_ok and int(first_district.get("jobs", -1)) == 2
				settlement_detail_ok = settlement_detail_ok and int(first_district.get("completed_buildings", 0)) >= 1
				settlement_detail_ok = settlement_detail_ok and int(second_district.get("population", -1)) == 0
				# Selecting the other settlement swaps every derived value; no
				# identity, district, or policy from the first record survives.
				settlement_detail_ok = settlement_detail_ok and _select_settlement(second_settlement_id)
				_update_dashboard()
				var second_detail_section: String = _settlements_section(
					core.settlements(), second_district, core.district_policies_state(SEED)
				)
				settlement_detail_ok = settlement_detail_ok and second_detail_section.contains(i18n.t("base.ui.dashboard.body.settlement_detail", second_detail_params))
				settlement_detail_ok = settlement_detail_ok and second_detail_section.contains(unowned_membership_line)
				settlement_detail_ok = settlement_detail_ok and second_detail_section.contains(i18n.t("base.ui.dashboard.body.settlement_detail_district", {
					"radius": i18n.format_float(DISTRICT_RADIUS_M, 0),
					"residents": i18n.format_int(int(second_district.get("population", 0))),
					"housing": i18n.format_int(int(second_district.get("housing_capacity", 0))),
					"vacant": i18n.format_int(int(second_district.get("vacancies", 0))),
				}))
				settlement_detail_ok = settlement_detail_ok and second_detail_section.contains(i18n.t("base.ui.dashboard.body.settlement_detail_jobs", {
					"jobs": i18n.format_int(int(second_district.get("jobs", 0))),
					"employed": i18n.format_int(int(second_district.get("employed", 0))),
					"unemployed": i18n.format_int(int(second_district.get("unemployed", 0))),
				}))
				settlement_detail_ok = settlement_detail_ok and second_detail_section.contains(i18n.t("base.ui.dashboard.body.district_needs", {
					"happiness": i18n.NOT_AVAILABLE,
					"needs": i18n.NOT_AVAILABLE,
					"commute": i18n.format_float(float(second_district.get("average_commute_m", 0.0)), 0),
				}))
				settlement_detail_ok = settlement_detail_ok and second_detail_section.contains(i18n.t("base.ui.dashboard.body.settlement_detail_policy_none"))
				settlement_detail_ok = settlement_detail_ok and not second_detail_section.contains("Smoke Detail")
				settlement_detail_ok = settlement_detail_ok and not second_detail_section.contains(i18n.t("base.ui.dashboard.body.settlement_detail", first_detail_params))
				settlement_detail_ok = settlement_detail_ok and not second_detail_section.contains("!base.ui.")
				# A foreign seed is a bridge `seed_mismatch`: the block keeps
				# the record identity and clears every derived value.
				var foreign_district: Dictionary = core.district_summary(
					SEED + 1, first_settlement_center.x, first_settlement_center.y, DISTRICT_RADIUS_M
				)
				settlement_detail_ok = settlement_detail_ok and not bool(foreign_district.get("ok", true))
				var foreign_detail_section: String = _settlements_section(
					core.settlements(), foreign_district, core.district_policies_state(SEED)
				)
				settlement_detail_ok = settlement_detail_ok and foreign_detail_section.contains(i18n.t("base.ui.dashboard.body.district_unavailable"))
				settlement_detail_ok = settlement_detail_ok and foreign_detail_section.contains(i18n.t("base.ui.dashboard.body.settlement_detail", second_detail_params))
				settlement_detail_ok = settlement_detail_ok and not foreign_detail_section.contains(i18n.t("base.ui.dashboard.body.settlement_detail_district", {
					"radius": i18n.format_float(DISTRICT_RADIUS_M, 0),
					"residents": i18n.format_int(int(second_district.get("population", 0))),
					"housing": i18n.format_int(int(second_district.get("housing_capacity", 0))),
					"vacant": i18n.format_int(int(second_district.get("vacancies", 0))),
				}))
				settlement_detail_ok = settlement_detail_ok and not foreign_detail_section.contains(i18n.t("base.ui.dashboard.body.settlement_detail_policy_none"))
				settlement_detail_ok = settlement_detail_ok and not foreign_detail_section.contains("!base.ui.")
				# A malformed centre yields no block; an `ok=false` payload with
				# a matching centre keeps the identity and the unavailable line.
				var malformed_detail: String = _settlement_detail_section(
					{
						"id": first_settlement_id,
						"name": "Malformed",
						"owner": "player",
						"center": [NAN, 0.0],
						"radius_m": 64.0,
					},
					first_district,
					core.district_policies_state(SEED)
				)
				settlement_detail_ok = settlement_detail_ok and malformed_detail.is_empty()
				var failed_detail: String = _settlement_detail_section(
					second_record,
					{
						"ok": false,
						"center": PackedFloat32Array([second_settlement_center.x, second_settlement_center.y]),
					},
					core.district_policies_state(SEED)
				)
				settlement_detail_ok = settlement_detail_ok and failed_detail.contains(i18n.t("base.ui.dashboard.body.district_unavailable"))
				settlement_detail_ok = settlement_detail_ok and not failed_detail.contains("residents")
				settlement_detail_ok = settlement_detail_ok and not failed_detail.contains("!base.ui.")
				# Removing both policies restores the containing-policy miss and
				# leaves the placement/checksum state untouched.
				var detail_outer_removed: Dictionary = core.district_policy_remove(SEED, detail_outer_id)
				var detail_removed: Dictionary = core.district_policy_remove(SEED, detail_policy_id)
				settlement_detail_ok = settlement_detail_ok and bool(detail_outer_removed.get("ok", false))
				settlement_detail_ok = settlement_detail_ok and bool(detail_removed.get("ok", false))
				settlement_detail_ok = settlement_detail_ok and int(detail_removed.get("count", -1)) == 0
				settlement_detail_ok = settlement_detail_ok and str(core.world_placement_checksum()) == policy_world_checksum
				settlement_detail_ok = settlement_detail_ok and _select_settlement(first_settlement_id)
				_update_dashboard()
				var clean_detail_text := dashboard_text.text
				settlement_detail_ok = settlement_detail_ok and not clean_detail_text.contains("Smoke Detail")
				settlement_detail_ok = settlement_detail_ok and clean_detail_text.contains(i18n.t("base.ui.dashboard.body.settlement_detail_policy_none"))
				settlement_detail_ok = settlement_detail_ok and clean_detail_text.contains(i18n.t("base.ui.dashboard.body.settlement_detail", first_detail_params))
				# Deterministic repeat: none -> same settlement rebuilds the
				# identical block.
				settlement_detail_ok = settlement_detail_ok and _select_settlement(-1)
				settlement_detail_ok = settlement_detail_ok and _select_settlement(first_settlement_id)
				_update_dashboard()
				settlement_detail_ok = settlement_detail_ok and dashboard_text.text == clean_detail_text
				settlement_detail_ok = settlement_detail_ok and _select_settlement(second_settlement_id)
				_update_dashboard()
				print("SETTLEMENT_DETAIL first_id=%d first_population=%d second_id=%d second_population=%d first_completed=%d policies=%d" % [
					first_settlement_id,
					int(first_district.get("population", -1)),
					second_settlement_id,
					int(second_district.get("population", -1)),
					int(first_district.get("completed_buildings", -1)),
					int(core.district_policies_state(SEED).get("count", -1)),
				])
				# F6 building focus still toggles, but the explicit settlement
				# override keeps the district centre on the settlement.
				settlement_select_ok = settlement_select_ok and _select_placement(1)
				var settlement_building_focus := _toggle_district_focus()
				_update_dashboard()
				var override_center: Vector2 = _district_center_xz()
				settlement_select_ok = settlement_select_ok and settlement_building_focus and district_focus_selection
				settlement_select_ok = settlement_select_ok and override_center.distance_to(second_settlement_center) < 0.001
				settlement_select_ok = settlement_select_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.settlement_focus", {"name": "Smoke Harbour"}))
				settlement_select_ok = settlement_select_ok and not _toggle_district_focus()
				# The choice survives repeated dashboard rebuilds and the
				# load-path settlement rebuild while the id still exists.
				_update_dashboard()
				_update_dashboard()
				settlement_select_ok = settlement_select_ok and selected_settlement_id == second_settlement_id
				_rebuild_settlements()
				_update_dashboard()
				settlement_select_ok = settlement_select_ok and selected_settlement_id == second_settlement_id
				settlement_select_ok = settlement_select_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.settlement_focus", {"name": "Smoke Harbour"}))
				i18n.set_locale("de")
				_update_dashboard()
				settlement_select_ok = settlement_select_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.section.settlements"))
				settlement_select_ok = settlement_select_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.settlement_row_selected", second_row_params))
				settlement_select_ok = settlement_select_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.settlement_focus", {"name": "Smoke Harbour"}))
				settlement_select_ok = settlement_select_ok and not dashboard_text.text.contains("!base.ui.")
				settlement_detail_ok = settlement_detail_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.settlement_detail_title"))
				settlement_detail_ok = settlement_detail_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.settlement_detail", second_detail_params))
				settlement_detail_ok = settlement_detail_ok and dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.settlement_detail_policy_none"))
				settlement_detail_ok = settlement_detail_ok and not dashboard_text.text.contains("!base.ui.")
				i18n.set_locale("en")
				_update_dashboard()
				# A foreign id must not crash or query a stale centre: the
				# dashboard validation clears it and the camera takes over. The
				# detail block disappears with the selection.
				selected_settlement_id = 999999
				_update_dashboard()
				settlement_select_ok = settlement_select_ok and selected_settlement_id == -1
				settlement_detail_ok = settlement_detail_ok and not dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.settlement_detail_title"))
				var foreign_center: Vector2 = _district_center_xz()
				settlement_select_ok = settlement_select_ok and foreign_center.distance_to(Vector2(mayor_camera.target.x, mayor_camera.target.z)) < 0.001
				settlement_select_ok = settlement_select_ok and not _select_settlement(999999)
				settlement_select_ok = settlement_select_ok and selected_settlement_id == -1
				_update_dashboard()
				settlement_select_ok = settlement_select_ok and not dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.settlement_focus", {"name": "Smoke Harbour"}))
				settlement_detail_ok = settlement_detail_ok and not dashboard_text.text.contains(i18n.t("base.ui.dashboard.body.settlement_detail_title"))
				settlement_detail_ok = settlement_detail_ok and not dashboard_text.text.contains("!base.ui.")
				settlement_select_ok = settlement_select_ok and _select_settlement(-1)
				settlement_select_ok = settlement_select_ok and selected_settlement_id == -1
				# Shift+T: the one settlement write control. The helper cycle is
				# pure; unowned records reject locally; a claimed record persists
				# through settlement_set_tax and refreshes the same selection.
				dashboard_tax_ok = _next_settlement_tax(0) == 5
				dashboard_tax_ok = dashboard_tax_ok and _next_settlement_tax(95) == 100
				dashboard_tax_ok = dashboard_tax_ok and _next_settlement_tax(100) == 0
				dashboard_tax_ok = dashboard_tax_ok and _next_settlement_tax(-3) == 5
				dashboard_tax_ok = dashboard_tax_ok and _next_settlement_tax(250) == 0
				_select_settlement(first_settlement_id)
				var tax_before := int(_selected_settlement().get("tax_rate", -1))
				var tax_checksum_before := str(core.settlement_checksum())
				_set_modal("", false)
				_smoke_press_key(KEY_T, true)
				dashboard_tax_ok = dashboard_tax_ok and modal_label.text == "settlement.unowned"
				dashboard_tax_ok = dashboard_tax_ok and int(_selected_settlement().get("tax_rate", -2)) == tax_before
				dashboard_tax_ok = dashboard_tax_ok and str(core.settlement_checksum()) == tax_checksum_before
				dashboard_tax_ok = dashboard_tax_ok and selected_settlement_id == first_settlement_id
				# The shared smoke fixture founds unowned records. Claiming one
				# makes the same key exercise the real bridge write and the
				# detail refresh without touching any other scene or save.
				var tax_claim: Dictionary = core.settlement_claim(
					first_settlement_id,
					1,
					int(_selected_settlement().get("membership_revision", 0))
				)
				dashboard_tax_ok = dashboard_tax_ok and bool(tax_claim.get("ok", false))
				dashboard_tax_ok = dashboard_tax_ok and _select_settlement(first_settlement_id)
				_set_modal("", false)
				_smoke_press_key(KEY_T, true)
				var owned_tax := int(_selected_settlement().get("tax_rate", -1))
				dashboard_tax_ok = dashboard_tax_ok and owned_tax == _next_settlement_tax(tax_before)
				dashboard_tax_ok = dashboard_tax_ok and selected_settlement_id == first_settlement_id
				dashboard_tax_ok = dashboard_tax_ok and modal_label.text.contains("Settlement tax: %d%%" % owned_tax)
				dashboard_tax_ok = dashboard_tax_ok and modal_label.text.contains(str(core.settlement_checksum()))
				dashboard_tax_ok = dashboard_tax_ok and dashboard_text.text.contains("tax %d%%" % owned_tax)
				_set_modal("", false)
				_smoke_press_key(KEY_T, true)
				var owned_tax_next := int(_selected_settlement().get("tax_rate", -1))
				dashboard_tax_ok = dashboard_tax_ok and owned_tax_next == _next_settlement_tax(owned_tax)
				dashboard_tax_ok = dashboard_tax_ok and modal_label.text.contains("Settlement tax: %d%%" % owned_tax_next)
				dashboard_tax_ok = dashboard_tax_ok and str(core.settlement_checksum()) == str(core.settlements().get("checksum", ""))
				# No selection: the key is gated in the input handler, so neither
				# the modal nor the store changes.
				_select_settlement(-1)
				_set_modal("", false)
				var tax_no_selection_checksum := str(core.settlement_checksum())
				_smoke_press_key(KEY_T, true)
				dashboard_tax_ok = dashboard_tax_ok and modal_label.text == ""
				dashboard_tax_ok = dashboard_tax_ok and str(core.settlement_checksum()) == tax_no_selection_checksum
				print("DASHBOARD_TAX cycle=ok 0->5 95->100 100->0 unowned=ok owned=%d->%d checksum=%s" % [
					owned_tax,
					owned_tax_next,
					str(core.settlement_checksum()),
				])
				_toggle_dashboard()
				dashboard_ok = dashboard_ok and not dashboard_panel.visible
				var heatmap_checksum := ""
				heatmap_ui_ok = not heatmap_visible
				heatmap_ui_ok = heatmap_ui_ok and heatmap_root != null and heatmap_root.name == "DistrictHeatmap"
				heatmap_ui_ok = heatmap_ui_ok and heatmap_root.get_child_count() == 0
				heatmap_ui_ok = heatmap_ui_ok and status_label.text.contains(i18n.t("base.ui.heatmap.status", {
					"state": i18n.t("base.ui.heatmap.off"),
					"cells": i18n.format_int(0),
					"peak": i18n.format_int(0),
				}))
				heatmap_ui_ok = heatmap_ui_ok and _toggle_heatmap()
				var heatmap_state: Dictionary = heatmap_last_state
				heatmap_ui_ok = heatmap_ui_ok and bool(heatmap_state.get("ok", false))
				var heatmap_cols := int(heatmap_state.get("cols", 0))
				var heatmap_rows := int(heatmap_state.get("rows", 0))
				heatmap_ui_ok = heatmap_ui_ok and heatmap_cols == 32 and heatmap_rows == 32
				heatmap_ui_ok = heatmap_ui_ok and absf(float(heatmap_state.get("radius_m", 0.0)) - HEATMAP_RADIUS_M) < 0.001
				heatmap_ui_ok = heatmap_ui_ok and absf(float(heatmap_state.get("cell_m", 0.0)) - HEATMAP_CELL_M) < 0.001
				var heatmap_cell_list: Array = heatmap_state.get("cells", [])
				heatmap_ui_ok = heatmap_ui_ok and heatmap_cell_list.size() == heatmap_cols * heatmap_rows
				heatmap_ui_ok = heatmap_ui_ok and heatmap_cell_list.size() <= HEATMAP_MAX_CELLS
				var heatmap_nonzero := 0
				var heatmap_expected_peak := 0.0
				for heatmap_cell in heatmap_cell_list:
					var heatmap_population := float(int(heatmap_cell.get("population", 0)))
					if heatmap_population > 0.0:
						heatmap_nonzero += 1
						heatmap_expected_peak = maxf(heatmap_expected_peak, heatmap_population)
				heatmap_ui_ok = heatmap_ui_ok and heatmap_nonzero == heatmap_cells
				heatmap_ui_ok = heatmap_ui_ok and is_equal_approx(heatmap_expected_peak, heatmap_peak)
				heatmap_ui_ok = heatmap_ui_ok and heatmap_root.get_child_count() == 1
				var heatmap_pool: MultiMeshInstance3D = heatmap_multimesh_instance
				heatmap_ui_ok = heatmap_ui_ok and heatmap_pool != null and is_instance_valid(heatmap_pool)
				heatmap_ui_ok = heatmap_ui_ok and heatmap_pool.get_parent() == heatmap_root
				heatmap_ui_ok = heatmap_ui_ok and heatmap_multimesh != null
				heatmap_ui_ok = heatmap_ui_ok and heatmap_multimesh.visible_instance_count == heatmap_nonzero
				heatmap_ui_ok = heatmap_ui_ok and heatmap_multimesh.instance_count <= HEATMAP_MAX_CELLS
				heatmap_ui_ok = heatmap_ui_ok and heatmap_multimesh.instance_count >= heatmap_nonzero
				heatmap_ui_ok = heatmap_ui_ok and heatmap_multimesh.use_colors
				heatmap_checksum = str(heatmap_state.get("checksum", ""))
				heatmap_ui_ok = heatmap_ui_ok and heatmap_checksum.length() == 16
				print("HEATMAP_UI cells=%d instances=%d peak=%.3f checksum=%s" % [
					heatmap_nonzero,
					heatmap_multimesh.visible_instance_count if heatmap_multimesh != null else -1,
					heatmap_peak,
					heatmap_checksum,
				])
				var heatmap_center: PackedFloat32Array = heatmap_state.get("center", PackedFloat32Array())
				heatmap_ui_ok = heatmap_ui_ok and heatmap_center.size() == 2
				if heatmap_center.size() == 2:
					var heatmap_repeat: Dictionary = core.district_heatmap(
						SEED, heatmap_center[0], heatmap_center[1], HEATMAP_RADIUS_M, HEATMAP_CELL_M
					)
					heatmap_ui_ok = heatmap_ui_ok and bool(heatmap_repeat.get("ok", false))
					heatmap_ui_ok = heatmap_ui_ok and str(heatmap_repeat.get("checksum", "")) == heatmap_checksum
				else:
					heatmap_ui_ok = false
				# An explicit settlement selection moves the F7 query centre;
				# clearing it restores the camera-centred checksum pin.
				var camera_heatmap_checksum := str(heatmap_last_state.get("checksum", ""))
				# Camera-centred fixture pin. It depends only on the fixture
				# placements and the fixed 512 m / 32 m query, so a change here
				# means the fixture or the query drifted.
				heatmap_ui_ok = heatmap_ui_ok and camera_heatmap_checksum == "3cba652232a478a0"
				settlement_select_ok = settlement_select_ok and _select_settlement(second_settlement_id)
				var settlement_heatmap_center: PackedFloat32Array = heatmap_last_state.get("center", PackedFloat32Array())
				heatmap_ui_ok = heatmap_ui_ok and bool(heatmap_last_state.get("ok", false))
				heatmap_ui_ok = heatmap_ui_ok and settlement_heatmap_center.size() == 2
				if settlement_heatmap_center.size() == 2:
					heatmap_ui_ok = heatmap_ui_ok and absf(settlement_heatmap_center[0] - second_settlement_center.x) < 0.001
					heatmap_ui_ok = heatmap_ui_ok and absf(settlement_heatmap_center[1] - second_settlement_center.y) < 0.001
				var settlement_heatmap_checksum := str(heatmap_last_state.get("checksum", ""))
				heatmap_ui_ok = heatmap_ui_ok and settlement_heatmap_checksum.length() == 16
				heatmap_ui_ok = heatmap_ui_ok and settlement_heatmap_checksum != camera_heatmap_checksum
				settlement_select_ok = settlement_select_ok and _select_settlement(-1)
				heatmap_ui_ok = heatmap_ui_ok and str(heatmap_last_state.get("checksum", "")) == camera_heatmap_checksum
				heatmap_ui_ok = heatmap_ui_ok and status_label.text.contains(i18n.t("base.ui.heatmap.legend", {
					"metric": i18n.t("base.ui.heatmap.metric.population"),
				}))
				if heatmap_nonzero > 0:
					heatmap_ui_ok = heatmap_ui_ok and not status_label.text.contains(i18n.t("base.ui.heatmap.empty"))
				else:
					# The fixture may have too few placements for a populated
					# cell; keep the grid/instance contract and show the empty
					# legend instead of requiring residents.
					heatmap_ui_ok = heatmap_ui_ok and status_label.text.contains(i18n.t("base.ui.heatmap.empty"))
				heatmap_ui_ok = heatmap_ui_ok and status_label.text.contains(i18n.t("base.ui.heatmap.status", {
					"state": i18n.t("base.ui.heatmap.on"),
					"cells": i18n.format_int(heatmap_nonzero),
					"peak": _heatmap_peak_text(),
				}))
				heatmap_ui_ok = heatmap_ui_ok and hud.text.contains("F7 heatmap")
				# Heatmap modes: F7 opens on population, Shift+F7 cycles
				# jobs -> happiness -> population. Each mode redraws through
				# the same bounded pool, the metric line names the active
				# metric, and the colour/peak contract is recomputed from the
				# bridge payload rather than trusting the renderer.
				heatmap_modes_ok = heatmap_metric == "population"
				heatmap_modes_ok = heatmap_modes_ok and status_label.text.contains(i18n.t("base.ui.heatmap.metric_line", {
					"metric": i18n.t("base.ui.heatmap.metric.population"),
					"peak": _heatmap_peak_text(),
				}))
				heatmap_modes_ok = heatmap_modes_ok and status_label.text.contains("Shift+F7")
				var heatmap_mode_keys: Array[String] = [heatmap_rebuild_key]
				var heatmap_mode_peaks: Array[float] = [heatmap_peak]
				var heatmap_mode_colors: Array[Color] = []
				for heatmap_mode in ["jobs", "happiness"]:
					var heatmap_previous_key := heatmap_rebuild_key
					var heatmap_cycled: String = _cycle_heatmap_metric()
					var heatmap_cycle_ok: bool = heatmap_cycled == heatmap_mode
					var heatmap_metric_ok: bool = heatmap_metric == heatmap_mode
					var heatmap_key_ok: bool = heatmap_rebuild_key != heatmap_previous_key \
						and heatmap_rebuild_key.begins_with(heatmap_mode + "|")
					var heatmap_mode_nonzero := 0
					var heatmap_mode_peak := 0.0
					for heatmap_cell in heatmap_cell_list:
						var heatmap_value := _heatmap_metric_value(heatmap_cell)
						if heatmap_value > 0.0:
							heatmap_mode_nonzero += 1
							heatmap_mode_peak = maxf(heatmap_mode_peak, heatmap_value)
					var heatmap_peak_ok: bool = is_finite(heatmap_peak) and heatmap_peak >= 0.0 \
						and is_equal_approx(heatmap_mode_peak, heatmap_peak)
					var heatmap_mode_instances := heatmap_multimesh.visible_instance_count if heatmap_multimesh != null else -1
					var heatmap_instances_ok: bool = heatmap_mode_instances == heatmap_mode_nonzero \
						and heatmap_mode_instances <= HEATMAP_MAX_CELLS
					var heatmap_mode_label: String = str(i18n.t(HEATMAP_METRIC_KEYS.get(heatmap_mode, "base.ui.heatmap.metric.population")))
					var heatmap_label_ok: bool = status_label.text.contains(i18n.t("base.ui.heatmap.metric_line", {
						"metric": heatmap_mode_label,
						"peak": _heatmap_peak_text(),
					}))
					heatmap_label_ok = heatmap_label_ok and not status_label.text.contains("!base.ui.")
					# Colour determinism: recompute the first rendered cell's
					# value/peak ratio from the bridge payload and the active
					# metric. Headless MultiMesh colour readback returns defaults,
					# so the contract is the deterministic colour helper input.
					var heatmap_color_ok: bool = true
					var heatmap_expected_color := Color.TRANSPARENT
					if heatmap_mode_instances > 0:
						var heatmap_first_value := -1.0
						for heatmap_cell in heatmap_cell_list:
							var heatmap_value := _heatmap_metric_value(heatmap_cell)
							if heatmap_value <= 0.0:
								continue
							var heatmap_x := float(heatmap_cell.get("x", 0.0))
							var heatmap_z := float(heatmap_cell.get("z", 0.0))
							if _heatmap_ground_at(heatmap_x, heatmap_z) >= 0.0:
								heatmap_first_value = heatmap_value
								break
						heatmap_color_ok = heatmap_first_value > 0.0
						if heatmap_color_ok:
							heatmap_expected_color = _heatmap_color(heatmap_first_value / heatmap_peak)
							heatmap_color_ok = heatmap_expected_color.a > 0.0
					heatmap_mode_colors.append(heatmap_expected_color)
					heatmap_mode_peaks.append(heatmap_peak)
					heatmap_modes_ok = heatmap_modes_ok and heatmap_cycle_ok and heatmap_metric_ok \
						and heatmap_key_ok and heatmap_mode_nonzero == heatmap_cells \
						and heatmap_peak_ok and heatmap_instances_ok and heatmap_label_ok \
						and heatmap_color_ok and (heatmap_mode_instances > 0 or heatmap_mode_nonzero == 0)
					heatmap_mode_keys.append(heatmap_rebuild_key)
					print("HEATMAP_MODE metric=%s cells=%d instances=%d peak=%.3f key=%s cycle=%s key_ok=%s peak_ok=%s label_ok=%s color=%s" % [
						heatmap_mode,
						heatmap_mode_nonzero,
						heatmap_mode_instances,
						heatmap_peak,
						heatmap_rebuild_key,
						heatmap_cycle_ok,
						heatmap_key_ok,
						heatmap_peak_ok,
						heatmap_label_ok,
						heatmap_expected_color,
					])
				heatmap_modes_ok = heatmap_modes_ok and heatmap_mode_keys[0] != heatmap_mode_keys[1]
				heatmap_modes_ok = heatmap_modes_ok and heatmap_mode_keys[1] != heatmap_mode_keys[2]
				heatmap_modes_ok = heatmap_modes_ok and heatmap_mode_keys[0] != heatmap_mode_keys[2]
				# The 0..1 happiness scale must stay bounded, and the colour ramp
				# must produce distinct low/high values deterministically.
				heatmap_modes_ok = heatmap_modes_ok and heatmap_mode_peaks.size() == 3 \
					and heatmap_mode_peaks[2] >= 0.0 and heatmap_mode_peaks[2] <= 1.0
				heatmap_modes_ok = heatmap_modes_ok and _heatmap_color(0.0) != _heatmap_color(1.0)
				heatmap_modes_ok = heatmap_modes_ok and _heatmap_color(0.25) != _heatmap_color(0.75)
				if heatmap_cells > 0:
					heatmap_modes_ok = heatmap_modes_ok and heatmap_mode_colors.size() == 2 \
						and heatmap_mode_colors[0].a > 0.0 and heatmap_mode_colors[1].a > 0.0
				# Localized metric labels resolve in DE and EN.
				i18n.set_locale("de")
				_update_hud()
				heatmap_modes_ok = heatmap_modes_ok and status_label.text.contains("Kennzahl: Zufriedenheit")
				heatmap_modes_ok = heatmap_modes_ok and status_label.text.contains("Umschalt+F7")
				heatmap_modes_ok = heatmap_modes_ok and not status_label.text.contains("!base.ui.")
				i18n.set_locale("en")
				_update_hud()
				heatmap_modes_ok = heatmap_modes_ok and status_label.text.contains("Metric: Happiness")
				heatmap_modes_ok = heatmap_modes_ok and status_label.text.contains("Shift+F7")
				heatmap_modes_ok = heatmap_modes_ok and not status_label.text.contains("!base.ui.")
				# Colorblind-safe ramp contract: at least five ordered stops,
				# distinct endpoints and neighbours, alpha inside the
				# 0.62..0.74 readability window, and non-decreasing luminance
				# across 0..1. The legend names the active metric and spells
				# out Low/High in both locales, so the scale never depends on
				# hue alone.
				var heatmap_ramp_adjacent_ok := true
				for heatmap_ramp_index in range(HEATMAP_RAMP_STOPS.size() - 1):
					if HEATMAP_RAMP_STOPS[heatmap_ramp_index] == HEATMAP_RAMP_STOPS[heatmap_ramp_index + 1]:
						heatmap_ramp_adjacent_ok = false
				var heatmap_ramp_alpha_ok := true
				for heatmap_ramp_stop in HEATMAP_RAMP_STOPS:
					# Color stores float32, so 0.74 reads back a hair above
					# 0.74; keep a small tolerance on the documented window.
					heatmap_ramp_alpha_ok = heatmap_ramp_alpha_ok \
						and (heatmap_ramp_stop.a >= 0.62 - 0.0005 and heatmap_ramp_stop.a <= 0.74 + 0.0005)
				var heatmap_ramp_luminance_ok := true
				var heatmap_ramp_previous_luminance := -1.0
				for heatmap_ramp_sample in range(17):
					var heatmap_ramp_sample_luminance := _heatmap_ramp_luminance(
						_heatmap_color(float(heatmap_ramp_sample) / 16.0))
					if heatmap_ramp_sample_luminance < heatmap_ramp_previous_luminance:
						heatmap_ramp_luminance_ok = false
					heatmap_ramp_previous_luminance = heatmap_ramp_sample_luminance
				var heatmap_legend_en_ok: bool = status_label.text.contains("Low") \
					and status_label.text.contains("High") \
					and status_label.text.contains("Happiness")
				i18n.set_locale("de")
				_update_hud()
				var heatmap_legend_de_ok: bool = status_label.text.contains("Niedrig") \
					and status_label.text.contains("Hoch") \
					and status_label.text.contains("Zufriedenheit") \
					and not status_label.text.contains("!base.ui.")
				i18n.set_locale("en")
				_update_hud()
				var heatmap_ramp_ok: bool = HEATMAP_RAMP_STOPS.size() >= 5 \
					and heatmap_ramp_adjacent_ok and heatmap_ramp_alpha_ok \
					and heatmap_ramp_luminance_ok and heatmap_legend_en_ok \
					and heatmap_legend_de_ok \
					and _heatmap_color(0.0) != _heatmap_color(1.0)
				heatmap_modes_ok = heatmap_modes_ok and heatmap_ramp_ok
				print("HEATMAP_RAMP stops=%d low_high=%s adjacent=%s alpha=%s luminance=%s legend_en=%s legend_de=%s status=%s" % [
					HEATMAP_RAMP_STOPS.size(),
					"diff" if _heatmap_color(0.0) != _heatmap_color(1.0) else "same",
					"diff" if heatmap_ramp_adjacent_ok else "same",
					"ok" if heatmap_ramp_alpha_ok else "fail",
					"ok" if heatmap_ramp_luminance_ok else "fail",
					"ok" if heatmap_legend_en_ok else "fail",
					"ok" if heatmap_legend_de_ok else "fail",
					"ok" if heatmap_ramp_ok else "fail",
				])
				# Wrap back to population so the off-state check stays the
				# same numeric contract as the pre-modes smoke.
				var heatmap_wrapped: String = _cycle_heatmap_metric()
				heatmap_modes_ok = heatmap_modes_ok and heatmap_wrapped == "population"
				heatmap_modes_ok = heatmap_modes_ok and heatmap_metric == "population"
				i18n.set_locale("de")
				_update_hud()
				heatmap_ui_ok = heatmap_ui_ok and status_label.text.contains("Stadtteil-Heatmap: an")
				heatmap_ui_ok = heatmap_ui_ok and hud.text.contains("F7: Heatmap")
				i18n.set_locale("en")
				_update_hud()
				heatmap_ui_ok = heatmap_ui_ok and not _toggle_heatmap()
				heatmap_ui_ok = heatmap_ui_ok and heatmap_root.get_child_count() == 0
				heatmap_ui_ok = heatmap_ui_ok and heatmap_multimesh_instance == null
				heatmap_ui_ok = heatmap_ui_ok and heatmap_multimesh == null
				heatmap_ui_ok = heatmap_ui_ok and heatmap_ground.is_empty()
				heatmap_ui_ok = heatmap_ui_ok and heatmap_last_state.is_empty()
				heatmap_ui_ok = heatmap_ui_ok and heatmap_rebuild_key == ""
				heatmap_modes_ok = heatmap_modes_ok and heatmap_root.get_child_count() == 0
				heatmap_modes_ok = heatmap_modes_ok and heatmap_multimesh == null
				heatmap_modes_ok = heatmap_modes_ok and heatmap_ground.is_empty()
				heatmap_modes_ok = heatmap_modes_ok and heatmap_rebuild_key == ""
				heatmap_ui_ok = heatmap_ui_ok and status_label.text.contains(i18n.t("base.ui.heatmap.status", {
					"state": i18n.t("base.ui.heatmap.off"),
					"cells": i18n.format_int(0),
					"peak": _heatmap_peak_text(),
				}))
				i18n.set_locale("de")
				_update_dashboard()
				_update_hud()
				hud_i18n_ok = dashboard_title.text.contains("Stadt") and dashboard_text.text.contains("Bevölkerung")
				dashboard_body_i18n_ok = dashboard_text.text.contains("Bewohner")
				dashboard_body_i18n_ok = dashboard_body_i18n_ok and dashboard_text.text.contains("Arbeitsplätze")
				dashboard_body_i18n_ok = dashboard_body_i18n_ok and dashboard_text.text.contains("Zufriedenheit")
				dashboard_body_i18n_ok = dashboard_body_i18n_ok and not dashboard_text.text.contains("!base.ui.")
				district_ui_ok = district_ui_ok and dashboard_text.text.contains("Stadtteil")
				district_ui_ok = district_ui_ok and dashboard_text.text.contains("Im Umkreis von 256 m")
				district_ui_ok = district_ui_ok and _district_section({}).contains(i18n.t("base.ui.dashboard.body.district_unavailable"))
				district_ui_ok = district_ui_ok and not _district_section({}).contains("!base.ui.")
				advisor_i18n_ok = dashboard_text.text.contains("Haushaltsdefizit")
				advisor_i18n_ok = advisor_i18n_ok and i18n.advisor_title("city.unknown", "Fallback") == "Fallback"
				advisor_detail_i18n_ok = i18n.advisor_detail("city.budget_deficit", "Fallback", {"net": "-1"}).contains("verliert")
				i18n.set_locale("en")
				advisor_detail_i18n_ok = advisor_detail_i18n_ok and i18n.advisor_detail("city.budget_deficit", "Fallback", {"net": "-1"}).contains("losing")
				i18n.set_locale("de")
				hud_body_i18n_ok = hud.text.contains("Bürgermeistermodus")
				hud_body_i18n_ok = hud_body_i18n_ok and status_label.text.contains("Platzierungen:")
				hud_body_i18n_ok = hud_body_i18n_ok and not hud.text.contains("!base.ui.")
				hud_body_i18n_ok = hud_body_i18n_ok and not status_label.text.contains("!base.ui.")
				hud_status_i18n_ok = status_label.text.contains("Zeitalter:")
				hud_status_i18n_ok = hud_status_i18n_ok and status_label.text.contains("Forschung:")
				hud_status_i18n_ok = hud_status_i18n_ok and status_label.text.contains("Industrie:")
				_set_modal(i18n.t("base.ui.modal.crew", {"count": 4}), false)
				hud_status_i18n_ok = hud_status_i18n_ok and modal_label != null and modal_label.text.contains("Bautrupp")
				i18n.set_locale("en")
				_update_dashboard()
				_update_hud()
				hud_i18n_ok = hud_i18n_ok and dashboard_title.text.contains("City Dashboard") and dashboard_text.text.contains("Population")
				dashboard_body_i18n_ok = dashboard_body_i18n_ok and dashboard_text.text.contains("residents")
				dashboard_body_i18n_ok = dashboard_body_i18n_ok and dashboard_text.text.contains("jobs filled")
				dashboard_body_i18n_ok = dashboard_body_i18n_ok and dashboard_text.text.contains("happiness")
				dashboard_body_i18n_ok = dashboard_body_i18n_ok and not dashboard_text.text.contains("!base.ui.")
				district_ui_ok = district_ui_ok and dashboard_text.text.contains("District")
				district_ui_ok = district_ui_ok and dashboard_text.text.contains("Within 256 m of the camera target")
				advisor_i18n_ok = advisor_i18n_ok and dashboard_text.text.contains("The city is running a deficit")
				hud_body_i18n_ok = hud_body_i18n_ok and hud.text.contains("Mayor Mode")
				hud_body_i18n_ok = hud_body_i18n_ok and status_label.text.contains("Placements:")
				hud_body_i18n_ok = hud_body_i18n_ok and not hud.text.contains("!base.ui.")
				hud_body_i18n_ok = hud_body_i18n_ok and not status_label.text.contains("!base.ui.")
				hud_status_i18n_ok = hud_status_i18n_ok and status_label.text.contains("Age:")
				hud_status_i18n_ok = hud_status_i18n_ok and status_label.text.contains("Research:")
				hud_status_i18n_ok = hud_status_i18n_ok and status_label.text.contains("Industry:")
				_set_modal(i18n.t("base.ui.modal.crew", {"count": 4}), false)
				hud_status_i18n_ok = hud_status_i18n_ok and modal_label != null and modal_label.text.contains("Construction crew")
				_set_modal("", false)
				dashboard_ok = dashboard_ok and hud_i18n_ok
				var saved_selected := selected_id
				selected_id = -1
				i18n.set_locale("de")
				_cycle_industry()
				modal_i18n_ok = modal_label.text.contains("Zuerst ein Gebäude auswählen")
				i18n.set_locale("en")
				_cycle_industry()
				modal_i18n_ok = modal_i18n_ok and modal_label.text.contains("Select a building first")
				i18n.set_locale("de")
				number_i18n_ok = i18n.format_int(1000) == "1.000" and i18n.format_float(1234.5, 1) == "1.234,5"
				var expected_de: String = i18n.format_float(float(economy_after.get("balance", 0.0)), 0)
				_update_dashboard()
				number_i18n_ok = number_i18n_ok and dashboard_text.text.contains(expected_de)
				i18n.set_locale("en")
				number_i18n_ok = number_i18n_ok and i18n.format_int(1000) == "1,000" and i18n.format_float(1234.5, 1) == "1,234.5"
				var expected_en: String = i18n.format_float(float(economy_after.get("balance", 0.0)), 0)
				_update_dashboard()
				number_i18n_ok = number_i18n_ok and dashboard_text.text.contains(expected_en)
				number_i18n_ok = number_i18n_ok and expected_de != expected_en
				selected_id = saved_selected
				_set_modal("", false)
				var balance_before_loan := float(economy_after.get("balance", 0.0))
				var loan_state: Dictionary = core.economy_borrow(SEED, LOAN_AMOUNT)
				var tax_state: Dictionary = core.economy_set_taxes(SEED, 100.0, 8.0)
				var economy_recovered: Dictionary = core.economy_state()
				economy_ok = bool(loan_state.get("ok", false)) and bool(tax_state.get("ok", false))
				economy_ok = economy_ok and absf(
					float(economy_recovered.get("balance", 0.0)) - (balance_before_loan + LOAN_AMOUNT)
				) < 0.01
				economy_ok = economy_ok and float(economy_recovered.get("debt", 0.0)) > 49999.0
				economy_ok = economy_ok and float(economy_recovered.get("daily_net", 0.0)) > 0.0
				occupancy_after_ok = occupancy_after_ok and economy_ok
				var economy_balance := float(economy_recovered.get("balance", 0.0))
				var economy_net := float(economy_recovered.get("daily_net", 0.0))
				var economy_debt := float(economy_recovered.get("debt", 0.0))
				var research_generation: Dictionary = core.research_tick(SEED, 86400.0)
				var research_before: Dictionary = core.research_state(SEED)
				var research_checksum: String = str(core.research_checksum())
				var research_points := float(research_before.get("points", 0.0))
				research_ok = bool(research_generation.get("ok", false))
				research_ok = research_ok and float(research_generation.get("gained", 0.0)) > 8.0
				research_ok = research_ok and bool(research_before.get("ok", false))
				research_ok = research_ok and int(research_before.get("node_count", 0)) >= 14
				research_ok = research_ok and str(research_before.get("age", "")) == "wood"
				research_ok = research_ok and research_points > 50.0
				var timber_result: Dictionary = core.research_unlock(SEED, "construction.timber_frame")
				research_ok = research_ok and bool(timber_result.get("ok", false))
				research_ok = research_ok and int(timber_result.get("completed_count", 0)) == 1
				research_ok = research_ok and float(timber_result.get("points", 0.0)) < research_points
				var duplicate_result: Dictionary = core.research_unlock(SEED, "construction.timber_frame")
				research_ok = research_ok and not bool(duplicate_result.get("ok", true))
				research_ok = research_ok and str(duplicate_result.get("error_code", "")) == "already_unlocked"
				var locked_result: Dictionary = core.research_unlock(SEED, "construction.concrete")
				research_ok = research_ok and not bool(locked_result.get("ok", true))
				research_ok = research_ok and str(locked_result.get("error_code", "")) == "age_locked"
				research_checksum = str(core.research_checksum())
				var research_points_after := float(core.research_state(SEED).get("points", 0.0))
				occupancy_after_ok = occupancy_after_ok and research_ok
				var starter_catalog: Dictionary = core.starter_library(SEED)
				starter_ok = bool(starter_catalog.get("ok", false))
				starter_ok = starter_ok and str(starter_catalog.get("age", "")) == "wood"
				var starter_items: Array = starter_catalog.get("items", [])
				starter_ok = starter_ok and starter_items.size() >= 7
				var starter_available := 0
				for item in starter_items:
					if bool(item.get("available", false)):
						starter_available += 1
				starter_ok = starter_ok and starter_available >= 7
				var starter_bundle: Dictionary = core.starter_blueprint(SEED, "base:cottage")
				starter_ok = starter_ok and bool(starter_bundle.get("ok", false))
				var starter_path := str(starter_bundle.get("path", ""))
				starter_ok = starter_ok and starter_path != "" and FileAccess.file_exists(starter_path)
				var starter_info: Dictionary = core.world_blueprint_info(starter_path)
				starter_ok = starter_ok and bool(starter_info.get("ok", false))
				starter_ok = starter_ok and str(starter_info.get("id", "")) == "base:cottage"
				starter_ok = starter_ok and int(starter_info.get("resident_capacity", 0)) == 4
				var starter_structure: Dictionary = starter_info.get("structure", {})
				starter_ok = starter_ok and bool(starter_structure.get("present", false))
				var starter_mesh: ArrayMesh = core.build_blueprint_mesh(starter_path)
				starter_ok = starter_ok and starter_mesh.get_surface_count() > 0
				if occupancy_after_ok:
					var population_checksum: String = str(core.population_checksum())
					var economy_checksum: String = str(core.economy_checksum())
					var industry_checksum: String = str(core.industry_checksum())
					var industry_stock := float(core.industry_state().get("food", 0.0))
					var utility_checksum: String = str(core.utilities_checksum())
					var utility_state_before: Dictionary = core.utilities_state()
					var utility_water := float(utility_state_before.get("water_supply", 0.0))
					var utility_power := float(utility_state_before.get("power_supply", 0.0))
					var service_checksum: String = str(core.services_checksum())
					var service_state_before: Dictionary = core.services_state()
					var service_school := float(service_state_before.get("school_supply", 0.0))
					var service_research := float(service_state_before.get("research_per_day", 0.0))
					var populated := _save_world()
					core.population_clear()
					core.economy_clear()
					core.industry_clear()
					core.utilities_clear()
					core.services_clear()
					core.research_clear()
					var reloaded := _load_world()
					var city_persisted: Dictionary = core.city_summary()
					var households: Dictionary = core.population_state()
					occupancy_after_ok = populated and reloaded
					occupancy_after_ok = occupancy_after_ok and int(city_persisted.get("population", -1)) == 2
					occupancy_after_ok = occupancy_after_ok and int(city_persisted.get("vacancies", -1)) == 0
					occupancy_after_ok = occupancy_after_ok and absf(float(city_persisted.get("daily_wages", 0.0)) - 40.0) < 0.01
					occupancy_after_ok = occupancy_after_ok and absf(float(city_persisted.get("average_commute_m", -1.0))) < 0.001
					var household_list: Array = households.get("households", [])
					occupancy_after_ok = occupancy_after_ok and household_list.size() == 1
					if household_list.size() == 1:
						var saved_household: Dictionary = household_list[0]
						occupancy_after_ok = occupancy_after_ok and absf(float(saved_household.get("commute_m", -1.0))) < 0.001
						occupancy_after_ok = occupancy_after_ok and absf(float(saved_household.get("income_per_day", 0.0)) - 40.0) < 0.01
					occupancy_after_ok = occupancy_after_ok and str(core.population_checksum()) == population_checksum
					occupancy_after_ok = occupancy_after_ok and str(core.economy_checksum()) == economy_checksum
					var economy_persisted: Dictionary = core.economy_state()
					occupancy_after_ok = occupancy_after_ok and bool(economy_persisted.get("open", false))
					occupancy_after_ok = occupancy_after_ok and absf(float(economy_persisted.get("balance", 0.0)) - economy_balance) < 0.01
					occupancy_after_ok = occupancy_after_ok and absf(float(economy_persisted.get("daily_net", 0.0)) - economy_net) < 0.01
					occupancy_after_ok = occupancy_after_ok and absf(float(economy_persisted.get("debt", 0.0)) - economy_debt) < 0.01
					occupancy_after_ok = occupancy_after_ok and not bool(economy_persisted.get("bankrupt", true))
					var industry_persisted: Dictionary = core.industry_state()
					occupancy_after_ok = occupancy_after_ok and bool(industry_persisted.get("open", false))
					occupancy_after_ok = occupancy_after_ok and int(industry_persisted.get("site_count", -1)) == 1
					occupancy_after_ok = occupancy_after_ok and absf(float(industry_persisted.get("food", 0.0)) - industry_stock) < 0.01
					occupancy_after_ok = occupancy_after_ok and str(core.industry_checksum()) == industry_checksum
					occupancy_after_ok = occupancy_after_ok and industry_nodes.size() == 1
					var utility_persisted: Dictionary = core.utilities_state()
					var utility_persisted_ok := bool(utility_persisted.get("open", false))
					utility_persisted_ok = utility_persisted_ok and int(utility_persisted.get("site_count", 0)) == 2
					utility_persisted_ok = utility_persisted_ok and int(utility_persisted.get("water_sites", 0)) == 1
					utility_persisted_ok = utility_persisted_ok and int(utility_persisted.get("power_sites", 0)) == 1
					utility_persisted_ok = utility_persisted_ok and absf(float(utility_persisted.get("water_supply", 0.0)) - utility_water) < 0.01
					utility_persisted_ok = utility_persisted_ok and absf(float(utility_persisted.get("power_supply", 0.0)) - utility_power) < 0.01
					utility_persisted_ok = utility_persisted_ok and str(core.utilities_checksum()) == utility_checksum
					utility_persisted_ok = utility_persisted_ok and str(utility_persisted.get("checksum", "")) == str(core.utilities_checksum())
					utility_persisted_ok = utility_persisted_ok and utility_nodes.size() == 2
					utility_ok = utility_ok and utility_persisted_ok
					var service_persisted: Dictionary = core.services_state()
					var service_persisted_ok := bool(service_persisted.get("open", false))
					service_persisted_ok = service_persisted_ok and int(service_persisted.get("site_count", 0)) == 1
					service_persisted_ok = service_persisted_ok and int(service_persisted.get("school_sites", 0)) == 1
					service_persisted_ok = service_persisted_ok and absf(float(service_persisted.get("school_supply", 0.0)) - service_school) < 0.01
					service_persisted_ok = service_persisted_ok and absf(float(service_persisted.get("research_per_day", 0.0)) - service_research) < 0.01
					service_persisted_ok = service_persisted_ok and str(core.services_checksum()) == service_checksum
					service_persisted_ok = service_persisted_ok and service_nodes.size() == 1
					service_ok = service_ok and service_persisted_ok
					var research_persisted: Dictionary = core.research_state(SEED)
					research_ok = research_ok and bool(research_persisted.get("open", false))
					research_ok = research_ok and int(research_persisted.get("completed_count", -1)) == 1
					research_ok = research_ok and absf(float(research_persisted.get("points", 0.0)) - research_points_after) < 0.001
					research_ok = research_ok and str(core.research_checksum()) == research_checksum
				if not build_done_ok:
					status = "construction_complete_failed"
				elif not render_ok:
					status = "render_failed"
				elif not structure_ok:
					status = "structure_failed"
				elif not age_ok:
					status = "age_failed"
				elif not research_ok:
					status = "research_failed"
				elif not (occupancy_before_ok and occupancy_after_ok):
					status = "occupancy_failed"
				elif not named_ok:
					status = "named_failed"
				elif not citizens_ok:
					status = "citizens_failed"
				elif not follow_ok:
					status = "follow_failed"
				elif not citizen_i18n_ok:
					status = "citizen_i18n_failed"
				elif not materialization_ok:
					status = "materialization_failed"
				elif not materialization_fade_ok:
					status = "materialization_fade_failed"
				elif not industry_ok:
					status = "industry_failed"
				elif not utility_ok:
					status = "utility_failed"
				elif not service_ok:
					status = "service_failed"
				elif not tutorial_ok:
					status = "tutorial_failed"
				elif not dashboard_ok:
					status = "dashboard_failed"
				elif not district_ui_ok:
					status = "district_ui_failed"
				elif not policy_ui_ok:
					status = "policy_ui_failed"
				elif not policy_edit_ok:
					status = "policy_edit_failed"
				elif not heatmap_ui_ok:
					status = "heatmap_ui_failed"
				elif not heatmap_modes_ok:
					status = "heatmap_modes_failed"
			if not load_ok:
				status = "load_failed"

	if status == "ok":
		# A failed load must leave the active store, deltas, and nodes alone.
		var before_failed_load := placed_root.get_child_count()
		var before_failed_edits: int = core.terrain_delta_count()
		var before_failed_checksum: String = str(core.terrain_delta_checksum())
		var before_failed_lots: int = core.lot_count()
		var before_failed_lot_checksum: String = str(core.lot_checksum())
		var before_failed_settlements: int = core.settlement_count()
		var before_failed_settlement_checksum: String = str(core.settlement_checksum())
		var before_failed_economy: String = str(core.economy_checksum())
		var before_failed_industry: String = str(core.industry_checksum())
		var before_failed_utility: String = str(core.utilities_checksum())
		var before_failed_service: String = str(core.services_checksum())
		var blueprint_backup := blueprint_path + ".bak"
		if DirAccess.rename_absolute(blueprint_path, blueprint_backup) != OK:
			status = "load_guard_backup_failed"
		else:
			var failed_load := _load_world()
			if failed_load or core.world_placement_count() != 1 or placed_root.get_child_count() != before_failed_load:
				status = "load_failure_changed_scene"
			elif core.terrain_delta_count() != before_failed_edits or str(core.terrain_delta_checksum()) != before_failed_checksum:
				status = "load_failure_changed_deltas"
			elif str(core.economy_checksum()) != before_failed_economy:
				status = "load_failure_changed_economy"
			elif str(core.industry_checksum()) != before_failed_industry:
				status = "load_failure_changed_industry"
			elif str(core.utilities_checksum()) != before_failed_utility:
				status = "load_failure_changed_utility"
			elif str(core.services_checksum()) != before_failed_service:
				status = "load_failure_changed_service"
			elif core.lot_count() != before_failed_lots or str(core.lot_checksum()) != before_failed_lot_checksum:
				status = "load_failure_changed_lots"
			elif core.settlement_count() != before_failed_settlements or str(core.settlement_checksum()) != before_failed_settlement_checksum:
				status = "load_failure_changed_settlements"
			if DirAccess.rename_absolute(blueprint_backup, blueprint_path) != OK:
				status = "load_guard_restore_failed"

	if status == "ok":
		var before_named: Dictionary = core.named_citizens_state(SEED)
		var before_population := int(core.population_state().get("population", -1))
		var zero_tick: Dictionary = _tick_named_lifecycle(0.0)
		lifecycle_scene_ok = bool(zero_tick.get("ok", false))
		lifecycle_scene_ok = lifecycle_scene_ok and int(zero_tick.get("count", -1)) == int(before_named.get("count", -2))
		lifecycle_scene_ok = lifecycle_scene_ok and int(zero_tick.get("aged", -1)) == 0
		lifecycle_scene_ok = lifecycle_scene_ok and int(zero_tick.get("died", -1)) == 0
		lifecycle_scene_ok = lifecycle_scene_ok and int(zero_tick.get("applied_deaths", -1)) == 0
		var one_tick: Dictionary = _tick_named_lifecycle(1.0)
		lifecycle_scene_ok = lifecycle_scene_ok and bool(one_tick.get("ok", false))
		lifecycle_scene_ok = lifecycle_scene_ok and int(one_tick.get("count", -1)) > 0
		lifecycle_scene_ok = lifecycle_scene_ok and int(one_tick.get("aged", -1)) == int(one_tick.get("count", -2))
		lifecycle_scene_ok = lifecycle_scene_ok and int(one_tick.get("died", -1)) == 0
		lifecycle_scene_ok = lifecycle_scene_ok and int(one_tick.get("applied_deaths", -1)) == 0
		lifecycle_scene_ok = lifecycle_scene_ok and int(one_tick.get("population", -1)) == before_population
		lifecycle_scene_ok = lifecycle_scene_ok and int(one_tick.get("average_age_days", -1)) > 0
		if not lifecycle_scene_ok:
			status = "lifecycle_failed"

	if status == "ok":
		var developer_state: Dictionary = core.construction_time_scale_state()
		var developer_checksum := str(developer_state.get("checksum", ""))
		campaign_hud_ok = bool(developer_state.get("ok", false))
		campaign_hud_ok = campaign_hud_ok and str(developer_state.get("mode", "")) == "developer_seconds"
		campaign_hud_ok = campaign_hud_ok and developer_checksum.length() == 16
		campaign_hud_ok = campaign_hud_ok and status_label.text.contains("Clock: Developer")
		if campaign_hud_ok:
			var toggled_campaign := _toggle_construction_clock()
			var campaign_state: Dictionary = core.construction_time_scale_state()
			campaign_hud_ok = toggled_campaign and bool(campaign_state.get("ok", false))
			campaign_hud_ok = campaign_hud_ok and str(campaign_state.get("mode", "")) == "campaign_days"
			campaign_hud_ok = campaign_hud_ok and absf(float(campaign_state.get("seconds_per_game_day", 0.0)) - 60.0) < 0.001
			campaign_hud_ok = campaign_hud_ok and str(campaign_state.get("checksum", "")) == construction_clock_checksum
			campaign_hud_ok = campaign_hud_ok and str(campaign_state.get("checksum", "")) != developer_checksum
			campaign_hud_ok = campaign_hud_ok and status_label.text.contains("Clock: Campaign · 60 s/day · Day 0")
			i18n.set_locale("de")
			_update_hud()
			campaign_hud_ok = campaign_hud_ok and status_label.text.contains("Uhr: Kampagne · 60 s/Tag · Tag 0")
			campaign_hud_ok = campaign_hud_ok and not status_label.text.contains("!base.ui.")
			i18n.set_locale("en")
			_update_hud()
			var toggled_developer := _toggle_construction_clock()
			var restored_state: Dictionary = core.construction_time_scale_state()
			campaign_hud_ok = campaign_hud_ok and toggled_developer and bool(restored_state.get("ok", false))
			campaign_hud_ok = campaign_hud_ok and str(restored_state.get("mode", "")) == "developer_seconds"
			campaign_hud_ok = campaign_hud_ok and str(restored_state.get("checksum", "")) == developer_checksum
			campaign_hud_ok = campaign_hud_ok and status_label.text.contains("Clock: Developer")
		if not campaign_hud_ok:
			status = "campaign_hud_failed"

	if status == "ok" and not movement_lod_ok:
		status = "movement_lod_failed"

	if status == "ok" and not district_select_ok:
		status = "district_select_failed"

	if status == "ok" and not settlement_select_ok:
		status = "settlement_select_failed"

	if status == "ok" and not settlement_detail_ok:
		status = "settlement_detail_failed"

	if status == "ok" and not dashboard_tax_ok:
		status = "dashboard_tax_failed"

	if status == "ok" and rooms != 4:
		status = "rooms_mismatch"

	print("WORLD_SCENE rooms=%d preview=%s water=%s rotate=%s edits=%s chunk_cells=%s place=%s roads=%s lots=%s settlements=%s build=%s done=%s trades=%s render=%s structure=%s age=%s research=%s tutorial=%s starter=%s named=%s citizens=%s citizen_rows=%s content_i18n=%s content_ui=%s follow=%s citizen_i18n=%s materialization=%s materialization_step=%s materialization_fade=%s movement_scene=%s movement_pool=%s movement_routes=%s movement_lod=%s occupancy=%s industry=%s utilities=%s services=%s economy=%s dashboard=%s district_ui=%s policy_ui=%s policy_edit=%s district_select=%s settlement_select=%s settlement_detail=%s dashboard_tax=%s heatmap_ui=%s heatmap_modes=%s hud_i18n=%s hud_body_i18n=%s hud_status_i18n=%s dashboard_body_i18n=%s advisor_i18n=%s advisor_detail_i18n=%s modal_i18n=%s number_i18n=%s traffic=%s select=%s enter=%s return=%s save=%s load=%s guard=%s lifecycle_scene=%s campaign_hud=%s status=%s" % [
		rooms,
		"ok" if preview_ok else "failed",
		"ok" if water_ok else "failed",
		"ok" if rotation_ok else "failed",
		"ok" if edit_ok else "failed",
		"ok" if chunk_cells_ok else "failed",
		"ok" if place_ok else "failed",
		"ok" if road_ok else "failed",
		"ok" if lot_ok else "failed",
		"ok" if settlement_ok else "failed",
		"ok" if build_ok else "failed",
		"ok" if build_done_ok else "failed",
		"ok" if trades_ok else "failed",
		"ok" if render_ok else "failed",
		"ok" if structure_ok else "failed",
		"ok" if age_ok else "failed",
		"ok" if research_ok else "failed",
		"ok" if tutorial_ok else "failed",
		"ok" if starter_ok else "failed",
		"ok" if named_ok else "failed",
		"ok" if citizens_ok else "failed",
		"ok" if citizen_rows_ok else "failed",
		"ok" if content_i18n_ok else "failed",
		"ok" if content_ui_ok else "failed",
		"ok" if follow_ok else "failed",
		"ok" if citizen_i18n_ok else "failed",
		"ok" if materialization_ok else "failed",
		"ok" if materialization_step_ok else "failed",
		"ok" if materialization_fade_ok else "failed",
		"ok" if movement_scene_ok else "failed",
		"ok" if movement_pool_ok else "failed",
		"ok" if movement_routes_ok else "failed",
		"ok" if movement_lod_ok else "failed",
		"ok" if occupancy_before_ok and occupancy_after_ok else "failed",
		"ok" if industry_ok else "failed",
		"ok" if utility_ok else "failed",
		"ok" if service_ok else "failed",
		"ok" if economy_ok else "failed",
		"ok" if dashboard_ok else "failed",
		"ok" if district_ui_ok else "failed",
		"ok" if policy_ui_ok else "failed",
		"ok" if policy_edit_ok else "failed",
		"ok" if district_select_ok else "failed",
		"ok" if settlement_select_ok else "failed",
		"ok" if settlement_detail_ok else "failed",
		"ok" if dashboard_tax_ok else "failed",
		"ok" if heatmap_ui_ok else "failed",
		"ok" if heatmap_modes_ok else "failed",
		"ok" if hud_i18n_ok else "failed",
		"ok" if hud_body_i18n_ok else "failed",
		"ok" if hud_status_i18n_ok else "failed",
		"ok" if dashboard_body_i18n_ok else "failed",
		"ok" if advisor_i18n_ok else "failed",
		"ok" if advisor_detail_i18n_ok else "failed",
		"ok" if modal_i18n_ok else "failed",
		"ok" if number_i18n_ok else "failed",
		"ok" if traffic_ok else "failed",
		"ok" if select_ok else "failed",
		"ok" if enter_ok else "failed",
		"ok" if return_ok else "failed",
		"ok" if save_ok else "failed",
		"ok" if load_ok else "failed",
		"ok" if status == "ok" else "failed",
		"ok" if lifecycle_scene_ok else "failed",
		"ok" if campaign_hud_ok else "failed",
		status,
	])
	get_tree().quit(0 if status == "ok" else 1)
