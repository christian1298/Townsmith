---
title: Localization
project: Townsmith
status: proposed
updated: 2026-11-21
tags:
  - townsmith
  - localization
  - i18n
  - modding
---

# 16 — Localization

> [!decision] D-022 — English + German at launch, moddable language packs
> Every user-facing string is externalized from the first implementation. English is the source language; German ships at launch. Additional languages are added through language packs, including community/mod packs later.

> [!info] Coverage audit
> [[69 - Localization Audit]] inventories the remaining hard-coded strings (HUD, dashboard shell and body, tutorial, content names, advisor, errors), separates player-facing from developer-only text, proposes key names and interpolation rules, and defines the phased migration and smoke/CI gate. It reflects the current inline 96-key editor table plus the 211-key `godot/world_i18n.gd` table; there is no `godot/locale/` at this commit. HUD shell and dashboard title/hint/sections/body are keyed; remaining status lines, modals, content names, and advisor text remain English. The static gate (`scripts/check_locale.sh`) parses all three tables with independent floors (editor 96, world 211, content 32). A separate data-only `godot/content_i18n.gd` table holds 32 EN / 32 DE display-name keys for starters, industries, services, and tutorial steps. The static gate parses it with a 32-key floor and the same parity/placeholder checks; the citizen panel and selected-content labels resolve through it. `WorldI18n.format_int`/`format_float` provide EN/DE grouping and decimal separators; coverage is partial and smoke-checked by `number_i18n=ok`. There is no `godot/locale/` at this commit.

## 1. Goals

- Ship **English and German** at launch.
- Never hard-code user-facing text in GDScript, Rust, scenes, or content data.
- Let each player choose their language independently, including in multiplayer.
- Make language packs **data**, not code, so mods and community translators can add languages without rebuilding the game.
- Keep the door open for more languages, right-to-left scripts, and voiceover later.

## 2. Non-goals for v1

- Full voiceover in every language.
- Right-to-left UI support at launch.
- Machine-translated launch text. German should be reviewed by a fluent speaker.
- Translating server logs, console commands, IDs, or player chat.

## 3. Supported languages

| Language | Code | Launch | Notes |
|---|---|---|---|
| English | `en` | ✅ | Source language and fallback |
| German | `de` | ✅ | Reviewed translation; umlauts and ß must render correctly |
| Community/mod | any | Later | Language packs, namespaced keys, fallback to English |

- The player can select a language in Settings; the default follows the OS locale when supported, otherwise English.
- Language changes apply immediately where possible; no restart for UI strings.
- Content mods can ship their own translations; missing keys fall back to English, then to the key itself.

## 4. What is localized

- UI: menus, buttons, tooltips, settings, build tools, overlays, alerts.
- Content: building names, room names, prop names, material names, job titles, needs, policies, ages, technologies.
- Tutorials, advisor lines, notifications, newspapers, event text.
- Errors shown to players (not internal logs).
- Benchmark UI and result explanations.
- Subtitles for any voiced lines (voiceover itself is later).

## 5. What is not localized

- Internal IDs, save keys, database columns, content IDs, mod IDs.
- Console commands and admin logs.
- Player chat and player-authored names.
- Code, comments, and developer documentation.
- Network protocol fields.

## 6. Architecture

### Key-based strings

Every string has a stable key:

```text
base.ui.build_menu.title
base.building.house.small.name
base.building.house.small.description
base.job.lumberjack.name
mod.copper_age.item.copper_ore.name
```

- `base.` is the base-game namespace.
- Mods use their own namespace, e.g. `mod.<mod_id>.`.
- Keys never change when text changes; changing a key is a content migration.
- Parameters use named placeholders: `{count}`, `{name}`, `{amount}`. Never concatenate translated fragments.

### Runtime lookup

```text
player locale
  → base language (e.g. de)
  → English (en)
  → key itself (visible fallback for missing translations)
```

- The client owns the player's locale.
- The server sends **keys + parameters**, never pre-localized strings.
- Each client resolves the key locally, so two players can see different languages in the same session.
- The server logs keys/English, not the client's language.

### Source format

- Base game: spreadsheet-friendly CSV or JSON with `key`, `en`, `de`, and `context` columns/fields.
- Build step validates keys, duplicates, missing translations, and placeholder mismatches.
- The build compiles the table into the runtime format used by both Godot and the Rust core.
- Mod language packs: JSON files under `lang/<code>.json` in the mod package.

Example CSV:

