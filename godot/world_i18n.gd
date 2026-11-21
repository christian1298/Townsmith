extends RefCounted

## Slice-local world UI string table and runtime lookup.
##
## The citizen panel and the follow hint are the first world-scene strings to
## move behind keys. The plan in `docs/16 - Localization.md` compiles content
## tables into this format; this file is the single-table slice stand-in until
## the shared `base.ui.*` pipeline lands. Citizen names stay proper nouns and
## never pass through here.
##
## Lookup rules:
## - the locale comes from `OS.get_locale()` unless one is passed explicitly;
## - `en` and `de` are supported, everything else falls back to English;
## - a key missing from the active locale falls back to English, then `!key!`;
## - placeholders are named (`{name}`, `{id}`, ...) and substituted verbatim.
##
## See `docs/67 - Named Citizens.md` and `docs/69 - Localization Audit.md`.

const DEFAULT_LOCALE := "en"
const SUPPORTED_LOCALES := ["en", "de"]

const TABLES := {
	"en": {
		# Citizen panel.
		"base.ui.citizen.title": "Citizens",
		"base.ui.citizen.close_hint": "F2 or Esc to close",
		"base.ui.citizen.no_selection": "No home selected.",
		"base.ui.citizen.no_selection_hint": "Right-click a completed home to inspect its residents.",
		"base.ui.citizen.selection_unavailable": "The selected placement is no longer available.",
		"base.ui.citizen.no_named": "No named residents (vacant home or not yet synced).",
		"base.ui.citizen.resident_line": "· {name}  (slot {slot}, age {age}, {employment})",
		"base.ui.citizen.employed": "employed",
		"base.ui.citizen.unemployed": "unemployed",
		"base.ui.citizen.household": "Household",
		"base.ui.citizen.household_line": "{residents} / {capacity} residents · {employed} employed",
		"base.ui.citizen.commute_income": "Commute {commute} m · income {income}/day",
		"base.ui.citizen.needs": "Food {food}% · water {water}% · rest {rest}%",
		"base.ui.citizen.household_unavailable": "Household runtime is not available yet.",

		# HUD body.
		"base.ui.hud.mayor.title": "Townsmith — Mayor Mode  (seed {seed})",
		"base.ui.hud.architect.title": "Townsmith — Architect View (read-only)  (seed {seed})",
		"base.ui.hud.blueprint": "Blueprint: {name}",
		"base.ui.hud.blueprint_mayor": "Blueprint: {name}  ·  {rooms} rooms  ·  capacity {capacity}",
		"base.ui.hud.structure": "Structure: —",
		"base.ui.hud.structure_valid": "Structure: {system}  {stories}/{max_stories} stories · span {span}/{max_span} m · foundation {foundation}/{required} m",
		"base.ui.hud.structure_invalid": "Structure: {system}  INVALID: {error} ({code})",
		"base.ui.hud.selected_none": "Selected: none",
		"base.ui.hud.selected": "Selected #{id}: {name}  ·  {rooms} rooms  ·  capacity {capacity}  ·  {jobs} jobs  ·  {footprint}",
		"base.ui.hud.valid": "VALID",
		"base.ui.hud.invalid": "INVALID: {error}",
		"base.ui.hud.target": "Target ({x}, {z})  rotation {rotation}°  {state}",
		"base.ui.hud.placements": "Placements: {placements}   Roads: {roads}   Lots: {lots}   Settlements: {settlements}   Terrain edits: {edits}",
		"base.ui.hud.city": "City: population {population} / housing {housing} ({vacant} vacant) · jobs {jobs} ({employed} employed) · happiness {happiness}",
		"base.ui.hud.needs": "Needs: {needs}",
		"base.ui.hud.treasury": "Treasury: {treasury}",
		"base.ui.hud.construction": "Construction: {active} active / {stalled} stalled   Crew: {crew}/{capacity}   Trades: {trades}   Materials: {materials} L   Save: {save}",
		"base.ui.help.move": "WASD pan · Q/E rotate · wheel zoom · R rotate · X raise · Z lower · N found settlement",
		"base.ui.help.edit": "LMB place · RMB select · Enter: enter selected building",
		"base.ui.help.tools": "C crew · I industry · U utility · K services · B starter · T tax · L loan · Y research · F1 dashboard · F2 citizens · {follow} · F4 materialization · V +10k · F5 save · F9 load",
		"base.ui.help.architect": "First-person view at the selected door · M / Esc returns to Mayor",

		# Dashboard shell.
		"base.ui.dashboard.title": "City Dashboard",
		"base.ui.dashboard.close_hint": "F1 or Esc to close",
		"base.ui.dashboard.section.population": "Population",
		"base.ui.dashboard.section.age": "Age",
		"base.ui.dashboard.section.research": "Research",
		"base.ui.dashboard.section.tutorial": "Tutorial",
		"base.ui.dashboard.section.work": "Work",
		"base.ui.dashboard.section.wellbeing": "Wellbeing",
		"base.ui.dashboard.section.utilities": "Utilities",
		"base.ui.dashboard.section.civic": "Civic services",
		"base.ui.dashboard.section.treasury": "Treasury",
		"base.ui.dashboard.section.industry": "Industry",
		"base.ui.dashboard.section.roads": "Roads",
		"base.ui.dashboard.section.construction": "Construction",
		"base.ui.dashboard.section.advisor": "Advisor",

		# Follow camera.
		"base.ui.citizen.follow_hint": "F3 follows the selected home · Esc stops",
		"base.ui.citizen.following": "Following home #{id} · Esc stops",
		"base.ui.citizen.follow_started": "Following home #{id} · F3 or Esc stops",
		"base.ui.citizen.follow_stopped": "Follow stopped",
		"base.ui.citizen.follow_select_first": "Select a completed home first (RMB)",
		"base.ui.help.follow": "F3 follow home",
	},
	"de": {
		# Bürgerpanel.
		"base.ui.citizen.title": "Bürger",
		"base.ui.citizen.close_hint": "F2 oder Esc zum Schließen",
		"base.ui.citizen.no_selection": "Kein Zuhause ausgewählt.",
		"base.ui.citizen.no_selection_hint": "Rechtsklick auf ein fertiges Haus, um die Bewohner zu sehen.",
		"base.ui.citizen.selection_unavailable": "Die ausgewählte Platzierung ist nicht mehr verfügbar.",
		"base.ui.citizen.no_named": "Keine benannten Bewohner (leer stehend oder noch nicht synchronisiert).",
		"base.ui.citizen.resident_line": "· {name}  (Platz {slot}, Alter {age}, {employment})",
		"base.ui.citizen.employed": "beschäftigt",
		"base.ui.citizen.unemployed": "arbeitslos",
		"base.ui.citizen.household": "Haushalt",
		"base.ui.citizen.household_line": "{residents} / {capacity} Bewohner · {employed} beschäftigt",
		"base.ui.citizen.commute_income": "Arbeitsweg {commute} m · Einkommen {income}/Tag",
		"base.ui.citizen.needs": "Essen {food}% · Wasser {water}% · Ruhe {rest}%",
		"base.ui.citizen.household_unavailable": "Haushaltsdaten sind noch nicht verfügbar.",

		# HUD-Körper.
		"base.ui.hud.mayor.title": "Townsmith — Bürgermeistermodus  (Seed {seed})",
		"base.ui.hud.architect.title": "Townsmith — Architektenansicht (schreibgeschützt)  (Seed {seed})",
		"base.ui.hud.blueprint": "Bauplan: {name}",
		"base.ui.hud.blueprint_mayor": "Bauplan: {name}  ·  {rooms} Räume  ·  Kapazität {capacity}",
		"base.ui.hud.structure": "Tragwerk: —",
		"base.ui.hud.structure_valid": "Tragwerk: {system}  {stories}/{max_stories} Geschosse · Spannweite {span}/{max_span} m · Fundament {foundation}/{required} m",
		"base.ui.hud.structure_invalid": "Tragwerk: {system}  UNGÜLTIG: {error} ({code})",
		"base.ui.hud.selected_none": "Auswahl: keine",
		"base.ui.hud.selected": "Ausgewählt #{id}: {name}  ·  {rooms} Räume  ·  Kapazität {capacity}  ·  {jobs} Stellen  ·  {footprint}",
		"base.ui.hud.valid": "GÜLTIG",
		"base.ui.hud.invalid": "UNGÜLTIG: {error}",
		"base.ui.hud.target": "Ziel ({x}, {z})  Drehung {rotation}°  {state}",
		"base.ui.hud.placements": "Platzierungen: {placements}   Straßen: {roads}   Grundstücke: {lots}   Siedlungen: {settlements}   Geländebearbeitungen: {edits}",
		"base.ui.hud.city": "Stadt: Bevölkerung {population} / Wohnraum {housing} ({vacant} frei) · Arbeitsplätze {jobs} ({employed} beschäftigt) · Zufriedenheit {happiness}",
		"base.ui.hud.needs": "Bedarf: {needs}",
		"base.ui.hud.treasury": "Staatskasse: {treasury}",
		"base.ui.hud.construction": "Bau: {active} aktiv / {stalled} stockend   Crew: {crew}/{capacity}   Gewerke: {trades}   Material: {materials} L   Speichern: {save}",
		"base.ui.help.move": "WASD: schwenken · Q/E: drehen · Mausrad: zoomen · R: drehen · X: anheben · Z: absenken · N: Siedlung gründen",
		"base.ui.help.edit": "LMB: platzieren · RMB: auswählen · Enter: ausgewähltes Gebäude betreten",
		"base.ui.help.tools": "C: Crew · I: Industrie · U: Versorgung · K: Dienste · B: Startpaket · T: Steuern · L: Kredit · Y: Forschung · F1: Dashboard · F2: Bürger · {follow} · F4: Materialisierung · V: +10k · F5: speichern · F9: laden",
		"base.ui.help.architect": "Ego-Ansicht an der ausgewählten Tür · M / Esc kehrt zum Bürgermeistermodus zurück",

		# Dashboard-Hülle.
		"base.ui.dashboard.title": "Stadt-Dashboard",
		"base.ui.dashboard.close_hint": "F1 oder Esc zum Schließen",
		"base.ui.dashboard.section.population": "Bevölkerung",
		"base.ui.dashboard.section.age": "Zeitalter",
		"base.ui.dashboard.section.research": "Forschung",
		"base.ui.dashboard.section.tutorial": "Tutorial",
		"base.ui.dashboard.section.work": "Arbeit",
		"base.ui.dashboard.section.wellbeing": "Wohlergehen",
		"base.ui.dashboard.section.utilities": "Versorgung",
		"base.ui.dashboard.section.civic": "Bürgerdienste",
		"base.ui.dashboard.section.treasury": "Staatskasse",
		"base.ui.dashboard.section.industry": "Industrie",
		"base.ui.dashboard.section.roads": "Straßen",
		"base.ui.dashboard.section.construction": "Bau",
		"base.ui.dashboard.section.advisor": "Berater",

		# Folgekamera.
		"base.ui.citizen.follow_hint": "F3 folgt dem ausgewählten Zuhause · Esc beendet",
		"base.ui.citizen.following": "Folgt Zuhause #{id} · Esc beendet",
		"base.ui.citizen.follow_started": "Folgt Zuhause #{id} · F3 oder Esc beendet",
		"base.ui.citizen.follow_stopped": "Folgen beendet",
		"base.ui.citizen.follow_select_first": "Zuerst ein fertiges Haus auswählen (Rechtsklick)",
		"base.ui.help.follow": "F3: Zuhause folgen",
	},
}

