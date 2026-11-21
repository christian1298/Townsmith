---
title: Named Citizens
project: Townsmith
status: active
updated: 2026-11-21
tags:
  - townsmith
  - simulation
  - citizens
  - agents
  - planning
---

# 67 — Named Citizens

> [!info] What this note is
> The plan for the first persistent individual-citizen layer: named identities, selection, a small materialized agent set, and a v1 lifecycle. It is additive to the existing aggregate household runtime; it does not replace cohorts or add a second population simulation.

> [!decision] D-007 / D-014 / D-017 — Identity where the player looks
> Every resident can have a stable identity, but only a bounded set is named, materialized, or simulated individually. Households and cohorts remain authoritative for the 1M population. NPCs still do not pathfind through building interiors (D-014).

## 1. Goals and non-goals

### Goals

- Persistent, deterministic **named citizens** for selection, stories, advisor samples, and the "follow one citizen" exit proof in [[49 - Phase 3 Slice Plan]].
- A stable identity that survives materialization/dematerialization, save/load, and job changes.
- Named citizens attach to the existing household model; aggregate counts remain the source of truth.
- A materialization policy that fits the **2,000 active / 200 detailed** budget ([[19 - Active Agent Benchmark]]).
- A small, readable v1 lifecycle: move-in, job change, needs view, aging, death.
- Deterministic tests and benchmarks with fixed seeds and no wall-clock dependence.
- Localizable UI and privacy-safe fictional names.

### Non-goals (v1)

- Simulating every citizen individually at 1M.
- Per-citizen pathfinding or interior navigation.
- Dialogue trees, conversation, voice, or quests.
- Family trees, marriages, births/pregnancies, or inheritance.
- Per-citizen inventories, phones, money, or consumption.
- A save migration framework. The first slice adds an optional, tolerant field only.
- Player-authored or real names; no user text in the first slice.
- District/cohort membership at 1M (the first slice is household-anchored).

## 2. The model: aggregate, named, materialized, detailed

| Level | Count | Authority | Update | Persistence |
|---|---|---|---|---|
| **Aggregate resident** | up to 1M | Household (`residents`, `workers`, needs) | 1 Hz household tick | Count only |
| **Named citizen** | pool cap 4,096 per save in v1 | Household slot + identity record | 1 Hz sync; events | Committed records only |
| **Materialized agent** | ≤ 2,000 city-wide | Named identity + route/position | 10 Hz | Position is presentation; not saved |
| **Detailed citizen** | ≤ 200 per client | Materialized + selected/visible | 10 Hz + UI | Selection is a client preference |

Key invariants:

1. The household is authoritative for population, housing, work counts, needs, and income. A named citizen never adds a resident.
2. `sum(named residents in a household) <= household.residents`. One named citizen per occupied slot.
3. Named citizens are a **view + identity** over the household. Their needs are `household needs + deterministic variance`, clamped.
4. Only lifecycle events (move-in, job change, death) mutate the household. Materialize/dematerialize never does.
5. The 2,000 active cap is server-wide; the 200 detailed cap is per client. Vehicles and animals use separate budgets.
6. No citizen is named twice; `CitizenId` is unique for the save lifetime.

The first slice uses a bounded named pool, not one identity per resident. This is a deliberate departure from the eventual 1M identity store: it keeps saves small and avoids a 1M-row generator before it is needed. The same identity function scales later; the pool cap is configuration, not architecture.

## 3. Identity, naming, and seed rules

### Identity

```text
citizen_seed = mix64(world_seed, placement_id, slot, generation, IDENTITY_VERSION)
citizen_id   = first 63 bits of citizen_seed, with collision probe
```

- `placement_id`: the home placement id (u64), stable across saves.
- `slot`: resident index `0..capacity`, stable while occupied.
- `generation`: increments when a slot is reused after a death or move-out, so a new person never inherits an old id.
- `IDENTITY_VERSION`: bumped only with a deliberate identity migration; changing it invalidates old derived names.
- IDs are nonzero, sorted in output, and never reused within a save.
- Collision handling: linear probe `+1` until unused; deterministic because allocation order is sorted.
- No UUIDs, no wall clock, no global RNG.

### Naming

