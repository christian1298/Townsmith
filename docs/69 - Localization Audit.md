---
title: Localization Audit
project: Townsmith
status: active
updated: 2026-11-23
tags:
  - townsmith
  - localization
  - i18n
  - audit
---

# 69 — Localization Audit

> [!info] What this note is
> A coverage audit of user-facing strings at the Phase 3 slice, plus a concrete expansion plan. It records the current state, separates player-facing text from developer-only text, proposes key conventions and interpolation rules, and defines the migration phases and the smoke/CI gate. No code or locale files were changed by this audit.

> [!warning] Runtime layout differs from the original brief
> There is **no `godot/locale/` directory** and no `en.json` / `de.json` at this commit. The only runtime table is inline in `godot/editor_i18n.gd` (`TABLES`), with **111 EN and 111 DE keys**. [[38 - Editor Localization]] documents this as a deliberate slice stand-in for the compiled pipeline in [[16 - Localization]]. The audit below targets the actual files, not the assumed layout.

## 1. Current coverage

| Surface | Runtime source | EN keys | DE keys | Parity | Gate |
|---|---|---:|---:|---|---|
| First-person volume editor | `godot/editor_i18n.gd` | 111 | 111 | complete | `check_locale.sh` + `smoke_locale.sh` |
| World-placement HUD, status, help | `godot/world_placement.gd` | 226 world keys | 0 | `world_i18n.gd` | dashboard shell/body, HUD shell/status, all role-player modals, and advisor titles/details keyed; content names/numbers raw |
| Dashboard shell and body | `world_placement.gd` + Rust `core_city` payload | 0 | 0 | n/a | none |
| Advisor titles | Rust `core_city::advisor` | 22 code titles | 0 | `world_i18n.gd` | titles and parameterized details keyed by code |
| Tutorial titles and hints | Rust payload read by `world_placement.gd` | 0 | 0 | n/a | none |
| Starter, industry, service, tax names | Rust payload + `kind.capitalize()` | 0 | 0 | n/a | none |
| Content display names | `godot/content_i18n.gd` | 34 | 34 | complete | static gate; HUD/panel/modal/3D labels wired; `content_ui=ok` |
| Player-visible error modals | GDScript literals + `error_code` | 0 | 0 | n/a | none |

Facts about the current table:

- `TABLES` contains the same 111 keys under `en` and `de`; the locale smoke asserts parity and required key groups.
- Lookup returns `!key!` for a missing key, so gaps are visible instead of silently falling back.
- Placeholders are `{0}`, `{1}`, … for arrays and `{name}` for dictionaries. There is no plural helper and no locale-aware number formatter.
- The editor table is owned by the editor slice. It is not yet the shared content registry or the moddable language-pack pipeline from [[16 - Localization]].

> [!note] Count method
> Key counts are a static count of the two `TABLES` dictionaries. The Godot smoke was not run in this audit; `scripts/smoke_locale.sh` is the authority for parity and rendering.

## 2. Hard-coded player-facing strings

Number formatting now has `WorldI18n.format_int` / `format_float`: EN `1,234.5`, DE `1.234,5`. HUD and dashboard quantities use them; ids, coordinates, seeds, advisor detail parameters, citizen-panel values, and loan-modal amounts stay raw. Smoke checks `number_i18n=ok` plus a DE/EN dashboard grouping assertion.

The largest gap is `godot/world_placement.gd`. Almost every string in the Mayor/Architect HUD, dashboard shell, modal line, and content label is a literal. The Rust dashboard and advisor are a second, separate gap: they cannot be fixed in GDScript alone.

### 2.1 HUD, status, and help

