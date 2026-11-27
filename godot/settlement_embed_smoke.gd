extends Node

## Shared-core smoke for the embedded settlement screen. The screen must read
## and mutate the host's TownsmithCore, never create a second store, and never
## free the injected core when it leaves the tree.

const ScreenScene = preload("res://settlement_screen.tscn")
const SEED := 2026

func _ready() -> void:
	var core = TownsmithCore.new()
	var found: Dictionary = core.settlement_found(SEED, "Embed Harbor", "player", 0, 0, 64)
	if not bool(found.get("ok", false)):
		_fail("found", str(found.get("error_code", "unknown")))
		return
	var before := str(core.settlement_checksum())

	var screen = ScreenScene.instantiate()
	screen.configure_shared(core, SEED)
	add_child(screen)
	var snapshot: Dictionary = screen.embed_snapshot()
	if not bool(snapshot.get("shared", false)) or not bool(snapshot.get("ok", false)):
		_fail("shared", str(snapshot))
		return
	if int(snapshot.get("rows", 0)) != 1:
		_fail("rows", str(snapshot))
		return
	var id := int((core.settlements_page(0, 8).get("settlements", [{}])[0]).get("id", -1))
	if not screen.focus_settlement(id):
		_fail("focus", str(id))
		return
	if int(screen.embed_snapshot().get("selected_id", -1)) != id:
		_fail("selection", str(screen.embed_snapshot()))
		return

	screen.queue_free()
	await get_tree().process_frame
	var after := str(core.settlement_checksum())
	if before != after:
		_fail("checksum", "%s != %s" % [before, after])
		return
	var page: Dictionary = core.settlements_page(0, 8)
	if not bool(page.get("ok", false)) or int(page.get("count", 0)) != 1:
		_fail("core_after_free", str(page))
		return
	print("SETTLEMENT_EMBED_SMOKE status=ok settlements=1 selected=%d checksum=%s" % [id, after])
	get_tree().quit(0)


func _fail(step: String, detail: String) -> void:
	print("SETTLEMENT_EMBED_SMOKE status=fail step=%s detail=%s" % [step, detail])
	get_tree().quit(1)
