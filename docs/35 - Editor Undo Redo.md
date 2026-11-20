---
title: Editor Undo Redo
project: Townsmith
status: active
updated: 2026-11-20
tags:
  - townsmith
  - vertical-slice
  - editor
  - undo
---

# 35 — Editor Undo/Redo

> [!info] What this note is
> The editor's first undo/redo history for voxel edits. See [[30 - Editor Shell]].

## 1. Behavior

- `Ctrl+Z` undoes the last edit.
- `Ctrl+Y` redoes the last undone edit.
- Place and remove both record one change: cell, old material, new material.
- The history is capped at 500 entries; the oldest is dropped.
- A new edit clears the redo stack.
- Loading a blueprint clears both stacks so undo cannot cross volume identities.
- Undo/redo marks the volume dirty and uses the same 250 ms debounced remesh.
- The room overlay, if visible, refreshes after the remesh.

## 2. Data shape

```text
_history: Array[batch]
  batch: Array[{ x, y, z, old, new }]
_redo: Array[batch]
```

Batches intentionally allow future multi-voxel brushes to undo as one action.

## 3. Smoke

`scripts/smoke_editor.sh` now runs three headless contracts:

1. `EDITOR_SMOKE`
2. `ROOM_OVERLAY_SMOKE`
3. `UNDO_SMOKE`

Current undo output:

```text
UNDO_SMOKE undo=ok redo=ok status=ok
```

The smoke sets a voxel, undoes it to air, and redoes it to the same material.

## 4. Limitations

- Only place/remove single-voxel edits are recorded; paint, boxes, copy/paste, and blueprint loads are not undoable yet.
- No coalescing of rapid repeated edits; every click is one history entry.
- History is session-only and not saved.
- Undo/redo is main-thread and triggers a full volume remesh; a dirty-chunk path is still future work.
- No UI buttons yet; keyboard only.

## 5. Next

1. Prop placement.
2. Room selection and stats panel.
3. EN/DE strings.
4. Dirty-chunk remeshing.
5. 21-house performance pass.