| String (representative) | Location | Exposure |
|---|---|---|
| `Townsmith — Mayor Mode  (seed %d)` | `world_placement.gd` `_update_hud` | Player-facing, every HUD tick |
| `Townsmith — Architect View (read-only)  (seed %d)` | `_update_hud` | Player-facing |
| `Blueprint: %s  ·  %d rooms  ·  capacity %d` | `_update_hud` | Player-facing |
| `Structure: %s` | `_update_hud` | Player-facing |
| `%s  %d/%d stories · span %.1f/%.1f m · foundation %.1f/%.1f m` | `_update_hud` | Player-facing |
| `%s  INVALID: %s (%s)` | `_update_hud` | Player-facing; embeds an error code |
| `Selected: none` | `_update_hud` | Player-facing |
| `Selected #%d: %s  ·  %d rooms  ·  capacity %d  ·  %d jobs  ·  %s` | `_update_hud` | Player-facing |
| `? × ? m` | `_update_hud` | Player-facing fallback |
| `wilderness`, `owner`, `industry`, `utility`, `service`, `stories` | `_update_hud` | Player-facing fragments |
| `VALID` / `INVALID: %s` | `_update_hud` | Player-facing status |
| `WASD pan · Q/E rotate · wheel zoom · R rotate · X raise · Z lower · N found settlement` | `_update_hud` | Player-facing help |
| `LMB place · RMB select · Enter: enter selected building` | `_update_hud` | Player-facing help |
| `C crew · I industry · U utility · K services · B starter · T tax · L loan · Y research · F1 dashboard · V +10k · F5 save · F9 load` | `_update_hud` | Player-facing help |
| `First-person view at the selected door · M / Esc returns to Mayor` | `_update_hud` | Player-facing help |

The help lines are one long sentence each. They must be split into keyed rows or a keyed multi-line block; translating them as concatenated fragments will break word order.

### 2.2 Dashboard shell and body

| String | Location | Exposure |
|---|---|---|
| `City Dashboard` | `world_placement.gd` `_setup_ui` | Player-facing panel title |
| `F1 or Esc to close` | `_setup_ui` | Player-facing hint |
| `Population`, `Jobs`, `Happiness`, `Needs`, `Treasury`, `Utilities`, `Civic`, `Roads`, `Construction`, `Advisor` | Rust dashboard payload, rendered in the panel | Player-facing section labels |
| `Population 2 residents / 2 housing (0 vacant)` and similar value lines | Rust payload | Player-facing; needs plural rules |
| `▲ The city is running a deficit — Raise taxes, borrow deliberately, or wait...` | Rust advisor | Player-facing |

The dashboard body is produced by `core_city` and passed through the bridge. Its labels, units, and advisor text are English data, not UI code.

### 2.3 Advisor messages

`core_city::advisor` returns `{ code, severity, title, detail }` for codes such as `city.bankrupt`, `city.food_shortage`, `city.unhappy`, `city.budget_deficit`, `city.debt`, `city.long_commutes`, `city.traffic_disconnected`, and `city.construction_stalled` (full list in [[57 - Dashboard and Advisor]]).

| Field | Localization status | Notes |
|---|---|---|
| `code` | Developer-only | Stable key; never translate |
| `severity` | Developer-only enum | Maps to color/icon, not text |
| `title` | Player-facing, English literal | `base.advisor.<code>.title` (EN/DE); bridge title fallback |
| `detail` | Player-facing, keyed by code | `base.advisor.<code>.detail` renders named params |

The protocol should send the code plus parameters; each client resolves the title and detail in its own locale, per D-022.

### 2.4 Modals, rejections, and errors

