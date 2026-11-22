extends RefCounted

## Static display-name table for content IDs.
##
## The HUD, dashboard, and tutorial currently render raw IDs or
## `kind.capitalize()` labels. This table gives the next UI slice a single
## place to resolve a stable content ID to an English or German display name
## without touching scene code. It is deliberately data-only: nothing reads
## it from `world_placement.gd` yet.
##
## Key layout:
## - `base.content.starter.<slug>` where `<slug>` is the starter ID without
##   the `base:` namespace (`base:stone_house` -> `...starter.stone_house`);
## - `base.content.utility.<kind>` for `water`, `power`;
## - `base.content.industry.<kind>` for `lumberjack`, `farm`, `hunt`;
## - `base.content.service.<kind>` for `school`, `clinic`, `fire`, `police`;
## - `base.content.tutorial.<step>` for the five tutorial step IDs.
##
## Lookup rules:
## - an explicit locale wins; an empty locale follows `OS.get_locale()`;
## - `en` and `de` are supported, everything else resolves to English;
## - a key missing from the active locale falls back to English, then to the
##   key itself (no `!key!` marker, so callers can show the ID as a fallback).
##
## Values are plain display names, never sentences or interpolated templates.
## See `docs/16 - Localization.md` and `docs/69 - Localization Audit.md`.

const DEFAULT_LOCALE := "en"
const SUPPORTED_LOCALES := ["en", "de"]

const _STARTER_IDS := [
	"base:cottage",
	"base:tent",
	"base:longhouse",
	"base:workshop",
	"base:farmhouse",
	"base:market",
	"base:sawmill",
	"base:stone_house",
	"base:town_hall",
	"base:school",
	"base:clinic",
	"base:bakery",
	"base:tenement",
	"base:factory",
	"base:apartment",
	"base:office",
	"base:hospital",
	"base:tower",
	"base:university",
	"base:data_center",
]

const _UTILITY_KINDS := ["water", "power"]
const _INDUSTRY_KINDS := ["lumberjack", "farm", "hunt"]
const _SERVICE_KINDS := ["school", "clinic", "fire", "police"]
const _TUTORIAL_STEPS := ["lumber_camp", "farm", "well", "house", "school"]

