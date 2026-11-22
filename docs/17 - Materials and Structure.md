---
title: Materials and Structure
project: Townsmith
status: proposed
updated: 2026-11-23
tags:
  - townsmith
  - building
  - materials
  - structure
  - progression
---

# 17 — Materials and Structure

> [!decision] D-024 — Material-based structural limits and composite construction
> Every material and structural system has a readable maximum height, span, and load. Buildings can mix systems — a stone ground floor with timber above — but the weakest support in the load path sets the limit. Glass facades are cladding, not structure.
>
> **Implementation:** Phase 4.2 ships the first readable subset in [[60 - Structural Limits]]: system tiers, story/height/span/foundation checks, and a stone-ground + timber-upper bonus. Full load-path and service-gate analysis remains future work.

## 1. Goals

- Start with **tents and simple wooden sheds**, then progress through wood, stone, half-timber, brick, concrete, steel, and glass.
- Make material choice a **structural** decision, not just a cosmetic one.
- Keep the rules readable: the player sees “timber frame: 3 stories max” and why.
- Allow **composite construction**: stone ground floor + timber upper floors, concrete podium + steel tower, etc.
- Make skyscrapers require more than strong material: foundations, bracing, elevators, fire safety, water pressure, and engineering.
- Keep the model simple enough for a city builder; no full finite-element stress simulation in v1.

## 2. Progression at a glance

| Stage | Typical building | Structural system | Initial max |
|---|---|---|---|
| **Camp** | Tent, lean-to | Poles + canvas | 1 temporary level |
| **Early wood** | Shed, hut | Timber posts + boards | 1 story |
| **Wood** | Family house | Timber frame / post-and-beam | **2–3 stories + roof** |
| **Stone** | Stone house, civic hall | Load-bearing stone walls | 2–3 stories + roof |
| **Half-timber** | Stone ground floor + timber upper | Composite stone/timber | **3–4 stories + roof** (one higher than timber alone) |
| **Brick** | Tenement, warehouse, school | Load-bearing brick / masonry | 4–6 stories |
| **Concrete** | Apartment block, office | Reinforced concrete frame | **20–30 stories** |
| **Steel** | Skyscraper | Steel frame | 40–80 stories |
| **Composite** | Supertall | Steel + reinforced concrete core | 80+ stories |
| **Glass facade** | Curtain-wall tower | Non-structural cladding on concrete/steel | Height set by the frame, not the glass |

These are **initial targets**, not final numbers. They should be tuned in the Phase 0/4 structural tests and playtests.

> [!important] Maximums, not minimums
> A material never forces a minimum height. A one-story concrete house is structurally valid, even if concrete is economically overkill. A one-story stone house is fine. Service gates are triggered by height, occupancy, and building systems — not by the material itself.

## 3. Material vs structural system

A material is not a building system by itself.

- **Timber** is strong in compression and tension, but combustible and limited in span.
- **Stone** is strong in compression, weak in tension; load-bearing walls must be thick and openings small.
- **Brick** is a masonry unit; strength depends on mortar, wall thickness, and reinforcement.
- **Reinforced concrete** combines concrete's compression with steel's tension; it needs rebar, formwork, and curing.
- **Steel** is strong in both directions and allows long spans and tall frames, but needs fire protection and bracing.
- **Glass** is a facade material, not a primary structure. A glass curtain wall hangs on a concrete/steel frame.

The structural system defines the actual limits:

