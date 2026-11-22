extends RefCounted

## Slice-local editor string table and runtime lookup.
##
## Every user-facing editor string lives here under the `base.editor.` key
## namespace. The plan in `docs/16 - Localization.md` compiles JSON/CSV tables
## into a runtime format; this file is the single-table slice stand-in so the
## editor never hard-codes visible text. Node names, smoke markers, file paths,
## and internal ids stay English and are never passed through here.
##
## Lookup rules:
## - `t(key)` resolves against the active locale only.
## - A missing key returns `!key!`, a visible marker the locale smoke asserts
##   never reaches a rendered control.
## - Placeholders are `{0}`, `{1}`, ... for array parameters, or `{name}` for
##   a dictionary. Parameters are substituted verbatim; leftover placeholders
##   stay visible instead of failing silently.
##
## See `docs/38 - Editor Localization.md`.

const DEFAULT_LOCALE := "en"
const SUPPORTED_LOCALES := ["en", "de"]

const TABLES := {
	"en": {
		# Window and HUD.
		"base.editor.window.title": "Townsmith — Voxel Editor",
		"base.editor.hud.language": "Language: {0}  (L switches)",
		"base.editor.language.en": "English",
		"base.editor.language.de": "German",

		# Status line.
		"base.editor.status.capture_mouse": "Esc releases the mouse",
		"base.editor.status.click_to_capture": "Click to capture the mouse",
		"base.editor.status.language": "Language: {0}",
		"base.editor.status.no_voxel_in_reach": "No voxel in reach",
		"base.editor.status.pad_not_editable": "The pad is not editable; place a voxel first",
		"base.editor.status.remove_rejected": "Remove rejected at {0}",
		"base.editor.status.removed": "Removed {0} at {1}",
		"base.editor.status.no_surface_in_reach": "No surface in reach",
		"base.editor.status.no_empty_cell": "No empty cell against the hit face",
		"base.editor.status.outside_volume": "Placement outside the volume",
		"base.editor.status.cell_occupied": "Placement cell is occupied",
		"base.editor.status.place_rejected": "Place rejected at {0}",
		"base.editor.status.placed": "Placed {0} at {1}",
		"base.editor.status.no_prop_cell": "No empty cell for a prop",
		"base.editor.status.prop_place_rejected": "Prop placement rejected at {0}",
		"base.editor.status.prop_placed": "Placed {0} at {1}",
		"base.editor.status.no_prop_cell_in_reach": "No prop cell in reach",
		"base.editor.status.no_prop_at": "No prop at {0}",
		"base.editor.status.prop_removed": "Removed prop at {0}",
		"base.editor.status.nothing_to_undo": "Nothing to undo",
		"base.editor.status.undo_failed": "Undo failed at {0}",
		"base.editor.status.undo": "Undo",
		"base.editor.status.nothing_to_redo": "Nothing to redo",
		"base.editor.status.redo_failed": "Redo failed at {0}",
		"base.editor.status.redo": "Redo",
		"base.editor.status.structure_failed": "Structure change failed: {0}",
		"base.editor.status.room_view_off": "Room view off",
		"base.editor.status.room_overlay_failed": "Room overlay failed: {0}",
		"base.editor.status.room_view_summary": "Room view: {0} room(s), capacity {1}, jobs {2}  ([ ] select, R hide)",
		"base.editor.status.room_view_off_hint": "Room view is off (R)",
		"base.editor.status.no_rooms": "No rooms to select",
		"base.editor.status.room_selected": "Selected {0} · {1}  ([ ] cycle, R hide)",
		"base.editor.status.material_selected": "Material {0}: {1}",
		"base.editor.status.prop_selected": "Prop: {0}  (right click place, left click remove)",
		"base.editor.status.blueprint_name_empty": "Blueprint name is empty",
		"base.editor.status.save_failed": "Save failed: {0}",
		"base.editor.status.saved": "Saved {0}  hash={1}",
		"base.editor.status.load_failed": "Load failed: {0}",
		"base.editor.status.loaded": "Loaded {0}  hash={1}",

		# Palette labels and content names.
		"base.editor.palette.materials": "Materials  (keys 1-5)",
		"base.editor.palette.material_row": "{0} {1}  {2}",
		"base.editor.palette.structure": "H  structure: {0} ({1}/{2} stories)",
		"base.editor.palette.structure_invalid": " — {0}",
		"base.editor.palette.props": "Props  (keys 6-9)",
		"base.editor.palette.prop_row": "{0} {1}  {2}",
		"base.editor.material.timber": "timber",
		"base.editor.material.stone": "stone",
		"base.editor.material.roof": "roof",
		"base.editor.material.glass": "glass",
		"base.editor.material.door": "door",
		"base.editor.prop.bed": "bed",
		"base.editor.prop.stove": "stove",
		"base.editor.prop.toilet": "toilet",
		"base.editor.prop.desk": "desk",

		# Stats overlay.
		"base.editor.stats.solid_quads": "solid={0}  quads={1}  mesh={2} ms",
		"base.editor.stats.chunks_volume": "chunks={0}  volume={1} x {2} x {3} m",
		"base.editor.stats.cost": "cost={0}",
		"base.editor.stats.build": "build={0} ms  builds={1}",
		"base.editor.stats.remesh_pending": "remesh pending...",

		# Help / controls.
		"base.editor.help.move": "WASD move   Space/Ctrl up/down   Shift boost   Esc release mouse",
		"base.editor.help.edit": "Left click remove   Right click place   Keys 1-5 choose material",
		"base.editor.help.tools": "R room view   H structure   [ ] select room   L language   Ctrl+Z undo   Ctrl+Y redo   F5 save   F9 load",

		# Blueprint bar and dialogs.
		"base.editor.blueprint.name_placeholder": "Blueprint name",
		"base.editor.blueprint.name_tooltip": "Name used for the saved .tsbp package",
		"base.editor.blueprint.default_name": "River House",
		"base.editor.blueprint.save": "Save",
		"base.editor.blueprint.save_tooltip": "Save the current volume (F5)",
		"base.editor.blueprint.load": "Load",
		"base.editor.blueprint.load_tooltip": "Load a .tsbp package (F9)",
		"base.editor.blueprint.save_title": "Save Blueprint",
		"base.editor.blueprint.load_title": "Load Blueprint",
		"base.editor.blueprint.filter": "*.{0} ; Townsmith Blueprint",

		# Room panel and overlay.
		"base.editor.rooms.header": "ROOM STATS   [ ] select room   R hide",
		"base.editor.rooms.summary": "rooms {0} · capacity {1} · jobs {2} · class {3}",
		"base.editor.rooms.solid": "solid {0} · material kinds {1}",
		"base.editor.rooms.cost": "material cost {0}",
		"base.editor.rooms.none": "no enclosed rooms",
		"base.editor.rooms.unavailable": "stats unavailable: {0}",
		"base.editor.rooms.line": "{0} {1} · {2} · floor {3} · {4} m² · beds {5} · desks {6} · doors {7} · windows {8}",
		"base.editor.rooms.detail_selected": "selected {0} · {1} · floor {2}",
		"base.editor.rooms.detail_metrics": "area {0} m² · volume {1} m³ · cap +{2} · jobs +{3}",
		"base.editor.rooms.detail_bounds": "bounds x {0}–{1} m · z {2}–{3} m · y {4}–{5} m",
		"base.editor.rooms.label": "{0}{1}\n{2}\n{3} m²",

		# Structure names resolved from internal ids.
		"base.editor.structure.timber_frame": "Timber-frame",
		"base.editor.structure.timber_shed": "Timber shed",
		"base.editor.structure.tent": "Tent",
		"base.editor.structure.load_bearing_stone": "Load-bearing stone",
		"base.editor.structure.half_timber": "Half-timber",
		"base.editor.structure.load_bearing_brick": "Load-bearing brick",
		"base.editor.structure.reinforced_concrete": "Reinforced concrete",
		"base.editor.structure.steel_frame": "Steel frame",
		"base.editor.structure.composite_core": "Composite core",

		# Content names resolved from internal ids.
		"base.editor.room_type.bedroom": "bedroom",
		"base.editor.room_type.kitchen": "kitchen",
		"base.editor.room_type.bathroom": "bathroom",
		"base.editor.room_type.office": "office",
		"base.editor.room_type.living_dining": "living/dining",
		"base.editor.room_type.empty": "empty",
		"base.editor.room_type.room": "room",
		"base.editor.room_class.lower_middle": "lower middle",
		"base.editor.room_class.middle": "middle",
		"base.editor.room_class.upper": "upper",

		# Fallbacks.
		"base.editor.generic.unknown": "unknown",
		"base.editor.generic.unknown_error": "unknown error",
		"base.editor.generic.blueprint": "blueprint",
		"base.editor.generic.prop": "prop",
		"base.editor.generic.room": "room",
	},
	"de": {
		# Fenster und HUD.
		"base.editor.window.title": "Townsmith — Voxel-Editor",
		"base.editor.hud.language": "Sprache: {0}  (L: wechseln)",
		"base.editor.language.en": "Englisch",
		"base.editor.language.de": "Deutsch",

		# Statuszeile.
		"base.editor.status.capture_mouse": "Esc gibt die Maus frei",
		"base.editor.status.click_to_capture": "Klicken, um die Maus wieder zu erfassen",
		"base.editor.status.language": "Sprache: {0}",
		"base.editor.status.no_voxel_in_reach": "Kein Voxel in Reichweite",
		"base.editor.status.pad_not_editable": "Die Baufläche ist nicht editierbar – zuerst ein Voxel platzieren",
		"base.editor.status.remove_rejected": "Entfernen bei {0} abgelehnt",
		"base.editor.status.removed": "{0} bei {1} entfernt",
		"base.editor.status.no_surface_in_reach": "Keine Oberfläche in Reichweite",
		"base.editor.status.no_empty_cell": "Keine freie Zelle an der getroffenen Fläche",
		"base.editor.status.outside_volume": "Platzierung außerhalb des Volumens",
		"base.editor.status.cell_occupied": "Die Platzierungszelle ist belegt",
		"base.editor.status.place_rejected": "Platzieren bei {0} abgelehnt",
		"base.editor.status.placed": "{0} bei {1} platziert",
		"base.editor.status.no_prop_cell": "Keine freie Zelle für ein Objekt",
		"base.editor.status.prop_place_rejected": "Objektplatzierung bei {0} abgelehnt",
		"base.editor.status.prop_placed": "{0} bei {1} platziert",
		"base.editor.status.no_prop_cell_in_reach": "Keine freie Zelle für ein Objekt in Reichweite",
		"base.editor.status.no_prop_at": "Kein Objekt bei {0}",
		"base.editor.status.prop_removed": "Objekt bei {0} entfernt",
		"base.editor.status.nothing_to_undo": "Keine Änderung zum Rückgängigmachen",
		"base.editor.status.undo_failed": "Rückgängig bei {0} fehlgeschlagen",
		"base.editor.status.undo": "Rückgängig",
		"base.editor.status.nothing_to_redo": "Keine Änderung zum Wiederholen",
		"base.editor.status.redo_failed": "Wiederholen bei {0} fehlgeschlagen",
		"base.editor.status.redo": "Wiederholen",
		"base.editor.status.structure_failed": "Tragwerk konnte nicht geändert werden: {0}",
		"base.editor.status.room_view_off": "Raumansicht aus",
		"base.editor.status.room_overlay_failed": "Raumansicht fehlgeschlagen: {0}",
		"base.editor.status.room_view_summary": "Raumansicht: {0} Raum/Räume erkannt · Kapazität {1} · Arbeitsplätze {2}  ([ ] wechseln, R ausblenden)",
		"base.editor.status.room_view_off_hint": "Die Raumansicht ist aus (R)",
		"base.editor.status.no_rooms": "Keine Räume zur Auswahl",
		"base.editor.status.room_selected": "Ausgewählt: {0} · {1}  ([ ] wechseln, R ausblenden)",
		"base.editor.status.material_selected": "Material {0}: {1}",
		"base.editor.status.prop_selected": "Objekt: {0}  (Rechtsklick platzieren, Linksklick entfernen)",
		"base.editor.status.blueprint_name_empty": "Der Blaupausenname ist leer",
		"base.editor.status.save_failed": "Speichern fehlgeschlagen: {0}",
		"base.editor.status.saved": "Gespeichert: {0}  Hash={1}",
		"base.editor.status.load_failed": "Laden fehlgeschlagen: {0}",
		"base.editor.status.loaded": "Geladen: {0}  Hash={1}",

		# Palette und Inhaltsnamen.
		"base.editor.palette.materials": "Materialien  (Tasten 1–5)",
		"base.editor.palette.material_row": "{0} {1}  {2}",
		"base.editor.palette.structure": "H  Tragwerk: {0} ({1}/{2} Geschosse)",
		"base.editor.palette.structure_invalid": " — {0}",
		"base.editor.palette.props": "Objekte  (Tasten 6–9)",
		"base.editor.palette.prop_row": "{0} {1}  {2}",
		"base.editor.material.timber": "Holz",
		"base.editor.material.stone": "Stein",
		"base.editor.material.roof": "Dach",
		"base.editor.material.glass": "Glas",
		"base.editor.material.door": "Tür",
		"base.editor.prop.bed": "Bett",
		"base.editor.prop.stove": "Herd",
		"base.editor.prop.toilet": "Toilette",
		"base.editor.prop.desk": "Schreibtisch",

		# Statistik-Overlay.
		"base.editor.stats.solid_quads": "Voxel={0}  Quads={1}  Mesh={2} ms",
		"base.editor.stats.chunks_volume": "Chunks={0}  Volumen={1} x {2} x {3} m",
		"base.editor.stats.cost": "Kosten={0}",
		"base.editor.stats.build": "Berechnung {0} ms · Durchläufe {1}",
		"base.editor.stats.remesh_pending": "Neuberechnung ausstehend …",

		# Hilfe und Steuerung.
		"base.editor.help.move": "WASD: bewegen   Leertaste/Strg: hoch/runter   Umschalt: schneller   Esc: Maus freigeben",
		"base.editor.help.edit": "Linksklick entfernen   Rechtsklick platzieren   Tasten 1–5: Material wählen",
		"base.editor.help.tools": "R Raumansicht   H Tragwerk   [ ] Raum wechseln   L Sprache   Strg+Z rückgängig   Strg+Y wiederholen   F5 speichern   F9 laden",

		# Blaupausenleiste und Dialoge.
		"base.editor.blueprint.name_placeholder": "Blaupausenname",
		"base.editor.blueprint.name_tooltip": "Name der gespeicherten .tsbp-Datei",
		"base.editor.blueprint.default_name": "Flusshaus",
		"base.editor.blueprint.save": "Speichern",
		"base.editor.blueprint.save_tooltip": "Aktuelles Volumen speichern (F5)",
		"base.editor.blueprint.load": "Laden",
		"base.editor.blueprint.load_tooltip": "Eine .tsbp-Datei laden (F9)",
		"base.editor.blueprint.save_title": "Blaupause speichern",
		"base.editor.blueprint.load_title": "Blaupause laden",
		"base.editor.blueprint.filter": "*.{0} ; Townsmith-Blaupause",

		# Raumstatistik und Overlay.
		"base.editor.rooms.header": "RAUMSTATISTIK   [ ] Raum wechseln   R ausblenden",
		"base.editor.rooms.summary": "Räume: {0} · Kapazität {1} · Arbeitsplätze {2} · Klasse {3}",
		"base.editor.rooms.solid": "Voxel {0} · Materialarten {1}",
		"base.editor.rooms.cost": "Materialkosten {0}",
		"base.editor.rooms.none": "keine geschlossenen Räume",
		"base.editor.rooms.unavailable": "Statistik nicht verfügbar: {0}",
		"base.editor.rooms.line": "{0} {1} · {2} · Ebene {3} · {4} m² · Betten {5} · Schreibtische {6} · Türen {7} · Fenster {8}",
		"base.editor.rooms.detail_selected": "ausgewählt: {0} · {1} · Ebene {2}",
		"base.editor.rooms.detail_metrics": "Fläche {0} m² · Volumen {1} m³ · Kapazität +{2} · Arbeitsplätze +{3}",
		"base.editor.rooms.detail_bounds": "Ausdehnung x {0}–{1} m · z {2}–{3} m · y {4}–{5} m",
		"base.editor.rooms.label": "{0}{1}\n{2}\n{3} m²",

		# Tragwerksnamen aus internen IDs.
		"base.editor.structure.timber_frame": "Holzrahmen",
		"base.editor.structure.timber_shed": "Holzschuppen",
		"base.editor.structure.tent": "Zelt",
		"base.editor.structure.load_bearing_stone": "Tragendes Steinmauerwerk",
		"base.editor.structure.half_timber": "Fachwerk",
		"base.editor.structure.load_bearing_brick": "Tragendes Ziegelmauerwerk",
		"base.editor.structure.reinforced_concrete": "Stahlbeton",
		"base.editor.structure.steel_frame": "Stahlrahmen",
		"base.editor.structure.composite_core": "Verbundkern",

		# Inhaltsnamen aus internen IDs.
		"base.editor.room_type.bedroom": "Schlafzimmer",
		"base.editor.room_type.kitchen": "Küche",
		"base.editor.room_type.bathroom": "Badezimmer",
		"base.editor.room_type.office": "Büro",
		"base.editor.room_type.living_dining": "Wohn-/Essbereich",
		"base.editor.room_type.empty": "Leer",
		"base.editor.room_type.room": "Raum",
		"base.editor.room_class.lower_middle": "untere Mittelschicht",
		"base.editor.room_class.middle": "Mittelschicht",
		"base.editor.room_class.upper": "Oberschicht",

		# Rückfalltexte.
		"base.editor.generic.unknown": "unbekannt",
		"base.editor.generic.unknown_error": "unbekannter Fehler",
		"base.editor.generic.blueprint": "Blaupause",
		"base.editor.generic.prop": "Objekt",
		"base.editor.generic.room": "Raum",
	},
}