- A name is **given name + family name** from content tables (`base:names/given`, `base:names/family`) with weights and optional locale tags.
- Deterministic draw: `mix64(citizen_seed, NAME_TABLE_VERSION)` selects one entry per list; no per-name RNG stream.
- Family names are shared by household members by default (drawn once per household from the household id), so a household reads as a household without a family tree.
- Occupation, age band, and needs do not affect names.
- Names are **proper nouns**, not translated UI. They are stored as display strings from the shipped or mod pool. UI labels around them are key-based ([[16 - Localization]]).
- Pools are validated at load: non-empty, no duplicate ids, length limits (1–32 chars), no control characters.
- No real personal data; no player names copied into NPC names; no user-supplied names in v1.
- A shipped blocklist filters slurs and reserved names; mod pools are validated with the same filter on a server.

### Age

- `age_days` starts from a deterministic band at move-in: adults 18–64, with a household-level offset so a household is not all the same age.
- Age advances only while the citizen is named and the world advances time. No aging while paused.
- Bands: child (<18), adult (18–64), elder (65+). v1 has no children (no births); named citizens are adults or elders.
- `age_days` is persisted; the display is derived.

> [!note] Current bridge placeholder
> Until N4 persists lifecycle state, `named_citizens_for` derives a display age of 18–64 years from the identity id (`age_years`). It is deterministic and never saved; `age_days` persistence and aging remain N4.

## 4. Home, work, and service assignment

### Home

- A named citizen is bound to `(placement_id, slot)`. The household is created by the existing move-in path ([[50 - Households and Needs]]).
- On move-in, new residents are eligible for naming in ascending slot order. Naming is lazy: a slot is named when selected, sampled for a story, or included in the named-pool fill.
- On move-out or death, the slot is retired and the household `residents` decreases. A later move-in can reuse the slot with `generation + 1`.
- Demolition or removal retires all named citizens in that household.

### Work

- The household job assignment stays the existing nearest-slot algorithm with road distance ([[51 - Jobs and Wages]], [[56 - Traffic]]).
- Named workers are a **view** over the household's assignment: worker slots are assigned in ascending citizen slot order to the household's workplace assignment sequence.
- A named citizen stores `workplace: PlacementId`, `job_slot: u16`, `role: ContentId`, and `wage_per_day`.
- If the household assignment changes, affected named workers emit `CitizenJobChanged` and update. The household `workers` count is unchanged by the named view.
- Unemployed named adults keep `job: None`; they are not hidden.
- No shifts, attendance, qualifications, or per-citizen wages in v1. The wage is the fixed household wage divided across workers for display, not a second economy.

> [!note] Derived employment
> Until per-slot jobs exist, `named_citizens_for` marks the first `household.workers` resident slots as employed (`slot_is_employed`). The aggregate count stays authoritative and the view is not saved.

### Services

- Service assignment is **derived from coverage**, not a queue: school, clinic, fire, police, and transit coverage come from the service and utility stores ([[55 - Utilities]], [[62 - Civic Services]]).
- A named citizen resolves `service_school`, `service_clinic`, and similar to the nearest covering site for the UI. These are not persisted; they are recomputed on demand.
- In v1, children and school assignment are out of scope; the clinic is the only service with a per-citizen story hook ("visited the clinic").
- Commute is the household `commute_m`; a named worker's route uses the same home/work pair. Do not run a second assignment.

## 5. Lifecycle v1

```text
Unnamed slot → Named (move-in / sample)
Named → Employed / Unemployed (household assignment)
Named → Materialized → Detailed (camera / selection)
Materialized → Named (dematerialize)
Named → Retired (death / move-out / demolition)
```

### Move-in

- Household move-in is unchanged. A new resident increments `residents`; the slot becomes eligible for naming.
- `CitizenMovedIn` fires once per named identity, not per aggregate resident.
- A named citizen's arrival is deterministic from `(placement_id, slot, generation)`.

### Job change

- On assignment change, compare the stored job to the new view; emit one event per changed named worker.
- No quit or refusal model in v1; the household assignment is authoritative.

### Needs

