---
title: Research
project: Townsmith
status: active
updated: 2026-11-23
tags:
  - townsmith
  - phase-4
  - research
  - progression
---

# 61 — Research

> [!info] What this note is
> Phase 4.3 adds the city-wide research pool, a data-driven technology tree, and age gates on structural systems. Population generates a small administration rate. The population-derived age gates structural systems; the tree records unlocks and is ready to gate content as the library grows. Civic-building research output and a full tech-tree panel arrive with the content and tutorial slices.

## 1. Scope

In:

- `core_city::research`: `TechNode`, `TechTree`, `ResearchStore`, unlock rules, validation, JSON, checksum.
- A compiled-in 14-node tree across Wood, Masonry, Industrial, Modern, and Information.
- A starting pool of 50 points so a new Wood-age city can make its first choice.
- Population-based generation: **0.05 points per resident per game day**.
- Age, prerequisite, duplicate, and point-cost checks with stable error codes.
- Structural systems gain a minimum age; placement and preview reject an out-of-age building with `age_locked`.
- Bridge APIs, HUD line, dashboard section, `Y` unlock shortcut, save/load, and scene smoke.

Out:

- Per-building research output beyond schools. Basic/improved/advanced schools now add 8/20/50 points/day ([[62 - Civic Services]]); universities and libraries are still content work.
- A graph-panel UI with branches and search. The dashboard lists the pool; `Y` unlocks the cheapest available node.
- Research speed modifiers, policies, scholars, tech trading, or modded tree loading at runtime.
- Full content gating for materials, props, and blueprints. Only structural systems are gated in this slice.
- Age-up transitions or research-driven age thresholds; ages remain population-driven.

## 2. Model

```rust
pub struct TechNode {
    pub id: String,          // "construction.masonry"
    pub name: String,        // "Masonry construction"
    pub age: Age,            // minimum age
    pub cost: f64,           // research points
    pub prerequisites: Vec<String>,
    pub unlocks: Vec<String>, // content ids, for example "structure:steel_frame"
}

pub struct TechTree {
    pub format_version: u16,
    pub nodes: BTreeMap<String, TechNode>,
}

pub struct ResearchStore {
    pub format_version: u16,
    pub seed: u64,
    pub points: f64,
    pub lifetime_points: f64,
    pub completed: BTreeSet<String>,
}
```

`TechTree::validate` checks ids, finite positive costs, known prerequisites, and acyclic dependencies. `TechTree::from_json` accepts the same schema, so a future resource pack can replace the compiled tree without changing the pool or the bridge.

### Generation

```text
rate_per_day = population × 0.05
points += rate_per_day × seconds / 86,400
```

The starting grant is **50 points**; `lifetime_points` records total generation plus the grant. Points cap at `1e12`. A school or library will add to the daily rate in 4.4.

### Unlock rules

`ResearchStore::unlock(id, tree, age)` requires, in order:

1. the node exists in the tree;
2. it is not already complete;
3. the city age is at least the node age;
4. every prerequisite is complete;
5. the pool can pay the cost.

A success subtracts the cost and adds the node to `completed`. The pool is not time-locked: a player can save points and unlock several nodes at once.

## 3. Default tree

| Age | Node | Cost | Prerequisite | Unlocks |
|---|---|---:|---|---|
| Wood | `construction.timber_frame` | 20 | — | timber frame |
| Wood | `industry.farming` | 25 | — | farm |
| Wood | `utilities.well` | 30 | — | water |
| Masonry | `construction.masonry` | 100 | timber frame | stone, half-timber, brick |
| Masonry | `utilities.aqueduct` | 90 | well | large water |
| Masonry | `services.school` | 120 | masonry | school |
| Industrial | `industry.factory` | 500 | masonry | factory |
| Industrial | `construction.concrete` | 600 | masonry | reinforced concrete |
| Industrial | `utilities.power` | 700 | aqueduct | power |
| Industrial | `construction.steel` | 2,500 | concrete | steel frame |
| Modern | `services.hospital` | 3,000 | school | hospital |
| Information | `utilities.solar` | 6,000 | power | solar |
| Information | `services.university` | 8,000 | school | university |
| Information | `construction.composite` | 9,000 | steel | composite core |

The costs are a draft curve. They are intentionally reachable at the population thresholds: a 500-resident Masonry city generates 25 points/day, so a 100-point node takes four days before civic output. A school adds 8–50 points/day on top ([[62 - Civic Services]]).

