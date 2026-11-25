---
title: Open Questions
project: Townsmith
status: active
updated: 2026-11-25
tags:
  - townsmith
  - questions
  - decisions
---

# 09 — Open Questions

> [!info] How to use this note
> Every unresolved question gets an ID (`Q-xxx`). Blocking questions must be answered before the phase that depends on them. Non-blocking questions can be answered later, but the current assumption is what the plan uses until then. When a question is answered, move it to the **Answered** section, record the date, and add an ADR in [[10 - Decisions]] if the answer changes a decision.

## Assumptions in force

These are not decisions. They are the defaults the plan currently uses. If an assumption is wrong, the affected docs must be updated.

| ID | Assumption | Affects |
|---|---|---|
| A-001 | Small team: **2–3 full-time generalists**, PC-first, premium game, no publisher mandates; Rust/Godot experience or willingness to learn | Engine, roadmap |
| A-002 | Co-op scale: 2–8 players per server, dedicated servers supported | Multiplayer, simulation |
| A-003 | Population scale: **1M+ citizens** as a late-game target on 8-core/16-thread + RTX 3060-class hardware; 100k+ buildings; cohort simulation with ~2k active agents | Voxel, simulation, rendering |
| A-004 | Real-time with pause and speed controls (not turn-based) | Simulation, UX |
| A-005 | Multi-scale, chunked, **effectively infinite** world: 1 m terrain + 10 cm buildings + LOD + streaming; build anywhere | Voxel, worldgen, rendering |
| A-006 | No destruction physics in v1; structural validation only | Voxel, simulation |
| A-007 | Direct placement and zoning both exist; zoning grows from the library | Design |
| A-008 | Data-first modding; sandboxed scripting added after the core loop | Modding |
| A-009 | Steam is the likely launch platform; Workshop is desirable but not required at launch | Distribution |
| A-010 | **English + German at launch**; all strings externalized; language packs moddable later | Production |
| A-011 | Citizens are NPCs only; a playable citizen mode is post-launch | Design |
| A-012 | Day/night at launch; **seasons at launch with an on/off toggle** (farming and hunting depend on them) | Simulation, art |
| A-013 | Time control: single-player Mode B auto-pauses; multiplayer uses admin `time.control` or a simple-majority online vote; pause is a shared design freeze (simulation stops, editing continues, no auto-resume) | Simulation, multiplayer, UX |
| A-014 | Construction is a phased, crew-driven process with trade jobs; placement creates a site, not a finished building; crews auto-assign with player-set priority | Simulation, jobs, UX |
| A-015 | Materials/structural systems have readable max stories/height/span; composite construction allowed; glass is non-structural cladding; rule-based validator; materials have maximums, not minimums | Building, simulation, UX |
| A-016 | Service gates: water/wastewater/egress/fire/elevators/water pressure/structural are hard for occupancy; telecom, parking, HVAC efficiency, backup power are soft; basements supported with depth/groundwater limits | Building, simulation, UX |

## Blocking questions

These must be answered before or during Phase 0. They change the architecture, the schedule, or the core fantasy.

### Q-001 — Team size, skills, and weekly time budget

**Priority:** Blocking · **Affects:** D-003, [[08 - Roadmap]]

**Question:** How many people are working on this, what are their strengths (Rust, C++, Godot, art, design, production), and how many hours per week?

**Why it matters:** The engine recommendation, the schedule, and the scope all depend on it. A solo Rust expert should seriously consider Bevy. A designer-heavy team should default to Godot + GDScript. A team with no C++/Rust experience should not start with a native core.

**Options**
- Solo hobbyist, 10–20 h/week: cut multiplayer and modding from v1, or expect a 3–5 year timeline.
- 2–3 full-time generalists: the current plan is sized for this.
- 4–5 with funding: parallelize art, simulation, and networking; still scope-controlled.
- Contractor-heavy: strong production and documentation requirements.

**Current assumption:** A-001 (2–3 full-time generalists).
**Recommendation:** Plan for 2–3 full-time generalists with Rust/Godot experience or willingness to learn. If solo, keep the architecture but cut the launch scope to single-player + data modding.
**Status:** **Answered 2026-11-19** — 2–3 full-time generalists.

### Q-002 — Target multiplayer scale

**Priority:** Blocking · **Affects:** [[05 - Multiplayer]], [[07 - Simulation and AI]]

**Question:** Is multiplayer for 2–8 friends on a self-hosted server, or for public servers with dozens of players?

**Why it matters:** Player count drives interest management, bandwidth, server CPU, anti-cheat, moderation, and whether a server browser is needed.

**Options**
- **2–4 co-op:** simplest; listen server is enough; minimal moderation.
- **8 co-op:** the current target; dedicated servers; roles and permissions matter.
- **16–32 public:** needs stronger moderation, anti-grief, server discovery, and bandwidth work.
- **100+ MMO:** out of scope; would require a different architecture.

**Current assumption:** A-002 (2–8 co-op).
**Recommendation:** Ship 8-player co-op and dedicated servers. Design the protocol so 16 is possible, but do not promise it.
**Status:** Open

### Q-003 — City size and population target

**Priority:** Blocking · **Affects:** [[03 - Voxel and Scale]], [[07 - Simulation and AI]]

**Question:** How big is a city, and how many citizens should feel alive?

**Why it matters:** This is the denominator for every performance budget. A 1 km² / 10k city is a different game from a 100 km² / 1M metropolis.

