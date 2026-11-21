---
title: Advisor Services
project: Townsmith
status: active
updated: 2026-11-21
tags:
  - townsmith
  - phase-4
  - advisor
  - services
  - dashboard
---

# 76 — Advisor Services

> [!info] What this note is
> Connects the civic-service supplies from [[62 - Civic Services]] to the city advisor from [[57 - Dashboard and Advisor]]. It adds advisor rules only: no simulation, save-format, or dashboard-layout change.

## 1. Scope

In:

- four optional civic supplies on `AdvisorInput`: `school_supply`, `clinic_supply`, `fire_supply`, `police_supply`;
- critical and warning rules for each supply, using stable codes;
- `city_advisor` reads `services_state()` and passes the live supplies;
- unit tests for thresholds, missing stores, full coverage, and the healthy report;
- a healthy city with full services can still report `city.healthy`.

Out:

- new save fields, checksums, or migrations;
- district, settlement, or per-building service advice;
- staffing, funding, response times, or service quality;
- happiness, health, fire-risk, or crime effects;
- dashboard layout changes or new smoke assertions;
- localization of the new titles and details.

## 2. Model

`core_city::advisor::AdvisorInput` gains four fields next to the existing utility supplies:

```rust
pub school_supply: Option<f32>,
pub clinic_supply: Option<f32>,
pub fire_supply: Option<f32>,
pub police_supply: Option<f32>,
```

The fields follow the existing supply contract:

- `Some(value)` is the measured service share, clamped to `0..=1` before comparison;
- `None` means no civic service store exists;
- for a populated city, `None` is treated as zero supply, exactly like a missing industry or utility store. A city with residents and no civic sites therefore reports the critical shortage, not an unknown.

The values are derived from `ServiceStore::school_supply`, `clinic_supply`, `fire_supply`, and `police_supply` after `ServiceStore::refresh`. They are not persisted by the advisor and do not change service state.

## 3. Rules

All four rules use the same thresholds as the food, water, and power rules:

| Code | Severity | Trigger |
|---|---|---|
| `city.school_shortage` | critical | school supply below 25% |
| `city.school_shortage` | warning | school supply below 99% |
| `city.clinic_shortage` | critical | clinic supply below 25% |
| `city.clinic_shortage` | warning | clinic supply below 99% |
| `city.fire_shortage` | critical | fire supply below 25% |
| `city.fire_shortage` | warning | fire supply below 99% |
| `city.police_shortage` | critical | police supply below 25% |
| `city.police_shortage` | warning | police supply below 99% |

Details point at the usual causes: uncovered buildings, exhausted capacity, and incomplete sites. The rules run only when `population > 0`; an empty city does not report civic shortages.

Each code is shared by its warning and critical levels, and the `severity` field distinguishes them, matching `city.food_shortage` and `city.water_shortage`.

### Healthy reports

`city.healthy` and `city.services_healthy` now require all four civic supplies to be at least 99%, in addition to their existing conditions. A missing civic store produces a critical message, so those branches are unreachable for a populated city without civic service.

- `city.healthy`: no other messages, food/water/power and all four civic supplies at least 99%, happiness at least 70%, non-negative daily net.
- `city.services_healthy`: same supply and happiness conditions with no treasury, so cash flow is untracked.

The message cap (`MAX_ADVISOR_MESSAGES = 12`), severity-first sort, code tie-break, and truncation are unchanged. Adding four codes extends the deterministic sort order; `city.clinic_shortage` and `city.fire_shortage` sort before `city.food_shortage` at the same severity.

## 4. Bridge

`TownsmithCore.city_advisor(seed)` now calls `services_state()` with the other summaries. When the returned payload is `open`, it copies the four supplies into `AdvisorInput`.

- A closed service store leaves the fields `None`; the advisor treats that as zero for residents, so `city_advisor` needs no special case.
- `services_state()` refreshes the derived service report before returning it, as `utilities_state()` already does. The advisor call remains derived state: no save field, no checksum, and no placement or household mutation.
- Locks are still taken and released per summary; `city_advisor` holds none while calling the next one.

| Supply | Source field |
|---|---|
| `school_supply` | `services_state().school_supply` |
| `clinic_supply` | `services_state().clinic_supply` |
| `fire_supply` | `services_state().fire_supply` |
| `police_supply` | `services_state().police_supply` |

## 5. Tests

`cargo test -p core_city advisor` covers:

- each civic rule independently (school, clinic, fire, police) at `0.99`, `0.98`, `0.25`, `0.24`, and `0.0`, asserting the warning/critical boundary and stable code;
- a missing civic store with residents: four critical messages and no `city.healthy`;
- a missing civic store with no residents: `city.quiet`, so empty cities are unaffected;
- a city with full civic coverage and the rest of the snapshot healthy: `city.healthy` only;
- the existing healthy, food/water/power shortage, missing utility/industry store, budget/debt, bankruptcy, vacancy/unemployment, road, empty-city, cap, and determinism tests, with the healthy fixture updated to full civic coverage.

The bridge behavior is exercised by the world-scene smoke, which already adds all four service kinds and calls `city_advisor`; no new smoke marker was added. A Rust bridge unit test would need a live Godot engine for `VarDictionary`.

## 6. Limits

- City-wide only; no district or settlement service advice.
- Fixed thresholds; no age, difficulty, or policy weighting.
- No staffing, funding, quality, response-time, or service-vehicle model.
- No grace period: a populated city with no civic sites immediately reports four critical messages. This is consistent with missing food, water, and power stores, but it is noisy for a brand-new settlement.
- A missing service store and a measured zero supply produce the same advice; the advisor's `Option` distinction is not surfaced in the report.
- The message cap can still hide lower-severity advice in a deeply unhealthy city; civic criticals sort ahead of warnings but compete with other criticals by code.
- New titles and details are English-only developer text until the localization pass.
- No dashboard layout change; the existing **Civic** line already shows the four supplies from `services_state()`.
- No save-format change, so existing saves, checksums, and the runner matrix are unaffected.

## 7. Related

- [[57 - Dashboard and Advisor]]
- [[62 - Civic Services]]
- [[10 - Decisions]]
