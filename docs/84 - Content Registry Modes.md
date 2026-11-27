---
title: Content Registry Modes
project: Townsmith
status: proposed
updated: 2026-11-27
tags:
  - townsmith
  - modding
  - multiplayer
  - server
  - content
---

# Content Registry Modes

> [!abstract] Proposal
> This document proposes how the authoritative server describes the content it will accept: base only, mods, resource packs, or a mix. It separates the active-ID list from an optional registry manifest with kinds and checksums, maps `.tsbp` dependencies onto that registry, and keeps every failure closed. It is a design for review, not an implemented feature.

Related: [[06 - Modding and Resource Packs]], [[28 - Blueprint Package]], [[42 - World Placement Bridge]], [[10 - Decisions]] D-006/D-027/D-034/D-035/D-036/D-037/D-042, [[09 - Open Questions]] Q-116/Q-117/Q-118/Q-127/Q-128.

> [!note] Parser landed (proposed)
> The pure manifest parser and validator now live in `crates/core_city/src/content_registry.rs`; tests run with `cargo test -p core_city`. The public API is:
>
> - `parse_content_registry_manifest(&str) -> Result<ContentRegistryManifest, ContentRegistryError>` (also `ContentRegistryManifest::parse` / `::validate`).
> - `ContentRegistryManifest::implicit_ids()`, `::select(&[&str])`, and `::provides(&str)`.
> - `ContentRegistryManifest { format_version, ids }` and `ContentRegistryEntry { id, kind, namespace, checksum }` with `ContentRegistryKind::{Mod, ResourcePack}`.
> - `ContentRegistryError::code()` returning the stable `content.invalid_registry`, `content.unsupported_registry_version`, `content.unknown_id`, and `content.kind_mismatch` codes.
>
> The parser rejects unknown fields, `format_version` other than 1, explicit `base`, invalid IDs/namespaces, missing or non-canonical `blake3:<64 lowercase hex>` checksums, duplicate IDs/namespaces, and cross-kind claims. It enforces the §8 caps (256 entries, 1 MiB, 64-byte IDs).
>
> **Opt-in implementation landed (proposed).** The core parser and the server `--content-registry PATH` startup path exist; the default remains base-only and no save, protocol, or world checksum changes. `--check-content-registry` validates the same manifest/selection without starting a server. D-042 remains **Proposed**; Q-127 and Q-128 remain **Open**.

## 1. Why now

The server already has a process-local active-content list:

- `--content-ids base[,mymod[,mymod:pack]]` sets `ServerWorld.active_content_ids`.
- The default is `["base"]`; IDs are trimmed, validated, sorted, and deduplicated.
- Placement loads each `.tsbp` with the active IDs and fails closed with `blueprint.missing_dependency`.
- The optional `--content-hash` pin checks the client's hello hash, not the package dependencies.

What is missing is the meaning of an ID. A mod and a resource pack can share a namespace, a pack can change its checksum without changing its ID, and the server cannot currently say "this ID is a resource pack, not a mod" or "this registry is the reviewed set." The bridge and server also need one rule for `MissingMod` versus `MissingResourcePack` so a package does not become placeable because a required pack was listed as a mod.

## 2. Scope

### In scope for v1

- Define the registry as a set of entries: `{ id, kind, namespace, checksum }`, where `kind` is `mod` or `resource_pack` and `base` is implicit.
- Keep `--content-ids` as the short form for base plus a list of IDs.
- Add an optional `--content-registry PATH` manifest for explicit kinds/checksums.
- Validate the manifest at startup, before the listener binds.
- Match `.tsbp` `mods` against `kind=mod` and `resource_packs` against `kind=resource_pack`; exact ID or namespace matching follows the existing rule.
- Fail closed on unknown IDs, kind mismatches, duplicate/ambiguous IDs, malformed manifests, and checksum mismatches.
- Keep the registry process configuration, not save data or protocol state.

### Out of scope

- Upload/fetch, Workshop publishing, or auto-download.
- Multiple simultaneously active *content packs* in the D-034 sense; one pack still owns the merged registry.
- Client-side resource-pack rendering; the server only decides what content a package may require.
- Registry hot reload, remote registry URLs, signatures, or a trust store.
- Changing the outer save format or the D-034 `content` identity object.

## 3. Registry model

```text
ContentRegistry {
  format_version: 1,
  ids: [
    { id: "mymod",       kind: "mod",           namespace: "mymod",       checksum: "blake3:..." },
    { id: "mymod:pack",  kind: "resource_pack", namespace: "mymod:pack",  checksum: "blake3:..." }
  ]
}
```

Rules:

- `base` is always present and never listed; listing it is `content.kind_mismatch` or a duplicate error.
- `id` is the canonical registry key, lowercase, namespaced where the content format requires it. `namespace` is the matching alias; it may equal `id`.
- IDs and namespaces are unique across the whole registry, including across kinds. A mod and a pack may not claim the same ID or namespace; that is `content.kind_mismatch` or `content.duplicate_id`.
- `checksum` is a `blake3:<64 lowercase hex>` content hash. It is required in the manifest form and absent in `--content-ids` short form.
- The order in the manifest is not significant; the server sorts by `(kind, id)` and deduplicates before use.
- The active registry checksum is a deterministic hash over the normalized entries, not over the file bytes.

