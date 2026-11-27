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
## Numbers are formatted here, not with `%d` / `%.0f` in UI code, so the group
## and decimal separators follow the locale (`1,234.5` in EN, `1.234,5` in DE).
## See `format_int` and `format_float`.
##
## See `docs/67 - Named Citizens.md` and `docs/69 - Localization Audit.md`.

const DEFAULT_LOCALE := "en"
const SUPPORTED_LOCALES := ["en", "de"]
## Rendered for NaN/INF; a value that cannot be read as a number renders as 0.
const NOT_AVAILABLE := "—"
const MAX_DECIMALS := 6

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
		"base.ui.hud.construction": "Construction: {active} active / {stalled} stalled   Crew: {crew}/{capacity}   Trades: {trades}   Materials: {materials} L   Save: {save}   {clock}",
		"base.ui.hud.clock_developer": "Clock: Developer",
		"base.ui.hud.clock_campaign": "Clock: Campaign · {seconds} s/day · Day {day}",

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
		"base.ui.hud.citizens_near": "Citizens near: {active} active · {detailed} detailed · {near} near · {far} far",
		"base.ui.heatmap.status": "District heatmap: {state} · {cells} cells · peak {peak}",
		"base.ui.heatmap.on": "on",
		"base.ui.heatmap.off": "off",
		"base.ui.heatmap.metric_line": "Metric: {metric} · peak {peak} · Shift+F7 cycles",
		"base.ui.heatmap.metric.population": "Population",
		"base.ui.heatmap.metric.jobs": "Jobs",
		"base.ui.heatmap.metric.happiness": "Happiness",
		"base.ui.heatmap.legend": "{metric} · Low → High · dark purple to bright yellow · 512 m radius · 32 m cells",
		"base.ui.heatmap.empty": "No data in range",
		"base.ui.heatmap.unavailable": "District heatmap unavailable: {code}",

		# District overview (developer panel) settlement member block.
		"base.ui.district.members.unowned": "Unowned",
		"base.ui.district.members.owner": "Owner #{id} · {role}",
		"base.ui.district.members.owner_named": "Owner {name} (#{id}) · {role}",
		"base.ui.district.members.row": "#{id} {role} · {permissions}",
		"base.ui.district.members.row_named": "{name} (#{id}) {role} · {permissions}",
		"base.ui.district.members.no_permissions": "no permissions",
		"base.ui.district.members.permission.build": "build",
		"base.ui.district.members.permission.manage": "manage",
		"base.ui.district.members.permission.tax": "tax",
		"base.ui.district.members.permission.members": "members",
		# Read-only settlement screen (docs/85 §7). Names, ids, and codes stay
		# verbatim; roles and permission tokens use localized display names.
		"base.ui.settlement_screen.title": "Settlements",
		"base.ui.settlement_screen.authority": "Authority: {mode}",
		"base.ui.settlement_screen.authority_local": "Local",
		"base.ui.settlement_screen.authority_connected": "Connected",
		"base.ui.settlement_screen.world_checksum": "World checksum: {checksum}",
		"base.ui.settlement_screen.connected_status": "Connected to {host}:{port} · player {name}",
		"base.ui.settlement_screen.page": "Page {page} of {pages} · {count} of {total}",
		"base.ui.settlement_screen.row": "{name}  ·  #{id}  ·  {owner}  ·  tax {tax}%  ·  rev {revision}  ·  {members} members",
		"base.ui.settlement_screen.owner": "{name} #{id}",
		"base.ui.settlement_screen.owner_unknown": "#{id}",
		"base.ui.settlement_screen.owner_unowned": "unowned",
		"base.ui.settlement_screen.owner_label": "Owner: {owner}",
		"base.ui.settlement_screen.person": "{name} #{id}",
		"base.ui.settlement_screen.person_unknown": "#{id}",
		"base.ui.settlement_screen.member": "{person} · {role} · {permissions}",
		"base.ui.settlement_screen.detail_title": "{name} (#{id})",
		"base.ui.settlement_screen.detail_center": "Centre ({x}, {z}) · radius {radius} m",
		"base.ui.settlement_screen.detail_tax": "Tax: {tax}%",
		"base.ui.settlement_screen.detail_revision": "Membership revision: {revision}",
		"base.ui.settlement_screen.detail_members": "Members ({count})",
		"base.ui.settlement_screen.detail_no_members": "No members.",
		"base.ui.settlement_screen.detail_empty": "Select a settlement to inspect it.",
		"base.ui.settlement_screen.footer_more": "+{count} more",
		"base.ui.settlement_screen.footer_end": "End of list",
		"base.ui.settlement_screen.help": "Pages: PgUp/PgDn · Rows: ↑/↓ or 1–8 · Actions: C/N/T/Y/I/M/Del/X",
		"base.ui.settlement_screen.help_connected": "Pages: PgUp/PgDn · Rows: ↑/↓ or 1–8 · Connected: N rename · T tax",
		"base.ui.settlement_screen.empty": "No settlements in this world.",
		"base.ui.settlement_screen.unavailable": "Settlements unavailable ({code}).",
		"base.ui.settlement_screen.projection": "Projected local multiplier: x{multiplier} (preview, D-039 proposed)",
		"base.ui.settlement_screen.projection_unavailable": "Projected local multiplier: unavailable ({code})",
		"base.ui.settlement_screen.projection_connected": "Projected local multiplier: unavailable (connected mode)",
		"base.ui.settlement_screen.permissions_none": "no permissions",
		"base.ui.settlement_screen.permission.build": "build",
		"base.ui.settlement_screen.permission.manage": "manage",
		"base.ui.settlement_screen.permission.tax": "tax",
		"base.ui.settlement_screen.permission.members": "members",
		"base.ui.settlement_screen.role.owner": "owner",
		"base.ui.settlement_screen.role.manager": "manager",
		"base.ui.settlement_screen.role.builder": "builder",
		"base.ui.settlement_screen.role.member": "member",
		"base.ui.settlement_screen.action.claim": "Claim",
		"base.ui.settlement_screen.action.rename": "Rename",
		"base.ui.settlement_screen.action.tax": "Tax +5%",
		"base.ui.settlement_screen.action.transfer": "Transfer",
		"base.ui.settlement_screen.action.invite": "Invite",
		"base.ui.settlement_screen.action.cycle_role": "Cycle role",
		"base.ui.settlement_screen.action.remove_member": "Remove member",
		"base.ui.settlement_screen.action.release": "Release",
		"base.ui.settlement_screen.action.button": "{key} {action}",
		"base.ui.settlement_screen.action.button_disabled": "{key} {action} · {reason}",
		"base.ui.settlement_screen.action.result_ok": "{action} accepted (revision {revision})",
		"base.ui.settlement_screen.action.result_failed": "{action} rejected: {code} (revision {revision})",
		"base.ui.settlement_screen.action.confirm_remove": "Press Delete again to remove {person}.",
		"base.ui.settlement_screen.action.confirm_release": "Press X again to release {name} #{id}.",
		"base.ui.settlement_screen.action.cancelled": "Action cancelled.",
		"base.ui.settlement_screen.rename.title": "Rename settlement #{id}",
		"base.ui.settlement_screen.rename.count": "{count}/48 characters",
		"base.ui.settlement_screen.rename.save": "Save",
		"base.ui.settlement_screen.rename.cancel": "Cancel",
		"base.ui.settlement_screen.rename.empty": "Name must not be empty.",
		"base.ui.settlement_screen.rename.too_long": "Name must be at most 48 characters.",
		"base.ui.settlement_screen.rename.unchanged": "The name is unchanged.",
		"base.ui.settlement_screen.rename.invalid": "Name contains control characters.",
		"base.ui.settlement_screen.rename.error": "{reason} ({code})",
		"base.ui.settlement_screen.rename.rejected": "Rename failed: {reason}",
		"base.ui.settlement_screen.paging.prev": "◀ Prev",
		"base.ui.settlement_screen.paging.next": "Next ▶",
		"base.ui.settlement.default_name": "Settlement {index}",
		"base.ui.settlement_screen.reason.no_core": "local core unavailable",
		"base.ui.settlement_screen.reason.no_selection": "select a settlement",
		"base.ui.settlement_screen.reason.settlement.already_owned": "already owned",
		"base.ui.settlement_screen.reason.settlement.unowned": "unowned",
		"base.ui.settlement_screen.reason.settlement.owner_required": "owner cannot be removed",
		"base.ui.settlement_screen.reason.settlement.unknown": "settlement not found",
		"base.ui.settlement_screen.reason.no_player": "no registered player",
		"base.ui.settlement_screen.reason.no_recipient": "no other registered player",
		"base.ui.settlement_screen.reason.no_invitee": "no eligible player to invite",
		"base.ui.settlement_screen.reason.no_member": "select a member",
		"base.ui.settlement_screen.reason.connected_unavailable": "connected action not available yet",
		"base.ui.hud.lifecycle": "Lifecycle: day {day} · {aged} aged · {died} died",
		"base.ui.hud.selected_owner": "  ·  owner {owner}",
		"base.ui.hud.selected_industry": "  ·  industry {kind}",
		"base.ui.hud.selected_utility": "  ·  utility {kinds}",
		"base.ui.hud.selected_service": "  ·  service {kind}",
		"base.ui.hud.selected_stories": "  ·  {system} {stories} stories",
		"base.ui.hud.selected_residents": "  ·  residents {names}",
		"base.ui.hud.wilderness": "wilderness",

		# Developer chat panel. Player names and message bodies stay raw.
		"base.ui.chat.title": "Chat (developer panel)",
		"base.ui.chat.placeholder": "Message (1–{max} characters)",
		"base.ui.chat.send": "Send",
		"base.ui.chat.help": "Enter: send · Esc: clear draft/error · Ctrl+L: focus draft",
		"base.ui.chat.empty_log": "No messages yet.",
		"base.ui.chat.char_counter": "{count}/{max} characters",
		"base.ui.chat.status_signed_in": "Signed in as {name} (#{id})",
		"base.ui.chat.status_no_identity": "No identity (set_identity)",
		"base.ui.chat.error.line": "Error: {message} ({code})",
		"base.ui.chat.error.unknown": "Unexpected chat error.",
		"base.ui.chat.error.invalid_fallback": "Invalid chat message.",
		"base.ui.chat.error.empty": "Chat message is empty.",
		"base.ui.chat.error.too_long": "Chat message is too long ({max} characters max).",
		"base.ui.chat.error.invalid": "Chat message contains a control character.",
		"base.ui.chat.error.identity": "Sign in before sending chat.",
		"base.ui.chat.error.malformed_sent": "chat_sent payload has a malformed entry",
		"base.ui.chat.error.malformed_log": "chat_log payload has no entries list",
		"base.ui.chat.error.malformed_entries": "chat_log payload has malformed entries",

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

		# Modal messages: rare world actions. Error/content IDs and generated
		# names stay raw parameters; only the sentence is translated.
		"base.ui.modal.blueprint_failed": "Blueprint failed: {path}",
		"base.ui.modal.blueprint_rejected": "Blueprint rejected: {code}",
		"base.ui.modal.blueprint_mesh_empty": "Blueprint mesh is empty",
		"base.ui.modal.blueprint_bounds_invalid": "Blueprint bounds are invalid",
		"base.ui.modal.lot_corner_cleared": "Lot corner cleared",
		"base.ui.modal.lot_mode_help": "Lot mode: click two corners · 1/2/3 zone · Esc cancels",
		"base.ui.modal.lot_zone": "Lot zone: {zone}",
		"base.ui.modal.lot_corner": "Lot corner ({x}, {z}) · click the opposite corner",
		"base.ui.modal.lot_min_size": "Lot needs at least 4 × 4 m",
		"base.ui.modal.lot_rejected": "Lot rejected: {code}",
		"base.ui.modal.starter_catalog_failed": "Starter catalog failed: {code}",
		"base.ui.modal.starter_none": "No starter buildings at this age",
		"base.ui.modal.starter_failed": "Starter failed: {code}",
		"base.ui.modal.starter_no_path": "Starter has no package path",
		"base.ui.modal.starter_package_rejected": "Starter package rejected",
		"base.ui.modal.starter_selected": "Starter: {name} ({index}/{total})",
		"base.ui.modal.industry_removed": "Industry removed",
		"base.ui.modal.industry_failed": "Industry failed: {code}",
		"base.ui.modal.industry_selected": "Industry: {kind}",
		"base.ui.modal.service_removed": "Civic service removed",
		"base.ui.modal.service_failed": "Civic service failed: {code}",
		"base.ui.modal.service_selected": "Civic service: {kind}",
		"base.ui.modal.utility_failed": "Utility failed: {code}",
		"base.ui.modal.utility_water": "Utility: water",
		"base.ui.modal.utility_water_power": "Utility: water + power",
		"base.ui.modal.utility_removed": "Utility removed",
		"base.ui.modal.tax_failed": "Tax change failed: {code}",
		"base.ui.modal.tax_policy": "Tax policy: {name}",
		"base.ui.modal.loan_rejected": "Loan rejected: {code}",
		"base.ui.modal.loan_taken": "Borrowed {amount} · debt {debt}",
		"base.ui.modal.settlement_rejected": "Settlement rejected: {code}",
		"base.ui.modal.settlement_founded": "Founded {name}",
		"base.ui.modal.settlements_cleared": "Settlements cleared",
		"base.ui.modal.architect_view": "Read-only Architect view at the door · M returns to Mayor · voxel editing uses the workshop editor",
		"base.ui.modal.no_terrain": "No terrain at the target",
		"base.ui.modal.terrain_edit_failed": "Terrain edit failed: {code}",
		"base.ui.modal.terrain_edit": "Terrain {height} m at ({x}, {z})",
		"base.ui.modal.road_mode_help": "Road mode: click start, then end · 8 m wide · Esc cancels",
		"base.ui.modal.road_start": "Road start ({x}, {z}) · click the end",
		"base.ui.modal.road_zero_length": "Road needs a non-zero length",
		"base.ui.modal.road_added": "Road {length} m at ({x1}, {z1})..({x2}, {z2})",
		"base.ui.modal.road_rejected": "Road rejected: {code}",
		"base.ui.modal.research_none": "Research: nothing is affordable yet",
		"base.ui.modal.researched": "Researched {name}",
		"base.ui.modal.research_failed": "Research failed: {error}",

		"base.ui.help.move": "WASD pan · Q/E rotate · wheel zoom · R rotate · X raise · Z lower · N found settlement",
		"base.ui.help.edit": "LMB place · RMB select · Enter: enter selected building",
		"base.ui.help.tools": "C crew · I industry · U utility · K services · B starter · T tax · L loan · Y research · F1 dashboard · F2 citizens · {follow} · F4 materialization · F6 district focus · F7 heatmap · {clock} · V +10k · F5 save · F9 load",
		"base.ui.help.clock": "F8 clock",
		"base.ui.help.architect": "First-person view at the selected door · M / Esc returns to Mayor",

		# Dashboard shell.
		"base.ui.dashboard.title": "City Dashboard",
		"base.ui.dashboard.close_hint": "F1 or Esc to close",
		"base.ui.dashboard.policy_hint": "P create · Shift+P remove · J construction · [ ] tax · , . funding",
		"base.ui.dashboard.section.population": "Population",
		"base.ui.dashboard.section.district": "District",
		"base.ui.dashboard.section.settlements": "Settlements",
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
		"base.ui.dashboard.section.policies": "Policies",
		"base.ui.dashboard.section.advisor": "Advisor",

		# Dashboard body.
		"base.ui.dashboard.body.population": "{residents} residents · {occupied} / {housing} housing · {vacant} vacant",
		"base.ui.dashboard.body.district": "{residents} residents · {housing} housing ({vacant} vacant)",
		"base.ui.dashboard.body.district_jobs": "{jobs} jobs · {employed} employed",
		"base.ui.dashboard.body.district_needs": "happiness {happiness} · {needs} · commute {commute} m",
		"base.ui.dashboard.body.district_hint": "Within {radius} m of the camera target · completed buildings only",
		"base.ui.dashboard.body.district_focus": "District focus: selected building",
		"base.ui.dashboard.body.district_focus_hint": "Within {radius} m of the selected building · completed buildings only",
		"base.ui.dashboard.body.district_focus_action_selection": "F6: focus the selected building",
		"base.ui.dashboard.body.district_focus_action_camera": "F6: back to camera focus",
		"base.ui.dashboard.body.district_unavailable": "District data unavailable.",
		"base.ui.dashboard.body.settlements_count": "{count} founded · selected: {selected}",
		"base.ui.dashboard.body.settlements_none": "No settlements founded.",
		"base.ui.dashboard.body.settlements_unavailable": "Settlement data unavailable.",
		"base.ui.dashboard.body.settlement_none": "none",
		"base.ui.dashboard.body.settlement_row": "{id}. {name} · {owner} · centre {x}, {z} · radius {radius} m",
		"base.ui.dashboard.body.settlement_row_selected": "▸ {id}. {name} · {owner} · centre {x}, {z} · radius {radius} m · selected",
		"base.ui.dashboard.body.settlement_focus": "Settlement focus: {name}",
		"base.ui.dashboard.body.settlement_focus_hint": "Within {radius} m of the settlement centre · completed buildings only",
		"base.ui.dashboard.body.settlement_hint": "PageUp / PageDown: cycle settlements, including none",
		"base.ui.dashboard.body.settlement_detail_title": "Settlement detail",
		"base.ui.dashboard.body.settlement_detail": "{name} · owner {owner} · id {id} · centre {x}, {z} · radius {radius} m",
		"base.ui.dashboard.body.settlement_detail_district": "Within {radius} m · {residents} residents · {housing} housing ({vacant} vacant)",
		"base.ui.dashboard.body.settlement_detail_jobs": "{jobs} jobs · {employed} employed · {unemployed} unemployed",
		"base.ui.dashboard.body.settlement_detail_buildings": "completed buildings {completed} · construction sites {sites}",
		"base.ui.dashboard.body.settlement_detail_policy": "Policy at the centre: {name} · tax ×{tax} · funding {funding}",
		"base.ui.dashboard.body.settlement_detail_policy_none": "No containing policy at the settlement centre.",
		"base.ui.dashboard.body.settlement_members_more": "+{count} more members",
		"base.ui.dashboard.body.policies_count": "Policies registered: {count}",
		"base.ui.dashboard.body.policy": "{name} · tax ×{tax} · funding {funding}",
		"base.ui.dashboard.body.policy_construction_allowed": "construction allowed",
		"base.ui.dashboard.body.policy_construction_blocked": "construction blocked",
		"base.ui.dashboard.body.policy_allowlist": "allowlist: {count}",
		"base.ui.dashboard.body.policy_none": "No policy covers this point.",
		"base.ui.dashboard.body.policies_unavailable": "Policy data unavailable.",
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

		# Settlement tax control (Shift+T). Only the sentence is translated;
		# bridge error codes, ids, and checksums stay verbatim parameters.
		"base.ui.dashboard.settlement_tax.projection": "Projected local multiplier: x{multiplier} (preview, D-039 proposed)",
		"base.ui.dashboard.settlement_tax.success": "Settlement tax: {rate}% · checksum {checksum}",
		"base.ui.dashboard.settlement_tax.unowned": "settlement.unowned",
		"base.ui.dashboard.settlement_tax.failed": "{code}: {message}",

		# Settlement claim control (Shift+C). Only the sentence is translated;
		# bridge error codes, ids, player ids, and checksums stay verbatim.
		"base.ui.dashboard.settlement_claim.success": "Claimed settlement #{id} as player #{player} · checksum {checksum}",
		"base.ui.dashboard.settlement_claim.already_owned": "Settlement #{id} is already owned by player #{player}",
		"base.ui.dashboard.settlement_claim.failed": "{code}: {message}",
		"base.ui.dashboard.settlement_claim.register_failed": "Player registration failed: {code}: {message}",

		# Settlement transfer control (Shift+Y). Only the sentence is
		# translated; bridge error codes, ids, player ids, and revisions stay
		# verbatim parameters.
		"base.ui.dashboard.settlement_transfer.success": "Transferred settlement #{id}: player #{from} → player #{to} · revision {revision}",
		"base.ui.dashboard.settlement_transfer.failed": "{code}: {message}",
		"base.ui.dashboard.settlement_transfer.unowned": "Settlement #{id} is unowned; claim it before transferring",
		"base.ui.dashboard.settlement_transfer.no_recipient": "No local transfer recipient for settlement #{id}",

		# Settlement release control (Shift+X, two presses). Only the
		# sentence is translated; bridge codes/ids/revisions stay parameters.
		"base.ui.dashboard.settlement_release.armed": "Press Shift+X again to release #{id}",
		"base.ui.dashboard.settlement_release.success": "Released settlement #{id} · revision {revision}",
		"base.ui.dashboard.settlement_release.unowned": "Settlement #{id} is unowned; nothing to release",
		"base.ui.dashboard.settlement_release.unavailable": "Settlement #{id} is no longer available",
		"base.ui.dashboard.settlement_release.failed": "{code}: {message}",

		# Settlement rename overlay (Shift+R). Only the sentences are
		# translated; codes, names, ids, revisions, and checksums stay verbatim.
		"base.ui.dashboard.settlement_rename.title": "Rename {name}",
		"base.ui.dashboard.settlement_rename.hint": "Enter renames · Esc cancels",
		"base.ui.dashboard.settlement_rename.empty": "A settlement name cannot be empty.",
		"base.ui.dashboard.settlement_rename.unchanged": "That is already the settlement name.",
		"base.ui.dashboard.settlement_rename.success": "Renamed settlement #{id} · revision {revision} · checksum {checksum}",
		"base.ui.dashboard.settlement_rename.rejected": "{code}: {message}",

		# Settlement membership block and controls (Shift+I/Shift+M/Shift+Delete).
		# Role names and permission tokens are localized; ids, codes, and
		# revisions stay verbatim parameters.
		"base.ui.dashboard.settlement_members.unowned": "Members — · revision {revision}",
		"base.ui.dashboard.settlement_members.unowned_action": "Settlement #{id} is unowned; claim it before managing members",
		"base.ui.dashboard.settlement_members.summary": "Owner #{owner} · {count} members · revision {revision} · tax {tax}%",
		"base.ui.dashboard.settlement_members.summary_named": "Owner #{owner} {name} · {count} members · revision {revision} · tax {tax}%",
		"base.ui.dashboard.settlement_members.empty": "No members yet.",
		"base.ui.dashboard.settlement_members.row": "#{id} {role} · {permissions}",
		"base.ui.dashboard.settlement_members.row_named": "#{id} {name} · {role} · {permissions}",
		"base.ui.dashboard.settlement_members.no_permissions": "no permissions",
		"base.ui.dashboard.settlement_members.permission.build": "build",
		"base.ui.dashboard.settlement_members.permission.manage": "manage",
		"base.ui.dashboard.settlement_members.permission.tax": "tax",
		"base.ui.dashboard.settlement_members.permission.members": "members",
		"base.ui.dashboard.settlement_members.hint_invite": "Shift+I: invite the next local player as member",
		"base.ui.dashboard.settlement_members.hint_role": "Shift+M: cycle the lowest member's role",
		"base.ui.dashboard.settlement_members.hint_remove": "Shift+Delete twice: remove the lowest member",
		"base.ui.dashboard.settlement_members.no_eligible": "No unlisted local player is available to invite.",
		"base.ui.dashboard.settlement_members.no_member": "Settlement #{id} has no non-owner member to change.",
		"base.ui.dashboard.settlement_members.remove_armed": "Press Shift+Delete again to remove player #{player}",
		"base.ui.dashboard.settlement_members.remove_armed_named": "Press Shift+Delete again to remove player #{player} {name}",
		"base.ui.dashboard.settlement_members.invite_success": "Invited player #{player} as {role} · revision {revision}",
		"base.ui.dashboard.settlement_members.invite_success_named": "Invited player #{player} {name} as {role} · revision {revision}",
		"base.ui.dashboard.settlement_members.role_success": "Player #{player} is now {role} · revision {revision}",
		"base.ui.dashboard.settlement_members.role_success_named": "Player #{player} {name} is now {role} · revision {revision}",
		"base.ui.dashboard.settlement_members.remove_success": "Removed player #{player} · revision {revision}",
		"base.ui.dashboard.settlement_members.remove_success_named": "Removed player #{player} {name} · revision {revision}",
		"base.ui.dashboard.settlement_members.failed": "{code}: {message}",
		"base.ui.dashboard.settlement_members.register_failed": "Player registration failed: {code}: {message}",

		# Compact dashboard policy editing. Bridge error codes, ids, and
		# policy names stay verbatim parameters.
		"base.ui.policies.created": "Policy {id} created: {name}",
		"base.ui.policies.updated": "Policy {id} updated",
		"base.ui.policies.removed": "Policy {id} removed",
		"base.ui.policies.no_target": "No policy covers the district centre. P creates one.",
		"base.ui.policies.failed": "Policy change failed: {code}",

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

		"base.advisor.city.bankrupt.detail": "Placement and borrowing are blocked. Raise taxes or repay debt before building.",
		"base.advisor.city.budget_deficit.detail": "The city is losing {net} per day.",
		"base.advisor.city.clinic_shortage.detail": "Clinic coverage is {supply}.",
		"base.advisor.city.construction_active.detail": "{active} construction sites are active.",
		"base.advisor.city.construction_stalled.detail": "{stalled} construction sites are stalled.",
		"base.advisor.city.debt.detail": "Outstanding debt is {debt}.",
		"base.advisor.city.fire_shortage.detail": "Fire coverage is {supply}.",
		"base.advisor.city.food_shortage.detail": "Food supply is {supply}.",
		"base.advisor.city.healthy.detail": "Needs, services, employment, and cash flow are all in good shape.",
		"base.advisor.city.long_commutes.detail": "Average commute is {commute} m.",
		"base.advisor.city.no_residents.detail": "{housing} housing units are vacant.",
		"base.advisor.city.no_roads.detail": "{population} residents commute without a road network.",
		"base.advisor.city.police_shortage.detail": "Police coverage is {supply}.",
		"base.advisor.city.power_shortage.detail": "Power supply is {supply}.",
		"base.advisor.city.quiet.detail": "Place a blueprint or let the simulation run to generate advice.",
		"base.advisor.city.school_shortage.detail": "School coverage is {supply}.",
		"base.advisor.city.services_healthy.detail": "Food, water, power, and civic services are meeting demand.",
		"base.advisor.city.traffic_disconnected.detail": "The road network has {components} components.",
		"base.advisor.city.unemployment.detail": "{unemployed} residents are unemployed; employment is {employment}.",
		"base.advisor.city.unhappy.detail": "Happiness is {happiness}.",
		"base.advisor.city.vacant_homes.detail": "{vacancies} of {housing} homes are vacant.",
		"base.advisor.city.water_shortage.detail": "Water supply is {supply}.",


		# Follow camera.
		"base.ui.citizen.follow_hint": "F3 follows the selected home · Esc stops",
		"base.ui.citizen.following": "Following home #{id} · Esc stops",
		"base.ui.citizen.follow_started": "Following home #{id} · F3 or Esc stops",
		"base.ui.citizen.follow_stopped": "Follow stopped",
		"base.ui.citizen.follow_select_first": "Select a completed home first (RMB)",
		"base.ui.help.follow": "F3 follow home",

		# Standalone settlement ownership editor (developer scene). Settlement
		# names, ids, role strings, and bridge error codes stay verbatim.
		"base.ui.settlement_editor.title": "Settlement ownership editor",
		"base.ui.settlement_editor.subtitle": "Developer bridge tool (docs/48 §2.2). In-memory only.",
		"base.ui.settlement_editor.seed": "Seed {seed}",
		"base.ui.settlement_editor.button.fixture": "Fixture",
		"base.ui.settlement_editor.button.refresh": "Refresh",
		"base.ui.settlement_editor.button.new_session": "New session (N)",
		"base.ui.settlement_editor.state": "Seed {seed} · {count} settlements · checksum {checksum}",
		"base.ui.settlement_editor.selected_record": "Selected record",
		"base.ui.settlement_editor.no_selection": "No settlement selected",
		"base.ui.settlement_editor.detail_empty": "No settlement selected.\nPick a row to inspect it.",
		"base.ui.settlement_editor.form.selected": "Selected #{id}",
		"base.ui.settlement_editor.label.name": "Name",
		"base.ui.settlement_editor.label.center_x": "Centre X",
		"base.ui.settlement_editor.label.center_z": "Centre Z",
		"base.ui.settlement_editor.label.radius_m": "Radius m",
		"base.ui.settlement_editor.label.player_id": "Player id",
		"base.ui.settlement_editor.label.role": "Role",
		"base.ui.settlement_editor.label.tax_percent": "Tax %",
		"base.ui.settlement_editor.action.found": "Found",
		"base.ui.settlement_editor.action.claim": "Claim",
		"base.ui.settlement_editor.action.rename": "Rename",
		"base.ui.settlement_editor.action.transfer": "Transfer",
		"base.ui.settlement_editor.action.invite": "Invite",
		"base.ui.settlement_editor.action.set_role": "Set role",
		"base.ui.settlement_editor.action.remove_member": "Remove member",
		"base.ui.settlement_editor.action.set_tax": "Set tax",
		"base.ui.settlement_editor.action.list": "List",
		"base.ui.settlement_editor.note": "Claim needs an explicit player id. Other actions use the current owner.",
		"base.ui.settlement_editor.ready": "Ready.",
		"base.ui.settlement_editor.help": "1–9 pick · F found · C claim · R name · T move\nI invite · P role · X drop · M tax · N reset",
		"base.ui.settlement_editor.footer": "Local bridge only · never contacts a server · errors never abort the tool.",
		"base.ui.settlement_editor.message.new_session": "New empty settlement session for seed {seed}.",
		"base.ui.settlement_editor.message.refreshed": "List refreshed.",
		"base.ui.settlement_editor.message.ok": "{context}: ok",
		"base.ui.settlement_editor.message.failed": "{context} failed: {code} ({message})",
		"base.ui.settlement_editor.message.no_core": "{context} failed: no_core (bridge not loaded)",
		"base.ui.settlement_editor.message.missing": "{context} failed: settlement.unknown (no record #{id})",
		"base.ui.settlement_editor.message.no_selection": "{context} failed: no_selection (select a settlement first)",
		"base.ui.settlement_editor.row": "#{id}  {name}  ·  {owner}  ·  rev {revision}  ·  tax {tax}%  ·  members {members}",
		"base.ui.settlement_editor.detail.identity": "id {id} · owned {owned} · owner id {owner} · owner label {label}",
		"base.ui.settlement_editor.detail.center": "center {center} · radius {radius} m · tax {tax}%",
		"base.ui.settlement_editor.detail.revision": "membership revision {revision} · checksum {checksum}",
		"base.ui.settlement_editor.detail.members": "members: {members}",
		"base.ui.settlement_editor.owner.unowned": "unowned",
		"base.ui.settlement_editor.members.none": "none",
		"base.ui.settlement_editor.owned.yes": "yes",
		"base.ui.settlement_editor.owned.no": "no",
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
		"base.ui.hud.construction": "Bau: {active} aktiv / {stalled} stockend   Crew: {crew}/{capacity}   Gewerke: {trades}   Material: {materials} L   Speichern: {save}   {clock}",
		"base.ui.hud.clock_developer": "Uhr: Entwickler",
		"base.ui.hud.clock_campaign": "Uhr: Kampagne · {seconds} s/Tag · Tag {day}",

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
		"base.ui.hud.citizens_near": "Bürger in der Nähe: {active} aktiv · {detailed} detailliert · {near} nah · {far} fern",
		"base.ui.heatmap.status": "Stadtteil-Heatmap: {state} · {cells} Zellen · Spitze {peak}",
		"base.ui.heatmap.on": "an",
		"base.ui.heatmap.off": "aus",
		"base.ui.heatmap.metric_line": "Kennzahl: {metric} · Spitze {peak} · Umschalt+F7 wechselt",
		"base.ui.heatmap.metric.population": "Bevölkerung",
		"base.ui.heatmap.metric.jobs": "Arbeitsplätze",
		"base.ui.heatmap.metric.happiness": "Zufriedenheit",
		"base.ui.heatmap.legend": "{metric} · Niedrig → Hoch · dunkles Violett bis helles Gelb · 512 m Radius · 32 m Zellen",
		"base.ui.heatmap.empty": "Keine Daten im Umkreis",
		"base.ui.heatmap.unavailable": "Stadtteil-Heatmap nicht verfügbar: {code}",

		# District overview (developer panel) settlement member block.
		"base.ui.district.members.unowned": "Ohne Eigentümer",
		"base.ui.district.members.owner": "Eigentümer #{id} · {role}",
		"base.ui.district.members.owner_named": "Eigentümer {name} (#{id}) · {role}",
		"base.ui.district.members.row": "#{id} {role} · {permissions}",
		"base.ui.district.members.row_named": "{name} (#{id}) {role} · {permissions}",
		"base.ui.district.members.no_permissions": "keine Berechtigungen",
		"base.ui.district.members.permission.build": "bauen",
		"base.ui.district.members.permission.manage": "verwalten",
		"base.ui.district.members.permission.tax": "Steuern",
		"base.ui.district.members.permission.members": "Mitglieder",
		# Read-only settlement screen (docs/85 §7). Namen, IDs und Codes bleiben
		# roh; Rollen und Berechtigungen nutzen lokalisierte Anzeigenamen.
		"base.ui.settlement_screen.title": "Siedlungen",
		"base.ui.settlement_screen.authority": "Autorität: {mode}",
		"base.ui.settlement_screen.authority_local": "Lokal",
		"base.ui.settlement_screen.authority_connected": "Verbunden",
		"base.ui.settlement_screen.world_checksum": "Weltprüfsumme: {checksum}",
		"base.ui.settlement_screen.connected_status": "Verbunden mit {host}:{port} · Spieler {name}",
		"base.ui.settlement_screen.page": "Seite {page} von {pages} · {count} von {total}",
		"base.ui.settlement_screen.row": "{name}  ·  #{id}  ·  {owner}  ·  Steuer {tax}%  ·  Rev. {revision}  ·  {members} Mitglieder",
		"base.ui.settlement_screen.owner": "{name} #{id}",
		"base.ui.settlement_screen.owner_unknown": "#{id}",
		"base.ui.settlement_screen.owner_unowned": "herrenlos",
		"base.ui.settlement_screen.owner_label": "Eigentümer: {owner}",
		"base.ui.settlement_screen.person": "{name} #{id}",
		"base.ui.settlement_screen.person_unknown": "#{id}",
		"base.ui.settlement_screen.member": "{person} · {role} · {permissions}",
		"base.ui.settlement_screen.detail_title": "{name} (#{id})",
		"base.ui.settlement_screen.detail_center": "Zentrum ({x}, {z}) · Radius {radius} m",
		"base.ui.settlement_screen.detail_tax": "Steuer: {tax}%",
		"base.ui.settlement_screen.detail_revision": "Mitgliedschaftsrevision: {revision}",
		"base.ui.settlement_screen.detail_members": "Mitglieder ({count})",
		"base.ui.settlement_screen.detail_no_members": "Keine Mitglieder.",
		"base.ui.settlement_screen.detail_empty": "Wähle eine Siedlung zur Ansicht.",
		"base.ui.settlement_screen.footer_more": "+{count} weitere",
		"base.ui.settlement_screen.footer_end": "Ende der Liste",
		"base.ui.settlement_screen.help": "Seiten: Bild↑/↓ · Zeilen: ↑/↓ oder 1–8 · Aktionen: C/N/T/Y/I/M/Entf/X",
		"base.ui.settlement_screen.help_connected": "Seiten: Bild↑/↓ · Zeilen: ↑/↓ oder 1–8 · Verbunden: N umbenennen · T Steuern",
		"base.ui.settlement_screen.empty": "Keine Siedlungen in dieser Welt.",
		"base.ui.settlement_screen.unavailable": "Siedlungen nicht verfügbar ({code}).",
		"base.ui.settlement_screen.projection": "Prognostizierter lokaler Multiplikator: x{multiplier} (Vorschau, D-039 vorgeschlagen)",
		"base.ui.settlement_screen.projection_unavailable": "Prognostizierter lokaler Multiplikator: nicht verfügbar ({code})",
		"base.ui.settlement_screen.projection_connected": "Prognostizierter lokaler Multiplikator: nicht verfügbar (verbundener Modus)",
		"base.ui.settlement_screen.permissions_none": "keine Berechtigungen",
		"base.ui.settlement_screen.permission.build": "bauen",
		"base.ui.settlement_screen.permission.manage": "verwalten",
		"base.ui.settlement_screen.permission.tax": "Steuern",
		"base.ui.settlement_screen.permission.members": "Mitglieder",
		"base.ui.settlement_screen.role.owner": "Eigentümer",
		"base.ui.settlement_screen.role.manager": "Verwalter",
		"base.ui.settlement_screen.role.builder": "Bauleiter",
		"base.ui.settlement_screen.role.member": "Mitglied",
		"base.ui.settlement_screen.action.claim": "Beanspruchen",
		"base.ui.settlement_screen.action.rename": "Umbenennen",
		"base.ui.settlement_screen.action.tax": "Steuer +5%",
		"base.ui.settlement_screen.action.transfer": "Übertragen",
		"base.ui.settlement_screen.action.invite": "Einladen",
		"base.ui.settlement_screen.action.cycle_role": "Rolle wechseln",
		"base.ui.settlement_screen.action.remove_member": "Mitglied entfernen",
		"base.ui.settlement_screen.action.release": "Freigeben",
		"base.ui.settlement_screen.action.button": "{key} {action}",
		"base.ui.settlement_screen.action.button_disabled": "{key} {action} · {reason}",
		"base.ui.settlement_screen.action.result_ok": "{action} angenommen (Revision {revision})",
		"base.ui.settlement_screen.action.result_failed": "{action} abgelehnt: {code} (Revision {revision})",
		"base.ui.settlement_screen.action.confirm_remove": "Erneut Entf drücken, um {person} zu entfernen.",
		"base.ui.settlement_screen.action.confirm_release": "Erneut X drücken, um {name} #{id} freizugeben.",
		"base.ui.settlement_screen.action.cancelled": "Aktion abgebrochen.",
		"base.ui.settlement_screen.rename.title": "Siedlung #{id} umbenennen",
		"base.ui.settlement_screen.rename.count": "{count}/48 Zeichen",
		"base.ui.settlement_screen.rename.save": "Speichern",
		"base.ui.settlement_screen.rename.cancel": "Abbrechen",
		"base.ui.settlement_screen.rename.empty": "Der Name darf nicht leer sein.",
		"base.ui.settlement_screen.rename.too_long": "Der Name darf höchstens 48 Zeichen haben.",
		"base.ui.settlement_screen.rename.unchanged": "Der Name ist unverändert.",
		"base.ui.settlement_screen.rename.invalid": "Der Name enthält Steuerzeichen.",
		"base.ui.settlement_screen.rename.error": "{reason} ({code})",
		"base.ui.settlement_screen.rename.rejected": "Umbenennen fehlgeschlagen: {reason}",
		"base.ui.settlement_screen.paging.prev": "◀ Zurück",
		"base.ui.settlement_screen.paging.next": "Weiter ▶",
		"base.ui.settlement.default_name": "Siedlung {index}",
		"base.ui.settlement_screen.reason.no_core": "lokaler Core nicht verfügbar",
		"base.ui.settlement_screen.reason.no_selection": "Siedlung auswählen",
		"base.ui.settlement_screen.reason.settlement.already_owned": "bereits im Besitz",
		"base.ui.settlement_screen.reason.settlement.unowned": "herrenlos",
		"base.ui.settlement_screen.reason.settlement.owner_required": "Eigentümer kann nicht entfernt werden",
		"base.ui.settlement_screen.reason.settlement.unknown": "Siedlung nicht gefunden",
		"base.ui.settlement_screen.reason.no_player": "kein registrierter Spieler",
		"base.ui.settlement_screen.reason.no_recipient": "kein anderer registrierter Spieler",
		"base.ui.settlement_screen.reason.no_invitee": "kein geeigneter Spieler zum Einladen",
		"base.ui.settlement_screen.reason.no_member": "Mitglied auswählen",
		"base.ui.settlement_screen.reason.connected_unavailable": "Aktion im verbundenen Modus noch nicht verfügbar",
		"base.ui.hud.lifecycle": "Lebenszyklus: Tag {day} · {aged} gealtert · {died} gestorben",
		"base.ui.hud.selected_owner": "  ·  Eigentümer {owner}",
		"base.ui.hud.selected_industry": "  ·  Industrie {kind}",
		"base.ui.hud.selected_utility": "  ·  Versorgung {kinds}",
		"base.ui.hud.selected_service": "  ·  Dienst {kind}",
		"base.ui.hud.selected_stories": "  ·  {system} {stories} Geschosse",
		"base.ui.hud.selected_residents": "  ·  Bewohner {names}",
		"base.ui.hud.wilderness": "Wildnis",

		# Entwickler-Chatpanel. Spielernamen und Nachrichtentexte bleiben roh.
		"base.ui.chat.title": "Chat (Entwicklerpanel)",
		"base.ui.chat.placeholder": "Nachricht (1–{max} Zeichen)",
		"base.ui.chat.send": "Senden",
		"base.ui.chat.help": "Enter: senden · Esc: Entwurf/Fehler löschen · Strg+L: Entwurf fokussieren",
		"base.ui.chat.empty_log": "Noch keine Nachrichten.",
		"base.ui.chat.char_counter": "{count}/{max} Zeichen",
		"base.ui.chat.status_signed_in": "Angemeldet als {name} (#{id})",
		"base.ui.chat.status_no_identity": "Keine Identität (set_identity)",
		"base.ui.chat.error.line": "Fehler: {message} ({code})",
		"base.ui.chat.error.unknown": "Unerwarteter Chatfehler.",
		"base.ui.chat.error.invalid_fallback": "Ungültige Chat-Nachricht.",
		"base.ui.chat.error.empty": "Die Chat-Nachricht ist leer.",
		"base.ui.chat.error.too_long": "Die Chat-Nachricht ist zu lang (maximal {max} Zeichen).",
		"base.ui.chat.error.invalid": "Die Chat-Nachricht enthält ein Steuerzeichen.",
		"base.ui.chat.error.identity": "Vor dem Senden von Chat anmelden.",
		"base.ui.chat.error.malformed_sent": "chat_sent-Nutzlast hat einen fehlerhaften Eintrag",
		"base.ui.chat.error.malformed_log": "chat_log-Nutzlast hat keine Eintragsliste",
		"base.ui.chat.error.malformed_entries": "chat_log-Nutzlast hat fehlerhafte Einträge",

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

		# Modalmeldungen: seltene Weltaktionen. Fehler-/Inhalts-IDs und
		# generierte Namen bleiben rohe Parameter; nur der Satz ist übersetzt.
		"base.ui.modal.blueprint_failed": "Bauplan fehlgeschlagen: {path}",
		"base.ui.modal.blueprint_rejected": "Bauplan abgelehnt: {code}",
		"base.ui.modal.blueprint_mesh_empty": "Bauplan-Netz ist leer",
		"base.ui.modal.blueprint_bounds_invalid": "Bauplan-Grenzen sind ungültig",
		"base.ui.modal.lot_corner_cleared": "Grundstücksecke gelöscht",
		"base.ui.modal.lot_mode_help": "Grundstücksmodus: zwei Ecken anklicken · 1/2/3 Zone · Esc bricht ab",
		"base.ui.modal.lot_zone": "Grundstückszone: {zone}",
		"base.ui.modal.lot_corner": "Grundstücksecke ({x}, {z}) · gegenüberliegende Ecke anklicken",
		"base.ui.modal.lot_min_size": "Grundstück braucht mindestens 4 × 4 m",
		"base.ui.modal.lot_rejected": "Grundstück abgelehnt: {code}",
		"base.ui.modal.starter_catalog_failed": "Startkatalog fehlgeschlagen: {code}",
		"base.ui.modal.starter_none": "Keine Startgebäude in diesem Zeitalter",
		"base.ui.modal.starter_failed": "Startgebäude fehlgeschlagen: {code}",
		"base.ui.modal.starter_no_path": "Startgebäude hat keinen Paketpfad",
		"base.ui.modal.starter_package_rejected": "Startpaket abgelehnt",
		"base.ui.modal.starter_selected": "Startgebäude: {name} ({index}/{total})",
		"base.ui.modal.industry_removed": "Industrie entfernt",
		"base.ui.modal.industry_failed": "Industrie fehlgeschlagen: {code}",
		"base.ui.modal.industry_selected": "Industrie: {kind}",
		"base.ui.modal.service_removed": "Bürgerdienst entfernt",
		"base.ui.modal.service_failed": "Bürgerdienst fehlgeschlagen: {code}",
		"base.ui.modal.service_selected": "Bürgerdienst: {kind}",
		"base.ui.modal.utility_failed": "Versorgung fehlgeschlagen: {code}",
		"base.ui.modal.utility_water": "Versorgung: Wasser",
		"base.ui.modal.utility_water_power": "Versorgung: Wasser + Strom",
		"base.ui.modal.utility_removed": "Versorgung entfernt",
		"base.ui.modal.tax_failed": "Steueränderung fehlgeschlagen: {code}",
		"base.ui.modal.tax_policy": "Steuerpolitik: {name}",
		"base.ui.modal.loan_rejected": "Kredit abgelehnt: {code}",
		"base.ui.modal.loan_taken": "Geliehen {amount} · Schulden {debt}",
		"base.ui.modal.settlement_rejected": "Siedlung abgelehnt: {code}",
		"base.ui.modal.settlement_founded": "{name} gegründet",
		"base.ui.modal.settlements_cleared": "Siedlungen gelöscht",
		"base.ui.modal.architect_view": "Schreibgeschützte Architektenansicht an der Tür · M kehrt zum Bürgermeistermodus zurück · Voxelbearbeitung nutzt den Workshop-Editor",
		"base.ui.modal.no_terrain": "Kein Gelände am Ziel",
		"base.ui.modal.terrain_edit_failed": "Geländebearbeitung fehlgeschlagen: {code}",
		"base.ui.modal.terrain_edit": "Gelände {height} m bei ({x}, {z})",
		"base.ui.modal.road_mode_help": "Straßenmodus: Start und Ende anklicken · 8 m breit · Esc bricht ab",
		"base.ui.modal.road_start": "Straßenstart ({x}, {z}) · Ende anklicken",
		"base.ui.modal.road_zero_length": "Straße braucht eine Länge ungleich null",
		"base.ui.modal.road_added": "Straße {length} m bei ({x1}, {z1})..({x2}, {z2})",
		"base.ui.modal.road_rejected": "Straße abgelehnt: {code}",
		"base.ui.modal.research_none": "Forschung: noch nichts bezahlbar",
		"base.ui.modal.researched": "{name} erforscht",
		"base.ui.modal.research_failed": "Forschung fehlgeschlagen: {error}",

		"base.ui.help.move": "WASD: schwenken · Q/E: drehen · Mausrad: zoomen · R: drehen · X: anheben · Z: absenken · N: Siedlung gründen",
		"base.ui.help.edit": "LMB: platzieren · RMB: auswählen · Enter: ausgewähltes Gebäude betreten",
		"base.ui.help.tools": "C: Crew · I: Industrie · U: Versorgung · K: Dienste · B: Startpaket · T: Steuern · L: Kredit · Y: Forschung · F1: Dashboard · F2: Bürger · {follow} · F4: Materialisierung · F6: Stadtteilfokus · F7: Heatmap · {clock} · V: +10k · F5: speichern · F9: laden",
		"base.ui.help.clock": "F8: Uhr",
		"base.ui.help.architect": "Ego-Ansicht an der ausgewählten Tür · M / Esc kehrt zum Bürgermeistermodus zurück",

		# Dashboard-Hülle.
		"base.ui.dashboard.title": "Stadt-Dashboard",
		"base.ui.dashboard.close_hint": "F1 oder Esc zum Schließen",
		"base.ui.dashboard.policy_hint": "P: anlegen · Umschalt+P: entfernen · J: Bau · [ ]: Steuer · , .: Förderung",
		"base.ui.dashboard.section.population": "Bevölkerung",
		"base.ui.dashboard.section.district": "Stadtteil",
		"base.ui.dashboard.section.settlements": "Siedlungen",
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
		"base.ui.dashboard.section.policies": "Richtlinien",
		"base.ui.dashboard.section.advisor": "Berater",

		# Dashboard-Körper.
		"base.ui.dashboard.body.population": "{residents} Bewohner · {occupied} / {housing} Wohnraum · {vacant} frei",
		"base.ui.dashboard.body.district": "{residents} Bewohner · {housing} Wohnungen ({vacant} frei)",
		"base.ui.dashboard.body.district_jobs": "{jobs} Arbeitsplätze · {employed} beschäftigt",
		"base.ui.dashboard.body.district_needs": "Zufriedenheit {happiness} · {needs} · Arbeitsweg {commute} m",
		"base.ui.dashboard.body.district_hint": "Im Umkreis von {radius} m um das Kameraziel · nur fertige Gebäude",
		"base.ui.dashboard.body.district_focus": "Stadtteilfokus: ausgewähltes Gebäude",
		"base.ui.dashboard.body.district_focus_hint": "Im Umkreis von {radius} m um das ausgewählte Gebäude · nur fertige Gebäude",
		"base.ui.dashboard.body.district_focus_action_selection": "F6: ausgewähltes Gebäude fokussieren",
		"base.ui.dashboard.body.district_focus_action_camera": "F6: zurück zum Kamera-Fokus",
		"base.ui.dashboard.body.district_unavailable": "Stadtteildaten nicht verfügbar.",
		"base.ui.dashboard.body.settlements_count": "{count} gegründet · ausgewählt: {selected}",
		"base.ui.dashboard.body.settlements_none": "Keine Siedlungen gegründet.",
		"base.ui.dashboard.body.settlements_unavailable": "Siedlungsdaten nicht verfügbar.",
		"base.ui.dashboard.body.settlement_none": "keine",
		"base.ui.dashboard.body.settlement_row": "{id}. {name} · {owner} · Zentrum {x}, {z} · Radius {radius} m",
		"base.ui.dashboard.body.settlement_row_selected": "▸ {id}. {name} · {owner} · Zentrum {x}, {z} · Radius {radius} m · ausgewählt",
		"base.ui.dashboard.body.settlement_focus": "Siedlungsfokus: {name}",
		"base.ui.dashboard.body.settlement_focus_hint": "Im Umkreis von {radius} m um das Siedlungszentrum · nur fertige Gebäude",
		"base.ui.dashboard.body.settlement_hint": "Bild auf / Bild ab: Siedlungen wechseln, inklusive keine",
		"base.ui.dashboard.body.settlement_detail_title": "Siedlungsdetail",
		"base.ui.dashboard.body.settlement_detail": "{name} · Eigentümer {owner} · ID {id} · Zentrum {x}, {z} · Radius {radius} m",
		"base.ui.dashboard.body.settlement_detail_district": "Im Umkreis von {radius} m · {residents} Bewohner · {housing} Wohnungen ({vacant} frei)",
		"base.ui.dashboard.body.settlement_detail_jobs": "{jobs} Arbeitsplätze · {employed} beschäftigt · {unemployed} arbeitslos",
		"base.ui.dashboard.body.settlement_detail_buildings": "{completed} fertige Gebäude · {sites} Baustellen",
		"base.ui.dashboard.body.settlement_detail_policy": "Richtlinie am Zentrum: {name} · Steuer ×{tax} · Förderung {funding}",
		"base.ui.dashboard.body.settlement_detail_policy_none": "Keine Richtlinie am Siedlungszentrum.",
		"base.ui.dashboard.body.settlement_members_more": "+{count} weitere Mitglieder",
		"base.ui.dashboard.body.policies_count": "Registrierte Richtlinien: {count}",
		"base.ui.dashboard.body.policy": "{name} · Steuer ×{tax} · Förderung {funding}",
		"base.ui.dashboard.body.policy_construction_allowed": "Bau erlaubt",
		"base.ui.dashboard.body.policy_construction_blocked": "Bau gesperrt",
		"base.ui.dashboard.body.policy_allowlist": "Positivliste: {count}",
		"base.ui.dashboard.body.policy_none": "Keine Richtlinie deckt diesen Punkt ab.",
		"base.ui.dashboard.body.policies_unavailable": "Richtliniendaten nicht verfügbar.",
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

		# Siedlungssteuerung (Umschalt+T). Nur der Satz ist übersetzt;
		# Bridge-Fehlercodes, IDs und Prüfsummen bleiben unveränderte Parameter.
		"base.ui.dashboard.settlement_tax.projection": "Voraussichtlicher lokaler Multiplikator: x{multiplier} (Vorschau, D-039 vorgeschlagen)",
		"base.ui.dashboard.settlement_tax.success": "Siedlungssteuer: {rate} % · Prüfsumme {checksum}",
		"base.ui.dashboard.settlement_tax.unowned": "Siedlung ohne Eigentümer (settlement.unowned)",
		"base.ui.dashboard.settlement_tax.failed": "{code}: {message}",

		# Siedlungsbeanspruchung (Umschalt+C). Nur der Satz ist übersetzt;
		# Bridge-Fehlercodes, IDs, Spieler-IDs und Prüfsummen bleiben Parameter.
		"base.ui.dashboard.settlement_claim.success": "Siedlung #{id} als Spieler #{player} beansprucht · Prüfsumme {checksum}",
		"base.ui.dashboard.settlement_claim.already_owned": "Siedlung #{id} gehört bereits Spieler #{player}",
		"base.ui.dashboard.settlement_claim.failed": "{code}: {message}",
		"base.ui.dashboard.settlement_claim.register_failed": "Spielerregistrierung fehlgeschlagen: {code}: {message}",

		# Siedlungsübertragung (Umschalt+Y). Nur der Satz ist übersetzt;
		# Bridge-Fehlercodes, IDs, Spieler-IDs und Revisionen bleiben Parameter.
		"base.ui.dashboard.settlement_transfer.success": "Siedlung #{id} übertragen: Spieler #{from} → Spieler #{to} · Revision {revision}",
		"base.ui.dashboard.settlement_transfer.failed": "{code}: {message}",
		"base.ui.dashboard.settlement_transfer.unowned": "Siedlung #{id} hat keinen Eigentümer; vor der Übertragung beanspruchen",
		"base.ui.dashboard.settlement_transfer.no_recipient": "Kein lokaler Empfänger für Siedlung #{id}",

		# Siedlungsfreigabe (Umschalt+X, zwei Tastenanschläge). Nur der Satz
		# ist übersetzt; Bridge-Codes/IDs/Revisionen bleiben Parameter.
		"base.ui.dashboard.settlement_release.armed": "Umschalt+X erneut drücken, um Siedlung #{id} freizugeben",
		"base.ui.dashboard.settlement_release.success": "Siedlung #{id} freigegeben · Revision {revision}",
		"base.ui.dashboard.settlement_release.unowned": "Siedlung #{id} hat keinen Eigentümer; nichts freizugeben",
		"base.ui.dashboard.settlement_release.unavailable": "Siedlung #{id} ist nicht mehr verfügbar",
		"base.ui.dashboard.settlement_release.failed": "{code}: {message}",

		# Umbenennungs-Overlay (Umschalt+R). Nur die Sätze sind übersetzt;
		# Codes, Namen, IDs, Revisionen und Prüfsummen bleiben Parameter.
		"base.ui.dashboard.settlement_rename.title": "{name} umbenennen",
		"base.ui.dashboard.settlement_rename.hint": "Eingabetaste benennt um · Esc bricht ab",
		"base.ui.dashboard.settlement_rename.empty": "Ein Siedlungsname darf nicht leer sein.",
		"base.ui.dashboard.settlement_rename.unchanged": "Das ist bereits der Name der Siedlung.",
		"base.ui.dashboard.settlement_rename.success": "Siedlung #{id} umbenannt · Revision {revision} · Prüfsumme {checksum}",
		"base.ui.dashboard.settlement_rename.rejected": "{code}: {message}",

		# Mitgliederblock und -steuerung (Umschalt+I/Umschalt+M/Umschalt+Entf).
		# Rollen und Berechtigungen sind übersetzt; IDs, Codes und Revisionen
		# bleiben unveränderte Parameter.
		"base.ui.dashboard.settlement_members.unowned": "Mitglieder — · Revision {revision}",
		"base.ui.dashboard.settlement_members.unowned_action": "Siedlung #{id} hat keinen Eigentümer; vor der Mitgliederverwaltung beanspruchen",
		"base.ui.dashboard.settlement_members.summary": "Eigentümer #{owner} · {count} Mitglieder · Revision {revision} · Steuer {tax} %",
		"base.ui.dashboard.settlement_members.summary_named": "Eigentümer #{owner} {name} · {count} Mitglieder · Revision {revision} · Steuer {tax} %",
		"base.ui.dashboard.settlement_members.empty": "Noch keine Mitglieder.",
		"base.ui.dashboard.settlement_members.row": "#{id} {role} · {permissions}",
		"base.ui.dashboard.settlement_members.row_named": "#{id} {name} · {role} · {permissions}",
		"base.ui.dashboard.settlement_members.no_permissions": "keine Berechtigungen",
		"base.ui.dashboard.settlement_members.permission.build": "bauen",
		"base.ui.dashboard.settlement_members.permission.manage": "verwalten",
		"base.ui.dashboard.settlement_members.permission.tax": "Steuern",
		"base.ui.dashboard.settlement_members.permission.members": "Mitglieder",
		"base.ui.dashboard.settlement_members.hint_invite": "Umschalt+I: nächsten lokalen Spieler als Mitglied einladen",
		"base.ui.dashboard.settlement_members.hint_role": "Umschalt+M: Rolle des niedrigsten Mitglieds wechseln",
		"base.ui.dashboard.settlement_members.hint_remove": "Umschalt+Entf zweimal: niedrigstes Mitglied entfernen",
		"base.ui.dashboard.settlement_members.no_eligible": "Kein nicht gelisteter lokaler Spieler zum Einladen verfügbar.",
		"base.ui.dashboard.settlement_members.no_member": "Siedlung #{id} hat kein Nicht-Eigentümer-Mitglied zum Ändern.",
		"base.ui.dashboard.settlement_members.remove_armed": "Umschalt+Entf erneut drücken, um Spieler #{player} zu entfernen",
		"base.ui.dashboard.settlement_members.remove_armed_named": "Umschalt+Entf erneut drücken, um Spieler #{player} {name} zu entfernen",
		"base.ui.dashboard.settlement_members.invite_success": "Spieler #{player} als {role} eingeladen · Revision {revision}",
		"base.ui.dashboard.settlement_members.invite_success_named": "Spieler #{player} {name} als {role} eingeladen · Revision {revision}",
		"base.ui.dashboard.settlement_members.role_success": "Spieler #{player} ist jetzt {role} · Revision {revision}",
		"base.ui.dashboard.settlement_members.role_success_named": "Spieler #{player} {name} ist jetzt {role} · Revision {revision}",
		"base.ui.dashboard.settlement_members.remove_success": "Spieler #{player} entfernt · Revision {revision}",
		"base.ui.dashboard.settlement_members.remove_success_named": "Spieler #{player} {name} entfernt · Revision {revision}",
		"base.ui.dashboard.settlement_members.failed": "{code}: {message}",
		"base.ui.dashboard.settlement_members.register_failed": "Spielerregistrierung fehlgeschlagen: {code}: {message}",

		# Kompakte Richtlinienbearbeitung im Dashboard. Fehlercodes, IDs und
		# Richtliniennamen bleiben unveränderte Parameter.
		"base.ui.policies.created": "Richtlinie {id} angelegt: {name}",
		"base.ui.policies.updated": "Richtlinie {id} aktualisiert",
		"base.ui.policies.removed": "Richtlinie {id} entfernt",
		"base.ui.policies.no_target": "Keine Richtlinie deckt das Stadtteilzentrum ab. P legt eine an.",
		"base.ui.policies.failed": "Richtlinienänderung fehlgeschlagen: {code}",

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

		"base.advisor.city.bankrupt.detail": "Platzieren und Leihen sind blockiert. Erhöhe Steuern oder zahle Schulden ab.",
		"base.advisor.city.budget_deficit.detail": "Die Stadt verliert {net} pro Tag.",
		"base.advisor.city.clinic_shortage.detail": "Die Klinikversorgung liegt bei {supply}.",
		"base.advisor.city.construction_active.detail": "{active} Baustellen sind aktiv.",
		"base.advisor.city.construction_stalled.detail": "{stalled} Baustellen stehen still.",
		"base.advisor.city.debt.detail": "Die Restschuld beträgt {debt}.",
		"base.advisor.city.fire_shortage.detail": "Die Feuerwehrversorgung liegt bei {supply}.",
		"base.advisor.city.food_shortage.detail": "Die Nahrungsversorgung liegt bei {supply}.",
		"base.advisor.city.healthy.detail": "Bedürfnisse, Dienste, Beschäftigung und Cashflow sind in gutem Zustand.",
		"base.advisor.city.long_commutes.detail": "Der durchschnittliche Arbeitsweg beträgt {commute} m.",
		"base.advisor.city.no_residents.detail": "{housing} Wohneinheiten stehen leer.",
		"base.advisor.city.no_roads.detail": "{population} Bewohner pendeln ohne Straßennetz.",
		"base.advisor.city.police_shortage.detail": "Die Polizeiversorgung liegt bei {supply}.",
		"base.advisor.city.power_shortage.detail": "Die Stromversorgung liegt bei {supply}.",
		"base.advisor.city.quiet.detail": "Platziere eine Blaupause oder lasse die Simulation laufen.",
		"base.advisor.city.school_shortage.detail": "Die Schulversorgung liegt bei {supply}.",
		"base.advisor.city.services_healthy.detail": "Nahrung, Wasser, Strom und Bürgerdienste decken den Bedarf.",
		"base.advisor.city.traffic_disconnected.detail": "Das Straßennetz hat {components} Komponenten.",
		"base.advisor.city.unemployment.detail": "{unemployed} Bewohner sind arbeitslos; Beschäftigung {employment}.",
		"base.advisor.city.unhappy.detail": "Die Zufriedenheit liegt bei {happiness}.",
		"base.advisor.city.vacant_homes.detail": "{vacancies} von {housing} Häusern stehen leer.",
		"base.advisor.city.water_shortage.detail": "Die Wasserversorgung liegt bei {supply}.",


		# Folgekamera.
		"base.ui.citizen.follow_hint": "F3 folgt dem ausgewählten Zuhause · Esc beendet",
		"base.ui.citizen.following": "Folgt Zuhause #{id} · Esc beendet",
		"base.ui.citizen.follow_started": "Folgt Zuhause #{id} · F3 oder Esc beendet",
		"base.ui.citizen.follow_stopped": "Folgen beendet",
		"base.ui.citizen.follow_select_first": "Zuerst ein fertiges Haus auswählen (Rechtsklick)",
		"base.ui.help.follow": "F3: Zuhause folgen",

		# Eigenständiger Siedlungsbesitz-Editor (Entwicklerszene). Siedlungsnamen,
		# IDs, Rollenstrings und Bridge-Fehlercodes bleiben unverändert.
		"base.ui.settlement_editor.title": "Editor für Siedlungsbesitz",
		"base.ui.settlement_editor.subtitle": "Entwickler-Bridge-Tool (docs/48 §2.2). Nur im Speicher.",
		"base.ui.settlement_editor.seed": "Seed {seed}",
		"base.ui.settlement_editor.button.fixture": "Fixture",
		"base.ui.settlement_editor.button.refresh": "Aktualisieren",
		"base.ui.settlement_editor.button.new_session": "Neue Sitzung (N)",
		"base.ui.settlement_editor.state": "Seed {seed} · {count} Siedlungen · Prüfsumme {checksum}",
		"base.ui.settlement_editor.selected_record": "Ausgewählter Datensatz",
		"base.ui.settlement_editor.no_selection": "Keine Siedlung ausgewählt",
		"base.ui.settlement_editor.detail_empty": "Keine Siedlung ausgewählt.\nZeile wählen zum Prüfen.",
		"base.ui.settlement_editor.form.selected": "Ausgewählt #{id}",
		"base.ui.settlement_editor.label.name": "Name",
		"base.ui.settlement_editor.label.center_x": "Zentrum X",
		"base.ui.settlement_editor.label.center_z": "Zentrum Z",
		"base.ui.settlement_editor.label.radius_m": "Radius m",
		"base.ui.settlement_editor.label.player_id": "Spieler-ID",
		"base.ui.settlement_editor.label.role": "Rolle",
		"base.ui.settlement_editor.label.tax_percent": "Steuer %",
		"base.ui.settlement_editor.action.found": "Gründen",
		"base.ui.settlement_editor.action.claim": "Beanspruchen",
		"base.ui.settlement_editor.action.rename": "Umbenennen",
		"base.ui.settlement_editor.action.transfer": "Übertragen",
		"base.ui.settlement_editor.action.invite": "Einladen",
		"base.ui.settlement_editor.action.set_role": "Rolle setzen",
		"base.ui.settlement_editor.action.remove_member": "Mitglied entfernen",
		"base.ui.settlement_editor.action.set_tax": "Steuer setzen",
		"base.ui.settlement_editor.action.list": "Liste",
		"base.ui.settlement_editor.note": "Beanspruchen braucht eine Spieler-ID. Andere Aktionen nutzen den Eigentümer.",
		"base.ui.settlement_editor.ready": "Bereit.",
		"base.ui.settlement_editor.help": "1–9 · F gründ · C claim · R name · T move\nI lad · P rolle · X weg · M steuer · N neu",
		"base.ui.settlement_editor.footer": "Nur lokale Bridge · kontaktiert nie einen Server · Fehler brechen das Werkzeug nie ab.",
		"base.ui.settlement_editor.message.new_session": "Neue leere Siedlungssitzung für Seed {seed}.",
		"base.ui.settlement_editor.message.refreshed": "Liste aktualisiert.",
		"base.ui.settlement_editor.message.ok": "{context}: ok",
		"base.ui.settlement_editor.message.failed": "{context} fehlgeschlagen: {code} ({message})",
		"base.ui.settlement_editor.message.no_core": "{context} fehlgeschlagen: no_core (Bridge nicht geladen)",
		"base.ui.settlement_editor.message.missing": "{context} fehlgeschlagen: settlement.unknown (kein Datensatz #{id})",
		"base.ui.settlement_editor.message.no_selection": "{context} fehlgeschlagen: no_selection (zuerst eine Siedlung auswählen)",
		"base.ui.settlement_editor.row": "#{id}  {name}  ·  {owner}  ·  Rev. {revision}  ·  Steuer {tax} %  ·  Mitglieder {members}",
		"base.ui.settlement_editor.detail.identity": "ID {id} · besessen {owned} · Eigentümer-ID {owner} · Eigentümerbezeichnung {label}",
		"base.ui.settlement_editor.detail.center": "Zentrum {center} · Radius {radius} m · Steuer {tax} %",
		"base.ui.settlement_editor.detail.revision": "Mitgliedschaftsrevision {revision} · Prüfsumme {checksum}",
		"base.ui.settlement_editor.detail.members": "Mitglieder: {members}",
		"base.ui.settlement_editor.owner.unowned": "ohne Eigentümer",
		"base.ui.settlement_editor.members.none": "keine",
		"base.ui.settlement_editor.owned.yes": "ja",
		"base.ui.settlement_editor.owned.no": "nein",
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


