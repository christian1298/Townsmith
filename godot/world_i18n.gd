extends RefCounted

## Slice-local world UI string table and runtime lookup.
##
## The citizen panel, the Mayor/Architect HUD, the dashboard shell plus body,
## and advisor titles are behind keys. The plan in
## `docs/16 - Localization.md` compiles content tables into this format; this
## file is the single-table slice stand-in until the shared `base.ui.*`
## pipeline lands. Citizen names stay proper nouns and never pass through here.
## Advisor details stay raw English until the bridge sends code+params.
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

		# HUD status block.
		"base.ui.hud.age": "Age: {value}",
		"base.ui.hud.age_final": "{age} · final age",
		"base.ui.hud.age_next": "{age} · next {next} at {threshold} ({progress}%)",
		"base.ui.hud.research_line": "Research: {value}",
		"base.ui.hud.research": "{points} pts · {completed}/{total} techs · +{rate}/day",
		"base.ui.hud.tutorial_line": "Tutorial: {value}",
		"base.ui.hud.tutorial_complete": "{completed}/{total} · complete",
		"base.ui.hud.tutorial_next": "{completed}/{total} · {title}",
		"base.ui.hud.commute": "Commute {commute} m · wages {wages}/day",
		"base.ui.hud.industry": "Industry: {value}",
		"base.ui.hud.industry_value": "wood {wood} · food {food} · hides {hides} · supply {supply}%",
		"base.ui.hud.utilities": "Utilities: {value}",
		"base.ui.hud.utilities_value": "water {water}% ({water_sites} sites) · power {power}% ({power_sites} sites)",
		"base.ui.hud.civic": "Civic: {value}",
		"base.ui.hud.civic_value": "school {school}% · clinic {clinic}% · fire {fire}% · police {police}% · +{research} research/day",
		"base.ui.hud.traffic": "Traffic: {value}",
		"base.ui.hud.traffic_value": "{nodes} nodes · {edges} edges · {length} m · {cars} cars",
		"base.ui.hud.treasury_value": "{balance} balance · {net}/day · tax {tax}/{business}",
		"base.ui.hud.treasury_debt": "{value} · debt {debt}",
		"base.ui.hud.treasury_bankrupt": "BANKRUPT · {value}",
		"base.ui.hud.needs_value": "food {food}% · water {water}% · rest {rest}%",
		"base.ui.hud.citizens_near": "Citizens near: {active} active · {detailed} detailed",
		"base.ui.hud.selected_owner": "  ·  owner {owner}",
		"base.ui.hud.selected_industry": "  ·  industry {kind}",
		"base.ui.hud.selected_utility": "  ·  utility {kinds}",
		"base.ui.hud.selected_service": "  ·  service {kind}",
		"base.ui.hud.selected_stories": "  ·  {system} {stories} stories",
		"base.ui.hud.selected_residents": "  ·  residents {names}",
		"base.ui.hud.wilderness": "wilderness",

		# Modal messages: selection, placement, crew, save/load.
		"base.ui.modal.select_building": "Select a building first (RMB)",
		"base.ui.modal.select_building_full": "Select a building first (right-click)",
		"base.ui.modal.no_job_slots": "This building has no job slots",
		"base.ui.modal.cannot_place": "Cannot place: {error}",
		"base.ui.modal.placement_failed": "Placement failed: {error}",
		"base.ui.modal.construction_site": "Construction site created",
		"base.ui.modal.crew": "Construction crew: {count}",
		"base.ui.modal.saved": "Saved {placements} placement(s), {edits} edit(s), {sites} site(s), {roads} road(s), {lots} lot(s), {settlements} settlement(s) to {path}",
		"base.ui.modal.save_failed": "Save failed: {error}",
		"base.ui.modal.load_failed": "Load failed: {error}",
		"base.ui.modal.loaded": "Loaded {placements} placement(s) from {path}",
		"base.ui.modal.loaded_mismatch": "Loaded {placements} placement(s) but rebuilt {rebuilt}",

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

		# Dashboard body.
		"base.ui.dashboard.body.population": "{residents} residents · {occupied} / {housing} housing · {vacant} vacant",
		"base.ui.dashboard.body.citizens_near": "Citizens near: {active} active · {detailed} detailed",
		"base.ui.dashboard.body.age_final": "{age} · final age",
		"base.ui.dashboard.body.age_next": "{age} · next {next} at {threshold} ({progress}%)",
		"base.ui.dashboard.body.research": "{points} points · +{rate}/day · {completed} / {total} technologies",
		"base.ui.dashboard.body.tutorial_complete": "{completed} / {total} complete",
		"base.ui.dashboard.body.tutorial_next": "{completed} / {total} · next: {title}",
		"base.ui.dashboard.body.work": "{filled} / {jobs} jobs filled · {unemployed} unemployed · commute {commute} m",
		"base.ui.dashboard.body.happiness": "happiness {value}",
		"base.ui.dashboard.body.needs": "needs {needs}",
		"base.ui.dashboard.body.needs_values": "food {food}% · water {water}% · rest {rest}%",
		"base.ui.dashboard.body.utilities_closed": "food — · water — · power —",
		"base.ui.dashboard.body.utilities": "food {food}% · water {water}% · power {power}%",
		"base.ui.dashboard.body.civic_closed": "school — · clinic — · fire — · police —",
		"base.ui.dashboard.body.civic": "school {school}% · clinic {clinic}% · fire {fire}% · police {police}% · +{research} research/day",
		"base.ui.dashboard.body.treasury_closed": "closed",
		"base.ui.dashboard.body.treasury": "{balance} balance · {net}/day · debt {debt}",
		"base.ui.dashboard.body.treasury_bankrupt": "BANKRUPT · {treasury}",
		"base.ui.dashboard.body.industry": "wood {wood} · food {food} · hides {hides}",
		"base.ui.dashboard.body.roads_closed": "no road store",
		"base.ui.dashboard.body.roads": "{nodes} nodes · {edges} edges · {length} m · {components} components",
		"base.ui.dashboard.body.construction": "{active} active · {stalled} stalled · {completed} completed",
		"base.ui.dashboard.body.no_advice": "No advice available.",
		"base.ui.dashboard.body.advisor_fallback": "Advisor",

		# Advisor titles, keyed by the stable code from `core_city::advisor`.
		# One title per code; the Rust detail stays English until the protocol
		# carries code+params. Unknown codes fall back to the bridge title.
		"base.advisor.city.bankrupt.title": "Treasury bankrupt",
		"base.advisor.city.food_shortage.title": "Food shortage",
		"base.advisor.city.water_shortage.title": "Water shortage",
		"base.advisor.city.power_shortage.title": "Power shortage",
		"base.advisor.city.school_shortage.title": "School shortage",
		"base.advisor.city.clinic_shortage.title": "Clinic shortage",
		"base.advisor.city.fire_shortage.title": "Fire coverage is low",
		"base.advisor.city.police_shortage.title": "Police coverage is low",
		"base.advisor.city.unemployment.title": "Residents are unemployed",
		"base.advisor.city.vacant_homes.title": "Homes are standing empty",
		"base.advisor.city.unhappy.title": "Residents are unhappy",
		"base.advisor.city.long_commutes.title": "Commutes are long",
		"base.advisor.city.no_residents.title": "No residents yet",
		"base.advisor.city.budget_deficit.title": "The city is running a deficit",
		"base.advisor.city.debt.title": "Outstanding loans",
		"base.advisor.city.traffic_disconnected.title": "Road network is disconnected",
		"base.advisor.city.no_roads.title": "No roads",
		"base.advisor.city.construction_stalled.title": "Construction is stalled",
		"base.advisor.city.construction_active.title": "Construction is in progress",
		"base.advisor.city.healthy.title": "The city is healthy",
		"base.advisor.city.services_healthy.title": "Services are healthy",
		"base.advisor.city.quiet.title": "No urgent issues",

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

		# HUD-Statusblock.
		"base.ui.hud.age": "Zeitalter: {value}",
		"base.ui.hud.age_final": "{age} · letztes Zeitalter",
		"base.ui.hud.age_next": "{age} · als Nächstes {next} bei {threshold} ({progress} %)",
		"base.ui.hud.research_line": "Forschung: {value}",
		"base.ui.hud.research": "{points} Pkt. · {completed}/{total} Techs · +{rate}/Tag",
		"base.ui.hud.tutorial_line": "Tutorial: {value}",
		"base.ui.hud.tutorial_complete": "{completed}/{total} · abgeschlossen",
		"base.ui.hud.tutorial_next": "{completed}/{total} · {title}",
		"base.ui.hud.commute": "Arbeitsweg {commute} m · Löhne {wages}/Tag",
		"base.ui.hud.industry": "Industrie: {value}",
		"base.ui.hud.industry_value": "Holz {wood} · Nahrung {food} · Felle {hides} · Versorgung {supply} %",
		"base.ui.hud.utilities": "Versorgung: {value}",
		"base.ui.hud.utilities_value": "Wasser {water} % ({water_sites} Stellen) · Strom {power} % ({power_sites} Stellen)",
		"base.ui.hud.civic": "Bürgerdienste: {value}",
		"base.ui.hud.civic_value": "Schule {school} % · Klinik {clinic} % · Feuerwehr {fire} % · Polizei {police} % · +{research} Forschung/Tag",
		"base.ui.hud.traffic": "Verkehr: {value}",
		"base.ui.hud.traffic_value": "{nodes} Knoten · {edges} Kanten · {length} m · {cars} Fahrzeuge",
		"base.ui.hud.treasury_value": "{balance} Saldo · {net}/Tag · Steuer {tax}/{business}",
		"base.ui.hud.treasury_debt": "{value} · Schulden {debt}",
		"base.ui.hud.treasury_bankrupt": "BANKROTT · {value}",
		"base.ui.hud.needs_value": "Essen {food} % · Wasser {water} % · Ruhe {rest} %",
		"base.ui.hud.citizens_near": "Bürger in der Nähe: {active} aktiv · {detailed} detailliert",
		"base.ui.hud.selected_owner": "  ·  Eigentümer {owner}",
		"base.ui.hud.selected_industry": "  ·  Industrie {kind}",
		"base.ui.hud.selected_utility": "  ·  Versorgung {kinds}",
		"base.ui.hud.selected_service": "  ·  Dienst {kind}",
		"base.ui.hud.selected_stories": "  ·  {system} {stories} Geschosse",
		"base.ui.hud.selected_residents": "  ·  Bewohner {names}",
		"base.ui.hud.wilderness": "Wildnis",

		# Modalmeldungen: Auswahl, Platzierung, Crew, Speichern/Laden.
		"base.ui.modal.select_building": "Zuerst ein Gebäude auswählen (RMB)",
		"base.ui.modal.select_building_full": "Zuerst ein Gebäude auswählen (Rechtsklick)",
		"base.ui.modal.no_job_slots": "Dieses Gebäude hat keine Arbeitsplätze",
		"base.ui.modal.cannot_place": "Platzieren nicht möglich: {error}",
		"base.ui.modal.placement_failed": "Platzierung fehlgeschlagen: {error}",
		"base.ui.modal.construction_site": "Baustelle angelegt",
		"base.ui.modal.crew": "Bautrupp: {count}",
		"base.ui.modal.saved": "{placements} Platzierung(en), {edits} Bearbeitung(en), {sites} Baustelle(n), {roads} Straße(n), {lots} Grundstück(e), {settlements} Siedlung(en) nach {path} gespeichert",
		"base.ui.modal.save_failed": "Speichern fehlgeschlagen: {error}",
		"base.ui.modal.load_failed": "Laden fehlgeschlagen: {error}",
		"base.ui.modal.loaded": "{placements} Platzierung(en) aus {path} geladen",
		"base.ui.modal.loaded_mismatch": "{placements} Platzierung(en) geladen, aber {rebuilt} neu aufgebaut",

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

		# Dashboard-Körper.
		"base.ui.dashboard.body.population": "{residents} Bewohner · {occupied} / {housing} Wohnraum · {vacant} frei",
		"base.ui.dashboard.body.citizens_near": "Bürger in der Nähe: {active} aktiv · {detailed} detailliert",
		"base.ui.dashboard.body.age_final": "{age} · letztes Zeitalter",
		"base.ui.dashboard.body.age_next": "{age} · als Nächstes {next} bei {threshold} ({progress} %)",
		"base.ui.dashboard.body.research": "{points} Punkte · +{rate}/Tag · {completed} / {total} Technologien",
		"base.ui.dashboard.body.tutorial_complete": "{completed} / {total} abgeschlossen",
		"base.ui.dashboard.body.tutorial_next": "{completed} / {total} · als Nächstes: {title}",
		"base.ui.dashboard.body.work": "{filled} / {jobs} Arbeitsplätze besetzt · {unemployed} arbeitslos · Arbeitsweg {commute} m",
		"base.ui.dashboard.body.happiness": "Zufriedenheit {value}",
		"base.ui.dashboard.body.needs": "Bedarf {needs}",
		"base.ui.dashboard.body.needs_values": "Essen {food}% · Wasser {water}% · Ruhe {rest}%",
		"base.ui.dashboard.body.utilities_closed": "Nahrung — · Wasser — · Strom —",
		"base.ui.dashboard.body.utilities": "Nahrung {food}% · Wasser {water}% · Strom {power}%",
		"base.ui.dashboard.body.civic_closed": "Schule — · Klinik — · Feuerwehr — · Polizei —",
		"base.ui.dashboard.body.civic": "Schule {school}% · Klinik {clinic}% · Feuerwehr {fire}% · Polizei {police}% · +{research} Forschung/Tag",
		"base.ui.dashboard.body.treasury_closed": "geschlossen",
		"base.ui.dashboard.body.treasury": "{balance} Saldo · {net}/Tag · Schulden {debt}",
		"base.ui.dashboard.body.treasury_bankrupt": "BANKROTT · {treasury}",
		"base.ui.dashboard.body.industry": "Holz {wood} · Nahrung {food} · Felle {hides}",
		"base.ui.dashboard.body.roads_closed": "kein Straßenspeicher",
		"base.ui.dashboard.body.roads": "{nodes} Knoten · {edges} Kanten · {length} m · {components} Komponenten",
		"base.ui.dashboard.body.construction": "{active} aktiv · {stalled} stockend · {completed} fertiggestellt",
		"base.ui.dashboard.body.no_advice": "Keine Ratschläge verfügbar.",
		"base.ui.dashboard.body.advisor_fallback": "Berater",

		# Beratertitel, nach dem stabilen Code aus `core_city::advisor`.
		# Ein Titel pro Code; der Rust-Detailtext bleibt vorerst englisch.
		"base.advisor.city.bankrupt.title": "Staatskasse bankrott",
		"base.advisor.city.food_shortage.title": "Nahrungsmangel",
		"base.advisor.city.water_shortage.title": "Wassermangel",
		"base.advisor.city.power_shortage.title": "Strommangel",
		"base.advisor.city.school_shortage.title": "Schulmangel",
		"base.advisor.city.clinic_shortage.title": "Klinikmangel",
		"base.advisor.city.fire_shortage.title": "Feuerwehr unterversorgt",
		"base.advisor.city.police_shortage.title": "Polizei unterversorgt",
		"base.advisor.city.unemployment.title": "Arbeitslosigkeit",
		"base.advisor.city.vacant_homes.title": "Leerstehende Wohnungen",
		"base.advisor.city.unhappy.title": "Unzufriedene Bewohner",
		"base.advisor.city.long_commutes.title": "Lange Arbeitswege",
		"base.advisor.city.no_residents.title": "Noch keine Bewohner",
		"base.advisor.city.budget_deficit.title": "Haushaltsdefizit",
		"base.advisor.city.debt.title": "Offene Kredite",
		"base.advisor.city.traffic_disconnected.title": "Straßennetz getrennt",
		"base.advisor.city.no_roads.title": "Keine Straßen",
		"base.advisor.city.construction_stalled.title": "Bau stockt",
		"base.advisor.city.construction_active.title": "Bau läuft",
		"base.advisor.city.healthy.title": "Stadt ist gesund",
		"base.advisor.city.services_healthy.title": "Dienste sind gesund",
		"base.advisor.city.quiet.title": "Keine dringenden Probleme",

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


## Localized advisor title for a stable code such as `city.budget_deficit`.
## Unknown codes, and codes without a DE key, keep the bridge title in English
## until the Rust protocol carries code+params.
func advisor_title(code: String, fallback: String) -> String:
	var key := "base.advisor.%s.title" % code
	if has_key(key) or has_key(key, DEFAULT_LOCALE):
		return t(key)
	return fallback


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