| String | Location | Notes |
|---|---|---|
| `Lot rejected: `, `Starter catalog failed: `, `Starter failed: `, `Industry failed: `, `Civic service failed: `, `Utility failed: `, `Tax change failed: `, `Loan rejected: `, `Settlement rejected: `, `Blueprint rejected: `, `Terrain edit failed: `, `Road rejected: `, `Save failed: `, `Load failed: `, `Research failed: ` | `world_placement.gd` | Prefix is player-facing; appended `error_code` is developer-only |
| `No starter buildings at this age`, `Starter has no package path`, `Starter package rejected` | `_cycle_starter` | Player-facing |
| `Select a building first (RMB)`, `This building has no job slots` | `_cycle_industry`, `_cycle_service`, `_cycle_utility` | Player-facing |
| `Blueprint mesh is empty`, `Blueprint bounds are invalid` | `_ensure_blueprint` | Player-facing |
| `Blueprint failed: ` | `_ready` | Player-facing |
| `Lot corner cleared`, `Settlements cleared`, `Industry removed`, `Civic service removed`, `Utility removed` | mode handlers | Player-facing confirmation |
| `Lot mode: click two corners · 1/2/3 zone · Esc cancels` | `_toggle_lot_mode` | Player-facing help |
| `Lot zone: `, `Lot corner (%d, %d) · click the opposite corner` | `_set_lot_zone`, `_lot_click` | Player-facing |
| `Lot needs at least 4 × 4 m` | `_lot_click` | Player-facing validation |
| `Starter: %s (%d/%d)`, `Industry: %s`, `Civic service: %s`, `Utility: water`, `Utility: water + power`, `Tax policy: %s`, `Founded %s`, `Researched %s` | action handlers | Player-facing; `%s` often a raw ID |
| `Borrowed %.0f · debt %.0f` | `_borrow` | Player-facing; number formatting needs locale rules |

### 2.5 Content names and generated labels

| String / pattern | Location | Problem |
|---|---|---|
| `kind.capitalize()` for `lumberjack`, `farm`, `hunt` | `_refresh_industry_visuals` | Displays `Lumberjack`; English case rules, not a translation |
| `kind.capitalize()` for `water`, `power` | `_refresh_utility_visuals` | Displays `Water` / `Power` |
| `kind.capitalize()` for `school`, `clinic`, `fire`, `police` | `_refresh_service_visuals` | Displays `School` / `Clinic` / `Fire` / `Police` |
| `Settlement %d` | `_found_settlement` | Generated default name; should be keyed and parameterized |
| `player` as owner label | `_found_settlement`, settlement Label3D | Internal owner id shown as text |
| `Acceptance House` | `_ensure_blueprint` default package name | Fixture name leaks into the HUD |
| `system_name` (`timber_frame`, …) | structure payload in HUD | ID-like label shown to the player |

### 2.6 Tutorial, starter, and service data

| Surface | Source | Status |
|---|---|---|
| Tutorial `current_title` | Rust tutorial payload read by `world_placement.gd` | English literal |
| Tutorial hints/details | Rust payload | English literal |
| Starter building names and descriptions | Rust `starter_library` / `starter_blueprint` payload | English data |
| Service names and descriptions | Rust civic-services payload | English data |
| Industry names and descriptions | Rust industry payload | English data |
| Tax policy names (`low`, `standard`, `high`) | `TAX_POLICIES` in `world_placement.gd` | English literals |

These are content names, so the durable fix is a content registry with `name_key` / `description_key`, not a larger GDScript table.

### 2.7 Content display-name table (not wired)

`godot/content_i18n.gd` adds a data-only EN/DE lookup for the Phase 2 content names: 20 starter buildings, 2 utility kinds, 3 industry kinds, 4 civic service kinds, and 5 tutorial step IDs, for **34 keys per locale** with full parity. It exposes `display(key, locale)`, plus `starters(locale)`, `industries(locale)`, `services(locale)`, and `tutorials(locale)` flatteners keyed by stable content ID/kind.

- Key layout is `base.content.starter.<slug>` (the `base:` prefix is stripped), `base.content.industry.<kind>`, `base.content.service.<kind>`, and `base.content.tutorial.<step>`.
- Missing keys fall back to English, then to the raw key; unsupported locales resolve to English. Values are display names only, with no placeholders or sentences.
- **This is not wired into `world_placement.gd`.** The HUD, dashboard, labels, and tutorial still read the Rust payload or call `kind.capitalize()`, so players keep seeing raw/English labels until the UI slice consumes this table.
- `scripts/check_locale.sh` does not parse this file yet; the counts above are a static audit, not a CI gate.
- The durable fix remains a Rust-side `name_key` content registry; this table is the slice bridge until that protocol lands.

## 3. Player-facing versus developer-only

