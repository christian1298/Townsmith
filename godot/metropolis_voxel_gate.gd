extends Node3D

## Voxel-proxy setup and frame-budget harness for the metropolis gate (docs/68 §7.7).
##
## Builds three deterministic `MultiMesh` spatial tiers from the Phase 0 demo
## voxel hut: 1k / 5k / 20k buildings, 26k total. Every building is assigned a
## static LOD by distance band around its layout centre:
##   LOD 0 `near`  existing greedy-meshed demo hut (56 quads / 112 triangles)
##   LOD 1 `mid`   12-triangle box with the hut AABB
##   LOD 2 `far`   2-triangle quad impostor card
## The bands are 150 m (near) and 400 m (mid); farther instances are far. This
## is a deterministic setup-time stand-in for per-frame camera-distance LOD
## selection: instances never switch LOD in this harness.
##
## It reports per-spatial-tier and per-LOD instance counts, quads/triangles,
## setup time, instance-buffer bytes, static-memory delta, and a stable
## checksum. `--lod` builds a single 26k-building layout instead of the three
## spatial tiers, so the LOD mix can be validated on its own.
##
## With a display it also flies a fixed 60 s camera route after a warm-up and
## records real frame times. Headless (`--setup-only`) validates the layout and
## upload path only; it never claims FPS.
##
## This is **voxel-proxy setup/memory evidence**, not the RTX 3060 metropolis
## gate: one reused low-poly hut, static LOD bands with no switching, no
## streaming, shadows (unless `--shadows`), GI/SSAO, simulation, or 250k
## buildings, and the route is a single run on whatever GPU is attached.
##
## Usage:
##   godot --headless --path godot res://metropolis_voxel_gate.tscn -- --setup-only
##   godot --headless --path godot res://metropolis_voxel_gate.tscn -- --setup-only --lod
##   godot --path godot res://metropolis_voxel_gate.tscn -- --short
##
## Options after `--`:
##   --setup-only        Build, validate, print setup JSON, exit 0
##   --lod               Build the single 26k-building LOD-band layout
##   --short             Use a 10 s measured route with a 3 s warm-up
##   --duration S        Measured seconds (default route value, 60)
##   --warmup S          Discarded warm-up seconds (default route value, 8)
##   --route PATH        Route JSON (default benchmarks/routes/metropolis_voxel_gate_v1.json)
##   --output PATH       Result JSON
##   --shadows           Let the MultiMeshes cast directional shadows (off by default)
##   --git-commit HASH   Provenance, recorded verbatim
##   --git-dirty BOOL    Provenance, recorded verbatim

const TIER_COUNTS := [1_000, 5_000, 20_000]
const TIER_NAMES := ["small", "medium", "large"]
# Fixed tier centres along X, 10 m grid. The three footprints do not overlap.
const TIER_CENTER_X := [-1700.0, -700.0, 900.0]
# `--lod` mode: one 26k-building layout, still 10 m spacing.
const LOD_ONLY_COUNT := 26_000
const LOD_ONLY_CENTER_X := 0.0
const LOD_ONLY_NAME := "lod_single"
# Static distance bands from each layout centre. Not a runtime LOD selector.
const LOD_NEAR_RADIUS_M := 150.0
const LOD_MID_RADIUS_M := 400.0
const LOD_NAMES := ["near", "mid", "far"]
# LOD 0 reuses the greedy-meshed demo hut. LOD 1 is a 12-triangle box with the
# same AABB. LOD 2 is a 2-triangle vertical quad impostor card.
const LOD_MESH_IDS := ["phase0_demo_hut", "phase0_lod1_box", "phase0_lod2_quad"]
const MESH_ID := "phase0_demo_hut"
const SEED := 2026
const GRID_SPACING_M := 10.0
const FLOATS_PER_INSTANCE := 12  # 3x4 transform, no color
const FLOAT_BYTES := 4
const DEFAULT_ROUTE := "benchmarks/routes/metropolis_voxel_gate_v1.json"
const DEFAULT_OUTPUT := "benchmarks/results/metropolis_voxel_gate.json"
const GROUND_MARGIN_M := 1_000.0
const FNV_OFFSET_BASIS := 2166136261
const FNV_PRIME := 16777619

var duration_s := 60.0
var warmup_s := 8.0
var route: Dictionary = {}
var waypoints: Array = []
var route_id := "unknown"
var route_path := ""
var output_path := ""
var git_commit := ""
var git_dirty := ""
var setup_only := false
var short_route := false
var shadows_enabled := false
var lod_mode := false

var camera: Camera3D
var light: DirectionalLight3D
var _container: Node3D
var _scene_stats: Dictionary = {}
var _settings: Dictionary = {}
var _hardware: Dictionary = {}
var _route_hash := 0

var elapsed := 0.0
var measured_s := 0.0
var measuring := false
var finished := false
var samples: Array[float] = []
var last_usec := 0

