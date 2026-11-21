---
title: Playtest Kit
project: Townsmith
status: active
updated: 2026-11-21
tags:
  - townsmith
  - playtest
  - usability
  - phase-1
  - acceptance
---

# 66 — Playtest Kit

> [!info] What this note is
> The kit for the Q-101 human playtest: a 30-minute route through the Phase 1 vertical slice, an observer checklist, timing and performance notes, five rating questions, and a place to record the result. Technical acceptance is already recorded in [[40 - Slice Acceptance Results]]; this kit covers the **human** half.

> [!warning] One session per tester
> Use a fresh tester who has not read [[65 - Tutorial]] or the rest of this vault. The point of Q-101 is to find where a new player gets stuck, not to confirm that the team can follow its own route.

## 1. Setup

Run from the worktree that contains the build under test:

```bash
cd /media/Y/repos/Townsmith-playtest

# Build and copy the GDExtension library (debug is fine for a playtest).
./scripts/build_godot_ext.sh release

# Headless bridge/world-scene smoke before a human sits down.
GODOT=/home/chris/.local/bin/godot ./scripts/smoke_world_scene.sh

# Launch the playable world scene.
GODOT=/home/chris/.local/bin/godot ./scripts/run_godot.sh
```

Setup checks before the timer starts:

- [ ] The smoke marker includes `status=ok` (and `tutorial=ok`, `trades=ok`, `starter=ok`).
- [ ] The window opens in **Mayor Mode** at seed 2026; the HUD shows a blueprint, structure, and the tutorial line.
- [ ] `GODOT --version` is recorded; the extension in `godot/bin/` matches this build.
- [ ] No old save is loaded on startup. Do **not** press `F9` until the save/load step; it replaces the current session.
- [ ] Screen recording or screenshots are enabled if the tester consents.

`GODOT` is not on `PATH` everywhere; `/home/chris/.local/bin/godot` is the path used for this kit. Substitute the local binary if the machine differs.

## 2. The 30-minute route

The route deliberately follows the tutorial order in [[65 - Tutorial]]: starter build, house, industry, utility, service, save/load. Steps are independent in the simulation, so a tester who skips ahead is still valid data — record where they went.

| Time | Segment | Tester does | Observer looks for |
|---:|---|---|---|
| 0–3 | Orient | Read the HUD, pan/rotate, press `F1` to open and close the dashboard | Can they find the map controls and the objective? Do they understand Mayor vs Architect? |
| 3–8 | Starter build | `N` to found a settlement, `B` until the modal names **Cottage**, `R` to rotate, `LMB` to place | Is `B` discoverable? Is the green/red ghost understood? Is the construction site obviously a site? |
| 8–13 | House | `B` to **Longhouse** (or a second Cottage), place it, `RMB` select it to read capacity/rooms/jobs | Do they wait for construction? Do they understand why the house is not usable yet? |
| 13–19 | Industry | Place a **Workshop** and a **Sawmill**, `RMB` each, `I` until the modal says **lumberjack** and **farm** | Do they connect jobs to buildings? Do they find `I` without being told? |
| 19–22 | Utility | Place a **Market** (or reuse a job-slot building), `RMB`, `U` once for **water** | Is the well/water site visible? Do they see supply change in the HUD? |
| 22–25 | Service | Select a job-slot building, `K` once for **school** | Do they understand that a service is attached after construction, not a separate building type? |
| 25–29 | Save/load | `F5`, read the modal, `F9`, compare placement/site counts and the tutorial line | Do they trust the save? Is the load modal readable? Does anything visibly disappear? |
| 29–30 | Ratings | Answer the five questions in §5 | Note hesitation and the words they use, not just the score. |

> [!tip] If a site stalls
> Construction advances in 0.25 s ticks. A stalled site is usually crew or materials: `C` restores the crew pool and `V` adds 10,000 L of materials. Record that the tester needed this, and how they discovered it.

