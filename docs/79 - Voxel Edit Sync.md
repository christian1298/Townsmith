---
title: Voxel Edit Sync
project: Townsmith
status: proposed
updated: 2026-11-25
tags:
  - townsmith
  - multiplayer
  - voxel
  - blueprint
  - design
---

# Voxel Edit Sync

> [!abstract] Proposal
> This document proposes the Phase 6 authoritative voxel-edit path for **placed blueprints**. It is a design for review, not an implemented feature. It defines a bounded edit batch, optimistic revision checks, a placement-scoped lock, interest replication, save representation, and a deterministic test plan. Arbitrary terrain editing, destruction physics, rollback/history, and upload/fetch stay out of scope.

Related: [[05 - Multiplayer]] §6, [[10 - Decisions]] D-026/D-032/D-033/D-038, [[12 - Technical Architecture]], [[28 - Blueprint Package]], [[35 - Editor Undo Redo]], [[42 - World Placement Bridge]], [[48 - Settlement Boundaries]], [[78 - Code Audit Findings]].

## 1. Why now

The game already has the pieces around the edit:

- D-026 makes workshop/creative authoring first and permissioned live editing later.
- D-032 makes `place_blueprint` server-authoritative; a placed building is a stable record with a content-addressed package hash.
- D-033 supplies stable player ids, settlement membership, and the `build` permission.
- D-034/D-035 pin content identity and define a read-only mode when a pack is missing.
- D-036/D-037 (proposed) make pack compatibility and content-ID migrations explicit.
- The server already has a bounded interest viewport, a per-session rate bucket, an audit ring, and advisory package locks.

What is missing is the authority for **changing the voxels of an existing placed building** while another player is looking at it, saving it, or editing the same blueprint. D-032 explicitly lists live-city voxel editing as out of scope; this proposal is the follow-on.

## 2. Scope

### In scope for v1

- Edit the volume of one **placed building instance** selected by `placement_id`.
- Add, remove, and replace solid voxels at integer local coordinates.
- A bounded batch (at most **1,024 edits** per command) with explicit `expected_revision`.
- Server-side validation of identity, role, settlement `build` permission, lock ownership, revision, bounds, materials, duplicate positions, and batch size.
- Incremental replication through the interest view, with a bounded per-building edit log.
- Optional save persistence as per-placement deltas, not full volumes.
- Deterministic conflict behavior: reject a stale edit, never merge silently.
- Undo/redo owned by the editing player; history stays local and bounded.

### Out of scope

- Destruction physics, structural collapse, debris, or fire/water simulation (D-008).
- Arbitrary terrain sculpting; terrain edit deltas have their own path.
- Live permission changes beyond the existing D-033 bits (D-026).
- Package upload/fetch, Workshop publishing, or mod auto-download.
- Server rollback/history, branching, or a full version-control UI.
- Cross-blueprint copy/paste, multi-building selection, or mass edits.
- CRDTs, OT, or automatic merge. The design is deliberately last-writer-loses.
- Editing a package in the shared library (that is a separate workshop concern).

## 3. Authoritative model

Each accepted placement gets an editable instance record:

```text
PlacedBlueprintEdit {
  placement_id: u64,
  blueprint_hash: "blake3:<64 hex>",
  base_revision: u32,          // package/library revision at placement time
  edit_revision: u32,          // starts at 0; +1 per accepted batch
  edits: Vec<VoxelEdit>,       // compact deltas in application order
  checksum: u64,               // FNV-1a over id/revision/hash/deltas
}

VoxelEdit {
  pos: [u16; 3],               // local voxel coordinate inside the package volume
  material: u16,               // 0 = empty/remove; otherwise a registry material
}
```

The server owns the authoritative volume. In v1 it may materialize the package volume lazily on the first edit and keep only the delta log plus a small cache; a full volume is never sent to clients. The placement record's `blueprint_hash` stays the immutable identity of the source package, so a save can always explain which base the deltas apply to.

The edit log is append-only for the life of the placement. Compaction (rewriting repeated edits to the same cell) is a storage optimization, not a semantic one; if added it must produce the same effective volume and checksum.

## 4. Wire contract (proposed, additive protocol v1)

### Commands

```text
edit_placement_voxels {
  placement_id: u64,
  expected_revision: u32,
  edits: [
    { pos: [u16, u16, u16], material: u16 }
  ]
}

placement_edits {
  placement_id: u64,
  after_revision: u32
}
```