```csv
key,en,de,context
base.ui.build_menu.title,Build, Bauen,Main build menu title
base.building.house.small.name,Small House,Kleines Haus,Building name
base.notification.low_water,Water pressure is low in {district}.,Der Wasserdruck in {district} ist niedrig.,Notification
```

Example mod language pack:

```json
{
  "locale": "de",
  "namespace": "mod.copper_age",
  "version": 1,
  "strings": {
    "item.copper_ore.name": "Kupfererz",
    "item.copper_ore.description": "Ein weiches Metall für Werkzeuge und Leitungen."
  }
}
```

## 7. Content localization

Content data references keys, not display names:

```json
{
  "id": "base:building/house_small",
  "name_key": "base.building.house.small.name",
  "description_key": "base.building.house.small.description"
}
```

- The UI resolves the key at display time.
- Search and filtering use the localized name, plus the stable ID as a fallback.
- Blueprint metadata stores keys; player-authored names are stored as text and are not translated.
- Mod content must provide English keys at minimum; other languages are optional with fallback.

## 8. Numbers, dates, units, and plurals

- Use locale-aware formatting for numbers, decimals, dates, and times.
- German uses `1.234,56`; English uses `1,234.56`.
- SI units stay the same (liters, m³, meters), but unit labels and spacing follow the locale.
- Use plural rules for counts: `{count} citizen` / `{count} citizens`; German `{count} Einwohner` (invariant) and other nouns with correct plural forms.
- Prefer full sentences with placeholders over concatenated fragments.
- Keep translated text ~30% longer than English in UI layouts; German compounds can be long.

## 9. Fonts and UI

- Fonts must cover German: `ä ö ü ß Ä Ö Ü`, quotes, dashes, and the euro sign.
- Use a font fallback chain; do not ship a font that silently drops glyphs.
- Test UI at 100%, 125%, and 150% text scale.
- Avoid fixed-width buttons for labels; use wrapping and flexible layouts.
- Right-to-left support is a later architecture concern; keep text flow direction abstracted.

## 10. Multiplayer

- Server messages, errors, and notifications are **keys + parameters**.
- Each client localizes with its own locale.
- Example: server sends `base.error.not_enough_money { "cost": 500 }`; the German client sees the German string.
- Server logs remain English for admin readability.
- Player chat is raw text and is never auto-translated.
- Language packs are client-side; the server does not need to install them unless a mod's content requires the same language keys. Content mods still need their base English keys on the server for validation.

## 11. Modding

- A mod can add a language pack without adding gameplay code.
- Language packs are namespaced and versioned.
- A mod can add keys, but cannot silently override base-game keys. Overrides require an explicit `override: true` and are listed in the mod manifest.
- Missing keys fall back to English; the UI can show a subtle “missing translation” marker in developer mode.
- Server manifests record language-pack hashes so clients can detect mismatches for modded content.

## 12. Testing and CI

- **Missing-key lint:** every `*_key` reference exists in English.
- **Duplicate-key lint:** no duplicate keys in a namespace.
- **Placeholder lint:** placeholders match across languages.
- **Pseudo-localization:** a generated locale with accented/expanded text to catch clipping.
- **German UI pass:** screenshots of the main menus, build tools, city dashboard, and notifications.
- **Plural tests:** 0/1/2/many for counts.
- **No hard-coded strings:** a lint/review rule for GDScript/Rust UI code.
- **Language-pack load test:** a mod adds a new locale and the UI switches to it.

## 13. Roadmap

| Phase | Work |
|---|---|
| Phase 0 | Decide format; create `en`/`de` tables; add missing-key lint |
| Phase 1 | Externalize editor UI strings; language selector; German pass on editor |
| Phase 2 | Externalize city UI, content names, notifications; German pass |
| Phase 5 | Localize benchmark UI and performance explanations |
| Phase 6 | Server messages as keys; per-player locale in multiplayer |
| Phase 7 | Mod language packs; manifest hashes; community translation flow |
| Phase 8 | Full German QA, pseudo-localization, text expansion pass |

## 14. Open questions

- **Q-079:** Which additional languages after German, and in what order?
- **Q-080:** Is voiceover in scope later, or subtitles only?
- **Q-081:** Community translation workflow: in-repo PRs, a translation platform, or mod-only packs?
- **Q-082:** Should the server enforce a language for logs/admin messages, or keep English only?

See [[09 - Open Questions]] and [[10 - Decisions]].