**Options**
- **Small:** 1 km², 10k aggregate, 500 active. Easiest; intimate.
- **Medium:** 4 km², 50k aggregate, 2k active. A large town.
- **Large:** 16 km², 200k aggregate, 2k active. A city.
- **Metropolis:** 50–100 km², **1M+ aggregate**, 2k active. The new target; requires cohort simulation and district-level management.

**Current assumption:** A-003 (1M+ target).
**Recommendation:** Treat 1M+ as the **late-game** target on recommended hardware. At 10k–20k people/km², 1M citizens occupy 50–100 km². Build the vertical slice at 200 m × 200 m / 300 citizens, prove the loop, then scale through 100k to 1M. Do not simulate 1M individuals; use cohorts (Q-064).
**Status:** **Answered 2026-11-19** — 1M+ is the late-game target; target hardware is 8-core/16-thread + RTX 3060-class; world is effectively infinite; cohorts are the intended simulation model (Q-064 explanation in [[07 - Simulation and AI]] §3).

### Q-004 — Terrain resolution: 1 m or 10 cm?

**Priority:** Blocking · **Affects:** D-002, [[03 - Voxel and Scale]]

**Question:** Can the terrain use a 1 m grid while buildings use 10 cm voxels, or must the terrain also be 10 cm?

**Why it matters:** A 1 km² terrain at 10 cm is ~1 billion voxels per meter of depth; a 100 km² metropolis would be 100 billion. This is the difference between a feasible city and an engine research project. Build-anywhere and 1M+ make 10 cm terrain effectively impossible; 1 m terrain is the only tractable city-scale option. It also affects terraforming, roads, and the visual consistency of the Minecraft look.

**Options**
- **1 m terrain + 10 cm buildings:** proposed; keeps the city tractable; terrain looks Minecraft-like; buildings look detailed.
- **10 cm terrain everywhere:** maximum consistency and terraforming detail; requires a different streaming/LOD architecture and a much larger budget.
- **50 cm terrain:** a compromise; less Minecraft-like, more detail than 1 m.
- **Hybrid:** 1 m far terrain, 10 cm detail near the player/camera; complex but possible later.

**Current assumption:** A-005 (1 m terrain).
**Recommendation:** Start with 1 m terrain. Test the visual mix in the Phase 0 spike. If it looks wrong, revisit before Phase 1, not after. At 1M+ scale, 10 cm terrain is not viable; do not design around it.
**Status:** **Answered 2026-11-19** — 1 m terrain + 10 cm buildings accepted. Phase 0 validates the visual mix and performance; 10 cm terrain is out.

### Q-005 — Where does Architect Mode building happen?

**Priority:** Blocking · **Affects:** [[02 - Game Design]], [[05 - Multiplayer]]

**Question:** Can players build anywhere in the world, and where does blueprint authoring happen?

**Why it matters:** "Build anywhere" is a core Minecraft fantasy: the player should be able to start a settlement in a forest, on a coast, or on a mountain, not only inside a pre-made city tile. Live voxel editing is the strongest version of that fantasy, but it creates griefing, permissions, sync, and structural-integrity problems. A separate workshop is safer and easier to sync, but less immersive.

**Options**
- **Build anywhere, live editing:** maximum immersion; hardest multiplayer and griefing story.
- **Build anywhere, workshop-authored blueprints:** the player can place buildings anywhere, but voxel authoring happens in a workshop; safest and simplest.
- **Both (recommended):** the world is open for placement anywhere; the workshop is the default authoring space; players with permission can enter a building anywhere and edit it in place, with a server lock.
- **Creative mode toggle:** a sandbox flag that allows unrestricted live editing in single-player.

**Current assumption:** Build anywhere; workshop + permissioned live editing.
**Recommendation:** Support placement anywhere from the start. Ship the workshop + blueprint placement first. Add live in-world editing after multiplayer is stable. This preserves the fantasy without betting the architecture on it.
**Status:** **Answered 2026-11-19** — workshop/creative authoring first; build anywhere for placement; permissioned live editing after multiplayer stabilizes (D-026).

### Q-006 — Is destruction required?

**Priority:** Blocking · **Affects:** D-008, [[03 - Voxel and Scale]], [[07 - Simulation and AI]]

**Question:** Do players need to dig terrain, damage buildings, collapse structures, or burn them down?

**Why it matters:** Full voxel destruction requires a different data structure, physics, networking, save format, and performance budget. Retrofitting it later is expensive. If it is core to the fantasy, it must be designed now.

**Options**
- **None (proposed):** static buildings; structural validation only; no debris.
- **Cosmetic damage:** cracks, rubble props, repair costs; no geometry changes.
- **Partial destruction:** remove voxels in combat/disasters; no physics; server-authoritative.
- **Full physics:** Teardown-style debris and collapse; a different project.

**Current assumption:** A-006 (none in v1).
**Recommendation:** No destruction in v1. Add cosmetic damage and repair later. If destruction becomes a core pillar, re-open before Phase 3.
**Status:** **Answered 2026-11-19** — none in v1; cosmetic damage and repair later (D-008 accepted).

## Design questions