### Responses

```text
placement_edits_applied {
  tick: u64,
  placement_id: u64,
  revision: u32,
  edit_count: u32,
  placement_checksum: String,   // 16 hex
  world_checksum: String        // 16 hex
}

placement_edits {
  tick: u64,
  placement_id: u64,
  revision: u32,
  checksum: String,
  edits: [ { revision, pos, material } ],
  truncated: bool
}
```

`placement_edits` returns at most **4,096** entries after `after_revision`; `truncated` tells the client to request again. The interest view adds a bounded `blueprint_revisions` list for visible placements:

```text
blueprint_revisions: [
  { placement_id, edit_revision, checksum }
]
```

A client compares revisions with its local cache and fetches only the missing slice. No push/broadcast is required in v1; the existing interest refresh is the synchronization point. A later push channel can reuse the same revision/checksum.

### Validation order

1. hello/resume and stable player identity;
2. global role (Mayor or Admin) and D-033 `build` at the placement centre;
3. placement exists, is not demolished, and belongs to a known package;
4. lock ownership (see §5);
5. `expected_revision == edit_revision`;
6. batch size 1–1,024 and total payload within the frame cap;
7. every coordinate inside the package volume and every material known;
8. no duplicate positions in one batch;
9. rate bucket;
10. structure/critical-support check after the batch is applied in a scratch copy; reject if the result is invalid.

Only after all checks pass does the server append the batch, increment `edit_revision`, refresh checksums, record the audit entry, and return the result. A rejection mutates nothing.

## 5. Locks and concurrency

The current advisory `blueprint_lock` is keyed by **package hash** and is process-local. Package-level locking is right for library/workshop editing, but two placed instances of the same package must be editable independently. The proposal is:

- **v1 live edit lock:** `placement_id` + holder `player_id`, with `acquired_tick` and a short lease. The existing package lock stays for package authoring.
- A player must hold the placement lock to send `edit_placement_voxels`; Admin may force-release.
- Locks are in-memory and released on disconnect. A lock expires after a fixed number of ticks (proposed 300) without a refresh; `refresh` is implicit on an accepted edit or an explicit `lock` command.
- A stale `expected_revision` is rejected with `blueprint.revision_stale`; the client reloads and re-applies its local intent.
- No automatic merge. If two players edit different voxels, the second one still needs the current revision; the design prefers a visible reject over a silent CRDT merge.
- Undo is local to the editor's session. The server sees an undo as another edit batch, so it is subject to the same lock/revision rules. Undo history remains capped at 500 operations ([[35 - Editor Undo Redo]]).

Q-119 and Q-120 carry the remaining policy choices: conflict UX and lock lease/ownership across reconnect.

## 6. Replication and rendering

- The client keeps the last known `edit_revision` and checksum per visible placement.
- On interest refresh, a changed revision triggers `placement_edits { after_revision }`.
- The client applies deltas to its local voxel mesh and rebuilds only the affected chunk(s); a full volume reload is the fallback when `truncated` is true.
- The server never sends the full package volume in the interest view.
- A joining client starts from the package volume plus the retained edit log slice. If the log has been compacted or truncated, the server sends the effective volume for that placement (bounded by the package cap) instead of the history.
- The 3D client's existing BoxMesh placement render is a placeholder; this proposal does not require the final mesher, but the test harness can validate deltas numerically before rendering exists.

## 7. Persistence

`ServerSave` gets one optional additive field:

```text
placement_edits: [
  {
    placement_id,
    blueprint_hash,
    edit_revision,
    checksum,
    edits: [ { pos, material } ]
  }
]
```

- Absent or empty means "no edits" and keeps the existing save bytes/checksum unchanged.
- A save with edits writes the field once; older servers reject it as unsupported only if the format version changes, so v1 should keep the outer save version and use `#[serde(default)]` for compatibility, matching the D-032 placement pattern.
- Load validates that each `placement_id` exists, the package hash matches the placement, revisions are contiguous from zero, and every coordinate/material is valid. A mismatch rejects the save with a stable `save.invalid_placement_edits` code; it never applies a partial log.
- On load, the server rebuilds the effective volume from the package plus the retained log. A missing package is governed by D-034/D-035: strict load fails closed, read-only inspection retains the edits as opaque data.
- Compaction may rewrite the log, but the persisted checksum is over the effective delta sequence and must remain stable across a save/load roundtrip.