## Locale-aware whole number for HUD/dashboard values.
## `1234` -> `1,234` (EN) / `1.234` (DE). Floats round to the nearest whole
## number; NaN/INF render as `—`; anything unreadable renders as `0`.
func format_int(value = 0) -> String:
	if value is int:
		return _group_digits(str(value))
	if value is float:
		if not is_finite(value):
			return NOT_AVAILABLE
		return _format_decimal(value, 0)
	if value is String and value.is_valid_float():
		return format_int(float(value))
	return "0"


## Locale-aware fixed-point number. `format_float(1234.5, 1)` ->
## `1,234.5` (EN) / `1.234,5` (DE). `decimals` is clamped to 0..6; NaN/INF
## render as `—`; anything unreadable renders as zero with the same decimals.
func format_float(value = 0.0, decimals: int = 0) -> String:
	var places := clampi(decimals, 0, MAX_DECIMALS)
	if value is int:
		return _format_decimal(float(value), places)
	if value is float:
		if not is_finite(value):
			return NOT_AVAILABLE
		return _format_decimal(value, places)
	if value is String and value.is_valid_float():
		return format_float(float(value), places)
	return _format_decimal(0.0, places)


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


func advisor_detail(code: String, fallback: String, params = {}) -> String:
	var key := "base.advisor.%s.detail" % code
	if has_key(key) or has_key(key, DEFAULT_LOCALE):
		return t(key, params)
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


