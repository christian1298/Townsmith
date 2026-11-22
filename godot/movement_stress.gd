extends Node3D

## Standalone headless 2,000-agent `MultiMesh` marker update stress.
##
## Synthetic marker path only. 2,000 deterministic agents commute between 400
## homes and 200 workplaces on 16 m grids. Each agent has a fixed phase offset
## and a straight out-and-back path; nothing calls the Rust bridge,
## `citizen_movement`, the cohort/active-agent simulation, or the renderer.
##
## One pooled `MultiMeshInstance3D` is filled at 10 Hz for 1,200 updates (120
## simulated seconds). The harness reports per-update wall time (median/p95/max),
## instance and update counts, and an FNV-1a checksum over the final transforms.
##
## This measures the GDScript transform-write path and the `MultiMesh` buffer,
## not GPU frame time, pathfinding, or bridge state. It is **not** the
## 1M-population / RTX 3060 gate and **not** evidence that the live
## `citizen_movement` marker path is fast.
##
## Usage:
##   godot --headless --path godot res://movement_stress.tscn
##   godot --headless --path godot res://movement_stress.tscn -- --smoke
##   godot --headless --path godot res://movement_stress.tscn -- --agents 500

const MAX_AGENTS := 2000
const HOME_COLUMNS := 20
const HOME_ROWS := 20
const WORK_COLUMNS := 20
const WORK_ROWS := 10
const HOME_COUNT := HOME_COLUMNS * HOME_ROWS
const WORKPLACE_COUNT := WORK_COLUMNS * WORK_ROWS
const RESIDENTS_PER_HOME := 5
const GRID_SPACING_M := 16.0
# Workplaces sit one grid block beyond the home square so the out-and-back
# paths cover a few hundred metres instead of a tiny local blob.
const WORK_OFFSET_Z_M := 240.0
const UPDATE_HZ := 10.0
const UPDATE_DT := 1.0 / UPDATE_HZ
const SIMULATED_SECONDS := 120.0
const FULL_UPDATES := 1200
const SMOKE_UPDATES := 100
const MARKER_SIZE := Vector3(0.32, 1.7, 0.32)
const MARKER_COLOR := Color(0.18, 0.95, 0.72)
const FNV_OFFSET_BASIS := 2166136261
const FNV_PRIME := 16777619
const CHECKSUM_SCALE := 1000.0  # quantise metres to millimetres

var _agents := MAX_AGENTS
var _smoke_mode := false
var _multimesh: MultiMesh
var _multimesh_instance: MultiMeshInstance3D
# Per-agent path endpoints and timing, built once from integer-safe rules.
var _home_x := PackedFloat64Array()
var _home_z := PackedFloat64Array()
var _work_x := PackedFloat64Array()
var _work_z := PackedFloat64Array()
var _phase := PackedFloat64Array()
var _inv_period := PackedFloat64Array()


func _ready() -> void:
	var args := OS.get_cmdline_user_args()
	_smoke_mode = args.has("--smoke")
	_agents = _parse_agents(args)
	if _smoke_mode:
		_run_smoke()
		return
	_run_full()


func _parse_agents(args: PackedStringArray) -> int:
	var index := args.find("--agents")
	if index < 0 or index + 1 >= args.size():
		return MAX_AGENTS
	var raw := args[index + 1]
	if not raw.is_valid_int():
		push_warning("Ignoring non-integer --agents value: %s" % raw)
		return MAX_AGENTS
	return clampi(int(raw), 1, MAX_AGENTS)


func _run_full() -> void:
	_build_agents(_agents)
	if not _build_pool():
		return
	var result := _run_updates(FULL_UPDATES)
	var samples: Array[float] = result.get("samples", [])
	var sorted := samples.duplicate()
	sorted.sort()
	var status := "ok"
	if _multimesh.instance_count != MAX_AGENTS:
		status = "pool_instances=%d" % _multimesh.instance_count
	elif _multimesh.visible_instance_count != _agents:
		status = "visible_instances=%d" % _multimesh.visible_instance_count
	elif samples.size() != FULL_UPDATES:
		status = "updates=%d" % samples.size()
	var report := {
		"scene": "movement_stress_2k",
		"agents": _agents,
		"agent_cap": MAX_AGENTS,
		"homes": HOME_COUNT,
		"workplaces": WORKPLACE_COUNT,
		"grid_spacing_m": GRID_SPACING_M,
		"pool_instances": MAX_AGENTS,
		"visible_instances": _multimesh.visible_instance_count,
		"update_hz": UPDATE_HZ,
		"simulated_seconds": SIMULATED_SECONDS,
		"updates": samples.size(),
		"update_ms_median": _round_ms(_percentile(sorted, 0.5)),
		"update_ms_p95": _round_ms(_percentile(sorted, 0.95)),
		"update_ms_max": _round_ms(_percentile(sorted, 1.0)),
		"update_ms_mean": _round_ms(_mean(samples)),
		"total_ms": _round_ms(float(result.get("total_ms", 0.0))),
		"checksum": _checksum_hex(int(result.get("checksum", 0))),
		"status": status,
	}
	print("MOVEMENT_STRESS " + JSON.stringify(report))
	get_tree().quit(0 if status == "ok" else 1)