var _locale: String = DEFAULT_LOCALE


func _init(locale: String = DEFAULT_LOCALE) -> void:
	set_locale(locale)


## Switch the active locale. Unknown codes are rejected and leave it unchanged.
func set_locale(locale: String) -> bool:
	if not TABLES.has(locale):
		return false
	_locale = locale
	return true


func locale() -> String:
	return _locale


## Switch to the next supported locale, wrapping around.
func toggle() -> String:
	var index := SUPPORTED_LOCALES.find(_locale)
	_locale = SUPPORTED_LOCALES[(index + 1) % SUPPORTED_LOCALES.size()]
	return _locale


## Resolve a key in the active locale, substituting `{0}` / `{name}` params.
##
## Returns `!key!` when the key is absent so a missing translation is visible
## in the UI and in the locale smoke instead of silently falling back.
func t(key: String, params = []) -> String:
	var table: Dictionary = TABLES.get(_locale, {})
	if not table.has(key):
		return "!%s!" % key
	return _apply_params(str(table[key]), params)


func has_key(key: String, locale: String = "") -> bool:
	var table: Dictionary = TABLES.get(locale if not locale.is_empty() else _locale, {})
	return table.has(key)


func keys(locale: String = "") -> PackedStringArray:
	var table: Dictionary = TABLES.get(locale if not locale.is_empty() else _locale, {})
	var result := PackedStringArray()
	for key in table.keys():
		result.append(str(key))
	result.sort()
	return result