| ID | Question | Current assumption / recommendation | Priority |
|---|---|---|---|
| **Q-007** | Campaign, sandbox, or both at launch? | Sandbox + one tutorial scenario; campaign later | Normal |
| **Q-008** | Visual target: per-voxel textures, flat colors, or a mix? How important are resource packs? | 16×16 textures per face, flat-color fallback, resource packs first-class | High |
| **Q-009** | Furniture: voxel-built, props, or both? | Both: voxels for structure, props for function | High |
| **Q-010** | Final blueprint size limits? | Initial: 8M solid voxels, 48×48 m footprint, 512 m height, 32 m depth; tune | Normal |
| **Q-011** | Global grid or local grid with rotation? How are slopes handled? | Local 10 cm grid, 90° rotation, foundation cut/fill; no arbitrary rotation v1 | High |
| **Q-012** | Setting and biomes? | Temperate river valley; one map for v1; moddable biomes later | Normal |
| **Q-013** | Are building interiors enterable? | Yes in Architect Mode; NPCs abstracted at the door; no interior navigation v1 | High |
| **Q-014** | Is the mayor a visible avatar? | Abstract camera; optional airship/drone cosmetic later | Low |
| **Q-015** | Playable citizens later? | Post-launch; keep the architecture open | Low |
| **Q-016** | Localization at launch? | **Answered:** English + German at launch; all strings externalized; moddable language packs later | Normal |
| **Q-017** | Windows dedicated server? | Linux first; Windows if cheap | Low |
| **Q-018** | Save compatibility across game and mod updates? | Saves survive game updates; mods may break; migrate where possible | High |
| **Q-019** | Monetization? | Premium + cosmetic/DLC later; no paid mods; no live service | Normal |
| **Q-020** | Accessibility baseline? | Colorblind overlays, remapping, text scale, pause, subtitles | High |
| **Q-021** | Direct placement, zoning, or both? | Both; zoning grows from the enabled blueprint library | High |
| **Q-022** | Real-time or turn-based? | Real-time with pause and speed | High |
| **Q-023** | Day/night cycle at launch? | Yes; affects sleep, power, crime; seasons are at launch with a toggle | Normal |
| **Q-024** | Disasters at launch? | Risk stats only; fires/floods post-launch | Normal |
| **Q-025** | Distant visual fidelity? | Impostors/merged LOD beyond ~500 m; test in spike | Normal |
| **Q-026** | Texture resolution per voxel face? | 16×16 with mipmaps; 8×8 for low-end; flat-color fallback | High |
| **Q-027** | Voxel lighting? | Baked/emissive props; no dynamic light propagation v1 | Normal |

## Multiplayer questions

| ID | Question | Current assumption / recommendation | Priority |
|---|---|---|---|
| **Q-028** | Shared treasury or personal budgets? | Shared by default; personal as a server option later | Normal |
| **Q-029** | Auto-download resource packs from the server? | No v1; manual install; Workshop later | Low |
| **Q-030** | Host migration? | No; dedicated servers are the durable option | Low |
| **Q-031** | Voice chat at launch? | Text + pings first; voice later | Low |
| **Q-032** | Steam networking/lobbies, or direct IP first? | Direct IP + invite codes first; Steam later | Normal |
| **Q-078** | Time-vote tuning: duration, cooldown, and quorum? | 60 s vote, 5 min failed-vote cooldown, 2 min success cooldown; simple majority of online players; tune in playtests | Normal |

## Settlement ownership questions

These support D-033, whose core data model and first server/bridge slices are implemented (accepted 2026-11-24). Q-111 is answered; Q-112–Q-115 remain open. `ServerWorld` now persists the player registry and an optional settlement store, `place_blueprint` enforces `build` inside a settlement, and the bridge exposes read-only owner/member fields. Protocol v1 still has no settlement command, the dashboard has no edit controls, and the only roles are global dev roles.

| ID | Question | Current assumption / recommendation | Priority |
|---|---|---|---|

| **Q-112** | When is a settlement unowned, who may claim it, is proximity or presence required, and when does an inactive owner lose it? | Only migrated or admin-released settlements are unowned; an explicit claim by id with the Mayor/Admin role. No automatic claim by entering the radius; abandonment rules deferred. `found_settlement` creates owned settlements, and `settlement_claim` implements the explicit claim by id with the Mayor/Admin ceiling. | High |
| **Q-113** | How does ownership transfer work when the owner is offline or gone (recipient acceptance, admin override, succession, dead-owner servers)? | Owner-initiated `settlement_transfer` to a registered player is implemented (previous owner becomes Manager), and `settlement_release` is the Admin dead-owner override. Recipient acceptance, succession, and dead-owner detection remain open. | Normal |
| **Q-114** | What are the per-settlement member, per-player settlement, manager, and membership-edit rate limits? | Provisional: 64 members per settlement, 16 settlements owned per player, same command bucket as placement; tune with playtests. | Normal |
| **Q-115** | Can a settlement restrict blueprints or mod content beyond the district allowlist, and how do modded placements interact with settlement permissions? | Keep v1 on the existing district blueprint allowlist; a settlement-level allowlist is later and must not bypass the server content manifest. | Normal |

## Modding questions

| ID | Question | Current assumption / recommendation | Priority |
|---|---|---|---|
| **Q-033** | Lua, WASM, or both for behavior modding? | Lua first for accessibility; evaluate WASM for performance/security | High |
| **Q-034** | Steam Workshop, mod.io, or both? | Steam Workshop if Steam; mod.io as a later cross-platform option | Normal |
| **Q-035** | How much simulation can mods write? | Read-mostly API; explicit write capabilities; server-authoritative | High |
| **Q-036** | Auto-download required content mods from servers? | No v1; manifest check + manual install; Workshop later | Low |
| **Q-037** | Are native (GDExtension) mods allowed? | Trusted opt-in only; never auto-downloaded | High |
| **Q-038** | Shader support in resource packs? | Yes with compatibility warnings; vanilla must not depend on it | Low |
| **Q-039** | How are modded blueprints shared? | Bundled with the mod or published separately; hash in the manifest | Normal |