func _ready() -> void:
	_configure_target_window()
	var args := OS.get_cmdline_user_args()
	setup_only = args.has("--setup-only")
	short_route = args.has("--short")
	shadows_enabled = args.has("--shadows")
	lod_mode = args.has("--lod")
	git_commit = _arg(args, "--git-commit", "")
	git_dirty = _arg(args, "--git-dirty", "")
	route_path = _arg(args, "--route", _project_root().path_join(DEFAULT_ROUTE))
	output_path = _arg(args, "--output", _project_root().path_join(DEFAULT_OUTPUT))
	if not _load_route(route_path):
		_fail("could not load route: " + route_path)
		return
	var fallback_duration := 10.0 if short_route else float(route.get("duration_seconds", 60.0))
	var fallback_warmup := 3.0 if short_route else float(route.get("warmup_seconds", 8.0))
	var duration_override := _arg(args, "--duration", "")
	var warmup_override := _arg(args, "--warmup", "")
	duration_s = float(duration_override) if duration_override != "" else fallback_duration
	warmup_s = float(warmup_override) if warmup_override != "" else fallback_warmup
	if duration_s <= 0.0 or warmup_s < 0.0:
		_fail("--duration must be > 0 and --warmup must be >= 0")
		return

	_build_scene()
	if _scene_stats.has("error"):
		_fail(str(_scene_stats["error"]))
		return
	_add_environment()
	_apply_route(0.0)
	_hardware = _hardware_info()
	_settings = _settings_info()
	_route_hash = _hash_route()
	last_usec = Time.get_ticks_usec()

	if setup_only:
		_write_setup()
		return

func _process(delta: float) -> void:
	if finished or camera == null:
		return
	elapsed += delta
	var progress := clampf((elapsed - warmup_s) / maxf(duration_s, 0.001), 0.0, 1.0)
	_apply_route(progress)

	var now := Time.get_ticks_usec()
	if not measuring and elapsed >= warmup_s:
		# Discard everything up to and including the warm-up boundary frame.
		measuring = true
		samples.clear()
		measured_s = 0.0
		last_usec = now
		return
	if measuring:
		var frame_ms := float(now - last_usec) / 1000.0
		last_usec = now
		if frame_ms > 0.0 and frame_ms < 10_000.0:
			samples.append(frame_ms)
			measured_s += frame_ms / 1000.0
	if elapsed >= warmup_s + duration_s:
		_finish()

