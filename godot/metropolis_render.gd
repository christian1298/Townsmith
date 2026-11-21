extends SceneTree

## Headless MultiMesh setup harness for the metropolis render scale.
##
## Builds deterministic box-grid cities as one `MultiMeshInstance3D` per tier.
## This is a **CPU setup and instance-memory** harness, not FPS evidence: it
## runs with `--headless`, so it never touches the GPU, the frame loop, LOD,
## impostors, streaming, shadows, or the real voxel building pipeline.
##
## What it measures:
## - wall time to fill the instance buffer, create the `MultiMesh`, and attach
##   the node (one bulk `MultiMesh.buffer` write, no per-instance property calls);
## - `MultiMesh` buffer bytes per tier and the process static-memory delta;
## - a deterministic checksum over the integer grid layout.
##
## Full mode (default): 10k / 50k / 250k boxes (310k total), 10 m grid, 4 m box.
## Smoke mode (`-- --metropolis-smoke`): 1k / 5k / 10k, asserts counts and the
## checked-in checksums, exits 0 or 1.
##
## Usage:
##   godot --headless --path godot --script res://metropolis_render.gd
##   godot --headless --path godot --script res://metropolis_render.gd -- --metropolis-smoke

const FULL_TIER_COUNTS := [10_000, 50_000, 250_000]
const SMOKE_TIER_COUNTS := [1_000, 5_000, 10_000]
const GRID_SPACING_M := 10.0
const BOX_EDGE_M := 4.0
const FLOATS_PER_INSTANCE := 12  # 3x4 transform, no color
const BYTES_PER_INSTANCE := FLOATS_PER_INSTANCE * 4
const FNV_OFFSET_BASIS := 2166136261
const FNV_PRIME := 16777619
# Derived from the deterministic integer layout. Values are 32-bit FNV-1a
# hashes of the tier count, grid shape, and every (row, column) grid cell.
const SMOKE_CHECKSUMS := {
	1_000: "ea15c7a1",
	5_000: "6fc08f05",
	10_000: "27593891",
}

var _smoke_mode := false
var _container: Node3D
var _tier_nodes: Array[MultiMeshInstance3D] = []
var _failed := false

func _initialize() -> void:
	var args := OS.get_cmdline_user_args()
	_smoke_mode = args.has("--metropolis-smoke") or OS.get_cmdline_args().has("--metropolis-smoke")
	var counts: Array = SMOKE_TIER_COUNTS if _smoke_mode else FULL_TIER_COUNTS

	var run_started_usec := Time.get_ticks_usec()
	_container = Node3D.new()
	_container.name = "MetropolisRenderTiers"
	root.add_child(_container)

	var total_instances := 0
	var total_setup_usec := 0
	var total_hash_usec := 0
	var total_buffer_bytes := 0
	var total_mem_kb := 0
	var combined_checksum := FNV_OFFSET_BASIS

	for tier in counts.size():
		var expected_count := int(counts[tier])
		var result := _build_tier(tier, expected_count)
		if result.has("error"):
			push_error("metropolis tier %d failed: %s" % [tier, result["error"]])
			_print_error(str(result["error"]))
			_cleanup()
			quit(1)
			return

		var count := int(result["instances"])
		total_instances += count
		total_setup_usec += int(result["setup_usec"])
		total_hash_usec += int(result["hash_usec"])
		total_buffer_bytes += int(result["buffer_bytes"])
		total_mem_kb += int(result["cpu_mem_kb_delta"])
		var tier_checksum := int(result["checksum"])
		combined_checksum = _mix(combined_checksum, tier_checksum)

		print(JSON.stringify({
			"mode": "smoke" if _smoke_mode else "full",
			"tier": tier,
			"tier_name": _tier_name(tier),
			"instances": count,
			"grid": [int(result["columns"]), int(result["rows"])],
			"spacing_m": GRID_SPACING_M,
			"box_edge_m": BOX_EDGE_M,
			"setup_ms": _ms(result["setup_usec"]),
			"hash_ms": _ms(result["hash_usec"]),
			"buffer_bytes": int(result["buffer_bytes"]),
			"buffer_mb": _round2(float(result["buffer_bytes"]) / (1024.0 * 1024.0)),
			"cpu_mem_kb_delta": int(result["cpu_mem_kb_delta"]),
			"checksum": _checksum_hex(tier_checksum),
		}))

		if _smoke_mode and count != expected_count:
			_print_error("tier %d count=%d expected=%d" % [tier, count, expected_count])
			_cleanup()
			quit(1)
			return

		if _smoke_mode:
			var expected_checksum := str(SMOKE_CHECKSUMS.get(expected_count, ""))
			if expected_checksum == "" or _checksum_hex(tier_checksum) != expected_checksum:
				_print_error("tier %d checksum=%s expected=%s" % [
					tier, _checksum_hex(tier_checksum), expected_checksum])
				_cleanup()
				quit(1)
				return

	var expected_total := _total_for(counts)
	var checksum_text := _checksum_hex(combined_checksum)
	if _smoke_mode and total_instances != expected_total:
		_print_error("total instances=%d expected=%d" % [total_instances, expected_total])
		_cleanup()
		quit(1)
		return

	var wall_ms := _ms(Time.get_ticks_usec() - run_started_usec)
	print("METROPOLIS_RENDER tiers=%d instances=%d checksum=%s setup_ms=%.2f buffer_mb=%.2f wall_ms=%.2f status=ok" % [
		counts.size(), total_instances, checksum_text,
		float(total_setup_usec) / 1000.0,
		float(total_buffer_bytes) / (1024.0 * 1024.0),
		wall_ms])
	print(JSON.stringify({
		"summary": true,
		"mode": "smoke" if _smoke_mode else "full",
		"tiers": counts.size(),
		"instances": total_instances,
		"setup_ms": _ms(total_setup_usec),
		"hash_ms": _ms(total_hash_usec),
		"wall_ms": wall_ms,
		"buffer_bytes": total_buffer_bytes,
		"buffer_mb": _round2(float(total_buffer_bytes) / (1024.0 * 1024.0)),
		"cpu_mem_kb_delta": total_mem_kb,
		"checksum": checksum_text,
		"status": "ok",
	}))

	_cleanup()
	quit(0)