| System | Materials | Max span | Max stories (initial) | Notes |
|---|---|---:|---:|---|
| Tent | Canvas, poles | 3 m | 1 | Temporary; no permanent foundation |
| Timber shed | Wood | 4 m | 1 | Cheap, fast, low comfort |
| Timber frame | Wood, joinery | 6–8 m | 2–3 + roof | Fire risk; needs bracing |
| Load-bearing stone | Stone, lime mortar | 4–6 m | 2–3 + roof | Thick walls; small windows |
| Half-timber composite | Stone ground floor + timber above | 6 m | 3–4 + roof | Stone base carries more load |
| Load-bearing brick | Brick, mortar | 6–8 m | 4–6 | Industrial density; needs pilasters |
| Reinforced concrete frame | Concrete, rebar | 8–12 m | 20–30 | Needs cement, steel, curing |
| Steel frame | Steel | 12–20 m | 40–80 | Needs fire protection, bracing |
| Composite core | Steel + concrete | 12–20 m | 80+ | Supertall engineering |
| Curtain wall | Glass, aluminum | n/a | n/a | Facade only; height from frame |

## 4. What the validator checks

Full stress simulation is out of scope for v1. The validator uses readable rules:

1. **Load path:** every solid voxel/room must trace support to the ground through walls, columns, or beams.
2. **Story/height limit:** the structural system's max stories and max height.
3. **Span:** beams, slabs, and openings cannot exceed the material/system span.
4. **Wall thickness and continuity:** load-bearing walls must be thick enough and continuous to the foundation.
5. **Openings:** doors/windows reduce wall strength; large openings need lintels/beams.
6. **Cantilever:** overhangs are limited by material and depth.
7. **Bracing:** tall/large buildings need lateral bracing (timber diagonals, shear walls, steel bracing).
8. **Foundation:** taller/heavier buildings need deeper foundations and better soil.
9. **Fire and egress:** higher buildings need fire separation, stairs, and sprinklers.
10. **Services:** elevators, water pressure, HVAC, and structural engineering gate very tall buildings.

The validator highlights the **first failing rule** with a clear reason, not a generic "invalid structure."

Validation is an **editor/save-time** operation. The resulting structural summary is cached in the blueprint and building instance; it is not re-run per voxel per tick. Revalidation happens when the blueprint, material properties, or content version changes.

## 5. Composite construction

Players can mix systems by floor or by element:

- Stone ground floor + timber upper floors: the stone base supports the timber; the timber's 2–3 story limit applies above the stone.
- Concrete podium + timber upper floors: the podium can carry more, but the timber above still has its own limit.
- Steel frame + concrete core: the core provides stiffness and services; the steel frame carries floors.
- Glass curtain wall: attaches to the frame; it does not add structural height.

Rules:

- The limit is determined by the **weakest support in the load path**, not the strongest material anywhere.
- A stronger lower floor can add height only if it actually carries the upper load and provides lateral stability.
- Mixing materials changes cost, construction trades, fire risk, and maintenance.
- The editor shows the effective limit, e.g. “Timber frame: 3 stories. Stone ground floor: +1 story. Effective: 4 stories.”

## 6. Service gates vs utility connections

These are related but not the same thing.

### Utility connections

Utilities are **connections to city networks**. They are required for a building to function, but they are not height gates by themselves.

| Utility | Required for | If missing |
|---|---|---|
| **Potable water** | Drinking, cooking, sanitation, firefighting | Cannot occupy; construction/commissioning blocked |
| **Wastewater/sewage** | Toilets, sinks, drains | Cannot occupy unless a septic/composting alternative exists |
| **Electricity** | Lighting, pumps, elevators, HVAC, production | Early buildings can use candles/wood; modern buildings cannot occupy without it |
| **Telecom/data** | Telephone, fiber, cable, offices, happiness | Optional; lowers land value/office efficiency/happiness |
| **Gas/district heat** | Heating, cooking, industry (later) | Building can use alternatives; higher cost/pollution |

These are separate from:

### Building service systems

Building service systems are **inside the building or site** and are triggered by height, occupancy, and use.