- The household `food`, `water`, and `rest` remain authoritative.
- A named citizen displays `need = clamp(household_need + variance, 0, 1)`, where `variance` is a fixed `i8` per need in `[-8, +8]` (about ±0.03) drawn from the citizen seed.
- Materialized agents may accumulate transient presentation modifiers (commute fatigue, noise) that decay; they never write back to the household unless a threshold event fires. There are no threshold events in v1.
- Needs do not update while paused.

### Aging and death

- Daily tick: `age_days += 1` for named citizens. At band transitions, emit `CitizenAged` (UI only).
- Death check at the daily tick:
  - base daily probability by band: adult `0.00002`, elder `0.0004`;
  - modified by clinic coverage, which reduces it by up to 25%;
  - rolled from `mix64(citizen_seed, day, LIFECYCLE_VERSION)`.
- On death: emit `CitizenDied`, retire the identity, and decrement household `residents` by 1.
- The vacancy can be refilled by normal move-in later; there are no births in v1.
- Lifecycle is **off by default** in the existing benchmark profiles and **on** in a dedicated `lifecycle` scenario. This keeps the pinned city-loop checksums stable until the slice that intentionally re-baselines them.
- Death runs only for named citizens. The named pool is a story sample, not a demographic model; city-scale mortality belongs to the cohort layer.

### Events

| Event | Meaning | Persisted? |
|---|---|---|
| `CitizenMovedIn` | A named identity was created for a household slot | Yes, in the journal |
| `CitizenJobChanged` | The household assignment changed this worker's job | Yes |
| `CitizenAged` | Age band changed | No (derived from `age_days`) |
| `CitizenDied` | The identity was retired and the household lost a resident | Yes |
| `CitizenMaterialized` | The citizen entered the active set | No (presentation) |
| `CitizenDematerialized` | The citizen left the active set | No (presentation) |

## 6. Materialization and dematerialization

### Budgets

| Budget | Value | Scope |
|---|---:|---|
| Active citizens | 2,000 | Server-wide |
| Detailed citizens | 200 | Per client |
| Named pool | 4,096 | Per save |
| Materialize/dematerialize per tick | 64 | Server-wide |
| Minimum dwell | 2 s | Per citizen |
| Materialize radius | 80 m | Camera |
| Dematerialize radius | 120 m | Camera |

- The active cap is shared across all players' areas of interest; each client requests candidates and the server allocates by priority.
- A selected citizen is pinned and always counts against the cap.
- Vehicles and animals use separate visual budgets; they do not consume the citizen cap.

### Priority

1. Selected or pinned by a story or tutorial.
2. Nearest to any camera, weighted by distance band rather than exact screen pixels.
3. Anti-starvation: longest time since last materialization.
4. `CitizenId` ascending as a deterministic tie-break.

### Hysteresis

- Materialize at ≤ 80 m; dematerialize at ≥ 120 m.
- Never dematerialize a selected or pinned citizen.
- At most 64 changes per 100 ms tick; defer the rest.
- No materialization churn while paused. Selection can pin a static citizen for inspection.

### Movement

- No per-citizen pathfinding. A materialized citizen follows a **cached route polyline** for its home → work pair.
- Routes are built once per `(home, work)` pair from the existing road graph and flow field ([[56 - Traffic]]); if the pair is disconnected, a straight line is used with a visible fallback.
- At most one route build per tick; the route cache is LRU with a hard cap (e.g., 512 pairs).
- Agents walk on sidewalks when the pedestrian graph exists; until then, use the road polyline with a lane offset.
- No interior movement: entering a building hides the agent (D-014).
- Active agents update at 10 Hz; rendering interpolates.

### State folding

- The household remains authoritative. Dematerialization discards presentation-only position and route state.
- Job changes and deaths are committed immediately; they are not deferred to dematerialization.
- Do not add materialized needs back into the household; that would double-count.
- The player must never see a citizen pop. Hide spawn and despawn behind distance, a door, or the selection card; fade over 0.2 s if within view.

## 7. Persistence sketch

The existing world JSON `population` object gets an optional sibling `named_citizens` object. This is additive; old saves load without it. No migration framework in the first slice.