func _build_scene() -> void:
	var scene_started_usec := Time.get_ticks_usec()
	var core := TownsmithCore.new()
	var mesh_started_usec := Time.get_ticks_usec()
	var near_mesh: ArrayMesh = core.build_demo_mesh()
	var mesh_usec := Time.get_ticks_usec() - mesh_started_usec
	if near_mesh == null or near_mesh.get_surface_count() < 1:
		_scene_stats = {"error": "demo_mesh_empty"}
		return
	var near_stats := _mesh_stats(near_mesh)
	var hut_aabb := near_mesh.get_aabb()
	var material := _hut_material()
	var meshes: Array[Mesh] = [
		near_mesh,
		_mid_lod_mesh(hut_aabb),
		_far_lod_mesh(hut_aabb),
	]
	var lod_meshes: Array = []
	var half_span := 0.0
	var max_height := 0.0
	for lod in meshes.size():
		var current_mesh: Mesh = meshes[lod]
		var stats := _mesh_stats_any(current_mesh)
		stats["lod"] = lod
		stats["name"] = LOD_NAMES[lod]
		stats["mesh"] = LOD_MESH_IDS[lod]
		lod_meshes.append(stats)
		var aabb: Array = stats["aabb"]
		var base_y := -float(aabb[1])
		max_height = maxf(max_height, float(aabb[1]) + float(aabb[4]) + base_y)
		half_span = maxf(half_span, maxf(float(aabb[3]), float(aabb[5])) * 0.5)

	_container = Node3D.new()
	_container.name = "MetropolisVoxelGateLods"
	add_child(_container)

	var layout_id := "lod_single_v1" if lod_mode else "tiers_v1"
	var layouts: Array = []
	if lod_mode:
		layouts.append({
			"tier": 0,
			"name": LOD_ONLY_NAME,
			"count": LOD_ONLY_COUNT,
			"center_x": LOD_ONLY_CENTER_X,
		})
	else:
		for tier in TIER_COUNTS.size():
			layouts.append({
				"tier": tier,
				"name": TIER_NAMES[tier],
				"count": int(TIER_COUNTS[tier]),
				"center_x": float(TIER_CENTER_X[tier]),
			})

	var tier_stats: Array = []
	var total_instances := 0
	var total_quads := 0
	var total_triangles := 0
	var total_buffer_bytes := 0
	var total_setup_usec := 0
	var total_mem_kb := 0
	var combined_checksum := FNV_OFFSET_BASIS
	combined_checksum = _mix_string(combined_checksum, layout_id)
	for mesh_id in LOD_MESH_IDS:
		combined_checksum = _mix_string(combined_checksum, str(mesh_id))
	combined_checksum = _mix(combined_checksum, SEED)
	combined_checksum = _mix(combined_checksum, int(round(GRID_SPACING_M * 1000.0)))
	combined_checksum = _mix(combined_checksum, int(round(LOD_NEAR_RADIUS_M * 1000.0)))
	combined_checksum = _mix(combined_checksum, int(round(LOD_MID_RADIUS_M * 1000.0)))

	for layout in layouts:
		var tier := int(layout["tier"])
		var count := int(layout["count"])
		var tier_started_usec := Time.get_ticks_usec()
		var tier_result := _build_layout(
			tier, str(layout["name"]), count, float(layout["center_x"]),
			layout_id, meshes, lod_meshes, half_span, max_height, material)
		if tier_result.has("error"):
			_scene_stats = {"error": str(tier_result["error"])}
			return
		var setup_usec := Time.get_ticks_usec() - tier_started_usec
		var stats: Dictionary = tier_result["stats"]
		stats["setup_ms"] = _ms(setup_usec)
		var tier_hash := _tier_checksum(tier, count, stats, layout_id)
		stats["checksum"] = _checksum_hex(tier_hash)
		combined_checksum = _mix(combined_checksum, tier_hash)
		tier_stats.append(stats)

		total_instances += count
		total_quads += int(stats["quads"])
		total_triangles += int(stats["triangles"])
		total_buffer_bytes += int(stats["buffer_bytes"])
		total_setup_usec += setup_usec
		total_mem_kb += int(stats["cpu_mem_kb_delta"])

	var lod_totals: Array = []
	var lod_instances := {}
	for lod in LOD_NAMES.size():
		var instances := 0
		var quads := 0
		var triangles := 0
		var buffer_bytes := 0
		var lod_setup_ms := 0.0
		for tier_entry in tier_stats:
			for lod_entry in (tier_entry["lod"] as Array):
				if int(lod_entry["lod"]) != lod:
					continue
				instances += int(lod_entry["instances"])
				quads += int(lod_entry["quads"])
				triangles += int(lod_entry["triangles"])
				buffer_bytes += int(lod_entry["buffer_bytes"])
				lod_setup_ms += float(lod_entry["setup_ms"])
		var mesh_entry: Dictionary = lod_meshes[lod]
		lod_totals.append({
			"name": LOD_NAMES[lod],
			"lod": lod,
			"mesh": LOD_MESH_IDS[lod],
			"mesh_quads": int(mesh_entry["quads"]),
			"mesh_triangles": int(mesh_entry["triangles"]),
			"instances": instances,
			"quads": quads,
			"triangles": triangles,
			"buffer_bytes": buffer_bytes,
			"buffer_mb": _round2(float(buffer_bytes) / (1024.0 * 1024.0)),
			"setup_ms": _round2(lod_setup_ms),
		})
		lod_instances[LOD_NAMES[lod]] = instances

	_scene_stats = {
		"mesh": MESH_ID,
		"seed": SEED,
		"spacing_m": GRID_SPACING_M,
		"layout": layout_id,
		"lod_model": {
			"mode": "static_distance_bands",
			"near_radius_m": LOD_NEAR_RADIUS_M,
			"mid_radius_m": LOD_MID_RADIUS_M,
			"levels": lod_meshes,
		},
		"instances": total_instances,
		"quads": total_quads,
		"triangles": total_triangles,
		"tiers": tier_stats,
		"lod_tiers": lod_totals,
		"lod_instances": lod_instances,
		"mesh_stats": near_stats,
		"mesh_build_ms": _ms(mesh_usec),
		"tier_setup_ms": _ms(total_setup_usec),
		"setup_ms": _ms(mesh_usec + total_setup_usec),
		"scene_build_ms": _ms(Time.get_ticks_usec() - scene_started_usec),
		"buffer_bytes": total_buffer_bytes,
		"buffer_mb": _round2(float(total_buffer_bytes) / (1024.0 * 1024.0)),
		"cpu_mem_kb_delta": total_mem_kb,
		"checksum": _checksum_hex(combined_checksum),
	}