## Content identity questions

These support D-034 (accepted 2026-11-23). The bridge v1 is implemented: a base-only save stays v1 and byte-identical, a save made with one live pack writes outer v2 and the `content` object, and load verifies it before any store or `.tsbp`. Q-117 has a bridge v1 answer in D-035 (Proposed): strict load stays fail-closed, while `world_load_content_checked(..., true)` inspects a missing or mismatched pack read-only, retains the recorded identity, and refuses later saves with `content_read_only`. Q-116 and Q-118 are answered by proposed ADRs D-036 and D-037; neither ADR is accepted and no loader code exists for either. Q-117 remains the only open content question: the server CLI, client mismatch screen, and simulation freeze list are still undecided.

| ID | Question | Current assumption / recommendation | Priority |
|---|---|---|---|
| **Q-117** | How does a content-bound world load when the required pack is missing, and what can a read-only mode safely preserve? | **D-035 bridge v1 (Proposed):** strict load stays fail-closed; the opt-in `world_load_content_checked(..., true)` opens a read-only inspector with status `missing`/`mismatch`, keeps unknown IDs opaque, retains the recorded identity, and blocks saves with `content_read_only`. The server CLI flag, client mismatch screen, and a content-dependent simulation freeze list remain open. | High |

## Voxel edit questions

These support D-038 (Proposed, 2026-11-25) and [[79 - Voxel Edit Sync]].

| ID | Question | Current assumption / recommendation | Priority |
|---|---|---|---|
| **Q-119** | What happens when two players edit the same voxel or submit overlapping batches? | **Recommendation:** exact-revision optimistic concurrency; reject a stale batch with `blueprint.revision_stale`, reload, and let the player retry. No CRDT or silent last-writer-wins. A merge UI and per-cell ownership are alternatives; if chosen, they must still produce one deterministic effective volume. | High |
| **Q-120** | How long does a placement edit lock live, and does it survive reconnect? | **Recommendation:** a 300-tick lease refreshed by accepted edits, released on disconnect, Admin force-release; reconnect does not silently reclaim. Alternatives are explicit unlock only or a resume-time reclaim keyed by the session token. | High |
| **Q-121** | Does the settlement tax compose multiplicatively with the D-030 district multiplier (option A, per-location cap 8.0), or additively (option C)? | **Recommendation:** option A: `min(8.0, m_i × (1 + s_i/100))` per location, then the existing weighted mean. It preserves the district signal and treats a settlement as a surcharge; option C is the balance alternative. | High |
| **Q-122** | When is settlement tax collected, and how are mid-day rate changes and dormant catch-up attributed? | **Recommendation:** once per economy day keyed by the persisted `days_elapsed` watermark; rate changes apply next tick; paused time collects nothing; dormant catch-up uses the stored rate per simulated day. | High |

## Journal and rollback questions

These support D-040 (Proposed, 2026-11-25) and [[82 - Journal and Rollback]]. The journal is not implemented; the existing audit ring and 300-tick resume window are unchanged.

### Q-123 — Journal retention, payload capture, and snapshot cadence

**Priority:** High · **Affects:** D-040, [[05 - Multiplayer]], [[12 - Technical Architecture]], [[75 - Save Load Stress]], [[82 - Journal and Rollback]]

**Question:** What are the default retention limits (entries and tick age), is raw command payload capture on by default or opt-in, and how often are state snapshots written?

**Why it matters:** The defaults decide whether arbitrary-target rollback is possible, how much disk the journal consumes, and how much accepted work a crash can lose. Payload capture also decides whether chat or personal text could ever enter the journal.

**Options**
- **Hash-only, 100k entries / 7 in-game days, snapshot every 1,000 accepted entries:** smallest and private; rollback is limited to exact snapshot ticks.
- **Payload capture with the same retention:** arbitrary-target rollback; larger disk and a wider privacy surface.
- **Short hash-only (10k / 1 day) with dense snapshots:** tight loss window, more snapshot I/O.
- **Long capture (1M / 30 days):** support-friendly, but not a Phase 6 disk or tick budget.

**Current assumption:** hash-only default; 100,000 entries and 7 in-game days, whichever is reached first; snapshot every 1,000 accepted entries, on clean shutdown, and before a rollback; payload capture is an Admin opt-in with a bounded total budget.
**Recommendation:** ship the hash-only default and measure append/replay cost at 100k entries before widening retention. Never capture chat, personal text, tokens, or credentials, even in payload mode.
**Status:** Open

### Q-124 — Rollback authorization and quorum

**Priority:** High · **Affects:** D-040, [[05 - Multiplayer]], [[82 - Journal and Rollback]]

**Question:** Who may authorize a rollback on a live server, and does it require a second Admin, the affected settlement owner, or an online-player vote?

**Why it matters:** A rollback discards accepted commands for every online player at once. Too little authority makes griefing or an accident easy; too much makes recovery impossible during an incident.

**Options**
- **Single Admin, audited:** fastest recovery; relies on the Admin role and the two-step preview.
- **Two Admin confirmations:** a second Admin or an operator token must confirm the preview; safer for shared servers.
- **Affected settlement owner consent:** limits blast radius but leaves city-wide state unexplained and adds an offline-owner dead end.
- **Online simple-majority vote:** matches D-021 time control, but recovery is not a gameplay preference and a griefer can stall the vote.

