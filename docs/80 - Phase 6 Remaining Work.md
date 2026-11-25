---
title: Phase 6 Remaining Work
project: Townsmith
status: proposed
updated: 2026-11-25
tags:
  - townsmith
  - multiplayer
  - roadmap
  - planning
---

# Phase 6 Remaining Work

> [!abstract] Purpose
> This is the dependency-ordered remainder of **Phase 6 — Multiplayer**. It replaces the stale Phase 6 checklist in [[08 - Roadmap]] with the actual code, gaps, acceptance criteria, and cut lines at master `c5e37b9` (2026-11-25). It is a plan, not an implementation claim.

Related: [[05 - Multiplayer]], [[08 - Roadmap]], [[10 - Decisions]] D-004/D-021/D-032/D-033/D-035/D-038/D-039/D-040/D-041, [[09 - Open Questions]] Q-101/Q-117/Q-119/Q-120/Q-121/Q-122/Q-123/Q-124/Q-125/Q-126, [[12 - Technical Architecture]], [[42 - World Placement Bridge]], [[48 - Settlement Boundaries]], [[79 - Voxel Edit Sync]], [[81 - Settlement Tax Composition]], [[82 - Journal and Rollback]], [[83 - Remote Transport and Authentication]].

## 1. Where Phase 6 stands

| State | Slice | Evidence |
|---|---|---|
| ✅ Done | Authoritative loop, protocol v1, validation, snapshots, roles, rate limits, audit ring | #262/#266/#287/#337 |
| ✅ Done | Reconnect v0 inside the process-local 300-tick window | #269, docs/05 |
| ✅ Done | Interest views (≤64 roads, ≤64 building records), time vote | #308, #287 |
| ✅ Done | D-032 authoritative `place_blueprint`, bridge references, catalog placement UI | #306/#308/#309/#311/#318 |
| ✅ Done | D-033 stable player ids, settlement persistence, bridge read/mutation, placement `build`, settlement protocol (found/list/at/claim/rename/transfer/release/invite/set-role/remove/set-tax) | #319/#321–#330/#342/#343 |
| ✅ Done | D-035 read-only missing-pack load, server content-hash pin, client content-mismatch screen | #331/#332/#340 |
| ✅ Done | Bounded pollable chat log, chat panel, settlement editor, district membership readout | #333/#344/#345/#346 |
| ✅ Done | Chat client smoke, blueprint locks v0, chat loopback, content-aware placement, voxel-edit ADR | #348–#352 |
| ✅ Done | CI UI smokes, district tax readout, chat-panel EN/DE, dashboard tax control, settlement benchmark, this plan | #353–#358 |
| ✅ Done | Settlement-tax ADR proposal (D-039), save-schema freeze tests, settlement loopback smoke, placement lock UI, settlement-editor/dashboard localization | #359–#364 |
| ✅ Done | Pure D-039 tax helper, CI multiplayer smokes, nested content-mismatch smoke fix, bounded lock leases, district-panel membership rows, D-040 journal/rollback proposal | #366–#371 |
| ✅ Done | Bounded outbound queue, lock-lease badge, opt-in lock retention, local bridge player registry, D-041 remote-auth proposal | #373–#377 |
| ⛔ Blocked | Q-101 human playtest; 1M/RTX 3060 render gate | person / reference hardware |

## 2. Dependency order

| # | Slice | Depends on | Why this order |
|---:|---|---|---|
| 1 | Dashboard + connected-client settlement write controls | #343 bridge mutations | Makes the existing settlement protocol usable in play without new protocol/save surface |
| 2 | Settlement tax treasury consumer after D-039 acceptance | 1, D-030/D-031 | The pure helper exists, but the rate is data-only; applying it needs a precedence decision against district weights |
| 3 | Blueprint lock retention policy + CLI | #349 v0 locks, #363/#374 UI, #369 leases, #375 opt-in retention | Lease expiry and opt-in retention are done; expose configuration and decide multi-session semantics |
| 4 | Concurrent listener sessions + outbound-queue wiring | #269, #350, #373 | The queue data structure exists; wire a writer and preserve deterministic order |
| 5 | Chat push/broadcast | 4 | Replaces polling; must not duplicate entries or reorder sequences |
| 6 | Voxel edit sync v1 | D-038 accepted, Q-119/Q-120, 3, 4 | The largest slice; do not start before the conflict/lease decisions |
| 7 | Journal/rollback v1 after D-040/Q-123/Q-124 | 6 | Needs the edit command stream and a crash-safe boundary |
| 8 | Save migration/versioning freeze | 1–7 | Freeze settlement/edit/journal/lock schemas before remote exposure |
| 9 | Remote transport + authentication | 4, 8 | Resume tokens are not auth; F-039 documents the gap |
| 10 | Phase 6 exit scenarios | 1–9 | The 4-player proof, not a feature |

## 3. Slice details

### 3.1 Settlement write controls