```json
"named_citizens": {
  "format_version": 1,
  "seed": 123456789,
  "identity_version": 1,
  "name_table_version": 1,
  "next_serial": 42,
  "pool_cap": 4096,
  "citizens": [
    {
      "id": 1844674407370955161,
      "home": 17,
      "slot": 0,
      "generation": 0,
      "given": "Mara",
      "family": "Voss",
      "age_days": 12140,
      "job": {
        "workplace": 42,
        "slot": 3,
        "role": "base:clerk",
        "wage_per_day": 20.0
      },
      "needs_variance": [-4, 2, 1],
      "flags": 0,
      "created_tick": 88210
    }
  ]
}
```

- Validation: id nonzero and unique; home exists and is completed; slot < household capacity; generation monotonic; age in range; job references a completed workplace and a valid slot; variance in `[-8, 8]`; strings valid UTF-8 and length-capped.
- Orphans: a named citizen whose home is missing or incomplete is dropped with a warning; the household sync already handles the aggregate.
- Checksums: `named_checksum` is separate. Existing city and population checksums stay pinned while named data is derived and lifecycle is off. The persistence slice re-baselines intentionally.
- Save size: about 96 B per record; 4,096 records ≈ 400 KB uncompressed. Fine for JSON.
- Position, route, camera, and selection are not saved. A selected citizen id is a client preference, not world state.
- No migration in the first slice: an unknown `format_version` is ignored with a warning and the named pool starts empty.

## 8. Bridge and UI surfaces

### Bridge

- `citizen_state(seed, citizen_id)` → identity, home, job, needs, age, flags.
- `citizens_in_household(seed, placement_id)` → bounded list for the building panel.
- `citizens_near(seed, center, radius, limit)` → candidates for selection; does not materialize.
- `citizen_select(seed, citizen_id, pinned)` → pins for materialization; presentation only.
- `citizen_checksum(seed)` → named-pool checksum.
- Batch arrays, not per-citizen calls; one call per panel refresh, not per frame.
- Events: `CitizenMovedIn`, `CitizenJobChanged`, `CitizenAged`, `CitizenDied`, `CitizenMaterialized`, `CitizenDematerialized`.
- Materialization events are presentation facts. They are not persisted or replayed as world facts.

### UI

- **Selection card:** name, age band, household, home, job, commute, needs bars, mood, "follow" and "show home/work".
- **Household panel:** named members, vacancies, aggregate needs, income; unnamed residents appear as a count, not fake names.
- **World label:** only selected or detailed citizens; no name labels at mayor altitude (Q-047).
- **Advisor and stories:** sample a named citizen from the affected household and phrase it as representative, not exhaustive ([[57 - Dashboard and Advisor]]).
- **Tutorial:** can reference a named citizen for "follow one home → work → home" ([[65 - Tutorial]]); the tutorial stays derived.
- **No UI for:** dialogue, inventory, relationships, family tree, or per-citizen taxes.

## 9. Deterministic test strategy

### Unit

- Identity derivation is stable for `(seed, home, slot, generation)`.
- Generation changes produce a new id; no reuse.
- Collision probe is deterministic.
- Name draw is stable per table version and rejects invalid pools.
- Household slot sync: grow, shrink, retire, reuse.
- Job view matches household assignment counts.
- Variance clamps to `[-8, 8]`.
- Death roll is stable and band rates are ordered.

### Integration

- Move-in → named-pool fill → select → materialize → dematerialize → select again: same identity, home, and job.
- Job reassignment emits exactly the changed workers.
- Save → clear named pool → load: same ids, names, ages, and checksum.
- Old save without `named_citizens` loads empty and regenerates on demand.
- Orphaned home is dropped with a warning.
- Lifecycle scenario: fixed seed, fixed days, exact death events.

### Headless

- 500 residents, 30 days, named on vs off: population, jobs, and happiness match exactly while lifecycle is off. Lifecycle on is a separate baseline.
- 2,000 materialized agents at 10 Hz: budget respected, no churn above the cap.
- Materialization storm: teleport the camera across the city; no more than 64 changes per tick, no duplicate agents, no missing selected agent.

### Golden and benchmark

- `named_checksum` fixture.
- Three-run median for materialization decisions and active tick.
- Existing city-loop checksums unchanged until the persistence slice.

### Rules

- Fixed seeds, simulated clock, no real network, no wall-clock sleeps.
- BTreeMap and sorted vectors; never iterate a HashMap.
- No thread-order-dependent sums.
- Tests run headless; Godot smoke covers selection and labels only.

