---
title: Phase 6 Remaining Work
project: Townsmith
status: proposed
updated: 2026-11-28
tags:
  - townsmith
  - multiplayer
  - roadmap
  - planning
---

# Phase 6 Remaining Work

> [!abstract] Purpose
> This is the dependency-ordered remainder of **Phase 6 — Multiplayer**. It replaces the stale Phase 6 checklist in [[08 - Roadmap]] with the actual code, gaps, acceptance criteria, and cut lines at master `9a53309` (2026-11-28). It is a plan, not an implementation claim.

Related: [[05 - Multiplayer]], [[08 - Roadmap]], [[10 - Decisions]] D-004/D-021/D-032/D-033/D-035/D-038/D-039/D-040/D-041/D-042, [[09 - Open Questions]] Q-101/Q-117/Q-119/Q-120/Q-121/Q-122/Q-123/Q-124/Q-125/Q-126/Q-127/Q-128, [[12 - Technical Architecture]], [[42 - World Placement Bridge]], [[48 - Settlement Boundaries]], [[79 - Voxel Edit Sync]], [[81 - Settlement Tax Composition]], [[82 - Journal and Rollback]], [[83 - Remote Transport and Authentication]], [[84 - Content Registry Modes]].

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
| ✅ Done | Voxel-edit and journal core models, bridge mutation faults, chat load smoke, full connected settlement mutations, shutdown readiness | #468–#474 |
| ✅ Done | Bridge save composition, degraded-network listener cases, status-panel watch, layered rejection smoke | #476–#479 |
| ✅ Done | Manifest-reader fault matrix, shutdown-under-load, chat-panel focus, connected settlement screen (read/paging/rename/tax), UI localization audit | #462–#466 |
| ✅ Done | Player roster/paging, roster benchmark, connected settlement probe, server-status failure matrix, retained-lock reconnect proof | #456–#460 |
| ✅ Done | Player roster, shutdown save, status panel, content benchmark, production settlement shell, local settlement mutations | #445–#454 |
| ✅ Done | Settlement paging/benchmark, bridge paging, editor paging, save composition matrix, connected transfer, member cap, chat badge, server status, chat rejection corpus, production UI plan | #428–#443 |
| ✅ Done | D-042 content-registry proposal, dashboard release, player-scoped lock release, client chat-push listener, concurrent-listener stress matrix | #391–#395 |
| ✅ Done | Wave reconciliation, Windows stress bound, chat unread HUD, audit-log filters, D-042 manifest parser, dashboard rename, extended listener stress | #396–#402 |
| ✅ Done | Bounded outbound queue, lock-lease badge, opt-in lock retention, local bridge player registry, D-041 remote-auth proposal | #373–#377 |
| ✅ Done | Server content-dependent placement, `--retain-locks`, settlement-editor player picker, dashboard claim, wired outbound writer | #379–#383 |
| ✅ Done | Audited Admin force-unlock, `--content-ids`, local release, dashboard transfer, server chat push over concurrent listen | #385–#389 |
| ✅ Done | Opt-in `--content-registry` startup loading, bridge manifest reader, chat panel badge, dashboard invite/role/remove, lock loopback coverage | #404–#408 |
| ✅ Done | Non-binding content-registry check, `chat_seq` watermark, resume catch-up smoke, deterministic catch-up merge, read-only D-039 tax preview | #410–#414 |
| ✅ Done | Dashboard member names, save crash/restart matrix, malformed-frame corpus, blueprint corruption corpus, full placement lock list | #416–#420 |
| ✅ Done | Optional-store save matrix, content-manifest corpus, connected settlement readout, content-identity corpus, district member names, settlement rejection corpus | #422–#427 |
| ✅ Done | Settlement list paging, list benchmark, connected-settlement reference, connected-client mutation actions | #428–#431 |
| ✅ Done | Bridge settlement paging, paging fault corpus, editor paging, bridge save/load composition matrix, connected-client transfer and paged lists | #433–#437 |
| ✅ Done | Dashboard member cap, chat panel badge, `server_status`, production-UI plan, chat rejection corpus | #439–#443 |
| ✅ Done | Server-status panel, paged player roster, shutdown save, content-registry benchmark, read-only production settlement shell | #445–#449 |
| ✅ Done | Roster save matrix, status-watch faults, newer-command protocol corpus, transport resilience, production screen focus | #481–#485 |
| ✅ Done | Pause restart semantics, status/district/editor a11y, settlement confirmation modal, start-menu volume settings | #521–#526 |
| ✅ Done | 3D HUD and start-menu screen-reader semantics | #528–#529 |
| ✅ Done | Pause in snapshot/status and clients, start menu, chat a11y, segmented transport, roster picker, paused wire pins, time-vote/settlement a11y, step-count latency proxy | #506–#519 |
| ✅ Done | Production settlement shell, local/connected mutations, shared-core embedding, gamepad navigation | #449/#454/#465/#472/#488/#497 |
| ✅ Done | Four-player checksum/save exit script (CI), concurrent status-watch load, lock-lease override | #493/#494/#496 |
| ⛔ Blocked | Q-101 human playtest; 1M/RTX 3060 render gate | person / reference hardware |