## Raw table text without parameter substitution, for lint and smoke checks.
func raw(key: String, locale: String = "") -> String:
	var table: Dictionary = TABLES.get(locale if not locale.is_empty() else _locale, {})
	return str(table.get(key, ""))


## Display name of a locale, rendered in the active locale.
func locale_name(locale: String = "") -> String:
	var code := locale if not locale.is_empty() else _locale
	return t("base.editor.language.%s" % code)


## Keys present in `reference` but missing from `target`.
func missing_keys(reference: String, target: String) -> PackedStringArray:
	var reference_table: Dictionary = TABLES.get(reference, {})
	var target_table: Dictionary = TABLES.get(target, {})
	var missing := PackedStringArray()
	for key in reference_table.keys():
		if not target_table.has(key):
			missing.append(str(key))
	missing.sort()
	return missing


## `{name}` placeholders in a raw string, sorted and unique.
static func placeholders(text: String) -> PackedStringArray:
	var found := PackedStringArray()
	var regex := RegEx.new()
	if regex.compile("\\{([a-z0-9_]+)\\}") != OK:
		return found
	for match_result in regex.search_all(text):
		var name := match_result.get_string(1)
		if not found.has(name):
			found.append(name)
	found.sort()
	return found


func _apply_params(text: String, params) -> String:
	var result := text
	if params is Dictionary:
		for key in params.keys():
			result = result.replace("{%s}" % str(key), str(params[key]))
	elif params is Array:
		for index in params.size():
			result = result.replace("{%d}" % index, str(params[index]))
	elif params != null:
		result = result.replace("{0}", str(params))
	return result