const DISPLAY := {
	"en": {
		# Starter library (`crates/core_city/src/library.rs`).
		"base.content.starter.cottage": "Cottage",
		"base.content.starter.tent": "Tent",
		"base.content.starter.longhouse": "Longhouse",
		"base.content.starter.workshop": "Workshop",
		"base.content.starter.farmhouse": "Farmhouse",
		"base.content.starter.market": "Market",
		"base.content.starter.sawmill": "Sawmill",
		"base.content.starter.stone_house": "Stone House",
		"base.content.starter.town_hall": "Town Hall",
		"base.content.starter.school": "School",
		"base.content.starter.clinic": "Clinic",
		"base.content.starter.bakery": "Bakery",
		"base.content.starter.tenement": "Tenement",
		"base.content.starter.factory": "Factory",
		"base.content.starter.apartment": "Apartment Block",
		"base.content.starter.office": "Office",
		"base.content.starter.hospital": "Hospital",
		"base.content.starter.tower": "Tower",
		"base.content.starter.university": "University",
		"base.content.starter.data_center": "Data Center",

		# Utility kinds (`crates/core_city/src/utilities.rs`).
		"base.content.utility.water": "Water",
		"base.content.utility.power": "Power",

		# Primary industry kinds (`crates/core_city/src/industry.rs`).
		"base.content.industry.lumberjack": "Lumberjack",
		"base.content.industry.farm": "Farm",
		"base.content.industry.hunt": "Hunt",

		# Civic service kinds (`crates/core_city/src/services.rs`).
		"base.content.service.school": "School",
		"base.content.service.clinic": "Clinic",
		"base.content.service.fire": "Fire",
		"base.content.service.police": "Police",

		# Tutorial steps (`crates/core_city/src/tutorial.rs`).
		"base.content.tutorial.lumber_camp": "Lumber Camp",
		"base.content.tutorial.farm": "Farm",
		"base.content.tutorial.well": "Well",
		"base.content.tutorial.house": "House",
		"base.content.tutorial.school": "School",
	},
	"de": {
		# Startbibliothek (`crates/core_city/src/library.rs`).
		"base.content.starter.cottage": "Kate",
		"base.content.starter.tent": "Zelt",
		"base.content.starter.longhouse": "Langhaus",
		"base.content.starter.workshop": "Werkstatt",
		"base.content.starter.farmhouse": "Bauernhaus",
		"base.content.starter.market": "Markt",
		"base.content.starter.sawmill": "Sägewerk",
		"base.content.starter.stone_house": "Steinhaus",
		"base.content.starter.town_hall": "Rathaus",
		"base.content.starter.school": "Schule",
		"base.content.starter.clinic": "Klinik",
		"base.content.starter.bakery": "Bäckerei",
		"base.content.starter.tenement": "Mietshaus",
		"base.content.starter.factory": "Fabrik",
		"base.content.starter.apartment": "Wohnblock",
		"base.content.starter.office": "Büro",
		"base.content.starter.hospital": "Krankenhaus",
		"base.content.starter.tower": "Turm",
		"base.content.starter.university": "Universität",
		"base.content.starter.data_center": "Rechenzentrum",

		# Versorgungsarten (`crates/core_city/src/utilities.rs`).
		"base.content.utility.water": "Wasser",
		"base.content.utility.power": "Strom",

		# Primäre Industriezweige (`crates/core_city/src/industry.rs`).
		"base.content.industry.lumberjack": "Holzfäller",
		"base.content.industry.farm": "Bauernhof",
		"base.content.industry.hunt": "Jagd",

		# Bürgerliche Dienste (`crates/core_city/src/services.rs`).
		"base.content.service.school": "Schule",
		"base.content.service.clinic": "Klinik",
		"base.content.service.fire": "Feuerwehr",
		"base.content.service.police": "Polizei",

		# Tutorial-Schritte (`crates/core_city/src/tutorial.rs`).
		"base.content.tutorial.lumber_camp": "Holzfällerlager",
		"base.content.tutorial.farm": "Bauernhof",
		"base.content.tutorial.well": "Brunnen",
		"base.content.tutorial.house": "Haus",
		"base.content.tutorial.school": "Schule",
	},
}


## Resolve a content key in `locale`, then English, then the raw key itself.
static func display(key: String, locale: String = "en") -> String:
	var table: Dictionary = DISPLAY.get(_resolve_locale(locale), {})
	if table.has(key):
		return str(table[key])
	var english: Dictionary = DISPLAY[DEFAULT_LOCALE]
	if english.has(key):
		return str(english[key])
	return key


## Starter display names keyed by stable content ID (`base:cottage`, ...).
static func starters(locale: String = "en") -> Dictionary:
	var result := {}
	for id in _STARTER_IDS:
		result[id] = display("base.content.starter.%s" % str(id).get_slice(":", 1), locale)
	return result


## Industry display names keyed by kind (`lumberjack`, `farm`, `hunt`).
static func industries(locale: String = "en") -> Dictionary:
	return _flatten("industry", _INDUSTRY_KINDS, locale)


## Service display names keyed by kind (`school`, `clinic`, `fire`, `police`).
static func services(locale: String = "en") -> Dictionary:
	return _flatten("service", _SERVICE_KINDS, locale)


## Tutorial display names keyed by step (`lumber_camp`, `farm`, `well`, ...).
static func tutorials(locale: String = "en") -> Dictionary:
	return _flatten("tutorial", _TUTORIAL_STEPS, locale)


static func _flatten(domain: String, ids: Array, locale: String) -> Dictionary:
	var result := {}
	for id in ids:
		result[id] = display("base.content.%s.%s" % [domain, id], locale)
	return result


static func _resolve_locale(code: String) -> String:
	var value := code.strip_edges().to_lower().replace("-", "_")
	if value.is_empty():
		value = OS.get_locale().strip_edges().to_lower().replace("-", "_")
	var language := value.get_slice("_", 0)
	if SUPPORTED_LOCALES.has(language):
		return language
	return DEFAULT_LOCALE