## 4. CLI and startup

### Short form

```text
townsmith-server --content-ids base,mymod,mymod:pack
```

- `base` is optional and implied.
- Each token is treated as an ID with unknown kind; matching is by ID/namespace against the package dependency kind.
- This remains backwards compatible with #386.
- Resource packs are allowed as IDs, but the server cannot verify their kind or checksum; that is the documented short-form limitation.

### Manifest form

```text
townsmith-server --content-registry /abs/content-registry.json
```

- `--content-registry` may be used alone or with `--content-ids`:
  - alone selects every non-base manifest ID plus implicit `base`;
  - with `--content-ids`, the short form is a requested subset of the manifest catalog (`manifest.select(requested)`), including namespace aliases and explicit `base`.
- The file is read before the listener binds, bounded to 1 MiB; a missing/unreadable file is `content.registry_io`.
- `format_version` must be `1`; any other value is `content.unsupported_registry_version`.
- An empty `ids` array is valid and means base-only.
- The selected IDs replace `ServerWorld.active_content_ids`; placement then resolves `.tsbp` dependencies against them.
- Startup prints one stable line:

```text
SERVER_CONTENT_REGISTRY status=ok path=<path> ids=<n> selected=<n> checksum=<16hex>
```

or

```text
SERVER_CONTENT_REGISTRY status=error code=<stable> error=<message>
```

and exits 2 before binding. A successful registry application is followed by the existing `SERVER_CONTENT status=ok ids=...` line. The checksum is FNV-1a over sorted selected IDs plus each manifest entry's kind, namespace, and checksum; `base` is implicit, and file bytes/path are excluded.

### Check mode (non-binding)

```text
townsmith-server --check-content-registry --content-registry /abs/content-registry.json [--content-ids IDS]
```

- Validates exactly the `--content-registry` / `--content-ids` combination the listen path would apply, then exits **0**.
- Prints the same two lines as startup, in the same order:

```text
SERVER_CONTENT_REGISTRY status=ok path=<path> ids=<n> selected=<n> checksum=<16hex>
SERVER_CONTENT status=ok ids=<comma-separated>
```

- On failure it prints `SERVER_CONTENT_REGISTRY status=error code=<stable> error=<message>` to stderr and exits **2**. A missing `--content-registry` is `content.registry_required`.
- It does **not** activate content, bind a socket, load a save, create a world, or write a save. `--listen`, `--once`, and `--save` are ignored, so an operator or CI job can validate one argument set and then reuse it to start the server.
- It is a dry run, so it has no effect on placement or protocol state; the manifest is still read once and bounded to 1 MiB.

### Content hash pin

`--content-hash` remains an independent client hello pin. It is not derived automatically from the registry, because the pin is a deployment decision and may cover client-only assets. A future convenience flag may derive it, but it must be explicit.

## 5. Dependency mapping

A `.tsbp` package may declare:

```text
dependencies {
  mods: ["mymod", "other:addon"],
  resource_packs: ["mymod:pack"]
}
```

Mapping:

| Requirement | Registry entry required | Missing code |
|---|---|---|
| `mods: ["mymod"]` | `kind=mod`, `id=mymod` or `namespace=mymod` | `MissingMod` |
| `mods: ["mymod:addon"]` | `kind=mod`, exact ID or namespace | `MissingMod` |
| `resource_packs: ["mymod:pack"]` | `kind=resource_pack`, exact ID or namespace | `MissingResourcePack` |

- An entry with the right ID but the wrong kind does **not** satisfy the requirement. It produces the requirement's missing code plus `content.kind_mismatch` detail.
- Base content is implicit; a package with no `dependencies` is valid in every mode.
- The existing bridge and server `blueprint.missing_dependency` payload stays; it gains `missing_kinds` and `registry_checksum` in the manifest mode.
- Placement rejects before metadata/idempotency/policy/age/terrain/overlap/charge/mutation, as it does today.

## 6. Errors and status

| Code | When | Exit |
|---|---|---|
| `content.invalid_registry` | Malformed JSON/schema, empty ID, invalid checksum, duplicate ID/namespace, unknown kind | 2 |
| `content.registry_io` | Manifest path missing/unreadable | 2 |
| `content.registry_required` | `--check-content-registry` was given without `--content-registry` | 2 |
| `content.unsupported_registry_version` | `format_version` is not 1 | 2 |
| `content.unknown_id` | `--content-ids` names an ID not in the manifest (manifest mode only) | 2 |
| `content.kind_mismatch` | Same ID/namespace used by both mod and pack, or a dependency resolves to the wrong kind | 2 at startup / placed rejection at runtime |
| `content.pin_mismatch` | `--content-hash` does not match the deployment pin or the registry-derived pin, when that check is enabled | 2 |
| `blueprint.missing_dependency` | A package requires a missing/wrong-kind ID at placement | placement rejection |

All codes are stable API names; messages are human-readable and may include the ID.

## 7. Compatibility