**Current assumption:** single Admin on a local/self-hosted server, always with the dry-run preview and explicit apply; when more than one player is online, a second Admin or operator confirmation is recommended.
**Recommendation:** keep rollback Admin-only and two-step; require a second confirmation for a multi-player server, not a player vote. Record the authorization mode in the journal and audit entry.
**Status:** Open

## Remote transport and authentication questions

These support D-041 (Proposed, 2026-11-25) and [[83 - Remote Transport and Authentication]]. The current listener is loopback-only and plaintext; the resume token is seed-derived and is not authentication (F-039). Nothing in this section is implemented.

### Q-125 — Launch account provider

**Priority:** High · **Affects:** D-041, [[05 - Multiplayer]], [[12 - Technical Architecture]], [[78 - Code Audit Findings]], [[83 - Remote Transport and Authentication]]

**Question:** Which account provider ships at launch, and is a local account file enough for self-hosted servers?

**Why it matters:** The provider decides whether a dedicated server can run without a third-party service, how accounts are provisioned and recovered, and whether `AccountId` is portable across saves and servers. It also decides whether Steam becomes a hard launch dependency.

**Options**
- **Local account file only:** operator-provisioned `{account_id, argon2id, display_name}` records; works offline and on DRM-free servers; no recovery flow beyond an operator reset.
- **Steam identity first:** familiar login and friend graph; excludes non-Steam servers and makes Valve availability part of the join path.
- **Both from day one:** best coverage, but two providers, two test matrices, and a UI that must explain the choice.
- **Operator token / console only:** fine for a trusted LAN, not a remote co-op product.

**Current assumption:** local account file first; Steam is the first later provider behind the same `AccountProvider` abstraction; no OAuth at launch.
**Recommendation:** ship the local provider and keep provider selection a config choice. Add Steam after the transport contract is stable; do not make it the only way to join. A first authenticated join may create the account link, but account creation stays operator-controlled in v1.
**Status:** Open

### Q-126 — Token lifetime, renewal, and TLS termination

**Priority:** High · **Affects:** D-041, [[05 - Multiplayer]], [[12 - Technical Architecture]], [[83 - Remote Transport and Authentication]]

**Question:** What are the session-token lifetime and renewal rules, and does TLS terminate inside the server process or at a proxy?

**Why it matters:** Token lifetime bounds the damage from a leaked token and the frequency of re-login. Rotation rules decide whether a lost response locks a player out. TLS termination decides whether channel binding is available or the process must trust a proxy header.

**Options**
- **In-process rustls, 24 h absolute token, rotate on every resume, reuse revokes the family:** simplest trust model and the strongest binding; a lost rotation response forces re-authentication.
- **Short access token (15 min) plus a longer refresh token:** standard web shape; more moving parts and a second secret for a game session.
- **Process-lifetime token with no rotation:** fewest reconnects; a leak lasts until restart or manual revocation.
- **Proxy termination with a trusted header and binding disabled:** familiar ops and certificate automation; the process cannot distinguish a replayed token from a legitimate proxy-forwarded one without extra work.
- **Proxy termination with a passthrough TLS exporter:** keeps binding, but requires proxy support and careful configuration.

**Current assumption:** rustls in-process; 24 h absolute expiry; rotate on every resume; a stale generation revokes the family; failure codes `auth.expired` and `resume.revoked`. Proxy termination is opt-in only.
**Recommendation:** keep TLS in-process by default and enforce server-side expiry with no client-clock input. Measure reconnect friction before lengthening the token; do not add a refresh-token family until a real deployment needs it. If a proxy is supported, require an explicit trusted-proxy address and document that channel binding is disabled in that mode.
**Status:** Open

### Q-127 — Multiple active content packs and namespace merging

**Priority:** Medium · **Affects:** D-034, D-036, D-042, [[06 - Modding and Resource Packs]], [[84 - Content Registry Modes]]

**Question:** Can more than one content pack or resource pack be active at once, and how do their IDs and namespaces merge?

**Why it matters:** D-034 currently binds a save to one active pack. A server registry with several entries is useful for mods and packs, but two packs claiming the same namespace make dependency resolution and save identity ambiguous.

**Options**
- **One D-034 pack plus explicit mods/packs:** preserves the current save identity; overlapping IDs are rejected.
- **Multiple packs with a deterministic merge order:** more flexible, but the save must pin the whole ordered set and checksums.
- **One pack only, no separate mod list:** simplest, but blocks mixed servers.

**Current assumption:** v1 keeps one D-034 content pack and an explicit registry of additional mods/packs; duplicate IDs/namespaces across entries are rejected.
**Recommendation:** keep the one-pack rule until a real mixed-server use case appears; then write a new ADR for ordered multi-pack identity rather than loosening D-034 silently.
**Status:** Open

### Q-128 — Registry distribution and trust

**Priority:** Medium · **Affects:** D-042, [[12 - Technical Architecture]], [[83 - Remote Transport and Authentication]], [[84 - Content Registry Modes]]

**Question:** How is the server content registry distributed and trusted across server and client, and who signs it?

**Why it matters:** An operator-supplied local file is enough for a self-hosted server. A public server ecosystem needs a way to verify that the registry and packs came from the expected publisher, and clients need a mismatch story that does not auto-download.