var _locale: String = DEFAULT_LOCALE


func _init(locale: String = "") -> void:
	_locale = _resolve_locale(locale)


## Switch the active locale. Codes such as `de_DE` map to their language and
## unsupported locales fall back to English. Returns the resolved locale.
func set_locale(locale: String) -> String:
	_locale = _resolve_locale(locale)
	return _locale


func locale() -> String:
	return _locale


## Resolve a key in the active locale, then English, then `!key!`.
func t(key: String, params = {}) -> String:
	var value := raw(key, _locale)
	if value == "":
		value = raw(key, DEFAULT_LOCALE)
	if value == "":
		return "!%s!" % key
	return _apply_params(value, params)


func raw(key: String, locale: String = "") -> String:
	var code := locale if not locale.is_empty() else _locale
	var table: Dictionary = TABLES.get(code, {})
	return str(table.get(key, ""))


func has_key(key: String, locale: String = "") -> bool:
	var code := locale if not locale.is_empty() else _locale
	var table: Dictionary = TABLES.get(code, {})
	return table.has(key)


func keys(locale: String = "") -> PackedStringArray:
	var code := locale if not locale.is_empty() else _locale
	var table: Dictionary = TABLES.get(code, {})
	var result := PackedStringArray()
	for key in table.keys():
		result.append(str(key))
	result.sort()
	return result


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


static func _resolve_locale(code: String) -> String:
	var value := code.strip_edges().to_lower().replace("-", "_")
	if value.is_empty():
		value = OS.get_locale().strip_edges().to_lower().replace("-", "_")
	var language := value.get_slice("_", 0)
	if SUPPORTED_LOCALES.has(language):
		return language
	return DEFAULT_LOCALE


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