## 2. Dependency order

| # | Slice | Depends on | Why this order |
|---:|---|---|---|
| 1 | ~~Production settlement UI~~ **done** | #343, #449/#454/#465/#472/#488/#497/#503 | The production shell, connected mutations, shared-core embedding, gamepad navigation, and modality footer are complete |
| 2 | Settlement tax treasury consumer after D-039 acceptance | D-030/D-031 | The pure helper exists, but the rate is data-only; applying it needs a precedence decision against district weights |
| 3 | Blueprint lock reconnect-retention UX | #349 v0 locks, #363/#374 UI, #369 leases, #375 retention, #380 CLI, #393 player scope | Lease expiry, retention, CLI, and concurrent-session release are done; decide automatic renewal and client lock list |
| 4 | Concurrent listener remote-load tuning | #269, #350, #373, #383, #389, #395, #402 | Slow readers, closed queues, shutdown, session cap, fan-out, sustained load, status watchers, and step-count latency injection are covered; remote WAN jitter/loss benchmarking remains |
| 5 | Chat reconnect/catch-up presentation | #389, #394, #395, #398, #406 | Server push, passive listener, HUD unread, and panel badge are done; remaining is reconnect catch-up presentation |
| 6 | Voxel edit sync v1 | D-038 accepted, Q-119/Q-120, 3, 4 | The largest slice; do not start before the conflict/lease decisions |
| 7 | Journal/rollback v1 after D-040/Q-123/Q-124 | 6 | Needs the edit command stream and a crash-safe boundary |
| 8 | Save migration/versioning freeze | 1–7 | Freeze settlement/edit/journal/lock schemas before remote exposure |
| 9 | Remote transport + authentication | 4, 8 | Resume tokens are not auth; F-039 documents the gap |
| 10 | Phase 6 exit scenarios | 1–9 | The 4-player proof, not a feature |

## 3. Slice details

### 3.1 Settlement write controls

