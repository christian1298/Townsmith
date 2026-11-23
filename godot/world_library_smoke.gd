extends SceneTree

## Headless bridge smoke for a portable world save with placement references.
##
## Builds a uniquely named package, saves it through world_save_with_library,
## deletes the source, then checks that world_load_with_library resolves the
## content-addressed reference and that plain load/failure/clear keep the
## expected state.

const SEED := 2026
const SITE_X := 169.0
const SITE_Z := 125.0

func _fail(code: String) -> void:
	print("WORLD_LIBRARY status=%s" % code)
	quit(1)

func _remove_tree(path: String) -> void:
	var dir := DirAccess.open(path)
	if dir == null:
		return
	dir.list_dir_begin()
	var name := dir.get_next()
	while name != "":
		if name != "." and name != "..":
			var child := path.path_join(name)
			if dir.current_is_dir():
				_remove_tree(child)
			else:
				DirAccess.remove_absolute(child)
		name = dir.get_next()
	dir.list_dir_end()
	DirAccess.remove_absolute(path)

func _plain_load_has_no_resolved_field(core, save_path: String) -> bool:
	var result: Dictionary = core.world_load(save_path, SEED)
	if bool(result.get("ok", false)):
		return false
	if str(result.get("error_code", "")) != "blueprint_reference_requires_library":
		return false
	var state: Dictionary = core.world_placements()
	if int(state.get("count", -1)) != 0:
		return false
	for placement in state.get("placements", []):
		if placement.has("resolved_blueprint_path"):
			return false
	return true