- D-034 content-bound saves keep their `content { pack_id, pack_checksum, registry_checksum }` object. The server registry is not written into the save and does not change outer v1/v2.
- D-035 read-only missing-pack inspection is unchanged; it never invents registry entries.
- D-036 prior-checksum compatibility and D-037 ID migrations apply before this registry check: a migrated ID must resolve in the active registry, and an unmapped ID still fails.
- Base-only servers and packages with no dependencies behave exactly as today.
- The short form alone keeps #386 semantics and output. The manifest form is additive and opt-in; both flags together treat the short form as a subset selector.
- A failed manifest load leaves the world untouched and exits before binding.
- The registry checksum is process-local; it is not part of `ServerWorld::checksum` or protocol goldens.

## 8. Security and limits

- The server never trusts a client-supplied registry, path, or checksum.
- The manifest is read once at startup; no hot reload in v1.
- Cap the manifest at 256 entries and 1 MiB; reject larger files.
- Reject control characters, path separators, absolute paths, `..`, and empty/oversized IDs (1–64 bytes, lowercase ASCII plus `:`, `_`, `-`).
- Duplicate detection is case-sensitive after lowercase normalization; no Unicode case folding.
- Checksum comparison is exact and constant-time where it is a secret-adjacent pin; IDs are not secrets.
- Logs include IDs and checksums, never file paths from packages.
- No auto-download, no fallback substitution, no "best effort" pack loading.

## 9. Non-goals

- A mod loader, script sandbox, or native-code policy.
- Multiple D-034 content packs active at once.
- Registry signatures or a public key infrastructure.
- Client UI for installing or repairing packs.
- Upload/fetch and quota policy (Q-107).
- Mod removal policy (Q-108) and per-player quotas (Q-109).

## 10. Test plan

Unit:

- Parse valid base-only, one-mod, one-pack, and mixed manifests; round-trip normalized checksum.
- Reject duplicate IDs/namespaces, base as an entry, invalid kinds, bad checksums, oversized files/IDs, control characters, and unsupported versions.
- Manifest alone selects every ID; both flags select a subset; unknown ID and kind mismatch in manifest mode; explicit `base`.

Integration:

- Server startup prints `SERVER_CONTENT_REGISTRY` then `SERVER_CONTENT`, and exits 2 on every error; `--content-ids` alone still prints only `SERVER_CONTENT`.
- `--check-content-registry` prints the same two lines and exits 0/2 without binding, loading, or creating a world; it ignores `--listen`, `--once`, and `--save` (covered by `crates/server/tests/content_registry_cli.rs`, including an occupied-port no-bind check).
- `crates/server/tests/content_registry_startup.rs` pins the real-binary startup matrix: missing/directory/unreadable/symlinked/space/relative manifest paths, the 256-entry and 1 MiB caps, duplicate/kind/version/empty/base cases, `--content-ids` subset and blank-token normalization, an occupied port that fails closed without binding or saving, and a bounded clean-startup line-order check. Permission and symlink cases are Unix-only and skip when running as root.
- Malformed, missing, oversized, and unsupported-version manifests fail before binding and leave the world base-only.
- A package requiring a mod places when the mod is active; rejects with `MissingMod` when absent; rejects with `MissingResourcePack` when the ID exists only as a mod.
- Namespace matching (`mymod` vs `mymod:pack`) works for both kinds.
- Placement rejection leaves placement/construction/treasury checksums unchanged.
- Save/load and content-bound saves are unaffected.

Deterministic:

- Fixed manifest and package fixture; two runs produce the same registry checksum and the same rejection payload.
- Loopback hello with `--content-hash` still matches or rejects exactly as before.

> [!note] Malformed-manifest corpus
> `crates/core_city/src/content_registry_faults.rs` is a test-only module (registered in `lib.rs`) holding a deterministic corpus of 41 tests over 180+ input documents. It covers empty/whitespace/truncated/trailing JSON, duplicate object keys, unknown fields at depth, wrong top-level types and nulls, BOM/lone-surrogate/raw-control strings, every `format_version`/ID/namespace/kind/checksum fault, duplicate and cross-kind claims, the 256-entry and 1 MiB boundaries, and `select`/`provides` ordering and ambiguity. Every rejection asserts the stable code and fails on a panic; valid cases pin base-first sorted IDs and reproducible checksum/serialization output. Run with `cargo test -p core_city content_registry_faults`.

## 11. Open questions

| ID | Question | Recommendation |
|---|---|---|
| Q-127 | Can more than one content pack or resource pack be active at once, and how do their namespaces merge? | v1 keeps one D-034 pack plus an explicit registry of mods/packs; revisit if overlapping namespaces become common. |
| Q-128 | How is the registry distributed and trusted across server/client, and who signs it? | Start with an operator-supplied local file and exact checksums; add signatures only when a public server ecosystem exists. |

## 12. Review trigger

Accept before the first non-loopback server with a non-base registry, before a resource pack is allowed to satisfy a package dependency, or before the registry is distributed outside the operator's machine. Revisit when D-036/D-037 are accepted, when multiple active packs are proposed, or when a registry hot-reload/remote URL is requested.