func _build_layout(
	tier: int,
	tier_name: String,
	count: int,
	center_x: float,
	layout_id: String,
	meshes: Array,
	lod_meshes: Array,
	half_span: float,
	max_height: float,
	material: Material) -> Dictionary:
	var columns := maxi(1, int(ceil(sqrt(float(count)))))
	var rows := maxi(1, int(ceil(float(count) / float(columns))))
	var extent_x := float(columns - 1) * GRID_SPACING_M
	var extent_z := float(rows - 1) * GRID_SPACING_M
	var x0 := center_x - extent_x * 0.5
	var z0 := -extent_z * 0.5
	var min_x := x0 - half_span
	var max_x := x0 + extent_x + half_span
	var min_z := z0 - half_span
	var max_z := z0 + extent_z + half_span
	var city_aabb := AABB(
		Vector3(min_x, 0.0, min_z),
		Vector3(max_x - min_x, maxf(max_height, 1.0), max_z - min_z))
	var mem_before_kb := OS.get_static_memory_usage() / 1024

	var indices_by_lod: Array = [[], [], []]
	for index in count:
		var column := index % columns
		var row := index / columns
		var x := x0 + float(column) * GRID_SPACING_M
		var z := z0 + float(row) * GRID_SPACING_M
		var lod := _lod_for_position(x, z, center_x)
		(indices_by_lod[lod] as Array).append(index)

	var lod_stats: Array = []
	var tier_quads := 0
	var tier_triangles := 0
	var tier_buffer_bytes := 0
	for lod in meshes.size():
		var indices: Array = indices_by_lod[lod]
		var current_mesh: Mesh = meshes[lod]
		var mesh_entry: Dictionary = lod_meshes[lod]
		var group_started_usec := Time.get_ticks_usec()
		var lod_result := _build_lod_group(
			tier, tier_name, lod, indices, columns, x0, z0,
			current_mesh, mesh_entry, city_aabb, material)
		if lod_result.has("error"):
			return {"error": str(lod_result["error"])}
		var lod_setup_usec := Time.get_ticks_usec() - group_started_usec
		var entry: Dictionary = lod_result
		entry["setup_ms"] = _ms(lod_setup_usec)
		lod_stats.append(entry)
		tier_quads += int(entry["quads"])
		tier_triangles += int(entry["triangles"])
		tier_buffer_bytes += int(entry["buffer_bytes"])

	var mem_after_kb := OS.get_static_memory_usage() / 1024
	var stats := {
		"name": tier_name,
		"tier": tier,
		"instances": count,
		"grid": [columns, rows],
		"center_x": center_x,
		"bounds": [min_x, min_z, max_x, max_z],
		"lod": lod_stats,
		"quads": tier_quads,
		"triangles": tier_triangles,
		"buffer_bytes": tier_buffer_bytes,
		"buffer_mb": _round2(float(tier_buffer_bytes) / (1024.0 * 1024.0)),
		"cpu_mem_kb_delta": mem_after_kb - mem_before_kb,
	}
	return {"stats": stats}

func _build_lod_group(
	tier: int,
	tier_name: String,
	lod: int,
	indices: Array,
	columns: int,
	x0: float,
	z0: float,
	mesh: Mesh,
	mesh_stats: Dictionary,
	city_aabb: AABB,
	material: Material) -> Dictionary:
	var count := indices.size()
	var mesh_quads := int(mesh_stats["quads"])
	var mesh_triangles := int(mesh_stats["triangles"])
	var stats := {
		"name": LOD_NAMES[lod],
		"lod": lod,
		"mesh": LOD_MESH_IDS[lod],
		"instances": count,
		"mesh_quads": mesh_quads,
		"mesh_triangles": mesh_triangles,
		"quads": mesh_quads * count,
		"triangles": mesh_triangles * count,
		"buffer_ms": 0.0,
		"buffer_bytes": 0,
		"buffer_mb": 0.0,
	}
	if count == 0:
		return stats

	var local_aabb: Array = mesh_stats["aabb"]
	var mesh_center := Vector3(
		float(local_aabb[0]) + float(local_aabb[3]) * 0.5,
		float(local_aabb[1]) + float(local_aabb[4]) * 0.5,
		float(local_aabb[2]) + float(local_aabb[5]) * 0.5)
	# BoxMesh and QuadMesh are centred on the origin; lift them so their base
	# sits on the ground like the hut mesh.
	var base_y := -float(local_aabb[1])

	var data := PackedFloat32Array()
	data.resize(count * FLOATS_PER_INSTANCE)
	for slot in count:
		var index := int(indices[slot])
		var column := index % columns
		var row := index / columns
		var x := x0 + float(column) * GRID_SPACING_M
		var z := z0 + float(row) * GRID_SPACING_M
		# Quarter-turn yaw from a pure integer expression, stable across Godot
		# versions and independent of the global RNG state.
		var yaw := int(posmod(SEED + index * 7 + tier * 13, 4))
		var angle := float(yaw) * PI * 0.5
		var c := cos(angle)
		var s := sin(angle)
		var rotated_center := _rotated_center(angle, mesh_center)
		var offset := slot * FLOATS_PER_INSTANCE
		data[offset + 0] = c
		data[offset + 1] = 0.0
		data[offset + 2] = -s
		data[offset + 3] = x - rotated_center.x
		data[offset + 4] = 0.0
		data[offset + 5] = 1.0
		data[offset + 6] = 0.0
		data[offset + 7] = base_y
		data[offset + 8] = s
		data[offset + 9] = 0.0
		data[offset + 10] = c
		data[offset + 11] = z - rotated_center.y

	var multimesh := MultiMesh.new()
	multimesh.mesh = mesh
	# transform_format and use_colors must be set before instance_count.
	multimesh.transform_format = MultiMesh.TRANSFORM_3D
	multimesh.use_colors = false
	multimesh.instance_count = count
	# Set the AABB before the bulk upload so Godot skips its own instance scan.
	multimesh.custom_aabb = city_aabb
	var buffer_started_usec := Time.get_ticks_usec()
	multimesh.buffer = data
	var buffer_usec := Time.get_ticks_usec() - buffer_started_usec

	var node := MultiMeshInstance3D.new()
	node.name = "VoxelLod%dTier%s" % [lod, tier_name]
	node.multimesh = multimesh
	node.material_override = material
	node.cast_shadow = (
		GeometryInstance3D.SHADOW_CASTING_SETTING_ON if shadows_enabled
		else GeometryInstance3D.SHADOW_CASTING_SETTING_OFF)
	_container.add_child(node)

	stats["buffer_ms"] = _ms(buffer_usec)
	stats["buffer_bytes"] = data.size() * FLOAT_BYTES
	stats["buffer_mb"] = _round2(float(data.size() * FLOAT_BYTES) / (1024.0 * 1024.0))
	return stats