| Height / use | Requirement | Hard or soft? |
|---|---|---|
| Any occupied building | Potable water, wastewater, safe egress | **Hard** |
| 4+ stories | Fire stairs, fire separation, structural continuity | **Hard** |
| 6+ stories | Elevator (or severe walk-up penalty for unusual low-rise designs) | **Hard** for normal occupancy |
| 6+ stories | Water pressure/pumps/water tower for upper floors | **Hard** above the pressure limit |
| 10+ stories | Fire suppression, wind bracing, structural engineer | **Hard** |
| 20+ stories | High-speed elevators, deep foundation, curtain-wall systems, HVAC | **Hard** for occupancy |
| 40+ stories | Core services, refuge floors, advanced engineering, backup power | **Hard** for occupancy |
| Any height | Telecom/fiber, parking, HVAC efficiency, backup power | **Soft** (penalties, not occupancy blocks) |

### Buildable vs occupiable

- You can **build** a 20-story concrete shell without elevators or fire systems. It is a construction project, not a usable building.
- It becomes **occupiable** only after commissioning passes the hard gates.
- Soft gates do not block occupancy; they reduce happiness, land value, office efficiency, health, or resilience.
- A 1-story concrete building is allowed; it simply has more structural capacity than it needs.

### Basements, cellars, and underground parking

Sub-ground floors are supported from the start of the building editor, with vehicle parking as a later content/system layer.

| Type | Use | Launch |
|---|---|---|
| **Cellar** | Storage, root cellar, pantry, workshop | ✅ v1 |
| **Utility basement** | Boiler, electrical, water tanks, sewage ejector | ✅ v1 |
| **Habitable basement** | Extra rooms, laundry, recreation | Later; needs egress/light/ventilation |
| **Underground parking** | Cars, service vehicles, deliveries | Later, after traffic |
| **Underground transit/service** | Tunnels, metro, logistics | Post-launch |

**Rules**

- Basements are **levels below grade**, not stories above ground. They do not count toward the material's max stories, but they add load, foundation depth, and excavation cost.
- Initial blueprint depth cap: **32 m**; practical early-game basements are 1–2 levels (3–6 m). Deeper levels need engineering, waterproofing, and stronger retaining walls. The cap is enforced by `core_structure::MAX_FOUNDATION_DEPTH_M` with the stable `structure_foundation_depth` error, so a declared depth above 32 m is rejected (see [[60 - Structural Limits]]).
- Basements can use different materials from the superstructure (e.g., concrete retaining walls under a timber house).
- **Water table and soil** affect cost, flooding risk, and maximum depth. High groundwater needs dewatering and tanking.
- **Excavation** is a construction phase: excavate, shore, foundation, waterproof, backfill. It creates jobs for excavator operators, foundation crews, and waterproofers.
- **Access:** stairs and/or elevator from the building; vehicle parking needs a ramp with clearance, lighting, ventilation, and fire separation.
- **Services:** sump pumps, drainage, sewage ejector (if below the sewer line), ventilation, and power. A cellar without these may flood or be unusable.
- **Egress:** habitable basements need a safe escape route; storage/utility cellars have lower requirements.
- **Validation:** retaining walls, depth vs. soil/water table, ventilation, egress, and vehicle ramp geometry.
- **Rendering:** underground spaces are culled when not visible/entered; interior lighting is local; no sky exposure.
- **Save/load:** underground voxels, terrain edits, ramp connections, and service state are persisted with the building/site.

**Vehicles later**

- Underground parking is a **parking capacity** with a ramp connection to the road network.
- Near the camera, vehicles can drive in/out; distant parking can be abstracted as capacity.
- Vehicle pathfinding adds ramp nodes, clearance checks, and garage-level routing.
- This is intentionally later: first ship cellars and utility basements, then parking.

These are **site/building** requirements, not utility connections.

## 7. Editor and player-facing UX

- **Structure overlay:** load-bearing walls/columns, span warnings, bracing, foundation depth.
- **Height meter:** “3 / 3 stories (timber frame)” or “4 / 4 (stone ground floor + timber).”
- **Material comparison:** cost, max stories, fire risk, insulation, maintenance, construction trades.
- **Upgrade preview:** “Replace timber frame with reinforced concrete: +17 stories, +cost, +construction time, +steel/cement demand.”
- **Clear failure reasons:** “Remove this wall or add a beam: 8 m span exceeds timber's 6 m limit.”
- **Sandbox override:** allow invalid blueprints in sandbox/testing, clearly marked and not placeable in normal play.