func _format_decimal(value: float, decimals: int) -> String:
	var text := (("%." + str(decimals) + "f") % value)
	# `%.0f` prints `-0` for small negative values; show zero without a sign.
	if text.begins_with("-") and _is_zero_text(text.substr(1)):
		text = text.substr(1)
	var parts := text.split(".")
	var whole := _group_digits(parts[0])
	if decimals <= 0:
		return whole
	var fraction: String = parts[1] if parts.size() > 1 else ""
	if fraction.length() < decimals:
		fraction += "0".repeat(decimals - fraction.length())
	return whole + _decimal_separator() + fraction


func _group_digits(text: String) -> String:
	if text.begins_with("-"):
		return "-" + _group_digits(text.substr(1))
	if text.begins_with("+"):
		return _group_digits(text.substr(1))
	var digits := text if not text.is_empty() else "0"
	var grouped := digits
	if digits.length() > 3:
		var parts := PackedStringArray()
		var first := digits.length() % 3
		if first > 0:
			parts.append(digits.substr(0, first))
		for index in range(first, digits.length(), 3):
			parts.append(digits.substr(index, 3))
		grouped = _group_separator().join(parts)
	return grouped


func _is_zero_text(text: String) -> bool:
	for index in text.length():
		var character := text.substr(index, 1)
		if character != "0" and character != ".":
			return false
	return true


func _decimal_separator() -> String:
	return "," if _locale == "de" else "."


func _group_separator() -> String:
	return "." if _locale == "de" else ","


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