func _tier_checksum(tier: int, count: int, stats: Dictionary, layout_id: String) -> int:
	var hash := FNV_OFFSET_BASIS
	hash = _mix_string(hash, layout_id)
	for mesh_id in LOD_MESH_IDS:
		hash = _mix_string(hash, str(mesh_id))
	hash = _mix(hash, tier)
	hash = _mix(hash, count)
	hash = _mix(hash, SEED)
	hash = _mix(hash, int(round(GRID_SPACING_M * 1000.0)))
	hash = _mix(hash, int(round(LOD_NEAR_RADIUS_M * 1000.0)))
	hash = _mix(hash, int(round(LOD_MID_RADIUS_M * 1000.0)))
	var center_x := float(stats.get("center_x", 0.0))
	hash = _mix(hash, int(round(center_x * 1000.0)))
	var columns := int((stats.get("grid", [1, 1]) as Array)[0])
	var rows := int((stats.get("grid", [1, 1]) as Array)[1])
	hash = _mix(hash, columns)
	hash = _mix(hash, rows)
	for lod_entry in (stats.get("lod", []) as Array):
		hash = _mix(hash, int(lod_entry.get("lod", 0)))
		hash = _mix(hash, int(lod_entry.get("mesh_quads", 0)))
		hash = _mix(hash, int(lod_entry.get("mesh_triangles", 0)))
	var x0 := center_x - float(columns - 1) * GRID_SPACING_M * 0.5
	var z0 := -float(rows - 1) * GRID_SPACING_M * 0.5
	for index in count:
		var column := index % columns
		var row := index / columns
		var yaw := int(posmod(SEED + index * 7 + tier * 13, 4))
		hash = _mix(hash, column)
		hash = _mix(hash, row)
		hash = _mix(hash, yaw)
		hash = _mix(hash, _lod_for_position(
			x0 + float(column) * GRID_SPACING_M,
			z0 + float(row) * GRID_SPACING_M,
			center_x))
	return hash

func _lod_for_position(x: float, z: float, center_x: float) -> int:
	var dx := x - center_x
	var distance_squared := dx * dx + z * z
	if distance_squared <= LOD_NEAR_RADIUS_M * LOD_NEAR_RADIUS_M:
		return 0
	if distance_squared <= LOD_MID_RADIUS_M * LOD_MID_RADIUS_M:
		return 1
	return 2

func _mid_lod_mesh(hut_aabb: AABB) -> BoxMesh:
	var mesh := BoxMesh.new()
	mesh.size = Vector3(hut_aabb.size.x, hut_aabb.size.y, hut_aabb.size.z)
	return mesh

func _far_lod_mesh(hut_aabb: AABB) -> QuadMesh:
	var mesh := QuadMesh.new()
	mesh.size = Vector2(hut_aabb.size.x, hut_aabb.size.y)
	return mesh

func _mesh_stats_any(mesh: Mesh) -> Dictionary:
	if mesh is ArrayMesh:
		return _mesh_stats(mesh as ArrayMesh)
	var faces := mesh.get_faces()
	var triangles := int(faces.size() / 3)
	var aabb := mesh.get_aabb()
	return {
		"surfaces": 1,
		"vertices": faces.size(),
		"triangles": triangles,
		"quads": int(triangles / 2),
		"aabb": [
			aabb.position.x, aabb.position.y, aabb.position.z,
			aabb.size.x, aabb.size.y, aabb.size.z,
		],
	}

func _mesh_stats(mesh: ArrayMesh) -> Dictionary:
	var triangles := _mesh_triangle_count(mesh)
	var vertices := 0
	for surface in mesh.get_surface_count():
		vertices += mesh.surface_get_array_len(surface)
	var aabb := mesh.get_aabb()
	return {
		"surfaces": mesh.get_surface_count(),
		"vertices": vertices,
		"triangles": triangles,
		"quads": int(triangles / 2),
		"aabb": [
			aabb.position.x, aabb.position.y, aabb.position.z,
			aabb.size.x, aabb.size.y, aabb.size.z,
		],
	}

func _mesh_triangle_count(mesh: ArrayMesh) -> int:
	var triangles := 0
	for surface in mesh.get_surface_count():
		var index_count := mesh.surface_get_array_index_len(surface)
		if index_count > 0:
			triangles += int(index_count / 3)
		else:
			triangles += int(mesh.surface_get_array_len(surface) / 3)
	return triangles