## 10. Performance budgets

| Work | Rate | Budget |
|---|---:|---:|
| Identity and name generation | On demand | ≤ 0.5 ms per 100 new |
| Named sync (10k records) | 1 Hz | ≤ 1 ms |
| Materialization decision (2k candidates) | 10 Hz | ≤ 1 ms |
| Active citizen update | 10 Hz | ≤ 1 ms added to the 10 ms active budget |
| Detailed card refresh (200) | 10 Hz | ≤ 0.5 ms |
| Named save/load (4k) | On save/load | ≤ 10 ms |
| Memory | Steady | ≤ 64 B per record; ≤ 512 KB persisted at 4k |

- These are targets to measure in the slice, not measured results.
- The existing 2,000-agent movement floor is about 0.19 ms sparse and 0.90 ms dense ([[19 - Active Agent Benchmark]]); the named layer must fit in the remaining active budget.
- No per-tick heap allocation in steady state.
- Route cache and spatial hash have hard caps.

## 11. Privacy and localization

### Privacy

- Names are fictional and generated; no real people, players, or personal data.
- No user-supplied names in v1; no chat or log scraping into names.
- Server logs use citizen ids, not names, for lifecycle events; names are a display concern.
- Telemetry is aggregate and opt-in; it never includes names or per-citizen records.
- Multiplayer: a client may see names of citizens in its area of interest; it must not receive the whole named pool. Selection is per player and is not shared as a "who is watching whom" signal.
- Mods can add name pools but cannot read player data; name pools are data.

### Localization

- Every UI label is a key (`base.ui.citizen.name`, `base.ui.citizen.job`, `base.ui.citizen.age`, and so on).
- Names are proper nouns and are **not** translated. Name pools may be locale-tagged; the server picks from the save's pool so the same citizen has the same name for every player.
- German UI must fit the longest plausible name + job + age string; the card wraps, it does not clip.
- Use named placeholders: `{name}`, `{job}`, `{age}`; never concatenate translated fragments.
- Missing keys fall back to English, then the key ([[16 - Localization]]).
- No gendered grammar assumptions in v1; job titles are neutral where possible.

## 12. Staged slices and acceptance criteria

| Slice | Deliverable | Acceptance |
|---|---|---|
| **N1 — Identity layer (derived)** | Deterministic ids, names, household slot sync, read-only bridge | **Done** — PR #98; 500-resident city produces stable names; `cargo test -p core_city named`; no save or checksum change; 4k identities generate in < 100 ms under the debug test guard |
| **N2 — Selection and card** | Pick/select, detail card, follow camera, household panel | **Partial — PR #108/#126; follow camera, EN/DE card, and per-citizen age/employment data done; scene rows remain** |
| **N3 — Materialization** | Active pool, hysteresis, route cache, fade, active cap | **Partial — PR #107/#123/#125; selection core, read-only bridge, F4 overlay, and hysteresis/pinning done ([[73 - Materialization Core]]); routes/fade/dwell remain** |
| **N4 — Persistence and lifecycle** | Optional save field, committed identities, aging/death, re-baseline | **Partial — PR #103**; persistence landed ([[70 - Named Persistence]]), lifecycle/aging/death next |
| **N5 — Cohort bridge (later)** | Sample from cohorts, district stories, 1M identity store | 1M cohort benchmark unchanged; materialization near camera; no per-citizen tick |

### N1 checks

- `cargo test -p core_city named` — pass.
- `cargo test --workspace` — pass.
- `city_loop --scenario mid` checksum unchanged.

### N1 implementation

`crates/core_city/src/named.rs` now implements the derived identity layer:

- `mix64`, `citizen_id(seed, placement_id, slot)`, and `name_for(id)`; `IDENTITY_VERSION = 1`.
- `NamedCitizen { id, placement_id, slot, name }`.
- `NamedCitizens::sync(&BTreeMap<u64, Household>)` adds one identity per resident slot, keeps existing ids/names stable, drops slots when a household shrinks or disappears, and caps the pool at `MAX_NAMED_CITIZENS = 4,096`.
- `citizens_of`, `get`, `clear`, `checksum`, and `validate`.
- No save field and no checksum change: the pool is rebuilt from the aggregate population after load.