| Player-facing (localize) | Developer-only (never localize) |
|---|---|
| HUD mode, blueprint, structure, selection, and help lines | `error_code` values (`no_road_frontage`, `place_failed`, `timber_frame`) |
| Dashboard panel title, hint, section labels, value lines | Node names (`Dashboard`, `DashboardText`, `Placed`) |
| Advisor titles and details | Smoke markers (`LOCALE_SMOKE`, `dashboard=ok`) |
| Tutorial titles and hints | File paths and names (`res://`, `user://`, `world.json`, `acceptance_house.tsbp`) |
| Starter, industry, service, tax, and settlement names | Console arguments (`--scene-smoke`, `--blueprint`) |
| Modal prefixes and validation messages | Internal IDs (`base:building/house_small`, `local:acceptance_house`) |
| Generated owner and default settlement labels | Checksums, seeds, benchmark metrics, server logs |
| Units, numbers, dates, and percentages | Code, comments, and developer documentation |

Ambiguous cases to decide explicitly:

- **Structure `system_name`**: currently shown raw (`timber_frame`). Treat it as player-facing and map it through `base.content.structure.<id>.name`, or hide it behind a developer overlay.
- **Settlement owner `player`**: the owner ID is stable; the display label should resolve to a player name or a localized “Player” fallback.
- **Player-authored names**: blueprint names and settlement names typed by a player are stored as text and are not translated. Default/generated names are translated.
- **Error codes in modals**: show a friendly localized message; show the raw code only in developer mode.

## 4. Proposed key conventions

The existing `base.editor.*` namespace stays as is. New work should use these prefixes:

| Area | Prefix | Example |
|---|---|---|
| HUD and help | `base.ui.hud.*`, `base.ui.help.*` | `base.ui.hud.mayor.title`, `base.ui.help.move` |
| Dashboard | `base.ui.dashboard.*` | `base.ui.dashboard.population`, `base.ui.dashboard.close_hint` |
| Modal and prompts | `base.ui.modal.*`, `base.ui.prompt.*` | `base.ui.modal.lot_rejected`, `base.ui.prompt.lot_zone` |
| Errors | `base.error.<domain>.<code>` | `base.error.lot.no_road_frontage` |
| Tutorial | `base.tutorial.<step>.title`, `.hint` | `base.tutorial.first_house.title` |
| Advisor | `base.advisor.<code>.title`, `.detail` | `base.advisor.city.budget_deficit.title` |
| Structure | `base.content.structure.<id>.name` | `base.content.structure.timber_frame.name` |
| Industry | `base.content.industry.<id>.name` | `base.content.industry.lumberjack.name` |
| Services | `base.content.service.<id>.name` | `base.content.service.school.name` |
| Starter | `base.content.starter.<id>.name` | `base.content.starter.house_small.name` |
| Tax policy | `base.economy.tax.<id>.name` | `base.economy.tax.standard.name` |
| Settlement defaults | `base.settlement.default_name`, `.owner_player` | `base.settlement.default_name` = `Settlement {index}` |
| Units | `base.unit.<unit>` | `base.unit.meter`, `base.unit.square_meter` |

Rules:

- One key per sentence or per complete UI row. Never concatenate translated fragments to build a sentence.
- Keys are stable; changing text does not change the key. Renaming a key is a content migration.
- A key must exist in English first. Other locales fall back to English, then to `!key!`.
- Mod content uses `mod.<mod_id>.*` and may not override `base.*` without an explicit manifest override.
- Do not embed BBCode, HTML, or color codes in translation strings. Keep markup in the UI layer or pass it through separate placeholders.
- Do not put file paths, IDs, or error codes in translation strings as literals; pass them as parameters.

### 4.1 Interpolation

- New keys use named placeholders only: `{count}`, `{name}`, `{amount}`, `{code}`, `{index}`, `{district}`.
- The legacy editor table may keep `{0}` / `{1}`; new strings must not add positional placeholders.
- Placeholder sets must match across locales. The validator compares the sets, not just the rendered output.
- Numbers are formatted by a locale-aware formatter before substitution. Do not pre-format with `%d` / `%.0f` in UI code when the value is player-facing.
- Do not build a sentence with `+` or `%` around a translated string. Put the whole sentence in one key.
- Literal braces are escaped as `{{` / `}}` if the formatter supports it; otherwise avoid literal braces in text.