func _hut_material() -> StandardMaterial3D:
	var material := StandardMaterial3D.new()
	material.albedo_color = Color(0.72, 0.62, 0.48)
	material.roughness = 1.0
	material.metallic = 0.0
	material.transparency = BaseMaterial3D.TRANSPARENCY_DISABLED
	return material

func _add_environment() -> void:
	light = DirectionalLight3D.new()
	light.rotation_degrees = Vector3(-50.0, -30.0, 0.0)
	light.light_energy = 1.2
	light.shadow_enabled = shadows_enabled
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

	var min_x := INF
	var min_z := INF
	var max_x := -INF
	var max_z := -INF
	for tier in _scene_stats.get("tiers", []):
		var bounds: Array = tier["bounds"]
		min_x = minf(min_x, float(bounds[0]))
		min_z = minf(min_z, float(bounds[1]))
		max_x = maxf(max_x, float(bounds[2]))
		max_z = maxf(max_z, float(bounds[3]))
	var span_x := max_x - min_x
	var span_z := max_z - min_z
	var ground_size := maxf(span_x, span_z) + GROUND_MARGIN_M * 2.0
	var ground := PlaneMesh.new()
	ground.size = Vector2(ground_size, ground_size)
	var material := StandardMaterial3D.new()
	material.albedo_color = Color(0.26, 0.30, 0.26)
	material.roughness = 1.0
	ground.material = material
	var ground_node := MeshInstance3D.new()
	ground_node.name = "VoxelGateGround"
	ground_node.mesh = ground
	ground_node.position = Vector3((min_x + max_x) * 0.5, -0.05, (min_z + max_z) * 0.5)
	ground_node.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	add_child(ground_node)

	camera = Camera3D.new()
	camera.current = true
	camera.near = 1.0
	camera.far = maxf(12_000.0, maxf(span_x, span_z) * 2.5)
	add_child(camera)

func _load_route(path: String) -> bool:
	var text := FileAccess.get_file_as_string(path)
	if text.is_empty():
		push_error("Could not read route: " + path)
		return false
	var parsed: Variant = JSON.parse_string(text)
	if typeof(parsed) != TYPE_DICTIONARY:
		push_error("Route is not a JSON object: " + path)
		return false
	route = parsed
	waypoints = route.get("waypoints", [])
	if waypoints.size() < 2:
		push_error("Route needs at least two waypoints: " + path)
		return false
	route_id = str(route.get("id", "unknown"))
	return true

func _apply_route(t: float) -> void:
	if camera == null or waypoints.size() < 2:
		return
	var left: Dictionary = waypoints[0]
	var right: Dictionary = waypoints[waypoints.size() - 1]
	for index in range(waypoints.size() - 1):
		var a: Dictionary = waypoints[index]
		var b: Dictionary = waypoints[index + 1]
		if t <= float(b.get("t", 1.0)):
			left = a
			right = b
			break
	var span := maxf(float(right.get("t", 1.0)) - float(left.get("t", 0.0)), 0.0001)
	var f := clampf((t - float(left.get("t", 0.0))) / span, 0.0, 1.0)
	var a_pos: Array = left.get("position", [0, 50, 0])
	var b_pos: Array = right.get("position", [0, 50, 0])
	var a_look: Array = left.get("look_at", [0, 0, 0])
	var b_look: Array = right.get("look_at", [0, 0, 0])
	var pos := Vector3(float(a_pos[0]), float(a_pos[1]), float(a_pos[2])).lerp(
		Vector3(float(b_pos[0]), float(b_pos[1]), float(b_pos[2])), f)
	var look := Vector3(float(a_look[0]), float(a_look[1]), float(a_look[2])).lerp(
		Vector3(float(b_look[0]), float(b_look[1]), float(b_look[2])), f)
	camera.position = pos
	camera.look_at(look)
	camera.fov = lerpf(float(left.get("fov", 70.0)), float(right.get("fov", 70.0)), f)

func _finish() -> void:
	if finished:
		return
	finished = true
	var sorted := samples.duplicate()
	sorted.sort()
	var count := sorted.size()
	var frame_total := 0.0
	for value in sorted:
		frame_total += value
	var frame_avg := frame_total / maxf(float(count), 1.0)
	var p95 := _percentile(sorted, 0.95)
	var p99 := _percentile(sorted, 0.99)
	var p999 := _percentile(sorted, 0.999)
	var hitches_16 := 0
	var hitches_33 := 0
	var hitches_50 := 0
	var stalls_250 := 0
	for value in sorted:
		if value > 250.0:
			stalls_250 += 1
		if value > 50.0:
			hitches_50 += 1
		if value > 33.0:
			hitches_33 += 1
		if value > 16.0:
			hitches_16 += 1
	var result := _base_result("fps")
	result["measured_s"] = _round2(measured_s)
	result["frames"] = count
	result["results"] = {
		"fps_avg": _round2(1000.0 / maxf(frame_avg, 0.001)),
		"fps_low_1pct": _round2(1000.0 / maxf(p99, 0.001)),
		"fps_low_0_1pct": _round2(1000.0 / maxf(p999, 0.001)),
		"frametime_ms_avg": _round2(frame_avg),
		"frametime_ms_median": _round2(_percentile(sorted, 0.50)),
		"frametime_ms_p95": _round2(p95),
		"frametime_ms_p99": _round2(p99),
		"frametime_ms_p99_9": _round2(p999),
		"frametime_ms_min": _round2(sorted[0]) if count > 0 else 0.0,
		"frametime_ms_max": _round2(sorted[count - 1]) if count > 0 else 0.0,
		"hitches_16ms": hitches_16,
		"hitches_33ms": hitches_33,
		"hitches_50ms": hitches_50,
		"stalls_250ms": stalls_250,
	}
	result["gate_result"] = "not_measured"
	result["gate_scope"] = "voxel_proxy_frame_budget"
	result["gate_reason"] = (
		"voxel-proxy frame budget observed with static LOD bands "
		+ "(hut/box/quad impostor): no runtime LOD selection or switching, "
		+ "streaming, GI/SSAO, simulation, or reference 3060 rig; not gate M5")
	_write_result(result, "METROPOLIS_VOXEL_GATE_RESULT")