**Options**
- **Local operator file only:** simplest; no signatures, no remote fetch.
- **Signed registry with an embedded publisher key:** verifies provenance, but adds key distribution and rotation.
- **Registry fetched from a URL with a pinned hash:** easier updates, but the URL becomes a trust and availability dependency.
- **Workshop/Steam-managed distribution:** platform-native later, not a v1 requirement.

**Current assumption:** v1 is an operator-supplied local manifest with exact checksums; no signatures, URLs, or auto-download.
**Recommendation:** start local and exact; add signatures only when public servers or third-party registries exist. Never let a client supply the registry.
**Status:** Open

## Construction questions

| ID | Question | Current assumption / recommendation | Priority |
|---|---|---|---|
| **Q-083** | How much construction management? | Auto-assign crews; player sets priority/budget; no individual worker micromanagement | High |
| **Q-084** | Public works or private contractors? | Public works crews + outside contractors at a premium; private construction companies later | High |
| **Q-085** | Can buildings be partially occupied during construction? | No; occupancy starts after commissioning/inspection | High |
| **Q-086** | Do roads and utilities use the same crew/trade system? | Yes, simplified crews/phases with the same blocker model | Normal |
| **Q-087** | How visible is material delivery? | Trucks/workers near the camera; aggregate flow off-screen | Normal |
| **Q-088** | Construction time scale? | Hours/days at 1×; small house days, factory weeks/months; fast-forward | High |
| **Q-089** | Can the player pay extra to expedite? | Overtime/contractor premium; worker fatigue later | Normal |

## Materials and structure questions

| ID | Question | Current assumption / recommendation | Priority |
|---|---|---|---|
| **Q-090** | Structural validation depth: rule-based limits or stress simulation? | Rule-based limits for v1; stress simulation later, if ever | High |
| **Q-092** | Does a roof count as a story? | No; roof adds height/load but not a story | Normal |
| **Q-093** | Can players mix materials per floor/wall? | Yes, with load-path and weakest-link rules | High |
| **Q-094** | Are glass facades cosmetic or functional? | Functional (energy/comfort/class) but non-structural | Normal |
| **Q-095** | Which service gates are hard blocks vs penalties? | Hard for water/wastewater/egress/fire/elevators/water pressure/structural; soft for telecom, parking, HVAC efficiency, backup power | High |
| **Q-096** | How do modded material changes migrate existing blueprints? | Revalidate and mark “needs review”; never silently break | High |
| **Q-097** | Basement launch scope? | Cellars and utility basements first; habitable basements/parking later | High |
| **Q-098** | Maximum basement depth and groundwater/flood model? | 32 m blueprint cap; practical 1–2 levels; deeper needs engineering | Normal |
| **Q-099** | When do underground parking and vehicle ramps arrive? | After traffic; parking as capacity + visible ramps near camera | Normal |
| **Q-100** | Underground tunnels/transit between buildings? | Post-launch; keep building basements separate at first | Low |

## Simulation questions

| ID | Question | Current assumption / recommendation | Priority |
|---|---|---|---|
| **Q-040** | Citizen simulation depth? | Cohorts/aggregate for the 1M population; individual agents near camera; 2k active | High |
| **Q-041** | Time scale: how long is a day/month at 1×? | 1 day ≈ 15 real minutes; 1 month ≈ 1–2 hours; tune | High |
| **Q-042** | Household or individual as the primary unit? | Household for housing/budget; individual for jobs/needs | High |
| **Q-043** | Traffic fidelity? | Macro flow + visual cars; no lane-level microsimulation v1 | High |
| **Q-044** | Economy tone: arcade or realistic? | Mid-weight, readable, forgiving; pressure without spreadsheet | High |
| **Q-045** | Crime and health depth? | Basic service coverage + stat effects; no full crime simulation v1 | Normal |
| **Q-046** | Interior pathfinding for NPCs? | No v1; room-level graph later if citizen mode happens | Low |
| **Q-047** | Individual citizens visible at mayor altitude? | Dots/massing only beyond ~300 m | Low |
| **Q-048** | Seasons and weather? | **Answered:** seasons at launch with an on/off toggle; weather later | High |

## Localization questions

| ID | Question | Current assumption / recommendation | Priority |
|---|---|---|---|
| **Q-079** | Which additional languages after German, and in what order? | English + German at launch; add by demand; language packs | Low |
| **Q-080** | Voiceover later, or subtitles only? | Subtitles only in v1; voice later | Low |
| **Q-081** | Community translation workflow? | In-repo PRs first; translation platform or mod packs later | Normal |
| **Q-082** | Should server logs/admin messages be localized? | Server logs stay English; player-facing messages use keys | Normal |

## Production and business questions

| ID | Question | Current assumption / recommendation | Priority |
|---|---|---|---|
| **Q-049** | Fixed release date or funding milestone? | None yet; plan by milestones | High |
| **Q-050** | Steam at launch? Workshop at launch? | Steam likely; Workshop after launch | High |
| **Q-051** | Is a demo/vertical slice needed for funding or marketing? | Build the vertical slice regardless; use it as a demo if needed | Normal |
| **Q-052** | Minimum hardware target? | Define in Phase 0; mid-range 2024–2026 desktop at 1080p/60 | High |
| **Q-101** | What are the top three usability issues from the 30-minute vertical-slice playtest? | Run the route in [[66 - Playtest Kit]] with a fresh tester, build a house without reading docs, and record the issues before closing Phase 1 | High |

## Technical questions

