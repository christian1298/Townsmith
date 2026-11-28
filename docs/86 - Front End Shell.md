---
title: Front End Shell
project: Townsmith
status: active
updated: 2026-11-28
tags:
  - townsmith
  - ui
  - shell
  - gamepad
---

# 86 — Front End Shell

> [!abstract] What this is
> The first player-facing entry scene. It replaces the old “boot straight into the Phase 0 terrain demo” behavior with a small keyboard/gamepad navigable launcher over the existing previews. It is not a campaign menu: there are no save slots, accounts, or settings persistence yet.

Related: [[00 - Start Here]], [[02 - Game Design]], [[30 - Editor Shell]], [[44 - Mayor Mode]], [[57 - Dashboard and Advisor]], [[85 - Connected Settlement Surface]].

## 1. Scene map

`godot/project.godot` now starts at `main_menu.tscn`.

| Menu item | Target | Notes |
|---|---|---|
| New City | `world_placement.tscn` | Existing Mayor dashboard / placement preview |
| Blueprint Editor | `editor.tscn` | Existing voxel authoring space |
| Terrain Preview | `main.tscn` | Phase 0 terrain and city benchmark scene |
| Multiplayer | `mp_world_client.tscn` | Needs `townsmith-server --listen PORT` |
| Settings | In-menu placeholder | No settings persistence yet |
| Quit | — | Exits the process |

The menu forwards `--smoke`, `--volume-smoke`, and `--world-placement-smoke` to `main.tscn`, so `scripts/smoke_godot.sh` and the existing headless gates keep working unchanged.

## 2. Controls

- Up/Down or D-pad: move focus with a visible ring.
- Enter / south face: activate.
- Esc / east face: close the settings overlay, or return focus to New City.
- The Settings overlay is a placeholder and says so; it does not write a config file.

All strings are keyed through `world_i18n.gd` in English and German. The menu adds 18 world keys; the suite currently reports **511** world keys after the chat and settlement accessibility additions.

## 3. Smoke

```bash
GODOT=/path/to/godot ./scripts/smoke_main_menu.sh
```

The script builds the debug extension, imports with one retry, runs the menu twice, and requires identical markers:

```text
MAIN_MENU status=ok buttons=6 focus=new_city checks=N checksum=3d4a58ba
MAIN_MENU_SMOKE runs=2 status=ok ...
MAIN_MENU_FORWARD --smoke chunks=9 status=ok
```

The forward probe runs the legacy `-- --smoke` path and asserts the terrain streaming marker, proving the menu does not hide the Phase 0 gate.

## 4. Limits

- No save-slot picker, continue screen, profile, or settings persistence.
- Multiplayer opens the debug world client; it does not discover or start a server.
- The menu is a launcher, not an onboarding flow; tutorial copy and first-run guidance remain Phase 8 work.
- No mouse-only affordance beyond the buttons themselves; focus is keyboard/gamepad first.
