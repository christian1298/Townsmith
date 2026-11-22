---
title: Editor Localization
project: Townsmith
status: active
updated: 2026-11-22
tags:
  - townsmith
  - vertical-slice
  - editor
  - localization
---

# 38 — Editor Localization

> [!info] What this note is
> The slice-local EN/DE string table for the first-person editor. It is a single-table stand-in for the compiled content localization system in [[16 - Localization]].

## 1. Behavior

- `L` toggles English ↔ German at runtime.
- Every user-facing editor string is keyed under `base.editor.*`.
- Missing keys render `!key!`; the locale smoke fails if that marker can reach a rendered control.
- German uses a decimal comma in numeric stats.
- Node names, smoke markers, file paths, and internal ids stay English.

## 2. Key groups

| Prefix | Contents |
|---|---|
| `base.editor.window` / `hud` | title, language HUD |
| `base.editor.status` | capture, edit, prop, undo, save/load, room selection |
| `base.editor.palette` | material/prop palette headings |
| `base.editor.material.*` | timber, stone, roof, glass, door |
| `base.editor.prop.*` | bed, stove, toilet, desk |
| `base.editor.stats` | solid/quads/mesh/chunks |
| `base.editor.help` | movement, editing, tools |
| `base.editor.blueprint` | save/load bar and dialogs |
| `base.editor.rooms` | headers, summary, room line/detail, overlay labels |
| `base.editor.room_type.*` | bedroom, kitchen, bathroom, office, living/dining, empty |
| `base.editor.room_class.*` | lower middle, middle, upper |
| `base.editor.generic` | fallbacks |

## 3. Smoke contract

```bash
GODOT=/path/to/godot ./scripts/smoke_locale.sh
```

Checks:

- EN/DE key parity (currently **111 keys** each).
- placeholder parity for every key.
- required room/material/prop/status keys.
- missing-key marker behavior.
- toggle EN → DE → EN.
- representative rendered strings contain no `!key!` or unresolved `{0}`.
- German room name check (`Schlafzimmer`).

`./scripts/smoke_layout.sh` additionally builds the real editor UI and checks 16 fixed-width labels/buttons against their minimum text size in EN and DE. It catches text-expansion regressions; it is not a pixel screenshot.

Output:

```text
LOCALE_SMOKE en=ok de=ok keys=111 status=ok
LAYOUT_SMOKE en=ok de=ok checks=16 status=ok
```

## 4. Limitations

- This is a slice-local table, not the compiled moddable language-pack pipeline.
- Missing keys intentionally render `!key!`; D-022's production fallback chain is not implemented yet.
- No font/layout screenshot test for German text expansion yet; the headless width budget is the current guard.
- Server/admin messages and developer logs remain English.
- Content names (materials/props) are still editor-local; the future content registry will own them.

## 5. Next

1. Move keys into the shared localization pipeline.
2. Add pseudo-localization and German text-expansion layout checks.
3. Add language selector settings persistence.
4. Localize the remaining slice/dialog strings as the UI grows.