## 4. Age gates

`StructuralSystem::min_age` returns a stable age id:

| Systems | Age |
|---|---|
| tent, timber shed, timber frame | wood |
| load-bearing stone, half-timber, load-bearing brick | masonry |
| reinforced concrete, steel frame | industrial |
| composite core | information |

`world_validate_placement` and `world_place_blueprint` compare the blueprint's structure (including a composite ground system) with the current population-derived age. A mismatch returns `age_locked` with the required and current age. Blueprints can still be authored and saved as drafts; only placement is blocked. This is the first content gate, not a full material or blueprint-age gate.

## 5. Bridge

| Method | Purpose |
|---|---|
| `research_state(seed)` | `open`, seed, age, points, lifetime, rate/day, completed count, node count, completed ids, node availability, checksum |
| `research_tick(seed, seconds)` | Add population-rate points; returns the state plus `gained` |
| `research_unlock(seed, node)` | Spend points; returns `ok`, `spent`, `node`, `name`, state, or `error_code` |
| `research_checksum()` | Stable FNV checksum |
| `research_clear()` | Close the pool; the next unlock starts a new 50-point pool |

Node availability flags are `unlocked`, `age_locked`, `prerequisites_met`, `affordable`, and `available` (all of the latter four true and not complete). The bridge validates the placement seed before opening or reading the pool.

## 6. Save/load

`world_save` writes an optional `research` object after industry. The load path:

1. parses and validates the pool against the compiled tree;
2. checks its seed;
3. rejects unknown completed nodes;
4. commits it atomically with every other store.

Old saves without `research` receive a new 50-point pool. Clearing placements does **not** clear research; city knowledge survives demolition, like the treasury.

## 7. HUD, dashboard, and controls

The status panel adds:

```text
Research: 50 pts · 0/14 techs · +0.1/day
```

The dashboard adds a **Research** section with points, rate, and completed count. `Y` unlocks the cheapest currently available node and reports the result in the modal. This is a developer shortcut; 4.4 replaces it with a proper tree panel.

The scene smoke:

- opens the pool after the acceptance house is occupied;
- generates points for one day and checks `gained > 0` and points above the 50-point starting grant;
- checks the 14-node tree and Wood age;
- unlocks timber framing, checks the cost and completed count;
- rejects a duplicate with `already_unlocked`;
- rejects reinforced concrete with `age_locked`;
- saves, clears the pool, reloads, and checks points, completed nodes, and checksum;
- marker includes `research=ok`.

## 8. Tests

- `core_city::research`: 14 tests — default tree and ages, starting pool, generation, unlock/spend, duplicate/unknown, age/prerequisite gates, insufficient points, availability flags, JSON/checksum, invalid state, prerequisite closure and spend consistency, invalid/cyclic trees, invalid costs.
- `core_city::progression`: 6 tests including age id aliases.
- `core_structure`: 19 tests + 1 doctest, including minimum-age mapping.
- `godot_ext`: 87 tests, including research JSON roundtrip/old save, unknown completed nodes, and structure age helpers.
- Workspace: **858 tests** (854 unit/integration + 4 doctests, all passing).

## 9. Limits

- Research generation is population plus covered school output; universities and libraries do not add output yet.
- The default tree is compiled in; runtime modded trees are not loaded.
- Only structural systems are gated by age; materials, props, and blueprint libraries are not.
- `Y` always chooses the cheapest available node; there is no branch UI or queue.
- The tree has no repeatable, mutually exclusive, or hidden nodes.
- Research is global, not per settlement or district.
- Costs and the 0.05 rate are draft balance, not a tuned campaign curve.
- `research_tick` is a developer bridge API with an arbitrary simulated delta; the authoritative server tick does not drive it yet.
- Changing or removing a node id can make an older save fail validation because completed node ids must exist in the compiled tree.
- Legacy blueprints with no structure metadata bypass the structural age gate.
- The tree records unlocks but placement still consults only the population-derived age.

## 10. Next

1. 4.4: starter content and age-specific trades; civic school output and service tiers are in [[62 - Civic Services]].
2. Load a tree from a resource pack and expose branch/search UI.
3. Gate materials and blueprint libraries by age, not only structure systems.
4. Add research policies and speed modifiers.
5. Feed research progress into the dashboard advisor and the 30-day runner.
