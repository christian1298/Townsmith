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
		"base.ui.citizen.resident_line": "· {name}  (resident slot {slot})",
		"base.ui.citizen.household": "Household",
		"base.ui.citizen.household_line": "{residents} / {capacity} residents · {employed} employed",
		"base.ui.citizen.commute_income": "Commute {commute} m · income {income}/day",
		"base.ui.citizen.needs": "Food {food}% · water {water}% · rest {rest}%",
		"base.ui.citizen.household_unavailable": "Household runtime is not available yet.",

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
		"base.ui.citizen.resident_line": "· {name}  (Bewohnerplatz {slot})",
		"base.ui.citizen.household": "Haushalt",
		"base.ui.citizen.household_line": "{residents} / {capacity} Bewohner · {employed} beschäftigt",
		"base.ui.citizen.commute_income": "Arbeitsweg {commute} m · Einkommen {income}/Tag",
		"base.ui.citizen.needs": "Essen {food}% · Wasser {water}% · Ruhe {rest}%",
		"base.ui.citizen.household_unavailable": "Haushaltsdaten sind noch nicht verfügbar.",

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