### 4.2 Plurals

Propose a `.one` / `.other` suffix resolved by locale rules, with an optional `.zero` for special cases:

```text
base.ui.dashboard.residents.one   = {count} resident
base.ui.dashboard.residents.other = {count} residents
```

- German currently needs `one` / `other`; some nouns are invariant (`{count} Einwohner`), but the key structure stays the same.
- Counts of 0 should use the `other` form unless a `.zero` key exists (`No residents`).
- The validator checks that every plural key family is complete in every locale.
- Do not implement plural logic by appending an “s” in code.

### 4.3 Tone

The current German editor uses a neutral infinitive/imperative style (“Klicken, um die Maus wieder zu erfassen”). Keep that style for HUD and modals; do not switch to “Sie” or “Du” in the middle of the UI.

## 5. Phased plan

| Phase | Scope | Approx. keys | Exit criterion |
|---|---|---:|---|
| 1. HUD + dashboard shell/body | `_update_hud`, `_setup_ui`, dashboard title/hint/body | **partial** — 226 world keys | HUD shell/status, dashboard shell/body, all role-player modals, and advisor titles/details keyed; content names/numbers remain |
| 2. Tutorial + content names | Tutorial titles/hints, starter/industry/service/tax names, `capitalize()` removal, structure names | 60–90 | No `capitalize()` or raw content ID reaches a player label |
| 3. Advisor + errors | Rust advisor title/detail keys, dashboard body labels, modal prefixes, error mapping | **partial** — 22 code titles | Advisor titles/details localized with params; errors and number formatting remain |

### Phase 1 — HUD and dashboard shell (recommended first slice)

Scope:

- Mayor and Architect HUD titles, blueprint/room/capacity line, structure line, selection line, owner/industry/utility/service fragments, valid/invalid status.
- Help block (split into `base.ui.help.move`, `.edit`, `.tools`, `.mayor`, `.architect`).
- `City Dashboard` title and `F1 or Esc to close` hint.
- Numeric formatting for capacity, jobs, span, foundation, and debt.

Acceptance checks:

- `world_placement.gd` `_update_hud` and `_setup_ui` contain no literal player-facing strings; only format calls and key lookups remain.
- EN/DE parity for every new key; placeholder sets match.
- Toggle EN → DE → EN at runtime with no `!key!` in the dashboard shell. The world smoke checks `hud_i18n=ok`, `hud_body_i18n=ok`, and `hud_status_i18n=ok` for the dashboard shell, HUD body/status, and common modal text in both locales.
- Dashboard shell fits at 100%, 125%, and 150% text scale in German; add it to the layout smoke.
- The locale smoke count contract is updated in the same change (see §6).

### Phase 2 — Tutorial and content names

Scope:

- Tutorial `current_title`, hint, and detail fields become keys or key+params.
- Starter, industry, service, tax-policy, and structure names resolve through `base.content.*` / `base.economy.*`.
- Replace `kind.capitalize()` with a lookup; unknown mod IDs fall back to a readable ID, not a crash.
- Default settlement name `Settlement {index}` and owner `player` label.
- Decide whether `Acceptance House` remains a fixture-only name or gets a key.

Acceptance checks:

- No `capitalize()` call feeds a player-visible `Label` or `Label3D`.
- Every content ID rendered in the HUD has a key or an explicit fallback.
- A mod-provided ID without a translation shows the English name or the ID in developer mode, and a localized fallback otherwise.
- Tutorial steps render in EN and DE with no clipped text in the tutorial panel.

### Phase 3 — Advisor and errors

Scope:

- `core_city::advisor` returns `code` + parameters; title/detail move to `base.advisor.<code>.*`.
- Dashboard body labels and value templates move to keys.
- Modal prefixes move to `base.ui.modal.*` / `base.error.*`; `error_code` is appended only in developer mode.
- Server logs stay English; clients resolve keys locally.