func _run_smoke() -> void:
	# Two independent passes from a fresh pool. The in-process comparison catches
	# state leaking between passes; the shell smoke runs the whole scene twice to
	# catch process-level nondeterminism.
	var first := _run_smoke_pass()
	_free_pool()
	var second := _run_smoke_pass()
	var status := "ok"
	if first.get("error", "") != "":
		status = str(first.get("error"))
	elif second.get("error", "") != "":
		status = str(second.get("error"))
	elif int(first.get("checksum", 0)) != int(second.get("checksum", 0)):
		status = "checksum_mismatch"
	elif int(first.get("instances", 0)) != int(second.get("instances", 0)):
		status = "instances_mismatch"
	elif int(first.get("visible", 0)) != int(second.get("visible", 0)):
		status = "visible_mismatch"
	elif int(first.get("updates", 0)) != SMOKE_UPDATES:
		status = "updates=%d" % int(first.get("updates", 0))
	elif int(second.get("updates", 0)) != SMOKE_UPDATES:
		status = "updates=%d" % int(second.get("updates", 0))
	print((
		"MOVEMENT_STRESS_SMOKE agents=%d updates=%d instances=%d visible=%d "
		+ "pass1_checksum=%s pass2_checksum=%s status=%s"
	) % [
			_agents,
			SMOKE_UPDATES,
			int(first.get("instances", 0)),
			int(first.get("visible", 0)),
			_checksum_hex(int(first.get("checksum", 0))),
			_checksum_hex(int(second.get("checksum", 0))),
			status,
		]
	)
	get_tree().quit(0 if status == "ok" else 1)


func _run_smoke_pass() -> Dictionary:
	_build_agents(_agents)
	if not _build_pool():
		return {"error": "pool_build_failed"}
	var result := _run_updates(SMOKE_UPDATES)
	return {
		"checksum": int(result.get("checksum", 0)),
		"instances": _multimesh.instance_count,
		"visible": _multimesh.visible_instance_count,
		"updates": int(result.get("updates", 0)),
	}


func _build_agents(count: int) -> void:
	_home_x.resize(count)
	_home_z.resize(count)
	_work_x.resize(count)
	_work_z.resize(count)
	_phase.resize(count)
	_inv_period.resize(count)
	for index in count:
		var home := index / RESIDENTS_PER_HOME
		var slot := index % RESIDENTS_PER_HOME
		var home_column := home % HOME_COLUMNS
		var home_row := home / HOME_COLUMNS
		var workplace := (home * 7 + slot * 3) % WORKPLACE_COUNT
		var work_column := workplace % WORK_COLUMNS
		var work_row := workplace / WORK_COLUMNS
		_home_x[index] = (float(home_column) - float(HOME_COLUMNS - 1) * 0.5) * GRID_SPACING_M
		_home_z[index] = (float(home_row) - float(HOME_ROWS - 1) * 0.5) * GRID_SPACING_M
		_work_x[index] = (float(work_column) - float(WORK_COLUMNS - 1) * 0.5) * GRID_SPACING_M
		_work_z[index] = (
			(float(work_row) - float(WORK_ROWS - 1) * 0.5) * GRID_SPACING_M
			+ WORK_OFFSET_Z_M
		)
		# Integer-derived phase and period: no RNG, no wall clock.
		_phase[index] = float((index * 613) % 1000) / 1000.0
		_inv_period[index] = 1.0 / (30.0 + float((index * 7) % 17))


func _build_pool() -> bool:
	var box := BoxMesh.new()
	box.size = MARKER_SIZE
	var material := StandardMaterial3D.new()
	material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	material.albedo_color = MARKER_COLOR
	_multimesh = MultiMesh.new()
	_multimesh.mesh = box
	# transform_format must be set before instance_count; Godot rejects it once
	# instances exist. The pool is fixed at MAX_AGENTS; --agents only changes the
	# visible count, mirroring the live near/far pool pattern.
	_multimesh.transform_format = MultiMesh.TRANSFORM_3D
	_multimesh.use_colors = false
	_multimesh.instance_count = MAX_AGENTS
	_multimesh.visible_instance_count = _agents
	# The path envelope is fixed, so the custom AABB is set once instead of
	# scanning every origin on each update. Per-update cost is the transform
	# fill, which is what this harness is measuring.
	_multimesh.custom_aabb = _path_aabb()
	_multimesh_instance = MultiMeshInstance3D.new()
	_multimesh_instance.name = "MovementStressPool"
	_multimesh_instance.multimesh = _multimesh
	_multimesh_instance.material_override = material
	_multimesh_instance.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	add_child(_multimesh_instance)
	return true