> [!note] Architect view is read-only in this slice
> `Enter` opens the selected building at its door; `M` or `Esc` returns to Mayor. Voxel editing happens in the separate editor scene, not here. Tell the observer, not the tester. If the tester expects to build voxel by voxel in-world, that is a finding.

## 3. Observer checklist

Record a timestamp for every item. Do not coach unless the tester has been stuck for two minutes; log the stuck moment either way.

### Controls (confirmed in the current build)

| Input | Action |
|---|---|
| `WASD` | Pan the Mayor camera |
| `Q` / `E` | Rotate the camera |
| Mouse wheel | Zoom |
| `R` | Rotate the placement 90° |
| `X` / `Z` | Raise / lower the placement target |
| `LMB` | Place at the target (24 m ahead) |
| `RMB` | Select a placed building |
| `N` | Found a settlement |
| `Enter` | Enter the selected building (read-only architect view) |
| `M` / `Esc` | Return to Mayor / release the mouse |
| `B` | Cycle the available starter buildings |
| `I` | Cycle industry: lumberjack → farm → hunt → off |
| `U` | Cycle utility: water → power → off |
| `K` | Cycle service: school → clinic → fire → police → off |
| `C` | Toggle the construction crew (0 / 4) |
| `T` | Cycle tax policy |
| `L` | Borrow 50,000 |
| `Y` | Research the next technology |
| `V` | Add 10,000 L of construction materials |
| `F1` | Toggle the dashboard |
| `F5` / `F9` | Save / load the world |
| `G` / `H` | Road mode / lot mode |
| `Delete` | Clear all placements (destructive; do not use during a scored run) |

### During the run

- [ ] First action after spawn: what did they try first, and why?
- [ ] Did they find `B` before placing, or place the default blueprint first?
- [ ] Did they notice the red/green ghost and the target cell?
- [ ] Did they understand the construction phase and trade label (`Foundation · Mason 42%`)?
- [ ] Did they use `RMB` to inspect a building, or only place?
- [ ] Did they connect `I`, `U`, and `K` to the selected building?
- [ ] Did the tutorial line (`Tutorial: n/5 · …`) influence the next action?
- [ ] Did the HUD's industry/utility/service/traffic lines change in a way they noticed?
- [ ] At the save step, did they expect a file picker, a toast, or something else?
- [ ] After `F9`, did they verify anything, or assume it worked?

### Always log

- Exact modal text when something failed or surprised them.
- Time lost to a single blocker.
- Any UI overlap, clipping, unreadable text, or missing cursor.
- Any stall, hitch, or freeze, with the action that triggered it.
- Quotes in the tester's own words; paraphrase only when necessary.

## 4. Timing and performance notes

This is a **functional walkthrough**, not a benchmark. Do not compare its frame times with [[15 - Benchmarks]], [[23 - City Rendering Benchmark]], or [[39 - Slice House Benchmark]].

Record:

| Item | Value |
|---|---|
| Hardware (CPU / GPU / RAM) | |
| OS and Godot version | |
| Resolution / window mode / vsync | |
| Input device | |
| Locale (EN / DE) | |
| Build commit (`git rev-parse --short HEAD`) | |

Watch for:

- Hitches longer than ~250 ms on placement, construction completion, save, or load.
- Terrain stream-in or mesh pop when panning quickly.
- Frame rate collapse after several buildings; note the placement count.
- Construction that never completes; check crew and materials before calling it a bug.
- Save/load that changes placement, road, lot, settlement, or construction counts. The load modal reports the rebuilt count; a mismatch is a real failure.

The recorded technical baseline is 21 houses at 6,195 FPS average / 4,484 1% low on an RTX 4080 at 1080p ([[40 - Slice Acceptance Results]]). That number is context, not a target for this test.

## 5. Five rating questions

Ask after the save/load step, before discussing anything. Use a 1–5 scale: **1 = strongly disagree, 5 = strongly agree**. Add one line of “why” for any score of 1–2 or 5.