func _write_setup() -> void:
	var result := _base_result("setup_lod" if lod_mode else "setup")
	result["setup_status"] = "ok"
	result["gate_result"] = "not_measured"
	result["gate_scope"] = "voxel_proxy_setup"
	result["gate_reason"] = (
		"voxel-proxy setup-only run with static LOD bands (hut/box/quad "
		+ "impostor): instance and per-LOD layout, quads/triangles, setup time, "
		+ "and instance-buffer memory only; no runtime LOD switching, no frames, "
		+ "no FPS claim, not gate M5")
	_write_result(result, "METROPOLIS_VOXEL_GATE_SETUP")

func _base_result(run_kind: String) -> Dictionary:
	return {
		"format_version": 2,
		"suite": "metropolis_voxel_gate",
		"gate": "G5",
		"scene": _scene_name(),
		"scene_kind": "voxel_multimesh_proxy_lod",
		"run_kind": run_kind,
		"layout": str(_scene_stats.get("layout", "")),
		"lod_mode": lod_mode,
		"lod_model": _scene_stats.get("lod_model", {}),
		"lod_instances": _scene_stats.get("lod_instances", {}),
		"mesh": MESH_ID,
		"seed": SEED,
		"spacing_m": GRID_SPACING_M,
		"instances": int(_scene_stats.get("instances", 0)),
		"quads": int(_scene_stats.get("quads", 0)),
		"triangles": int(_scene_stats.get("triangles", 0)),
		"setup_ms": float(_scene_stats.get("setup_ms", 0.0)),
		"mesh_build_ms": float(_scene_stats.get("mesh_build_ms", 0.0)),
		"tier_setup_ms": float(_scene_stats.get("tier_setup_ms", 0.0)),
		"buffer_bytes": int(_scene_stats.get("buffer_bytes", 0)),
		"buffer_mb": float(_scene_stats.get("buffer_mb", 0.0)),
		"cpu_mem_kb_delta": int(_scene_stats.get("cpu_mem_kb_delta", 0)),
		"checksum": str(_scene_stats.get("checksum", "")),
		"tiers": _scene_stats.get("tiers", []),
		"lod_tiers": _scene_stats.get("lod_tiers", []),
		"mesh_stats": _scene_stats.get("mesh_stats", {}),
		"route": route_id,
		"route_path": route_path,
		"route_hash": _checksum_hex(_route_hash),
		"duration_s": duration_s,
		"warmup_s": warmup_s,
		"headless": DisplayServer.get_name() == "headless",
		"engine": Engine.get_version_info(),
		"hardware": _hardware,
		"settings": _settings,
		"settings_hash": _checksum_hex(_hash_settings()),
		"git_commit": git_commit,
		"git_dirty": git_dirty,
		"evidence": "voxel_proxy_setup_and_frame_budget",
		"gate_note": "not the 3060 metropolis gate; see docs/68 §7.7",
	}

func _scene_name() -> String:
	var layout_tag := "lod_single" if lod_mode else "lod"
	return "metropolis_voxel_gate_%s_%dk" % [
		layout_tag, int(int(_scene_stats.get("instances", 0)) / 1000)]

func _write_result(result: Dictionary, marker: String) -> void:
	DirAccess.make_dir_recursive_absolute(output_path.get_base_dir())
	var file := FileAccess.open(output_path, FileAccess.WRITE)
	if file == null:
		push_error("Could not write result: " + output_path)
		get_tree().quit(1)
		return
	file.store_string(JSON.stringify(result, "  "))
	file.close()
	print(marker + " " + JSON.stringify(result))
	get_tree().quit(0)