Acceptance checks:

- The bridge payload contains no pre-localized English title/detail.
- A German client and an English client see different advisor text from the same server message.
- Unknown advisor/error codes show a generic localized fallback plus the code in developer mode.
- `smoke_world_scene.sh` asserts localized advisor titles in both locales.

## 6. Smoke and CI gate

### Current contract

### Static gate

`scripts/check_locale.sh` parses the `TABLES` const in `godot/editor_i18n.gd` and `godot/world_i18n.gd`, plus the `DISPLAY` const in `godot/content_i18n.gd`, and checks key parity, per-file key floors, empty values, duplicate keys, and `{0}`/`{name}` placeholder parity. The floors ratchet independently: editor `111`, world `226` (currently 226 keys), content `34`. Default mode prints one line per table plus a summary:

```text
LOCALE_CHECK file=godot/editor_i18n.gd keys=111 min=111 status=ok
LOCALE_CHECK file=godot/world_i18n.gd keys=226 min=226 status=ok
LOCALE_CHECK file=godot/content_i18n.gd keys=34 min=34 status=ok
LOCALE_CHECK editor=111 world=226 content=34 status=ok
```

Single-file mode (`scripts/check_locale.sh [FILE]`) keeps the original `LOCALE_CHECK keys=N status=ok` marker and derives the floor from the file name (`world_i18n.gd` gets 143, `content_i18n.gd` gets 34, anything else 96). `--self-test` covers twelve cases: the six editor cases (valid, missing DE key, extra DE key, empty value, placeholder mismatch, too few keys), a world-style fixture pass, a too-few world table, a world EN/DE placeholder mismatch, and the same three checks for the `DISPLAY` content table. The gate runs in `scripts/check.sh` and the Rust CI job before the Godot locale smoke.

A pseudo-localization gate follows the table check in default mode:

```bash
./scripts/check_pseudo_locale.py
PSEUDO_LOCALE editor=111 world=226 content=34 long=11 status=ok
```

It accents vowels, keeps `{name}`/`{0}` placeholders byte-for-byte, and expands text to 130% of the EN length with a `«…»` marker. It checks EN/DE parity both ways, duplicate/empty keys, placeholder parity, and a 120-character length limit with an explicit 11-key allowlist for long explanatory strings. This catches structural/layout risk before German QA; it is not a translation. `--self-test` covers 17 generator/check cases without writing repository files.

`scripts/smoke_locale.sh` parses the last `LOCALE_SMOKE` line and requires:

```bash
if [[ -z "$KEYS" || "$KEYS" -lt 111 || "$STATUS" != "ok" ]]; then
```

The exact `96` was a hand-edited constant in the runtime smoke. The static gate now enforces independent floors (`EDITOR_MIN_KEYS=111`, `WORLD_MIN_KEYS=143`, `CONTENT_MIN_KEYS=34`) plus EN/DE parity, empty values, and placeholder parity for all three tables; `smoke_locale.sh` accepts editor `keys >= 111`. Adding keys is no longer a red build.

### Proposed contract

1. Emit both counts: `LOCALE_SMOKE en=ok de=ok keys_en=96 keys_de=96 status=ok`.
2. Assert `keys_en == keys_de`, `status=ok`, and `keys_en >= 111` as a ratchet floor.
3. Commit the floor and required key groups in a small manifest (`scripts/locale_contract.json` or equivalent) so raising coverage is a visible one-line change in the same PR.
4. Keep the exact-count check only as an optional “no unintended key loss” warning, not a hard failure.

### New coverage check

Add `scripts/check_locale_coverage.sh` and run it from `scripts/check.sh` / CI:

- Scan `godot/*.gd` for player-facing literals: `.text = "`, `_set_modal("`, `Label3D` text assignments, `add_item("`, and `kind.capitalize()`.
- Allow an inline `# l10n-allow: <reason>` on lines that are deliberately developer-only (smoke markers, fixture paths, debug output).
- Fail with a `LOCALE_COVERAGE status=ok offenders=0` / non-zero marker and a file:line list.
- Do not scan `editor_i18n.gd` itself or markdown/docs.

