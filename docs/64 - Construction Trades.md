---
title: Construction Trades
project: Townsmith
status: active
updated: 2026-11-21
tags:
  - townsmith
  - phase-4
  - construction
  - trades
---

# 64 — Construction Trades

> [!info] What this note is
> Phase 4.4c adds the age-aware trade vocabulary and roster behind construction. It caps the shared crew pool by the trades a town can actually field, labels each site phase with its preferred trade, and makes the age transition visible in the HUD. Per-trade queues and training are later.

## 1. Scope

In:

- six trades: laborer, carpenter, mason, steelworker, electrician, engineer;
- an age-derived roster with counts;
- a preferred trade per construction phase;
- `effective_crew = min(configured crew, roster total)`;
- optional trade roster in the construction save;
- bridge summary and site payload fields;
- HUD/label trade display;
- smoke coverage and a `trades=ok` marker.

Out:

- per-trade crew queues or one worker per slot;
- trade skills, apprentices, training, or wages;
- construction offices/public works;
- trade-specific speed multipliers;
- imported contractors or strikes;
- material or prop gating by trade.

## 2. Roster

`core_city::trades`:

```rust
pub enum Trade {
    Laborer,
    Carpenter,
    Mason,
    Steelworker,
    Electrician,
    Engineer,
}

pub struct TradePool {
    pub capacity: BTreeMap<Trade, u16>,
}
```

| Age | Laborer | Carpenter | Mason | Steelworker | Electrician | Engineer | Total |
|---|---:|---:|---:|---:|---:|---:|---:|
| Wood | 4 | 2 | 1 | — | — | — | 7 |
| Masonry | 4 | 3 | 3 | — | — | 1 | 11 |
| Industrial | 4 | 3 | 3 | 2 | 2 | 1 | 15 |
| Modern | 4 | 3 | 3 | 3 | 3 | 2 | 18 |
| Information | 4 | 4 | 4 | 4 | 4 | 4 | 24 |

The roster is derived from `AgeState`, not saved as a separate age record. The construction store may carry a serialized `trades` option for exact save/load; older saves leave it `None` and keep the legacy aggregate crew.

## 3. Phase trades

| Phase | Preferred trade |
|---|---|
| Survey | Laborer |
| Foundation | Mason |
| Structure | Carpenter |
| Envelope | Carpenter |
| Utilities | Electrician |
| Finish | Carpenter |
| Inspection | Engineer |
| Complete | — |

If the roster has no crew for the preferred trade, the site reports a laborer instead. A Wood-age site therefore never displays an electrician it cannot field.

The simulation still uses one aggregate crew pool. The roster only caps its size:

```text
effective_crew = min(configured_crew_capacity, roster.total())
```

This keeps the deterministic Phase 3 scheduling while making later ages able to support larger crews. Per-trade contention is a later slice.

## 4. Bridge and UI

| Method / field | Purpose |
|---|---|
| `construction_set_trades(seed)` | Derive the current age and install its roster |
| `construction_summary().trades` | Roster entries `{trade, name, count}` |
| `construction_summary().effective_crew` | Capped crew pool |
| `construction_sites().sites[].trade` | Preferred trade for that site's phase |
| `construction_sites().sites[].trade_name` | Display label |

The scene refreshes the roster before each construction tick. The HUD shows:

```text
Construction: 1 active / 0 stalled   Crew: 4/4   Trades: 3   Materials: 1000000 L
```

The world-space construction label adds the phase trade:

```text
Foundation · Mason 42%
```

## 5. Persistence

`ConstructionStore.trades` is optional and serde-defaulted. A save written by the scene includes the roster; a legacy save or the headless runner omits it and uses the configured crew capacity unchanged. `validate` rejects empty/oversized trade entries and a total above 256.

## 6. Smoke

`smoke_world_scene.sh` checks after the first placement:

- the roster has at least one trade;
- effective crew is at least one;
- the site payload has a trade;
- save/load keeps the construction state valid;
- marker includes `trades=ok`.

## 7. Tests

- `core_city::trades`: 4 tests — Wood roster, later-age specialists, aliases, clamping/validation.
- `core_city::construction`: 3 new tests — roster cap and crew stall, JSON/legacy roundtrip, phase fallback.
- `core_city`: **260 tests**.
- `godot_ext`: **58 tests** (the scene smoke covers the bridge path).
- Workspace: **537 tests + 2 doctests (539 passed)**.

## 8. Limits

- One aggregate crew pool; the roster only caps total capacity.
- No per-trade queues, so a missing electrician does not stall utilities.
- No trade wages, training, apprentices, or construction offices.
- No speed bonus by trade beyond the total cap.
- The scene installs the roster on every construction tick; the runner leaves it unset.
- Trade counts are draft balance values.

## 9. Next

1. 4.5: tutorial scenario and the wood-to-industrial balance pass.
2. Per-trade crew queues and stall reasons.
3. Construction offices, training, and contractor imports.
4. Trade-aware wages and labor-market demand.
5. Trade coverage in the dashboard and advisor.