## 8. Failure codes

| Code | Meaning |
|---|---|
| `blueprint.edit_unknown` | No editable placement with that id. |
| `blueprint.edit_forbidden` | Global role or settlement `build` denies the edit. |
| `blueprint.edit_locked` | Another player holds the placement edit lock. |
| `blueprint.edit_lock_required` | The caller does not hold the lock. |
| `blueprint.revision_stale` | `expected_revision` is not current. |
| `blueprint.edit_batch_empty` | No edits supplied. |
| `blueprint.edit_batch_too_large` | More than 1,024 edits or over the payload cap. |
| `blueprint.edit_out_of_bounds` | A coordinate is outside the package volume. |
| `blueprint.edit_material_unknown` | Material id is not in the active registry. |
| `blueprint.edit_duplicate_position` | The batch edits the same cell twice. |
| `blueprint.edit_structure_invalid` | The resulting structure fails the structure check. |
| `blueprint.edit_unavailable` | Transport-only session or no authoritative executor. |
| `save.invalid_placement_edits` | Persisted edit state failed validation. |

All codes are stable API; messages are human-readable and not parsed.

## 9. Security and limits

- The server trusts no client dimensions, materials, or revision. Every coordinate and material is validated against the package and the live registry.
- Batch cap 1,024 edits; frame cap remains 1 MiB. A 1,000-edit batch is roughly 8–16 KB, matching the [[05 - Multiplayer]] estimate.
- Per-session rate limiting reuses the existing token bucket; a lock prevents one player from monopolizing a building.
- Audit entries record the command, actor, placement id, revision, and code; no player names in the wire audit.
- A structure check runs on a scratch copy; a batch that would make the building unsupported is rejected rather than partially applied.
- A read-only content mode (D-035) rejects edits with `content_read_only`; it cannot mutate a world whose pack is missing.
- No client-supplied package hash is accepted for the base volume; the placement record is authoritative.

## 10. Test plan

### Unit

- add/remove/replace one voxel; material zero means empty;
- coordinate bounds, duplicate positions, batch size, unknown material;
- revision increments once per accepted batch and never on a rejection;
- checksum stability over edit order and save/load;
- structure scratch-copy rejection leaves the original unchanged.

### Integration

- two connections: lock contention, holder edits, other player rejected, Admin force-release;
- stale revision after another edit, then reload-and-retry;
- disconnect releases the lock; reconnect does not silently restore it;
- settlement `build` denial and wilderness success;
- interest view revision bump, delta fetch, truncation, and full-volume fallback;
- transport-only session rejects with `blueprint.edit_unavailable`.

### Save/load

- empty edits omit the field and keep the existing checksum;
- roundtrip preserves revision, checksum, and effective volume;
- missing placement/package/wrong hash/contiguous-revision violation rejects with `save.invalid_placement_edits`;
- D-035 read-only load retains edits without applying them.

### Performance

- 1,000 edits in one batch under 1 ms of validation on the benchmark host;
- 2,000 active agents unaffected (no per-agent edit work);
- interest view stays within its 64-road/64-building caps with revision entries;
- save/load of 10k edited placements stays within the existing save budget or gets a documented exception.

## 11. Rollout

1. Land this ADR as **Proposed**; answer Q-119/Q-120.
2. Implement the pure core edit log and validation in `core_city`/`core_blueprint` with no protocol.
3. Add the placement lock and `edit_placement_voxels` command behind an opt-in server flag.
4. Add save persistence and a deterministic smoke.
5. Add interest revision/delta replication and a client cache.
6. Only then wire the editor UI and undo.

The feature is incomplete until it survives save/load, a second player, and a reconnect, per the Phase 6 definition of done.

## 12. Open questions

- **Q-119:** What is the conflict policy when two players edit the same voxel? The proposal rejects stale revisions; alternatives are a per-cell last-writer-wins rule or a merge UI.
- **Q-120:** How long should a placement edit lock live, and does it survive reconnect? The proposal uses a 300-tick lease and release on disconnect; alternatives are explicit unlock only or a resume-time reclaim.
- Compaction threshold and whether the persisted log is capped by bytes or entries.
- Whether structural invalidity should reject the batch or allow a "damaged" state.
- Whether the edit log belongs in `ServerSave` or a sidecar per-placement file.