Bridge:

- `named_citizens_state(seed)` returns `open`, `seed`, `count`, `max`, and `checksum`.
- `named_citizens_for(seed, placement_id)` returns the home's residents with `id`, `name`, and `slot`.
- Both validate the world and population seed; the pool is derived, not persisted.
- `population_clear` clears the pool.

N1 is read-only; selection, the detail card, materialization, and lifecycle are N2–N4. Names are fictional and generated; no user text.

### N2 implementation

The selected-building HUD line appends `· residents <names>` for the selected home, read from `named_citizens_for`. The scene smoke verifies that the completed two-resident house has two non-empty names and that a second query returns the same ids. Marker: `named=ok`.

N2 also adds a **Citizen** panel (`F2`, bottom-left):

- named residents with their resident slot;
- household residents/capacity, employed count, commute, income/day, and food/water/rest;
- explicit empty states for no selection, unavailable placement, and a vacant/unnamed home;
- a 0.5 s refresh while visible, matching the dashboard cadence;
- `Esc` closes the citizen panel before the dashboard, and entering Architect closes it.

The smoke opens the panel with no selection, checks the empty state, selects the occupied house, verifies a known name plus `2 / 2 residents` and `40/day`, then closes it. Marker: `citizens=ok`.

N2 follow/camera additions:

- `F3` smoothly focuses the Mayor camera on the selected home; `Esc`, a selection change, or entering Architect stops the follow.
- The camera hands control back if the player pans during a follow step.
- `world_i18n.gd` provides key-based EN/DE strings for the citizen panel and follow hint; names remain proper nouns.
- The smoke starts follow, steps the camera, verifies arrival, verifies selection-change and Architect cancellation, and renders the card in both EN and DE. Markers: `follow=ok`, `citizen_i18n=ok`.

N2 exposes per-citizen **age and employment data** in the bridge:

- `named_citizens_for` now returns `age_years` (deterministic 18–64 from the identity id via `mix64`) and `employed` (slots below `household.workers`) beside `id`, `name`, and `slot`.
- Both are pure derived views: `age_years(id)` and `slot_is_employed(household, slot)` live in `core_city::named`, add no save fields, and do not change any checksum.
- Employment is a placeholder mapping of the aggregate worker count onto the first slots until per-slot jobs exist.
- Core tests cover the age range and determinism plus the employment boundaries (`0`, `workers - 1`, `workers`, `residents - 1`).

Rendering the age/job rows in the scene panel is still pending.

Not yet implemented: rendering per-citizen job/age rows, per-slot job titles, and a full household list. The card is EN/DE but the wider HUD remains English-only until the localization phase.

### N2 checks

- `cargo test -p core_city named`
- `smoke_world_scene.sh` markers `citizens=ok`, `follow=ok`, `citizen_i18n=ok`
- `smoke_world_scene.sh` still reports `status=ok`
- Select, open card, follow, and return; identity stable
- Screenshot and EN/DE width check

### N3 UI implementation

- `F4` toggles a read-only materialization overlay at the selected home (or the Mayor camera target).
- Active citizens render as blue markers; detailed citizens render as larger amber markers.
- The overlay rebuilds on selection change or at 0.5 s while visible; nodes are freed on toggle off.
- HUD/status/dashboard report `X active · Y detailed`; the world smoke checks counts, node lifecycle, and marker `materialization=ok`.
- No save, simulation, route, or per-frame node changes. Hysteresis/route cache/fade remain.

### N3 checks

- `cargo run --release -p townsmith-bench --bin agents` plus named materialization
- Three-run median; active tick within 10 ms
- Camera teleport storm: cap respected and no duplicates
- No per-citizen route builds (instrument counter)

### N4 persistence implementation

`NamedSave` v1 stores `format_version`, `seed`, `max`, and sorted identity records (`id`, `placement_id`, `slot`, `name`). The bridge writes an optional `named_citizens` object after `population`, checks the seed, syncs with the loaded households, drops orphans, and commits atomically with the other stores. Old saves load an empty pool that fills on demand. Save/load results expose `named`, `named_max`, and `named_checksum`; population/city checksums are unchanged. See [[70 - Named Persistence]].