**Scope.** Dashboard and a connected client should found, claim, rename, transfer, invite, set role, remove, release (Admin), and set tax through the bridge/server commands that already exist. The dashboard currently has only the merged `Shift+T` tax cycle (#356); #370 adds full owner/member/permission rows to the read-only district panel; the settlement editor is the full developer surface.

**Protocol/save.** No new command or save field. Reuse `settlement_*` v1 shapes and `settlement_updated`.

**Permissions.** Global role is a ceiling; settlement `members`/`manage`/`tax`/`build` checks stay in the core/store. Admin-only release. Every rejection is code+params, never a pre-localized string.

**Tests.** Bridge unit tests for each mutation and revision conflict; Godot smoke for key routing, owned/unowned states, rejected calls leaving checksums unchanged; server loopback for a two-player invite/role/transfer sequence.

**Not bundled.** Per-district permissions, lot claims, settlement merge/split, membership UI beyond the selected record.

### 3.2 Settlement tax treasury consumer

**Scope.** Give `Settlement.tax_rate` a defined economic effect. The open decision is composition with D-030 home/workplace weights and D-031 service funding: settlement surcharge first, district multiplier first, or cap the combined rate. D-039 records the proposed option A, and #366 adds the pure helper; record acceptance before wiring a consumer.

**Protocol/save.** Prefer no protocol change; the rate already round-trips. If revenue is reported, extend the snapshot/audit read model, not the save.

**Permissions.** `tax` for setting; collection is server-side. No client-supplied base.

**Tests.** Unit: zero rate, 100%, rounding, population outside a settlement, overlapping settlements (smallest id or explicit rule), cap. Integration: city loop with a settlement surcharge; before/after treasury JSON with a fixed seed.

**Benchmark.** `city_loop` Early/Mid/Late with and without the surcharge; confirm no tick-budget regression.

**Not bundled.** Per-building tax, land value, bankruptcy changes, district budget UI.

### 3.3 Blueprint locks: UI, TTL, reconnect

**Scope.** #349 ships advisory in-memory locks and player-wide release on disconnect; #363 adds the placement-UI lock badge and `L` toggle; #369 adds a lazy 300-tick lease (1–3600 clamp) and refreshes on same-holder lock; #374 shows the lease; #375 adds opt-in retention across disconnect. Remaining: optional config/CLI switch for retention and lease length, a multi-session policy decision, and explicit Admin force-unlock audit.

**Protocol/save.** Additive fields on the existing lock responses; if locks become durable, a new optional `ServerSave` store and `save.invalid_blueprint_locks`. Do not make locks authoritative for package identity.

**Permissions.** Mayor lock/unlock; holder or Admin force; placement already checks the holder.

**Tests.** Deterministic clock/lease expiry, two players contending, reconnect before/after expiry, restart clearing, transport rejection, no checksum movement for expired-only state.

**Not bundled.** Collaborative editing, lock transfer, multi-package locks, package upload.

### 3.4 Concurrent listeners and outbound queue

**Scope.** The current listen loop accepts `--sessions N` but processes sequentially. #373 adds a bounded `OutboundQueue` data structure; wire a per-connection writer and the queue so one slow client cannot stall ticks or chat broadcast. Preserve deterministic command ordering per connection.

**Protocol/save.** No wire-shape change. Add backpressure/closure codes and a bounded queue metric.

**Tests.** Network-fault matrix from #317 plus slow-reader, half-closed, concurrent broadcast, and shutdown-under-load cases; deterministic tick/checksum.

**Benchmark.** 4 clients, 2k active agents, 10 Hz snapshots; p99 tick and queue depth.

**Not bundled.** Remote TLS/auth, matchmaking, public server browser.

### 3.5 Chat push/broadcast

**Scope.** Push `chat_sent`/`chat_log` deltas to connected clients instead of polling only; keep `chat_since` for reconnect. Bounded log and 64-line cap stay.

**Protocol/save.** Add a server push message or include chat deltas in the existing outbound stream; no save field. Sequence numbers remain authoritative.

**Tests.** Two-client broadcast, reconnect catch-up, duplicate suppression, rate-limit rejection, no tick advance, checksum stability.

**Not bundled.** Moderation, channels, DMs, voice, translation of player text.

### 3.6 Voxel edit sync v1

**Scope.** Implement [[79 - Voxel Edit Sync]] after **D-038 is accepted** and Q-119/Q-120 are answered. One placed blueprint, bounded batches, exact-revision optimistic concurrency, placement-scoped lease, interest-view revisions, optional additive `ServerSave.placement_edits`.

**Protocol/save.** `edit_placement_voxels` / `placement_edits` and `placement_edits_applied`; `save.invalid_placement_edits`; no full-volume network payloads.

**Permissions.** D-033 `build` on the containing settlement, global role ceiling, lock holder, revision, bounds, material, batch size.

**Tests.** Add/remove/replace, duplicate positions, bounds, stale revision, lock contention, two clients, reconnect, save/load round-trip, deterministic checksum, 1k edits/s benchmark.

**Not bundled.** Destruction physics (D-008), terrain sculpting, CRDT/OT, rollback, upload/fetch.

### 3.7 Journal/rollback v1

**Scope.** Crash-safe command journal for accepted mutations plus a bounded rollback/history surface. The journal is for recovery and audit, not a second simulation state. D-040 and [[82 - Journal and Rollback]] propose the v1 contract; Q-123/Q-124 remain open.

**Protocol/save.** Append-only journal with sequence and checksum; replay validates schema/version; rollback is Admin-only and audited.

**Tests.** Kill/restart mid-write, truncated tail, replay from snapshot, idempotent replay, corrupt journal fallback, save/load equivalence.

**Not bundled.** Branching history, per-player undo of city-wide commands, long-term archival.

### 3.8 Save migration/versioning freeze

**Scope.** Freeze the schema for settlements, placement edits, journal, and locks before remote exposure. Add migration tests for every outer/store version pair.

**Tests.** Golden v1/v2 fixtures, byte-identical no-op loads, reject unknown future versions, migration idempotence, content identity before any store.

**Not bundled.** Auto-downgrade, cross-pack conversion, data repair heuristics.

### 3.9 Remote transport and authentication

**Scope.** Move beyond loopback: TLS (or an operator-terminated tunnel), authenticated join, per-player identity that survives reconnect/restart, and a documented trusted-LAN/operator-token mode. F-039 applies: the resume token is derivable and not auth.

**Tests.** Bad token, expired token, replay, reconnect, certificate/hostname failure, rate-limit, role escalation attempts, no state mutation on rejection.

**Not bundled.** Steam/account integration, public server browser, matchmaking, anti-cheat beyond authority/validation.

### 3.10 Exit scenarios

Prove, on reference hardware and with four players:

1. Four players build and manage one city for one hour without desync or data loss.
2. One edits a blueprint while another places it; no corruption, stale write, or silent overwrite.
3. A client disconnects mid-edit and reconnects without losing the city or the lock (or gets a clear lease-expired path).
4. The dedicated server restarts and reloads the save with identical checksums.
5. Time pause/resume/fast-forward works via admin and vote; Mode B does not pause the server.
6. While paused, placement/removal/edits work and construction resumes correctly.
7. Bandwidth and tick time stay within target on the reference matrix.
8. A missing/mismatched content pack fails closed with the documented read-only or blocked path.

## 4. Cut lines

**Cannot cut:** server authority, save reliability, reconnect, D-021 time semantics, D-032 placement authority, D-033 settlement enforcement, content mismatch failure, the 4-player proof.

**Can cut/defer, in order:**

1. Chat push to polling (keep chat itself).
2. Blueprint lock UI polish (keep the server lock/lease).
3. Journal history UI (keep crash-safe recovery).
4. Settlement tax treasury consumer (keep the data-only rate).
5. Voxel edit sync beyond one blueprint/bounded batches.
6. Remote auth to a documented trusted-LAN/operator-token mode (never silently unauthenticated public play).
7. Public server browser; keep direct IP/invite.

## 5. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Voxel sync becomes a second voxel engine | Critical | Reuse `core_voxel`/placement records; one blueprint, bounded deltas, no full-volume wire |
| Lock lease deadlocks or stale locks | High | Injectable clock, explicit expiry, Admin force, audit; no tick-only lease |
| Tax consumer destabilizes the economy | High | ADR for composition, caps, headless before/after matrix |
| Sequential listener stalls under chat/edit push | High | Slice 4 before push/voxel broadcast |
| Save schema churn after remote exposure | Critical | Slice 8 freeze + migration fixtures |
| Auth treated as "resume token" | Critical | F-039; explicit auth slice, fail closed |
| Fake progress from UI-only controls | Medium | Every write control must hit the bridge/server and prove rejection paths |

## 6. Human and hardware gates

- **Q-101 30-minute playtest** — blocked on a human player/observer; capture form and summarizer are ready. It is a Phase 6 exit input, not an agent task.
- **1M / RTX 3060 render gate** — blocked on reference hardware; local cohort/save/streaming evidence remains `insufficient` without render metrics. Do not convert it into a pass.
- **Four-player session** — needs four humans or four instrumented clients; automate the checksums and command script first.

## 7. Immediate order

1. Verify the merged #353–#364 wave and refresh this table with [[08 - Roadmap]].
2. Settlement write controls (§3.1) and the tax composition ADR (§3.2).
3. Blueprint lock lease/UI (§3.3).
4. Concurrent listener queue (§3.4), then chat push (§3.5).
5. Accept or revise D-038; answer Q-119/Q-120; then voxel edit v1 (§3.6).
6. Journal (§3.7), schema freeze (§3.8), remote auth (§3.9).
7. Run the exit scenarios (§3.10) and the human/hardware gates.