The goal is that a player can reason about structure without an engineering degree.

## 8. Construction and trades

Material progression changes the construction process:

- Timber: carpenters, joiners, roofers; fast, low equipment.
- Stone: masons, stonecutters, laborers; slow, heavy.
- Half-timber: carpenters + masons; composite scheduling.
- Brick: bricklayers, scaffolders; repetitive, weather-sensitive.
- Concrete: formwork carpenters, rebar workers, concrete pump crews, inspectors; curing time.
- Steel: steelworkers, crane operators, welders, fireproofing crews.
- Glass facade: glaziers, curtain-wall crews, sealant workers; high-skill, weather-sensitive.
- Tall buildings: tower cranes, hoists, surveyors, structural engineers, safety inspectors.

This connects D-023 (construction crews/trades) to material progression.

## 9. Costs and building class

Material choice affects more than height:

- **Cost:** material liters + labor + equipment + permits.
- **Build time:** stone and concrete are slow; timber is fast; steel is fast but crane-dependent.
- **Fire risk:** timber/half-timber high; stone/brick lower; steel needs fireproofing; concrete good.
- **Insulation/comfort:** brick/stone good thermal mass; glass needs shading/HVAC; timber needs insulation.
- **Maintenance:** wood rots; stone weathers; concrete cracks; steel rusts; glass breaks.
- **Class/prestige:** glass facades and fine materials raise class; cheap materials lower it.
- **Environmental impact:** embodied carbon, quarrying, logging, mining (later).

## 10. Modding

- Materials are data: density, compressive/tensile strength, span, fire rating, cost, insulation, maintenance, age tier.
- Structural systems are data: max stories, max height, max span, required bracing/foundation, allowed materials.
- Mods can add materials and systems, but must declare properties and pass the same validator.
- Existing blueprints are revalidated when material properties or content versions change; invalid blueprints are marked “needs review,” not silently broken.
- No mod may bypass the structural validator on a multiplayer server.

## 11. Testing

| Test | What it proves |
|---|---|
| Height limit | A 4-story pure timber blueprint fails; a 4-story stone+t timber passes |
| Load path | A floating room fails; a supported room passes |
| Span | A long timber beam fails; a steel beam passes |
| Openings | A wall with an oversized opening fails without a lintel |
| Composite | Stone ground floor + timber upper gets the expected effective limit |
| Service gate | A 20-story concrete tower without elevators is blocked or penalized |
| Mod material | A modded material with declared limits is validated and saved |
| Save/load | Structural graph and material data roundtrip |

## 12. Open questions

- **Q-090:** How strict should structural validation be? Rule-based limits (proposal) or stress simulation later?
- **Q-091 (answered):** Are the initial max stories/height targets right? **Yes for now** (wood 2–3, half-timber 3–4, concrete 20–30, steel 40–80). Tune later if needed.
- **Q-092:** Does a roof count as a story? Proposal: no, but it adds height/load.
- **Q-093:** Can players mix materials per floor/wall freely? Proposal: yes, with load-path and weakest-link rules.
- **Q-094:** Are glass facades only cosmetic, or do they affect energy/comfort/class? Proposal: all three, but non-structural.
- **Q-095:** Which service gates should be hard blocks vs penalties? Proposal: water/wastewater/egress/fire/elevator/water-pressure/structural are hard for occupancy; telecom, parking, HVAC efficiency, backup power are soft.
- **Q-096:** How do modded materials/systems migrate existing blueprints when properties change?
- **Q-097:** Basement launch scope: cellars/utility basements first, habitable basements and parking later?
- **Q-098:** Maximum basement depth and groundwater/flood model?
- **Q-099:** When do underground parking and vehicle ramps arrive?
- **Q-100:** Should underground tunnels/transit connect buildings later, or stay separate?

See [[09 - Open Questions]] and [[10 - Decisions]].