| # | Question | Score (1–5) | Why / quote |
|---|---|---:|---|
| R1 | I always knew what to do next. | | |
| R2 | Placing a building and watching it get constructed felt clear and responsive. | | |
| R3 | I understood how industry, utilities, and services changed the city. | | |
| R4 | Saving and loading gave me confidence that nothing was lost. | | |
| R5 | I would play another 30 minutes without a guide. | | |

Optional single question at the end: **What is the one thing you would fix first?**

## 6. Top-three usability issue template

Rank by severity and frequency, not by how easy the fix is. Use one block per issue.

| Rank | Route step | Issue in one line | Severity | Evidence |
|---:|---|---|---|---|
| 1 | | | blocker / major / minor | |
| 2 | | | blocker / major / minor | |
| 3 | | | blocker / major / minor | |

### Issue 1

- **Title:**
- **Route step and timestamp:**
- **What the tester did:**
- **What they expected:**
- **What happened:**
- **Repro (exact keys/clicks):**
- **Severity:** blocker / major / minor
- **Evidence:** screenshot, recording, modal text, or HUD line
- **Suspected cause:**
- **Suggested fix (one line):**
- **Owner / follow-up:**

### Issue 2

- **Title:**
- **Route step and timestamp:**
- **What the tester did:**
- **What they expected:**
- **What happened:**
- **Repro (exact keys/clicks):**
- **Severity:** blocker / major / minor
- **Evidence:**
- **Suspected cause:**
- **Suggested fix (one line):**
- **Owner / follow-up:**

### Issue 3

- **Title:**
- **Route step and timestamp:**
- **What the tester did:**
- **What they expected:**
- **What happened:**
- **Repro (exact keys/clicks):**
- **Severity:** blocker / major / minor
- **Evidence:**
- **Suspected cause:**
- **Suggested fix (one line):**
- **Owner / follow-up:**

Severity guide:

- **Blocker:** the tester cannot continue the route without help.
- **Major:** the tester continues but misunderstands a core system or loses work.
- **Minor:** friction, wording, or polish; the route still completes.

## 7. Recording the Q-101 result

Copy this section into the run record, or fill it in place. Keep the raw notes; do not clean them up before the team reads them.

### Run metadata

| Field | Value |
|---|---|
| Date | |
| Tester (name or ID) | |
| Prior Townsmith experience | none / some / team member |
| Observer | |
| Build commit | |
| Godot version | |
| Hardware | |
| Locale | |
| Session length | |

### Route completion

| Segment | Completed? | Minutes | Notes |
|---|---|---:|---|
| Orient | yes / no | | |
| Starter build | yes / no | | |
| House | yes / no | | |
| Industry | yes / no | | |
| Utility | yes / no | | |
| Service | yes / no | | |
| Save/load | yes / no | | |
| Ratings | yes / no | | |

### Result

| Field | Value |
|---|---|
| R1–R5 scores | |
| Mean score | |
| Top issue | |
| Second issue | |
| Third issue | |
| Session verdict | pass / pass with issues / fail |
| Q-101 answered? | yes / no |

**Verdict note (two or three sentences):**

```text
```

**What the tester said at the end:**

```text
```

### Closing the loop

1. Add the three issues above (or link the filed issues) to the team backlog.
2. Move Q-101 to **Answered** in [[09 - Open Questions]] with the date, the mean score, and the top issue; add an ADR only if the finding changes a decision.
3. Update [[40 - Slice Acceptance Results]]: the technical result does not change, but the slice is only fully accepted once Q-101 is answered.
4. Update [[00 - Start Here]] status and next actions.
5. If a finding invalidates the tutorial order or wording, update [[65 - Tutorial]] in the same PR.
6. Keep this file as the template; add each run as a dated section or a linked run note, not by overwriting the template.

## 8. Limits

- One 30-minute session cannot cover multiplayer, localization switching, or the standalone editor.
- The route uses Wood-age starters only; later ages are out of scope.
- Architect view is read-only here, so the test does not measure voxel authoring usability.
- Construction timing in the scene is compressed for play; do not infer the Phase 4 economy balance from it.
- Observer notes are subjective; the scores are a signal, not a statistical result.