func _settings_info() -> Dictionary:
	var viewport := get_viewport()
	var window_size := DisplayServer.window_get_size()
	return {
		"resolution": [window_size.x, window_size.y],
		"viewport_size": [
			int(viewport.get_visible_rect().size.x), int(viewport.get_visible_rect().size.y)],
		"window_mode": DisplayServer.window_get_mode(),
		"vsync_mode": DisplayServer.window_get_vsync_mode(),
		"max_fps": Engine.max_fps,
		"rendering_method": str(ProjectSettings.get_setting(
			"rendering/renderer/rendering_method", "unknown")),
		"msaa_3d": viewport.msaa_3d,
		"screen_space_aa": viewport.screen_space_aa,
		"use_taa": viewport.use_taa,
		"use_debanding": viewport.use_debanding,
		"scaling_3d_mode": viewport.scaling_3d_mode,
		"scaling_3d_scale": viewport.scaling_3d_scale,
		"shadows": "on" if shadows_enabled else "off",
		"shadow_enabled": light.shadow_enabled if light != null else false,
		"shadow_blur": light.shadow_blur if light != null else 0.0,
	}

func _hardware_info() -> Dictionary:
	var window_size := DisplayServer.window_get_size()
	var info := {
		"os": OS.get_name(),
		"os_version": OS.get_version(),
		"processor": OS.get_processor_name(),
		"logical_processor_count": OS.get_processor_count(),
		"display_server": DisplayServer.get_name(),
		"window_size": [window_size.x, window_size.y],
		"vsync_mode": DisplayServer.window_get_vsync_mode(),
		"max_fps": Engine.max_fps,
		"refresh_rate_hz": DisplayServer.screen_get_refresh_rate(),
		"rendering_method": str(ProjectSettings.get_setting(
			"rendering/renderer/rendering_method", "unknown")),
	}
	var adapter := RenderingServer.get_video_adapter_name()
	if adapter != "":
		info["video_adapter"] = adapter
	var vendor := RenderingServer.get_video_adapter_vendor()
	if vendor != "":
		info["video_vendor"] = vendor
	var api := RenderingServer.get_video_adapter_api_version()
	if api != "":
		info["video_api"] = api
	return info

func _hash_route() -> int:
	var hash := FNV_OFFSET_BASIS
	hash = _mix_string(hash, route_id)
	hash = _mix(hash, int(duration_s * 1000.0))
	hash = _mix(hash, int(warmup_s * 1000.0))
	for waypoint in waypoints:
		var wp: Dictionary = waypoint
		hash = _mix(hash, int(float(wp.get("t", 0.0)) * 10_000.0))
		for key in ["position", "look_at"]:
			var values: Array = wp.get(key, [0, 0, 0])
			for value in values:
				hash = _mix(hash, int(round(float(value) * 1000.0)))
		hash = _mix(hash, int(round(float(wp.get("fov", 70.0)) * 100.0)))
	return hash

func _hash_settings() -> int:
	var hash := FNV_OFFSET_BASIS
	hash = _mix_string(hash, str(_settings.get("resolution", [])))
	hash = _mix(hash, int(_settings.get("vsync_mode", 0)))
	hash = _mix(hash, int(_settings.get("max_fps", 0)))
	hash = _mix_string(hash, str(_settings.get("rendering_method", "")))
	hash = _mix(hash, int(_settings.get("msaa_3d", 0)))
	hash = _mix(hash, int(_settings.get("screen_space_aa", 0)))
	hash = _mix(hash, 1 if bool(_settings.get("use_taa", false)) else 0)
	hash = _mix(hash, 1 if shadows_enabled else 0)
	return hash

func _configure_target_window() -> void:
	if DisplayServer.get_name() == "headless":
		return
	DisplayServer.window_set_size(Vector2i(1920, 1080))
	DisplayServer.window_set_vsync_mode(DisplayServer.VSYNC_DISABLED)
	Engine.max_fps = 0

func _rotated_center(angle: float, center: Vector3) -> Vector2:
	var c := cos(angle)
	var s := sin(angle)
	return Vector2(c * center.x - s * center.z, s * center.x + c * center.z)

func _percentile(sorted_values: Array[float], fraction: float) -> float:
	if sorted_values.is_empty():
		return 0.0
	var index := clampi(
		int(round(float(sorted_values.size() - 1) * fraction)),
		0, sorted_values.size() - 1)
	return sorted_values[index]

func _arg(args: PackedStringArray, name: String, fallback: String) -> String:
	var index := args.find(name)
	if index >= 0 and index + 1 < args.size():
		return args[index + 1]
	return fallback

func _project_root() -> String:
	return ProjectSettings.globalize_path("res://").path_join("..").simplify_path()

func _fail(message: String) -> void:
	push_error(message)
	print("METROPOLIS_VOXEL_GATE status=error message=%s" % message)
	get_tree().quit(1)

func _mix(hash: int, value: int) -> int:
	hash = (hash ^ (value & 0xFFFFFFFF)) & 0xFFFFFFFF
	return (hash * FNV_PRIME) & 0xFFFFFFFF

func _mix_string(hash: int, value: String) -> int:
	for index in value.length():
		hash = _mix(hash, value.unicode_at(index))
	return hash

func _checksum_hex(value: int) -> String:
	return "%08x" % (value & 0xFFFFFFFF)

func _ms(usec: int) -> float:
	return _round2(float(usec) / 1000.0)

func _round2(value: float) -> float:
	return round(value * 100.0) / 100.0