**Scope.** Dashboard and a connected client should found, claim, rename, transfer, invite, set role, remove, release (Admin), and set tax through the bridge/server commands that already exist. The dashboard has the `Shift+T` tax cycle (#356), `Shift+C` unowned claim (#382), `Shift+Y` transfer (#388), `Shift+X` release (#392), `Shift+R` rename (#401), and `Shift+I`/`Shift+M`/`Shift+Delete` invite/role/remove (#407); #370 adds full owner/member/permission rows to the read-only district panel; #381 adds local player selection and #387 release to the settlement editor; #424 adds the connected-client readout, #431 adds connected-client rename/set-tax/release/invite/set-role/remove/claim, and #428 adds server-side list paging. #437 adds connected-client transfer and paged lists, and #446 adds the paged player roster; #449 adds the first read-only production settlement shell, #454 its local mutations, #465/#472 its connected read/paging and full mutation set, #488 embeds it with a shared core, and #497 adds gamepad navigation plus virtual-keyboard intent. No remaining work in this slice.

**Protocol/save.** No new command or save field. Reuse `settlement_*` v1 shapes and `settlement_updated`.

**Permissions.** Global role is a ceiling; settlement `members`/`manage`/`tax`/`build` checks stay in the core/store. Admin-only release. Every rejection is code+params, never a pre-localized string.

**Tests.** Bridge unit tests for each mutation and revision conflict; Godot smoke for key routing, owned/unowned states, rejected calls leaving checksums unchanged; server loopback for a two-player invite/role/transfer sequence.

**Not bundled.** Per-district permissions, lot claims, settlement merge/split, membership UI beyond the selected record.

### 3.2 Settlement tax treasury consumer

**Scope.** Give `Settlement.tax_rate` a defined economic effect. The open decision is composition with D-030 home/workplace weights and D-031 service funding: settlement surcharge first, district multiplier first, or cap the combined rate. D-039 records the proposed option A; #366 adds the pure helper and #414 a read-only dashboard preview. Record acceptance before wiring a treasury consumer.

**Protocol/save.** Prefer no protocol change; the rate already round-trips. If revenue is reported, extend the snapshot/audit read model, not the save.

**Permissions.** `tax` for setting; collection is server-side. No client-supplied base.

**Tests.** Unit: zero rate, 100%, rounding, population outside a settlement, overlapping settlements (smallest id or explicit rule), cap. Integration: city loop with a settlement surcharge; before/after treasury JSON with a fixed seed.

**Benchmark.** `city_loop` Early/Mid/Late with and without the surcharge; confirm no tick-budget regression.

**Not bundled.** Per-building tax, land value, bankruptcy changes, district budget UI.

### 3.3 Blueprint locks: leases, retention, reconnect UX

**Scope.** #349 ships advisory in-memory locks and player-wide release on disconnect; #363 adds the placement-UI lock badge and `L` toggle; #369 adds a lazy 300-tick lease (1–3600 clamp) and refreshes on same-holder lock; #374 shows the lease; #375 adds opt-in retention across disconnect; #380 adds the `--retain-locks` CLI and `SERVER_LOCKS retention=on|off` marker; #393 scopes release to the stable player id across concurrent sessions. The lease override is done: `--lock-lease-ticks N` (1–3600, default 300) is applied after any `--load` and echoes `SERVER_LOCK_LEASE ticks=N` only when supplied. Remaining: reconnect-retention UX (automatic renewal) and a lock list in the connected client; #385 makes Admin force-unlock explicit and audits it.

**Protocol/save.** Additive fields on the existing lock responses; if locks become durable, a new optional `ServerSave` store and `save.invalid_blueprint_locks`. Do not make locks authoritative for package identity.

**Permissions.** Mayor lock/unlock; holder or Admin force; placement already checks the holder.

**Tests.** Deterministic clock/lease expiry, two players contending, reconnect before/after expiry, restart clearing, transport rejection, no checksum movement for expired-only state.

**Not bundled.** Collaborative editing, lock transfer, multi-package locks, package upload.

### 3.4 Concurrent listeners and outbound queue

**Scope.** #373 adds a bounded `OutboundQueue`; #383 wires a per-connection writer thread; #389 runs every accepted socket on its own session thread against the shared world and closes only a backpressured recipient; #395/#402 add the stress matrix (fan-out, sustained load, shutdown with queued writes, ordering); #493 adds four concurrent status watchers under chat traffic with an unchanged checksum. #500 adds a stalled/non-reading watcher that queues frames below the cap while healthy clients complete, and #509 adds a deterministic byte-fragmented proxy with direct/proxied peers, resume, and truncated-frame coverage. A step-count proxy now withholds whole frames without sleeps (this change); remote WAN jitter/loss benchmarking and real network latency remain operator/reference-hardware work.

**Protocol/save.** No wire-shape change. Add backpressure/closure codes and a bounded queue metric.

**Tests.** Network-fault matrix from #317 plus the #395 slow-reader, closed-queue, concurrent-broadcast, shutdown-under-load, and session-cap cases; deterministic tick/checksum.

**Benchmark.** 4 clients, 2k active agents, 10 Hz snapshots; p99 tick and queue depth.

**Not bundled.** Remote TLS/auth, matchmaking, public server browser.

### 3.5 Chat push/broadcast

**Scope.** Push `chat_sent`/`chat_log` deltas to connected clients instead of polling only; keep `chat_since` for reconnect. Bounded log and 64-line cap stay. Server push (#389), the 3D passive listener (#394), the unread HUD (#398), the panel badge (#406), the `chat_seq` watermark (#411), resume catch-up (#412), and deterministic catch-up merging (#413) are done. Multi-session ordering is covered by #395/#402.

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

**Implemented (wired schemas).** #502 adds `crates/server/tests/save_version_matrix.rs` (v1 byte identity, outer v2/v3 fail-closed, per-store version rejection, repeated load/save stability, unknown `content` stripping) and fixes `load_from` so a future-version primary never downgrades to `.bak`. Placement-edit and journal schemas are not wired yet, so their migration fixtures wait on D-038/D-040.

**Not bundled.** Auto-downgrade, cross-pack conversion, data repair heuristics.

### 3.9 Remote transport and authentication

**Scope.** Move beyond loopback: TLS (or an operator-terminated tunnel), authenticated join, per-player identity that survives reconnect/restart, and a documented trusted-LAN/operator-token mode. F-039 applies: the resume token is derivable and not auth. D-041/[[83 - Remote Transport and Authentication]] proposes the contract; Q-125/Q-126 are open.

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

> [!note] Automated so far (2026-11-27)
> `crates/server/tests/four_player_exit.rs` runs four real v1 TCP clients against the production `run_listen` on an ephemeral loopback port and scripts one city: distinct-player hello, settlement found/invite/set-role/set-tax, two-player advisory lock contention with an unchanged tick/checksum on rejection, one authoritative fixture placement plus an idempotent duplicate, a four-recipient chat broadcast, a 3–1 pause vote, one disconnect/resume, and a read-only tail. It pins tick `3` / checksum `cdbe19ba10ed8ec5`, asserts the save reloads to the same checksum with exactly one placement and no `.tmp`, and closes every session cleanly. The companion `crates/server/tests/four_player_admin.rs` starts a second `run_listen` world as **Admin** with two real clients: pause → edit while paused → resume, settlement found → force-release, foreign lock → audited force-unlock, valid and malformed `audit_log` filters, and a save reload that preserves `paused == false` and the final checksum. Pause is wire-visible in `snapshot.paused` and `server_status.paused`, and the test samples both while paused and after resume. Session roles still remain process-wide under `run_listen`, so mixed-role, protocol-level role-forbidden paths, per-player role assignment, the one-hour soak, bandwidth/tick timing, and "edit while another places" beyond lock contention still need humans or reference hardware.

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

1. Tax consumer after D-039 acceptance (§3.2); the connected confirmation modal is done.
2. Stale-revision badge/retry presentation in the connected settlement screen (§7.5 of [[85 - Connected Settlement Surface]]); reconfirmation already refuses a stale revision.
3. Concurrent listener remote-load tuning (§3.4); the status-watch, stalled-watcher, segmented-transport, and step-count latency-injection paths are covered. Real WAN jitter/loss remains operator/hardware work.
4. Resource-pack registry policy after D-042/Q-127/Q-128 (§3.5/§3.9); the parser, opt-in server manifest path, and bridge reader exist.
5. Accept or revise D-038; answer Q-119/Q-120; then voxel edit v1 (§3.6).
6. Journal after D-040/Q-123/Q-124 (§3.7), remote auth after D-041 (§3.9); the wired-schema version matrix is done in #502, while the edit/journal freeze waits on those schemas.
7. Extend the automated exit script (§3.10), then run the human/reference-hardware gates.