func _free_pool() -> void:
	if _multimesh_instance != null and is_instance_valid(_multimesh_instance):
		_multimesh_instance.free()
	_multimesh_instance = null
	_multimesh = null


func _run_updates(update_count: int) -> Dictionary:
	var samples: Array[float] = []
	var total_started := Time.get_ticks_usec()
	for step in update_count:
		# Absolute simulated time: no accumulated float drift across 1,200 steps.
		var sim_time := float(step + 1) * UPDATE_DT
		var started := Time.get_ticks_usec()
		_write_transforms(sim_time)
		samples.append(float(Time.get_ticks_usec() - started) / 1000.0)
	var total_ms := float(Time.get_ticks_usec() - total_started) / 1000.0
	return {
		"samples": samples,
		"total_ms": total_ms,
		"updates": update_count,
		"checksum": _checksum_transforms(float(update_count) * UPDATE_DT),
	}


func _write_transforms(sim_time: float) -> void:
	# Out-and-back triangle wave: 0 -> workplace -> 0 over `period` seconds,
	# offset per agent so the pool never moves in lockstep.
	var basis := Basis.IDENTITY
	for index in _agents:
		_multimesh.set_instance_transform(index, Transform3D(basis, _position_at(index, sim_time)))


func _position_at(index: int, sim_time: float) -> Vector3:
	var t := fmod(sim_time * _inv_period[index] + _phase[index], 1.0)
	var u := t + t
	if u > 1.0:
		u = 2.0 - u
	return Vector3(
		_home_x[index] + (_work_x[index] - _home_x[index]) * u,
		MARKER_SIZE.y * 0.5,
		_home_z[index] + (_work_z[index] - _home_z[index]) * u
	)


func _path_aabb() -> AABB:
	var half_x := GRID_SPACING_M * float(HOME_COLUMNS - 1) * 0.5
	var home_half_z := GRID_SPACING_M * float(HOME_ROWS - 1) * 0.5
	var work_half_z := GRID_SPACING_M * float(WORK_ROWS - 1) * 0.5
	var max_z := WORK_OFFSET_Z_M + work_half_z
	var marker_half := MARKER_SIZE * 0.5
	return AABB(
		Vector3(-half_x - marker_half.x, 0.0, -home_half_z - marker_half.z),
		Vector3(
			half_x * 2.0 + MARKER_SIZE.x,
			MARKER_SIZE.y,
			home_half_z + max_z + MARKER_SIZE.z
		)
	)


func _checksum_transforms(sim_time: float) -> int:
	# FNV-1a over the quantised final origins. The dummy headless renderer does
	# not preserve `MultiMesh` instance transforms for readback (Godot 4.7
	# returns identity), so the checksum hashes the same deterministic positions
	# the final update emitted, not the rendering-server buffer. Counts are mixed
	# in so a truncated pool cannot collide with a full one.
	var hash := FNV_OFFSET_BASIS
	hash = _fnv_mix(hash, _agents)
	hash = _fnv_mix(hash, int(round(sim_time * UPDATE_HZ)))
	for index in _agents:
		var origin := _position_at(index, sim_time)
		hash = _fnv_mix(hash, int(round(origin.x * CHECKSUM_SCALE)))
		hash = _fnv_mix(hash, int(round(origin.y * CHECKSUM_SCALE)))
		hash = _fnv_mix(hash, int(round(origin.z * CHECKSUM_SCALE)))
	return hash & 0xFFFFFFFF


func _fnv_mix(hash: int, value: int) -> int:
	return ((hash ^ (value & 0xFFFFFFFF)) * FNV_PRIME) & 0xFFFFFFFF


func _checksum_hex(value: int) -> String:
	return "%08x" % (value & 0xFFFFFFFF)


func _percentile(sorted_values: Array[float], fraction: float) -> float:
	if sorted_values.is_empty():
		return 0.0
	var index := clampi(
		int(round(float(sorted_values.size() - 1) * fraction)),
		0,
		sorted_values.size() - 1
	)
	return sorted_values[index]


func _mean(values: Array[float]) -> float:
	if values.is_empty():
		return 0.0
	var total := 0.0
	for value in values:
		total += value
	return total / float(values.size())


func _round_ms(value: float) -> float:
	return snappedf(value, 0.0001)