| ID | Question | Current assumption / recommendation | Priority |
|---|---|---|---|
| **Q-053** | One `core` crate first, or the full crate split from day one? | One crate first; split when boundaries stabilize | Normal |
| **Q-054** | `bevy_ecs` standalone or a custom SoA simulation layout? | `bevy_ecs` first; profile and replace hot paths if needed | High |
| **Q-055** | Godot ENet or a Rust QUIC transport for v1 multiplayer? | ENet first; keep transport abstracted | High |
| **Q-056** | SQLite for all state, or a binary snapshot for hot simulation data? | SQLite for structured state; binary snapshots only if profiling demands | Normal |
| **Q-057** | Mesh upload on the main thread or via `RenderingServer` from a worker? | Measure in the Phase 0 spike | High |
| **Q-058** | Blueprint overrides: new version or per-instance diff? | New version for shared edits; per-instance diff for live tweaks | Normal |
| **Q-102** | Should construction phases take game days rather than seconds? | Keep the developer slice at seconds; [[10 - Decisions]] D-029 proposes a mode-selected campaign clock (1 game day = 60 s, cottage 2–4 days, 30-day phase cap). Still open: human sign-off and a measured financing run with the live clock. | High |
| **Q-103** | Persistence scope: persist every named identity, or only committed/selected ones? | Commit on select, story, or lifecycle; regenerate uncommitted from seed | High |
| **Q-104** | Materialization policy: distance-only or selection/event priority, and the exact radii and hysteresis? | Selection > story > distance; 80 m in, 120 m out; 64 changes per tick | High |
| **Q-105** | Death and replacement: does a named death reduce aggregate population, and at what rates? | Yes for named slots; off in default benchmarks; dedicated lifecycle profile | High |
| **Q-106** | Household composition: singles, couples, or families in the first slice, and who owns the budget? | Household owns the budget; named members share it; no family tree | Normal |

## Art and content questions

| ID | Question | Current assumption / recommendation | Priority |
|---|---|---|---|
| **Q-059** | Citizen art style: stylized low-poly, voxel, or billboard? | Stylized low-poly; silhouettes readable at street level | Normal |
| **Q-060** | Visual variation between biomes/maps at launch? | 3–4 biomes in the procedural world (forest, plains, hills, coast); moddable later | High |
| **Q-061** | UI language: clean modern dashboard or diegetic/newspaper style? | Clean modern dashboard with optional diegetic accents | Normal |

## Scale and living world questions

| ID | Question | Current assumption / recommendation | Priority |
|---|---|---|---|
| **Q-067** | Ecosystem balance: overhunting, deforestation, predators, pests? | Light ecosystem: population pressure and regrowth, not a full ecology sim | High |
| **Q-068** | Visible transport for lumber, crops, and hunted goods? | Visible vehicles/workers near camera; abstract off-screen flow | Normal |
| **Q-070** | 1M multiplayer: players per server and server hardware? | 8 players, cap active regions (e.g., 4), 4–8 CPU cores, 16 GB RAM; measure | High |
| **Q-071** | Terrain editing resolution: 1 m or 10 cm? | 1 m terrain edits; 10 cm building detail; local foundation edits only | High |
| **Q-072** | How moddable is worldgen (biomes, features, resources)? Can mods replace the generator? | Data-driven additive biomes/features; sandboxed generators; no full replacement at launch | High |

## Benchmark questions

| ID | Question | Current assumption / recommendation | Priority |
|---|---|---|---|
| **Q-073** | Are Early/Mid/Late scenes enough, or do we need day/night, winter, rain, and heavy-traffic variants? | Three scenes + optional stress at launch; variants later | Normal |
| **Q-074** | Should players be able to share/upload benchmark results? | Local-only at launch; opt-in share code later | Low |
| **Q-075** | Should the default benchmark use a standard preset or the player's current settings? | Both; standard preset is the comparable one | Normal |
| **Q-076** | How long should benchmark routes be? | 90–180 s measured + 10–15 s warm-up | Normal |
| **Q-077** | Should modded benchmark scenes be supported at launch? | Base-only at launch; modded runs marked non-comparable | Normal |

## Answered

