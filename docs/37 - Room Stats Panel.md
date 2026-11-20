---
title: Room Stats Panel
project: Townsmith
status: active
updated: 2026-11-20
tags:
  - townsmith
  - vertical-slice
  - editor
  - rooms
  - ui
---

# 37 — Room Stats Panel

> [!info] What this note is
> The editor's read-only room summary: totals, one line per detected room, and a selected-room detail block. It is the inspector half of the room view; the 3D boxes and labels are described in [[34 - Room Overlay]].

## 1. Controls

| Input | Action |
|---|---|
| **R** | Toggle the room view: 3D boxes, labels, and this panel together |
| **]** | Select the next room (wraps) |
| **[** | Select the previous room (wraps) |

- The panel starts hidden at launch, like the overlay.
- **R hides the panel with the overlay.** Selection is retained while hidden, so reopening the room view returns to the same room.
- `[` / `]` use the layout-aware keycode, not the physical key position, so they follow the labelled bracket keys on non-US layouts.
- Cycling is only meaningful while the room view is visible; pressing `[` or `]` with it hidden prints `Room view is off (R)` instead of changing the selection.
- The panel is mouse-transparent (`MOUSE_FILTER_IGNORE`), so it never blocks crosshair editing or viewport clicks.

## 2. Panel contract

The panel is a right-side `PanelContainer` containing one `RichTextLabel`. Built in `_build_room_panel()`; populated by `_refresh_room_panel()`.

Plain-text layout (UTF-8 separators; `>` marks the selected room):

```text
ROOM STATS   [ ] select room   R hide
rooms 1 · capacity 1 · jobs 0 · class lower_middle
solid 34320 · material kinds 3

> room_0 · bedroom · floor 0 · 14.4 m² · beds 1 · doors 4 · windows 8

selected room_0 · bedroom · floor 0
area 14.4 m² · volume 43.3 m³ · cap +1 · jobs +0
bounds x 0.1-3.9 m · z 0.1-3.9 m · y 0.0-3.0 m
```

- **Totals:** room count, resident capacity, job slots, class, solid voxels, and the number of material kinds in `materials`.
- **Room list:** every detected room, in detector order (`room_0`, `room_1`, …), with type, floor, area, beds, doors, and windows.
- **Selected detail:** id, type, floor, area, volume, capacity contribution, job contribution, and the floor-band bounds in metres.
- `_room_panel_plain` keeps the untagged text for smoke assertions; the label currently renders the same text.

All numbers come from one `TownsmithCore.editor_room_stats()` snapshot (the same dictionary documented in [[34 - Room Overlay]]). No Rust API was added for this panel.

## 3. Behavior

- The overlay and the panel share one detection pass: `_rebuild_room_overlay()` obtains `stats`, clamps the selection, draws the boxes, then calls `_refresh_room_panel(stats)`.
- **Edits:** the panel refreshes when the debounced volume remesh rebuilds the room view (250 ms after an edit), matching the overlay.
- **Props:** placing or removing a prop rebuilds the room view immediately, so the panel updates at once.
- **Undo/redo:** both mark the volume dirty and go through the same remesh/rebuild path.
- **Load:** clears the selection, prop nodes, and history; the room view, if visible, is rebuilt from the loaded volume and the selection defaults to `room_0`.
- **Room count changes:** the selection is clamped to `0..rooms.size()-1`. With zero rooms it becomes `-1`, the list shows `no enclosed rooms`, and cycling reports `No rooms to select`.
- If detection fails, the panel hides and the status line shows the error.

## 4. Selection highlight

- The selected room's overlay box uses `ROOM_SELECTED_COLOR` (warm yellow) at alpha 0.45; other rooms keep their type color at alpha 0.22.
- The selected 3D label is prefixed with `>` and now shows the room id line as well.
- Labels keep the existing `Label3D` billboard, 48 px font, 8 px black outline, and no depth override, so they remain readable against walls and sky.

## 5. Derived values

- `cap +N` is the room's `beds` field, which is exactly the detector's capacity contribution.
- The bridge does not expose per-room desk counts. `jobs +N` is therefore counted in the UI from desk props (`kind 7`) whose origin falls inside the room's X/Z bounds and its 3 m floor band. This matches the detector's prop-to-room assignment for normal layouts, but it is an inference, not a second source of truth.
- Bounds are the floor-band interior rectangle with exclusive max, converted from the payload's `bounds_m`. `y` is the full 3 m band, even for a partial top band.

## 6. Smoke

`--room-overlay-smoke` now also builds the real panel in a minimal `CanvasLayer` and asserts:

- the panel exists and is visible after **R**;
- the text contains `room_0 · bedroom`;
- the text contains `capacity 1`;
- the selected index is valid and points at the bedroom.

Output:

```text
ROOM_OVERLAY_SMOKE rooms=1 nodes=2 type=bedroom capacity=1 status=ok
ROOM_PANEL_SMOKE rooms=1 selected=bedroom capacity=1 status=ok
```

`scripts/smoke_editor.sh` parses both markers and fails on a mismatch.

## 7. Limitations

- Read-only: no click-to-select, scrolling to a room, or editing from the panel. Selection is keyboard-only.
- One detection pass per room-view rebuild. Cycling with `[` / `]` rebuilds the whole overlay and re-runs detection (~2 ms for the test house); there is no dirty-room diff.
- The panel refreshes on the 250 ms remesh cadence for voxel edits, not per click.
- Fixed 500 × 360 px panel; on very narrow viewports it can overlap the bottom-left help text. Long room lists scroll inside the label.
- Job contribution is inferred from desk props (Section 5), not returned per room.
- English strings and ASCII/UTF-8 separators are hard-coded until the localization pass ([[16 - Localization]]).
- Room IDs renumber when rooms are added or removed, so a remembered selection follows the index, not the identity of the room.