func _build_tier(tier: int, count: int) -> Dictionary:
	var columns := int(ceil(sqrt(float(count))))
	if columns < 1:
		columns = 1
	var rows := int(ceil(float(count) / float(columns)))
	var extent_x := float(columns - 1) * GRID_SPACING_M
	var extent_z := float(rows - 1) * GRID_SPACING_M
	var origin_x := -extent_x * 0.5
	var origin_z := -extent_z * 0.5
	var half_box := BOX_EDGE_M * 0.5

	var mem_before_kb := OS.get_static_memory_usage() / 1024
	var setup_started_usec := Time.get_ticks_usec()

	var data := PackedFloat32Array()
	data.resize(count * FLOATS_PER_INSTANCE)
	for index in count:
		var column := index % columns
		var row := index / columns
		var offset := index * FLOATS_PER_INSTANCE
		# Row-major 3x4 transform: unit axes scaled by the 4 m box edge, then
		# the box center. The BoxMesh is centered on its origin.
		data[offset + 0] = BOX_EDGE_M
		data[offset + 1] = 0.0
		data[offset + 2] = 0.0
		data[offset + 3] = origin_x + float(column) * GRID_SPACING_M
		data[offset + 4] = 0.0
		data[offset + 5] = BOX_EDGE_M
		data[offset + 6] = 0.0
		data[offset + 7] = half_box
		data[offset + 8] = 0.0
		data[offset + 9] = 0.0
		data[offset + 10] = BOX_EDGE_M
		data[offset + 11] = origin_z + float(row) * GRID_SPACING_M

	var box := BoxMesh.new()
	box.size = Vector3(BOX_EDGE_M, BOX_EDGE_M, BOX_EDGE_M)

	var multimesh := MultiMesh.new()
	multimesh.mesh = box
	# transform_format must be set before instance_count; use_colors stays off
	# so the buffer is 12 floats per instance instead of 16.
	multimesh.transform_format = MultiMesh.TRANSFORM_3D
	multimesh.instance_count = count
	# Set the AABB before the bulk upload so Godot skips its own instance scan.
	# Pad by half the box so edge instances cannot be culled.
	multimesh.custom_aabb = AABB(
		Vector3(origin_x - half_box, 0.0, origin_z - half_box),
		Vector3(extent_x + BOX_EDGE_M, BOX_EDGE_M, extent_z + BOX_EDGE_M))
	multimesh.buffer = data

	var node := MultiMeshInstance3D.new()
	node.name = "MetropolisTier%d" % tier
	node.multimesh = multimesh
	node.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	_container.add_child(node)
	_tier_nodes.append(node)

	var setup_usec := Time.get_ticks_usec() - setup_started_usec
	var mem_after_kb := OS.get_static_memory_usage() / 1024

	var hash_started_usec := Time.get_ticks_usec()
	var checksum := FNV_OFFSET_BASIS
	checksum = _mix(checksum, count)
	checksum = _mix(checksum, int(GRID_SPACING_M))
	checksum = _mix(checksum, int(BOX_EDGE_M))
	checksum = _mix(checksum, columns)
	checksum = _mix(checksum, rows)
	for index in count:
		checksum = _mix(checksum, index % columns)
		checksum = _mix(checksum, index / columns)
	var hash_usec := Time.get_ticks_usec() - hash_started_usec

	return {
		"instances": count,
		"columns": columns,
		"rows": rows,
		"setup_usec": setup_usec,
		"hash_usec": hash_usec,
		"buffer_bytes": data.size() * 4,
		"cpu_mem_kb_delta": mem_after_kb - mem_before_kb,
		"checksum": checksum,
	}

func _cleanup() -> void:
	for node in _tier_nodes:
		if is_instance_valid(node):
			if node.get_parent() != null:
				node.get_parent().remove_child(node)
			node.free()
	_tier_nodes.clear()
	if _container != null:
		if is_instance_valid(_container):
			if _container.get_parent() != null:
				_container.get_parent().remove_child(_container)
			_container.free()
		_container = null

func _print_error(message: String) -> void:
	push_error(message)
	print("METROPOLIS_RENDER status=error message=%s" % message)

func _tier_name(tier: int) -> String:
	return ["small", "medium", "large"][tier]

func _total_for(counts: Array) -> int:
	var total := 0
	for count in counts:
		total += int(count)
	return total

## 32-bit FNV-1a over integer values only, so the checksum is platform-stable.
func _mix(hash: int, value: int) -> int:
	hash = (hash ^ (value & 0xFFFFFFFF)) & 0xFFFFFFFF
	return (hash * FNV_PRIME) & 0xFFFFFFFF

func _checksum_hex(value: int) -> String:
	return "%08x" % (value & 0xFFFFFFFF)

func _ms(usec: int) -> float:
	return _round2(float(usec) / 1000.0)

func _round2(value: float) -> float:
	return round(value * 100.0) / 100.0