| ID | Question | Answer | Date | ADR |
|---|---|---|---|---|
| **Q-003** | City size and population target? | 1M+ is the late-game target; at 10k–20k people/km² a metropolis occupies 50–100 km²; world is effectively infinite. | 2026-11-19 | D-017 |
| **Q-091** | Are the initial story/height targets right? | Yes for now: wood 2–3, half-timber 3–4, concrete 20–30, steel 40–80; tune later if needed. | 2026-11-19 | D-024 |
| **Q-016** | Localization at launch? | English + German at launch; all strings externalized; moddable language packs later. | 2026-11-19 | D-022 |
| **Q-048** | Seasons and weather? | Seasons at launch with an on/off toggle; weather later. | 2026-11-19 | D-018 |
| **Q-062** | 1M target and hardware? | Late-game target; 8-core/16-thread CPU, RTX 3060-class GPU, 16–32 GB RAM. Performance expectation is **~30 FPS at 1080p** for a 1M city on that hardware; 60 FPS for ~100k. Dedicated server recommended for simulation. Benchmark in Phase 0/5. | 2026-11-19 | D-017 |
| **Q-063** | Finite world or effectively infinite? | Effectively infinite; seed + edit deltas; no hard edge. | 2026-11-19 | D-016 |
| **Q-064** | Is cohort simulation acceptable for most of the 1M population? | **Yes.** Individual records for identity/selection; cohorts for tick updates; individual agents near camera/selection. This is the model that makes 1M feasible. | 2026-11-19 | D-017 |
| **Q-065** | Living world depth? | Hybrid: individual trees/animals/workers near the camera; aggregate populations/resources elsewhere. | 2026-11-19 | D-018 |
| **Q-066** | One metropolis or multiple settlements? | Multiple settlements; each player may manage their own, and one player may manage several. | 2026-11-19 | D-019 |
| **Q-069** | Seasons at launch? | Yes, with an on/off toggle; four seasons with simple growth/migration effects. | 2026-11-19 | D-018 |
| **Q-001** | Team size and skills? | 2–3 full-time generalists with Rust/Godot experience or willingness to learn. | 2026-11-19 | — |
| **Q-004** | Terrain resolution? | 1 m terrain + 10 cm buildings; 10 cm terrain is out. | 2026-11-19 | D-002 |
| **Q-005** | Where does blueprint authoring happen? | Workshop/creative authoring first; build anywhere for placement; permissioned live editing after multiplayer stabilizes. | 2026-11-19 | D-026 |
| **Q-006** | Is destruction required? | None in v1; cosmetic damage and repair later. | 2026-11-19 | D-008 |
| **Q-107** | How does a `.tsbp` package reach the authoritative server? | Pre-seeded absolute content-addressed server library for v1; the client sends only the hash. No upload, fetch, or auto-download. Provenance, quotas, and transit integrity remain future work. | 2026-11-23 | D-032 |
| **Q-108** | How are modded blueprints and dependencies resolved when the server lacks the mod? | The server must already hold the pack and its dependencies; unknown packages are rejected with a stable code and never auto-downloaded. Removal policy remains future work. | 2026-11-23 | D-032 |
| **Q-109** | Placement rate limits, quotas, idempotency, and audit? | Exact placement identity (hash + centre + rotation) is the idempotency key; a repeat returns the existing placement without a second charge. Reuses the existing per-session token bucket and audit ring; per-player quotas remain future work. | 2026-11-23 | D-032 |
| **Q-110** | Server placement save migration and rollback/history? | Optional `ServerSave` placement/construction fields with serde defaults; older saves without them load unchanged. No migration beyond load-old/ignore-missing; rollback, history, and GC remain future work. | 2026-11-23 | D-032 |
| **Q-111** | What is the stable player identity source — a server-issued per-save id, Steam ID, account/password, or the current display name? | Server-issued per-save `PlayerId(u64)`, with the display name as a mutable label; Steam/account authentication later. A free-form owner string is never identity. | 2026-11-24 | D-033 |
| **Q-116** | How stable must a pack checksum be across pack versions, and can a save accept a compatible newer pack? | Exact `pack_checksum` remains the default. A pack may declare a bounded list of prior pack/registry checksum pairs plus a `compatibility_reason`; loading one requires an explicit opt-in and records the accepted prior checksum. No version ranges, auto-accept, or silent rewrite; a changed `registry_checksum` still fails unless the exact pair is listed. **Proposed (D-036); no loader code yet.** | 2026-11-24 | D-036 |
| **Q-118** | How are renamed or removed content IDs migrated in saves and blueprints, and who owns the mapping? | Every pack manifest declares a monotonic `content_version` and explicit per-format `from -> to` maps; base migrations ship with the game. Unmapped IDs fail the load with no substitution or deletion. Applying a migration is an explicit save rewrite with a result summary, and saves and blueprints share the same map. **Proposed (D-037); no loader code yet.** | 2026-11-24 | D-037 |

## How to answer a question

1. Add the answer here, with the date and a one-line rationale.
2. If it changes a decision, write an ADR in [[10 - Decisions]] and update the affected docs.
3. Move the question from the open table to the **Answered** section.
4. Update the `updated` field on every doc you touched.
5. If the answer creates new questions, add them with the next IDs.

## Question ID index

| Range | Area |
|---|---|
| Q-001–Q-006 | Blocking architecture and scope |
| Q-007–Q-027 | Design |
| Q-028–Q-032 | Multiplayer |
| Q-033–Q-039 | Modding |
| Q-040–Q-048 | Simulation |
| Q-049–Q-052, Q-101 | Production and business |
| Q-053–Q-058 | Technical |
| Q-059–Q-061 | Art and content |
| Q-062–Q-072 | Scale, living world, and worldgen |
| Q-073–Q-077 | Benchmarks |
| Q-078 | Multiplayer time-vote tuning |
| Q-079–Q-082 | Localization |
| Q-083–Q-089 | Construction |
| Q-090–Q-100 | Materials, structure, and basements |
| Q-107–Q-110 | Server-authoritative blueprint placement |
| Q-111–Q-115 | Settlement ownership, membership, and permissions (Q-111 answered) |
| Q-116–Q-118 | Content-pack identity in world saves (D-034 accepted; D-035 bridge v1; Q-116/Q-118 answered as proposed ADRs) |
| Q-119–Q-120 | Authoritative placed-blueprint voxel edits (D-038 proposed) |
| Q-121–Q-122 | Settlement tax composition and collection (D-039 proposed) |
| Q-123–Q-124 | Authoritative command journal and Admin rollback (D-040 proposed) |
| Q-125–Q-126 | Remote transport and authentication (D-041 proposed) |
| Q-127–Q-128 | Content registry modes and trust (D-042 proposed) |