### Additional gates

- **Parity/placeholder:** the static gate covers `editor_i18n.gd`, `world_i18n.gd`, and `content_i18n.gd`; extend it to every `base.*` table once the shared content pipeline lands, including plural families.
- **Pseudo-localization:** generate an accented, +30% length locale to catch clipping before German QA.
- **German layout:** include HUD, dashboard, and one modal in the 100/125/150% width budgets.
- **No hard-coded strings:** the coverage check above is the enforcement; review alone is not enough.
- **Determinism:** keep all smoke markers, node names, and error codes English so tests stay stable across locales.

## 7. Risks

| Risk | Why it matters | Mitigation |
|---|---|---|
| Dynamic strings and concatenation | Word order, grammar, and gender break when fragments are joined; `capitalize()` is not translation | One key per sentence; named params; no `+` around translated text |
| Pluralization | English 2 forms, German 2, future languages more; counts like “1 rooms” are visible bugs | `.one` / `.other` families; 0-case key; validator checks completeness |
| German length | Compounds and +30% expansion overflow the 470 px dashboard and fixed HUD panel | Flexible layout, wrapping, width budget smoke at 125/150% |
| RTL future | Mirrored layout, bidi text, icons, and numeric ordering are not handled by the current LTR UI | Keep direction abstracted; avoid absolute left/right assumptions in new HUD code |
| Mod names | Player-authored and mod-authored names are not translated; mods may ship partial locales | Store names as text; translate only `name_key` content; namespace mod keys; English fallback |
| Rust data layer | Advisor, dashboard, tutorial, and content strings cannot be fixed in GDScript; a protocol change is required | Send codes + params from the core; resolve on the client; version the message shape |
| Key churn / save compatibility | Renaming keys on disk breaks old saves and mod packs | Stable keys; key rename is a migration; never reuse a retired key for new text |
| Error-code leakage | Raw codes are developer text and can confuse players | Map codes to `base.error.*`; show the code only in developer mode |
| Test fragility | Localized text changes break string-matching smokes | Assert markers and keys, not English sentences |

## 8. Recommended first slice

**Do Phase 1 only: the Mayor/Architect HUD plus the dashboard shell.** It is the highest-visibility gap, it is self-contained in `world_placement.gd`, it needs no Rust protocol change, and it exercises the hard parts (multi-line help, numbers, German length) before content and advisor work.

Concrete first keys:

```text
base.ui.hud.mayor.title
base.ui.hud.architect.title
base.ui.hud.blueprint
base.ui.hud.structure
base.ui.hud.selected_none
base.ui.hud.selected
base.ui.hud.owner
base.ui.help.move
base.ui.help.edit
base.ui.help.tools
base.ui.dashboard.title
base.ui.dashboard.close_hint
```

Sequence:

1. Add the keys to the runtime table (or a shared `base.ui.*` table) with EN/DE values.
2. Replace the literals in `_update_hud` and `_setup_ui`; pass numbers and names as parameters.
3. Update `smoke_locale.sh` to the new count/floor contract in the same commit.
4. Add the dashboard shell to `smoke_layout.sh`.
5. Only then start Phase 2 content names.

## 9. Open items

- `docs/22 - UI and UX.md` does not exist at this commit; reconcile HUD/dashboard conventions when it lands.
- The editor table now covers the player-facing labels, structure names, room fallback, palette rows, and default blueprint name. Remaining literals are developer/debug text, raw ids, core English error messages, and engine FileDialog chrome.
- Decide whether new UI keys live in `editor_i18n.gd`, a shared `base.ui.*` table, or the future compiled pipeline.
- Decide the developer-mode policy for showing error codes and raw content IDs.
- Decide settlement naming: auto-generated `Settlement N`, player-entered name, or both.
- Confirm the German reviewer and the pseudo-localization step before Phase 3.

See [[16 - Localization]], [[38 - Editor Localization]], [[57 - Dashboard and Advisor]], and [[09 - Open Questions]].