func _init() -> void:
	var base := OS.get_user_data_dir().path_join("world_library_smoke")
	_remove_tree(base)
	DirAccess.make_dir_recursive_absolute(base)
	var library := base.path_join("blueprints")
	DirAccess.make_dir_recursive_absolute(library)
	var package := base.path_join("world_library_unique_house.tsbp")
	var save_path := base.path_join("world.json")
	var plain_save_path := base.path_join("world_plain.json")

	var core = TownsmithCore.new()
	if not core.create_editor_volume(0, 0, 0) or not core.editor_build_acceptance_house():
		_fail("editor_failed")
		return
	var saved_blueprint: Dictionary = core.editor_save_blueprint(
		package, "local:world_library_house", "World Library House"
	)
	if not bool(saved_blueprint.get("ok", false)):
		_fail("blueprint_failed")
		return
	var placed: Dictionary = core.world_place_blueprint(SEED, package, SITE_X, SITE_Z, 0)
	if not bool(placed.get("ok", false)):
		_fail("place_failed:" + str(placed.get("error_code", "")))
		return
	var before: Dictionary = core.world_placements()
	var before_checksum := str(before.get("checksum", ""))
	var recorded := str(before.get("placements", [])[0].get("blueprint_path", ""))
	var saved: Dictionary = core.world_save_with_library(save_path, library)
	if not bool(saved.get("ok", false)):
		_fail("save_failed:" + str(saved.get("error_code", "")))
		return
	if int(saved.get("blueprint_references", 0)) != 1:
		_fail("save_reference_count")
		return
	var saved_text := FileAccess.get_file_as_string(save_path)
	if not saved_text.contains("\"blueprint_ref\""):
		_fail("save_missing_reference")
		return
	if saved_text.contains(library):
		_fail("save_contains_library_root")
		return
	var stamped: Dictionary = core.world_placements()
	var stamped_checksum := str(stamped.get("checksum", ""))
	if stamped_checksum == before_checksum:
		_fail("save_did_not_stamp_store")
		return
	var stamped_entry: Dictionary = stamped.get("placements", [])[0]
	var reference := str(stamped_entry.get("blueprint_ref", ""))
	if reference == "" or reference == recorded:
		_fail("live_reference_missing")
		return
	if str(stamped_entry.get("blueprint_path", "")) != recorded:
		_fail("recorded_path_changed")
		return
	if stamped_entry.has("resolved_blueprint_path"):
		_fail("save_invented_resolved_path")
		return

	# A later plain save keeps the references and writes no library root.
	var plain: Dictionary = core.world_save(plain_save_path)
	if not bool(plain.get("ok", false)):
		_fail("plain_save_failed:" + str(plain.get("error_code", "")))
		return
	if FileAccess.get_file_as_string(plain_save_path) != saved_text:
		_fail("plain_save_changed_json")
		return

	# While the legacy path still exists, a plain load ignores the reference.
	var path_core = TownsmithCore.new()
	var path_load: Dictionary = path_core.world_load(save_path, SEED)
	if not bool(path_load.get("ok", false)):
		_fail("path_load_failed:" + str(path_load.get("error_code", "")))
		return
	var path_entry: Dictionary = path_core.world_placements().get("placements", [])[0]
	if path_entry.has("resolved_blueprint_path"):
		_fail("path_load_invented_resolved_path")
		return

	if DirAccess.remove_absolute(package) != OK:
		_fail("source_remove_failed")
		return

	# A fresh plain load now reports the reference-specific error and never
	# invents a resolved field.
	var plain_core = TownsmithCore.new()
	if not _plain_load_has_no_resolved_field(plain_core, save_path):
		_fail("plain_load_changed")
		return

	var loaded: Dictionary = core.world_load_with_library(save_path, SEED, library)
	if not bool(loaded.get("ok", false)):
		_fail("portable_load_failed:" + str(loaded.get("error_code", "")))
		return
	if int(loaded.get("resolved_references", 0)) != 1:
		_fail("resolved_reference_count")
		return
	if int(loaded.get("legacy_path_fallbacks", -1)) != 0:
		_fail("legacy_fallback_count")
		return
	var state: Dictionary = core.world_placements()
	var entry: Dictionary = state.get("placements", [])[0]
	var resolved := str(entry.get("resolved_blueprint_path", ""))
	if resolved == "" or resolved == recorded:
		_fail("resolved_missing")
		return
	if str(entry.get("blueprint_ref", "")) != reference:
		_fail("loaded_reference_changed")
		return
	if str(entry.get("blueprint_path", "")) != recorded:
		_fail("loaded_path_changed")
		return
	if str(state.get("checksum", "")) != stamped_checksum:
		_fail("checksum_changed")
		return
	var info: Dictionary = core.world_blueprint_info(resolved)
	var mesh: ArrayMesh = core.build_blueprint_mesh(resolved)
	if not bool(info.get("ok", false)) or mesh.get_surface_count() == 0:
		_fail("resolved_render_failed")
		return

	# A plain load after the portable load still fails and leaves the previous
	# placement store and resolution map untouched.
	var plain_after: Dictionary = core.world_load(save_path, SEED)
	if bool(plain_after.get("ok", false)) or str(plain_after.get("error_code", "")) != "blueprint_reference_requires_library":
		_fail("plain_load_unexpected")
		return
	var after_plain: Dictionary = core.world_placements()
	if str(after_plain.get("checksum", "")) != stamped_checksum:
		_fail("plain_load_changed_state")
		return
	if str(after_plain.get("placements", [])[0].get("resolved_blueprint_path", "")) != resolved:
		_fail("plain_load_changed_map")
		return

	# Deleting the library package leaves the stale path visible; a failed
	# reload leaves the previous map and store untouched.
	if DirAccess.remove_absolute(resolved) != OK:
		_fail("library_remove_failed")
		return
	var stale: Dictionary = core.world_placements()
	if str(stale.get("placements", [])[0].get("resolved_blueprint_path", "")) != resolved:
		_fail("stale_not_reported")
		return
	var failed: Dictionary = core.world_load_with_library(save_path, SEED, library)
	if bool(failed.get("ok", false)) or str(failed.get("error_code", "")) != "blueprint_library_missing_file":
		_fail("failed_load_unexpected")
		return
	var after_failed: Dictionary = core.world_placements()
	if str(after_failed.get("checksum", "")) != stamped_checksum:
		_fail("failed_load_changed_state")
		return
	if str(after_failed.get("placements", [])[0].get("resolved_blueprint_path", "")) != resolved:
		_fail("failed_load_changed_map")
		return

	if not core.world_clear_placements():
		_fail("clear_failed")
		return
	if int(core.world_placements().get("count", -1)) != 0:
		_fail("clear_count")
		return

	print("WORLD_LIBRARY resolved=ok refs=ok plain=ok guard=ok stale=ok clear=ok checksum=%s status=ok" % stamped_checksum)
	_remove_tree(base)
	quit(0)