Lifecycle fields (age, job, death), migration, and a lifecycle-on baseline remain open.

### N4 checks

- Save/load roundtrip and tolerant old save — pass ([[70 - Named Persistence]]).
- Lifecycle scenario with fixed seed and expected events — open.
- `bench_city_loop.sh` baseline unchanged for lifecycle-off — pass.
- Docs 70 and the named save/load result fields are documented.

## 13. First-slice exclusions (explicit)

The first implementation slice (N1, N2, and the first part of N3) does **not** include:

- full per-citizen simulation at 1M;
- pathfinding per citizen;
- dialogue trees or conversation;
- family trees, marriages, births, or inheritance;
- save migration (only an optional add-only field);
- per-citizen inventories, money, taxes, or consumption;
- interior pathfinding or room-level navigation;
- schedules beyond home → work → home;
- children, school assignment, or education;
- multiplayer-specific citizen permissions beyond area-of-interest filtering;
- user-authored or real names;
- name labels at mayor altitude.

## 14. Proposed open questions

These are new; add them to [[09 - Open Questions]] only when accepted. They do not change any accepted ADR.

| ID | Question | Proposed default | Priority |
|---|---|---|---|
| **Q-102** | Name pools and cultural coverage: how many locales and pools at launch, and are names stored as strings or content IDs? | English + German pools; store display strings from content IDs; server-stable | Normal |
| **Q-103** | Persistence scope: persist every named identity, or only committed/selected ones? | Commit on select, story, or lifecycle; regenerate uncommitted from seed | High |
| **Q-104** | Materialization policy: distance-only or selection/event priority, and the exact radii and hysteresis? | Selection > story > distance; 80 m in, 120 m out; 64 changes per tick | High |
| **Q-105** | Death and replacement: does a named death reduce aggregate population, and at what rates? | Yes for named slots; off in default benchmarks; dedicated lifecycle profile | High |
| **Q-106** | Household composition: singles, couples, or families in the first slice, and who owns the budget? | Household owns the budget; named members share it; no family tree | Normal |

## 15. Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Identity drift between sessions | Stories contradict themselves | Version identity and name tables; golden tests; no per-session RNG |
| Aggregate/named divergence | Population and jobs disagree | Household is authoritative; named is a view; parity tests |
| Save bloat at 1M | Large saves, slow loads | Bounded pool; commit on demand; binary snapshot later |
| Death destabilizes balance | Pinned checksums break | Lifecycle off by default; dedicated scenario; deliberate re-baseline |
| Materialization popping | Visible artifacts | Hysteresis, minimum dwell, fade, distance and door hiding |
| Route cost per citizen | Frame spikes | Cached pair routes; one build per tick; no per-citizen A* |
| Multiplayer AOI overload | Server sends too much | Server-wide 2k cap; per-client priority; bounded payloads |
| Localized name mismatch | Two players see different names | Server picks from the save pool; names are proper nouns |
| Mod name pools | Offensive or invalid data | Schema validation, length limits, blocklist, server filter |

## 16. Related

- [[07 - Simulation and AI]] — tiers, cohort model, 2k active agents.
- [[12 - Technical Architecture]] — core/shell boundary, save layout, events.
- [[50 - Households and Needs]] — the authoritative household runtime.
- [[51 - Jobs and Wages]] — assignment, commute, wages.
- [[55 - Utilities]] — service coverage for needs and stories.
- [[56 - Traffic]] — road graph, flow fields, route cache.
- [[57 - Dashboard and Advisor]] — sampled stories and advice.
- [[59 - Progression and Ages]] — age gates; no age effect on identity yet.
- [[65 - Tutorial]] — follow-one-citizen checklist step.
- [[19 - Active Agent Benchmark]] — the 2k movement floor.
- [[49 - Phase 3 Slice Plan]] — "a named citizen can be followed" exit proof.
- [[58 - Runner Matrix]] — next step: materialize named citizens.
- [[16 - Localization]] — keys, placeholders, fallback.
- [[10 - Decisions]] — D-007, D-014, D-017, D-022.
- [[09 - Open Questions]] — Q-102..Q-106 land here when accepted.
