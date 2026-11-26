//! Pure bridge save/load composition matrix.
//!
//! `world_save*` and `world_load*` are `#[func]` methods and need a Godot
//! runtime, so this module runs the same pure helpers in the same order and
//! asserts the composition contract without constructing a Godot object:
//!
//! ```text
//! save: stamp refs (library only) -> placement JSON -> attach layers ->
//!       stamp content identity -> sync library -> durable write
//! load: read JSON -> verify content identity -> normalize v2 to v1 ->
//!       parse store/layers -> resolve packages -> commit
//! ```
//!
//! [`SimulatedBridge`] mirrors the bridge's commit order and carries the
//! read-only guard so the `content_read_only` refusal and the
//! `world_clear_placements` reset are covered. Error assertions pin stable
//! codes, not message text. Package paths stay legacy save values (D-027);
//! the matrix only requires identities and references to be path-free.

use super::*;
use crate::content_identity::{ContentIdentityStatus, CONTENT_IDENTITY_FORMAT_VERSION};
use std::sync::atomic::{AtomicU64, Ordering};

const SEED: u64 = 2026;
const PACK_ID: &str = "example:copper_age";
const PACK_CHECKSUM: u64 = 0xad4c_6458_69a4_11ed;
const REGISTRY_CHECKSUM: u64 = 0x330a_538d_0780_5b50;
const BASE_CHECKSUM: u64 = 0xc2df_0f24_2f5e_78d2;

static NEXT: AtomicU64 = AtomicU64::new(1);

fn base_active() -> ActiveContentIdentity {
    ActiveContentIdentity::base(BASE_CHECKSUM, BASE_CHECKSUM)
}

fn pack_active() -> ActiveContentIdentity {
    ActiveContentIdentity::pack(PACK_ID, PACK_CHECKSUM, REGISTRY_CHECKSUM, BASE_CHECKSUM)
}

fn other_pack_active() -> ActiveContentIdentity {
    ActiveContentIdentity::pack(
        "example:iron_age",
        PACK_CHECKSUM.wrapping_add(1),
        REGISTRY_CHECKSUM.wrapping_add(1),
        BASE_CHECKSUM,
    )
}

fn pack_checksum_mismatch() -> ActiveContentIdentity {
    ActiveContentIdentity::pack(
        PACK_ID,
        PACK_CHECKSUM.wrapping_add(1),
        REGISTRY_CHECKSUM,
        BASE_CHECKSUM,
    )
}

fn registry_checksum_mismatch() -> ActiveContentIdentity {
    ActiveContentIdentity::pack(
        PACK_ID,
        PACK_CHECKSUM,
        REGISTRY_CHECKSUM.wrapping_add(1),
        BASE_CHECKSUM,
    )
}

fn recorded_identity() -> WorldContentIdentity {
    WorldContentIdentity {
        format_version: CONTENT_IDENTITY_FORMAT_VERSION,
        pack_id: PACK_ID.to_string(),
        pack_checksum: PACK_CHECKSUM,
        registry_checksum: REGISTRY_CHECKSUM,
    }
}

/// Self-cleaning workspace for packages, libraries, and save files.
struct TempWorkspace {
    root: PathBuf,
}

impl TempWorkspace {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "townsmith-bridge-matrix-{}-{}-{label}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create matrix workspace");
        Self { root }
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn package(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn save(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn library(&self, name: &str) -> PathBuf {
        let root = self.root.join(name);
        std::fs::create_dir_all(&root).expect("create library root");
        root
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Write a deterministic 4 × 1 × 4 package and return its content hash.
fn write_package(path: &Path, id: &str, name: &str, fill: u16) -> String {
    let mut volume = VoxelVolume::new(VolumeConfig::new(1, 1, 1).expect("config")).expect("volume");
    volume
        .fill_box(VolumePos::new(0, 0, 0), VolumePos::new(4, 1, 4), fill)
        .expect("fill floor");
    let blueprint = Blueprint::new(BlueprintMetadata::new(id, name), volume);
    save_tsbp_with_hash(path, &blueprint).expect("save package")
}

/// Place a package record whose metadata matches [`write_package`].
fn place(
    store: &mut PlacementStore,
    package: &Path,
    hash: &str,
    id: &str,
    name: &str,
    x: f32,
) -> Placement {
    let request = PlacementRequest {
        blueprint_id: id.to_string(),
        blueprint_hash: hash.to_string(),
        blueprint_path: package.to_string_lossy().into_owned(),
        name: name.to_string(),
        bounds_m: [3.2, 3.2, 3.2],
        center_x: x,
        center_z: 100.0,
        rotation_deg: 0,
        material_cost: 100.0,
        resident_capacity: 0,
        job_slots: 0,
        solid_voxels: 16,
        structure: None,
    };
    store.place(&request, |_, _| 40).expect("placement")
}

struct ComposedSave {
    /// Exactly what the bridge would write.
    json: String,
    /// The stamped clone that was written (library saves carry references).
    saved_store: PlacementStore,
    identity: Option<WorldContentIdentity>,
    content_status: &'static str,
    content_bound: bool,
    library_sync: Option<WorldBlueprintSync>,
}

/// Mirror `world_save_impl` up to (but not including) the durable write.
fn compose_save(
    store: &PlacementStore,
    construction: &ConstructionStore,
    library: Option<&BlueprintLibrary>,
    active: &ActiveContentIdentity,
    recorded: Option<&WorldContentIdentity>,
) -> Result<ComposedSave, (&'static str, String)> {
    let mut saved_store = store.clone();
    // A plain save never calls this, so legacy records keep their exact JSON.
    if library.is_some() {
        stamp_world_blueprint_references(&mut saved_store);
    }
    let json = saved_store
        .to_json_pretty()
        .map_err(|error| ("save_failed", error.to_string()))?;
    let json = attach_construction(&json, construction).map_err(|error| ("save_failed", error))?;
    let stamp = stamp_world_content(&json, active, recorded)
        .map_err(|error| (error.code, error.message))?;
    let content_status = stamp.status_str();
    let content_bound = stamp.content_bound();
    let identity = stamp.identity;
    let json = stamp.json;
    let library_sync = match library {
        Some(library) => Some(sync_world_blueprints(&saved_store, library)?),
        None => None,
    };
    Ok(ComposedSave {
        json,
        saved_store,
        identity,
        content_status,
        content_bound,
        library_sync,
    })
}

struct LoadedSave {
    store: PlacementStore,
    construction: ConstructionStore,
    loaded: LoadedWorldPlacements,
    status: ContentIdentityStatus,
    identity: Option<WorldContentIdentity>,
    read_only: bool,
    format_version: u32,
}

/// Mirror `world_load_impl` after the JSON has been read.
fn load_save(
    json: &str,
    active: &ActiveContentIdentity,
    inspecting: bool,
    library: Option<&BlueprintLibrary>,
) -> Result<LoadedSave, (&'static str, String)> {
    let verified = if inspecting {
        verify_world_content_inspecting(json, active)
    } else {
        verify_world_content(json, active)
    }
    .map_err(|error| (error.code, error.message))?;
    let normalized = verified.normalized_json.as_deref().unwrap_or(json);
    let store = PlacementStore::from_json(normalized)
        .map_err(|error| ("invalid_placements", error.to_string()))?;
    let construction = construction_from_world_json(normalized)?.unwrap_or_default();
    let loaded = load_world_placement_packages(&store, library)?;
    let read_only = verified.read_only();
    let format_version = verified.format_version;
    let status = verified.status;
    let identity = verified.identity;
    Ok(LoadedSave {
        store,
        construction,
        loaded,
        status,
        identity,
        read_only,
        format_version,
    })
}

/// Bridge-like state: stores, recorded identity, and the read-only guard.
struct SimulatedBridge {
    store: PlacementStore,
    construction: ConstructionStore,
    recorded: Option<WorldContentIdentity>,
    read_only: bool,
}

impl SimulatedBridge {
    fn new() -> Self {
        Self {
            store: PlacementStore::new(SEED),
            construction: ConstructionStore::default(),
            recorded: None,
            read_only: false,
        }
    }

    fn with_world(store: PlacementStore) -> Self {
        Self {
            store,
            ..Self::new()
        }
    }

    /// Commit only after the whole load has succeeded.
    fn load(
        &mut self,
        json: &str,
        active: &ActiveContentIdentity,
        inspecting: bool,
        library: Option<&BlueprintLibrary>,
    ) -> Result<LoadedSave, (&'static str, String)> {
        let loaded = load_save(json, active, inspecting, library)?;
        self.store = loaded.store.clone();
        self.construction = loaded.construction.clone();
        self.recorded = loaded.identity.clone();
        self.read_only = loaded.read_only;
        Ok(loaded)
    }

    /// Mirror the read-only short-circuit, composition, and post-write commit.
    fn save(
        &mut self,
        path: &Path,
        active: &ActiveContentIdentity,
        library: Option<&BlueprintLibrary>,
    ) -> Result<ComposedSave, (&'static str, String)> {
        if self.read_only {
            let refusal = content_read_only_error();
            return Err((refusal.code, refusal.message));
        }
        let composed = compose_save(
            &self.store,
            &self.construction,
            library,
            active,
            self.recorded.as_ref(),
        )?;
        std::fs::write(path, &composed.json).map_err(|error| ("save_failed", error.to_string()))?;
        // The refreshed clone is committed only after the write is durable.
        self.store = composed.saved_store.clone();
        self.recorded = composed.identity.clone();
        Ok(composed)
    }

    /// `world_clear_placements` also drops the recorded identity and guard.
    fn clear_placements(&mut self) {
        self.store.clear();
        self.construction.clear();
        self.recorded = None;
        self.read_only = false;
    }
}

fn expect_code<T>(label: &str, result: Result<T, (&'static str, String)>, expected: &str) {
    match result {
        Ok(_) => panic!("{label} unexpectedly succeeded"),
        Err((code, message)) => {
            assert_eq!(code, expected, "{label}: {message}");
            assert!(!message.is_empty(), "{label} returned an empty error");
        }
    }
}

fn expect_verify_code(label: &str, json: &str, active: &ActiveContentIdentity, expected: &str) {
    let error = verify_world_content(json, active).expect_err(label);
    assert_eq!(error.code, expected, "{label}: {}", error.message);
}

fn value(json: &str) -> serde_json::Value {
    serde_json::from_str(json).expect("save JSON")
}

fn placement_value(value: &serde_json::Value, id: u64) -> &serde_json::Value {
    value["placements"]
        .as_array()
        .expect("placements array")
        .iter()
        .find(|placement| placement["id"] == id)
        .expect("placement id")
}

#[test]
fn plain_save_keeps_the_legacy_shape_and_restores_placements() {
    let workspace = TempWorkspace::new("plain");
    let package = workspace.package("house.tsbp");
    let hash = write_package(&package, "local:house", "Matrix House", EDITOR_FLOOR);
    let mut store = PlacementStore::new(SEED);
    place(
        &mut store,
        &package,
        &hash,
        "local:house",
        "Matrix House",
        100.0,
    );

    let construction = ConstructionStore::default();
    let composed = compose_save(&store, &construction, None, &base_active(), None)
        .expect("plain save composes");

    assert_eq!(composed.content_status, "absent");
    assert!(!composed.content_bound);
    assert!(composed.identity.is_none());
    assert!(composed.library_sync.is_none());
    assert!(
        composed.saved_store.placements[0].blueprint_ref.is_none(),
        "a plain save must not stamp library references"
    );

    // D-034: no live pack means the assembled JSON is untouched. This is the
    // pre-D-034 placement + construction shape byte for byte.
    let legacy = attach_construction(&store.to_json_pretty().expect("store JSON"), &construction)
        .expect("legacy composition");
    assert_eq!(composed.json, legacy);

    let envelope = value(&composed.json);
    assert_eq!(envelope["format_version"], 1);
    assert!(envelope.get("content").is_none());
    assert!(envelope.get("library").is_none());
    assert!(placement_value(&envelope, 1).get("blueprint_ref").is_none());

    let mut bridge = SimulatedBridge::with_world(store.clone());
    let loaded = bridge
        .load(&composed.json, &base_active(), false, None)
        .expect("plain load");
    assert_eq!(loaded.status, ContentIdentityStatus::Absent);
    assert_eq!(loaded.format_version, 0);
    assert!(loaded.identity.is_none());
    assert!(!loaded.read_only);
    assert_eq!(loaded.store, store);
    assert_eq!(loaded.store.checksum(), store.checksum());
    assert_eq!(loaded.loaded.solid_by_id.get(&1), Some(&16));
    assert_eq!(loaded.loaded.resolved_references, 0);
    assert_eq!(loaded.loaded.legacy_path_fallbacks, 0);
}

#[test]
fn library_save_stamps_references_and_resolves_them_first() {
    let workspace = TempWorkspace::new("library");
    let package = workspace.package("house.tsbp");
    let hash = write_package(&package, "local:house", "Matrix House", EDITOR_FLOOR);
    let mut store = PlacementStore::new(SEED);
    place(
        &mut store,
        &package,
        &hash,
        "local:house",
        "Matrix House",
        100.0,
    );
    let construction = ConstructionStore::default();

    let library = BlueprintLibrary::new(workspace.library("library")).expect("library");
    let composed = compose_save(&store, &construction, Some(&library), &base_active(), None)
        .expect("library save composes");

    let sync = composed.library_sync.as_ref().expect("library sync");
    assert_eq!(sync.unique, 1);
    assert_eq!(sync.inserted, 1);
    assert_eq!(sync.existing, 0);
    assert_eq!(sync.library_count, 1);
    assert_eq!(library.summary().expect("summary").count, 1);
    assert!(store.placements[0].blueprint_ref.is_none());

    let expected_reference = format!("tsbp:{hash}");
    assert_eq!(
        composed.saved_store.placements[0].blueprint_ref.as_deref(),
        Some(expected_reference.as_str())
    );
    let envelope = value(&composed.json);
    assert_eq!(
        placement_value(&envelope, 1)["blueprint_ref"],
        expected_reference.as_str()
    );

    // The reference resolves first even while the recorded path still works.
    let library_path = library.resolve(&hash).expect("library entry");
    let both = load_save(&composed.json, &base_active(), false, Some(&library))
        .expect("reference-first load");
    assert_eq!(both.loaded.resolved_references, 1);
    assert_eq!(both.loaded.legacy_path_fallbacks, 0);
    assert_eq!(
        both.loaded.resolved_paths.get(&hash).map(String::as_str),
        Some(library_path.to_string_lossy().as_ref())
    );
    assert_ne!(
        both.loaded.resolved_paths.get(&hash),
        Some(&package.to_string_lossy().into_owned())
    );
    assert_eq!(both.store, composed.saved_store);
    assert_eq!(both.loaded.solid_by_id.get(&1), Some(&16));

    // A reference is never treated as a path: no library, working path.
    let fallback =
        load_save(&composed.json, &base_active(), false, None).expect("legacy path fallback");
    assert_eq!(fallback.loaded.resolved_references, 0);
    assert_eq!(fallback.loaded.legacy_path_fallbacks, 1);
    assert_eq!(fallback.loaded.solid_by_id.get(&1), Some(&16));

    // Missing source and no library: fail closed with the stable code.
    std::fs::remove_file(&package).expect("remove source");
    expect_code(
        "reference without a library",
        load_save(&composed.json, &base_active(), false, None),
        "blueprint_reference_requires_library",
    );

    // Missing source and a library without the entry: fail closed.
    let empty = BlueprintLibrary::new(workspace.library("empty")).expect("empty library");
    expect_code(
        "missing library entry",
        load_save(&composed.json, &base_active(), false, Some(&empty)),
        "blueprint_library_missing_file",
    );

    // The stored copy still loads after the source disappears.
    let portable =
        load_save(&composed.json, &base_active(), false, Some(&library)).expect("portable load");
    assert_eq!(portable.loaded.resolved_references, 1);
    assert_eq!(portable.loaded.solid_by_id.get(&1), Some(&16));
}

#[test]
fn content_identity_is_v1_base_v2_pack_and_rejects_mismatches() {
    let workspace = TempWorkspace::new("content");
    let package = workspace.package("house.tsbp");
    let hash = write_package(&package, "local:house", "Matrix House", EDITOR_FLOOR);
    let mut store = PlacementStore::new(SEED);
    place(
        &mut store,
        &package,
        &hash,
        "local:house",
        "Matrix House",
        100.0,
    );
    let construction = ConstructionStore::default();

    let base = compose_save(&store, &construction, None, &base_active(), None)
        .expect("base save composes");
    assert_eq!(base.content_status, "absent");
    assert!(!base.content_bound);
    assert_eq!(value(&base.json)["format_version"], 1);
    assert!(value(&base.json).get("content").is_none());

    let pack = compose_save(&store, &construction, None, &pack_active(), None)
        .expect("pack save composes");
    assert_eq!(pack.content_status, "new");
    assert!(pack.content_bound);
    assert_eq!(pack.identity, Some(recorded_identity()));
    let envelope = value(&pack.json);
    assert_eq!(envelope["format_version"], 2);
    assert_eq!(envelope["content"]["format_version"], 1);
    assert_eq!(envelope["content"]["pack_id"], PACK_ID);
    assert_eq!(envelope["content"]["pack_checksum"], "ad4c645869a411ed");
    assert_eq!(envelope["content"]["registry_checksum"], "330a538d07805b50");

    // The same identity loads as a match and stays writable.
    let verified = verify_world_content(&pack.json, &pack_active()).expect("matching pack");
    assert_eq!(verified.status, ContentIdentityStatus::Match);
    assert!(!verified.read_only());
    assert_eq!(verified.identity, Some(recorded_identity()));
    let mut bridge = SimulatedBridge::with_world(store.clone());
    let loaded = bridge
        .load(&pack.json, &pack_active(), false, None)
        .expect("matching load");
    assert_eq!(loaded.status, ContentIdentityStatus::Match);
    assert!(!loaded.read_only);
    assert_eq!(bridge.recorded, Some(recorded_identity()));

    // Re-saving with the recorded identity matches instead of rebinding.
    let again = compose_save(
        &store,
        &construction,
        None,
        &pack_active(),
        bridge.recorded.as_ref(),
    )
    .expect("identical identity re-save");
    assert_eq!(again.content_status, "match");
    assert_eq!(again.identity, Some(recorded_identity()));

    // Different pack, checksum, or merged registry each fail with its code.
    expect_verify_code(
        "different pack",
        &pack.json,
        &other_pack_active(),
        "content_pack_mismatch",
    );
    expect_verify_code(
        "pack checksum",
        &pack.json,
        &pack_checksum_mismatch(),
        "content_pack_checksum_mismatch",
    );
    expect_verify_code(
        "registry checksum",
        &pack.json,
        &registry_checksum_mismatch(),
        "content_registry_checksum_mismatch",
    );

    // Missing pack: strict fails closed, inspection loads read-only.
    expect_verify_code(
        "missing pack strict",
        &pack.json,
        &base_active(),
        "content_pack_missing",
    );
    let inspected = verify_world_content_inspecting(&pack.json, &base_active())
        .expect("missing pack inspection");
    assert_eq!(inspected.status, ContentIdentityStatus::Missing);
    assert!(inspected.read_only());
    assert_eq!(inspected.identity, Some(recorded_identity()));

    // Save-side mismatches use the corresponding stable codes.
    let recorded = recorded_identity();
    expect_code(
        "base-only downgrade",
        compose_save(&store, &construction, None, &base_active(), Some(&recorded)),
        "content_save_requires_pack",
    );
    expect_code(
        "other active pack",
        compose_save(
            &store,
            &construction,
            None,
            &other_pack_active(),
            Some(&recorded),
        ),
        "content_save_pack_mismatch",
    );
    expect_code(
        "active pack checksum",
        compose_save(
            &store,
            &construction,
            None,
            &pack_checksum_mismatch(),
            Some(&recorded),
        ),
        "content_pack_checksum_mismatch",
    );
    expect_code(
        "active registry checksum",
        compose_save(
            &store,
            &construction,
            None,
            &registry_checksum_mismatch(),
            Some(&recorded),
        ),
        "content_registry_checksum_mismatch",
    );
}

#[test]
fn inspecting_load_is_read_only_until_a_match_or_clear() {
    let workspace = TempWorkspace::new("read-only");
    let store = PlacementStore::new(SEED);
    let construction = ConstructionStore::default();
    let pack = compose_save(&store, &construction, None, &pack_active(), None)
        .expect("pack save composes");

    let mut bridge = SimulatedBridge::new();
    let loaded = bridge
        .load(&pack.json, &base_active(), true, None)
        .expect("missing pack inspection");
    assert_eq!(loaded.status, ContentIdentityStatus::Missing);
    assert!(loaded.read_only);
    assert!(bridge.read_only);
    assert_eq!(bridge.recorded, Some(recorded_identity()));

    let save_path = workspace.save("read-only.json");
    expect_code(
        "read-only save",
        bridge.save(&save_path, &base_active(), None),
        "content_read_only",
    );
    assert!(
        !save_path.exists(),
        "the read-only refusal must run before any write"
    );
    assert!(bridge.read_only, "the refusal must leave the guard set");

    // A strict matching load clears the guard and restores writes.
    let matched = bridge
        .load(&pack.json, &pack_active(), false, None)
        .expect("matching load");
    assert_eq!(matched.status, ContentIdentityStatus::Match);
    assert!(!bridge.read_only);
    let written = bridge
        .save(&save_path, &pack_active(), None)
        .expect("save after matching load");
    assert_eq!(written.content_status, "match");
    assert!(save_path.exists());

    // Read-only again, then `world_clear_placements` drops the guard.
    bridge
        .load(&pack.json, &base_active(), true, None)
        .expect("read-only again");
    assert!(bridge.read_only);
    bridge.clear_placements();
    assert!(!bridge.read_only);
    assert_eq!(bridge.recorded, None);
    assert_eq!(bridge.store.placements.len(), 0);
    let cleared = bridge
        .save(&save_path, &base_active(), None)
        .expect("save after clear");
    assert_eq!(cleared.content_status, "absent");
    assert!(!cleared.content_bound);
}

#[test]
fn library_content_and_multiple_references_compose_together() {
    let workspace = TempWorkspace::new("combined");
    let first_package = workspace.package("first.tsbp");
    let second_package = workspace.package("second.tsbp");
    let first_hash = write_package(&first_package, "local:first", "Matrix First", EDITOR_FLOOR);
    let second_hash = write_package(
        &second_package,
        "local:second",
        "Matrix Second",
        EDITOR_WALL,
    );
    let mut store = PlacementStore::new(SEED);
    place(
        &mut store,
        &first_package,
        &first_hash,
        "local:first",
        "Matrix First",
        100.0,
    );
    place(
        &mut store,
        &first_package,
        &first_hash,
        "local:first",
        "Matrix First",
        200.0,
    );
    place(
        &mut store,
        &second_package,
        &second_hash,
        "local:second",
        "Matrix Second",
        300.0,
    );
    let construction = ConstructionStore::default();

    let library = BlueprintLibrary::new(workspace.library("library")).expect("library");
    let composed = compose_save(&store, &construction, Some(&library), &pack_active(), None)
        .expect("combined save composes");
    assert!(composed.content_bound);
    assert_eq!(composed.content_status, "new");
    assert_eq!(composed.saved_store.placements.len(), 3);
    let sync = composed.library_sync.as_ref().expect("sync");
    assert_eq!(sync.unique, 2);
    assert_eq!(sync.inserted, 2);
    assert_eq!(sync.library_count, 2);
    assert_eq!(
        composed
            .saved_store
            .placements
            .iter()
            .filter(|placement| placement.blueprint_ref.is_some())
            .count(),
        3
    );
    assert_eq!(value(&composed.json)["format_version"], 2);

    std::fs::remove_file(&first_package).expect("remove first source");
    std::fs::remove_file(&second_package).expect("remove second source");

    let loaded = load_save(&composed.json, &pack_active(), false, Some(&library))
        .expect("combined portable load");
    assert_eq!(loaded.status, ContentIdentityStatus::Match);
    assert!(!loaded.read_only);
    assert_eq!(loaded.loaded.resolved_references, 3);
    assert_eq!(loaded.loaded.legacy_path_fallbacks, 0);
    assert_eq!(loaded.store, composed.saved_store);
    assert_eq!(loaded.loaded.solid_by_id.len(), 3);
    assert_eq!(loaded.loaded.solid_by_id.get(&1), Some(&16));
    assert_eq!(loaded.loaded.solid_by_id.get(&2), Some(&16));
    assert_eq!(loaded.loaded.solid_by_id.get(&3), Some(&16));

    // The same portable save inspected without its pack stays read-only but
    // still resolves every package from the library.
    let inspected = load_save(&composed.json, &base_active(), true, Some(&library))
        .expect("combined read-only load");
    assert_eq!(inspected.status, ContentIdentityStatus::Missing);
    assert!(inspected.read_only);
    assert_eq!(inspected.loaded.resolved_references, 3);
    assert_eq!(inspected.store, composed.saved_store);
}

#[test]
fn empty_world_composes_cleanly_with_and_without_content() {
    let workspace = TempWorkspace::new("empty");
    let construction = ConstructionStore::default();
    let store = PlacementStore::new(SEED);

    let plain =
        compose_save(&store, &construction, None, &base_active(), None).expect("empty plain save");
    assert!(!plain.content_bound);
    assert_eq!(
        value(&plain.json)["placements"].as_array().unwrap().len(),
        0
    );
    let loaded = load_save(&plain.json, &base_active(), false, None).expect("empty plain load");
    assert!(loaded.store.placements.is_empty());
    assert!(loaded.loaded.solid_by_id.is_empty());

    let library = BlueprintLibrary::new(workspace.library("library")).expect("library");
    let bound = compose_save(&store, &construction, Some(&library), &pack_active(), None)
        .expect("empty content save");
    assert!(bound.content_bound);
    let sync = bound.library_sync.as_ref().expect("sync");
    assert_eq!(sync.unique, 0);
    assert_eq!(sync.inserted, 0);
    assert_eq!(sync.library_count, 0);
    assert_eq!(library.summary().expect("summary").count, 0);
    let loaded =
        load_save(&bound.json, &pack_active(), false, Some(&library)).expect("empty content load");
    assert_eq!(loaded.status, ContentIdentityStatus::Match);
    assert!(loaded.store.placements.is_empty());
    assert!(loaded.loaded.solid_by_id.is_empty());
    assert_eq!(loaded.loaded.resolved_references, 0);
}

#[test]
fn repeated_saves_are_byte_identical_and_identities_are_path_free() {
    let workspace = TempWorkspace::new("determinism");
    let package = workspace.package("house.tsbp");
    let hash = write_package(&package, "local:house", "Matrix House", EDITOR_FLOOR);
    let mut store = PlacementStore::new(SEED);
    place(
        &mut store,
        &package,
        &hash,
        "local:house",
        "Matrix House",
        100.0,
    );
    let construction = ConstructionStore::default();

    let library = BlueprintLibrary::new(workspace.library("library")).expect("library");
    let first = compose_save(&store, &construction, Some(&library), &pack_active(), None)
        .expect("first save");
    // The bridge commits the identity after the first durable write.
    let second = compose_save(
        &store,
        &construction,
        Some(&library),
        &pack_active(),
        first.identity.as_ref(),
    )
    .expect("second save");

    assert_eq!(first.json, second.json);
    assert_eq!(first.saved_store.checksum(), second.saved_store.checksum());
    assert_eq!(first.identity, second.identity);
    assert_eq!(first.content_status, "new");
    assert_eq!(second.content_status, "match");
    assert_eq!(
        second.library_sync.as_ref().expect("second sync").existing,
        1
    );

    let first_path = workspace.save("first.json");
    let second_path = workspace.save("second.json");
    std::fs::write(&first_path, &first.json).expect("write first");
    std::fs::write(&second_path, &second.json).expect("write second");
    assert_eq!(
        std::fs::read(&first_path).expect("read first"),
        std::fs::read(&second_path).expect("read second")
    );

    let envelope = value(&first.json);
    let content = envelope["content"].as_object().expect("content object");
    let mut keys: Vec<&str> = content.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "format_version",
            "pack_checksum",
            "pack_id",
            "registry_checksum"
        ]
    );
    let content_text = serde_json::to_string(content).expect("content text");
    let workspace_root = workspace.root().to_string_lossy();
    assert!(
        !content_text.contains(workspace_root.as_ref()),
        "content identity must not record a path"
    );
    assert!(!content_text.contains("docs/"));
    assert!(!first.json.contains("docs/"));
    assert!(!first.json.contains("library_root"));
    assert!(content["pack_id"]
        .as_str()
        .is_some_and(|id| !id.contains('/')));
    assert!(!content["pack_checksum"]
        .as_str()
        .unwrap_or("")
        .contains('/'));
    assert!(!content["registry_checksum"]
        .as_str()
        .unwrap_or("")
        .contains('/'));

    for placement in envelope["placements"].as_array().expect("placements") {
        let reference = placement["blueprint_ref"].as_str().expect("reference");
        assert_eq!(
            reference,
            format!("tsbp:{}", placement["blueprint_hash"].as_str().unwrap())
        );
        assert!(reference.starts_with("tsbp:blake3:"));
        assert!(!reference.contains('/'));
        assert!(!reference.contains(workspace_root.as_ref()));
    }
}

#[test]
fn v1_round_trips_byte_identically_and_v2_normalizes_without_downgrade() {
    let workspace = TempWorkspace::new("migration");
    let package = workspace.package("house.tsbp");
    let hash = write_package(&package, "local:house", "Matrix House", EDITOR_FLOOR);
    let mut store = PlacementStore::new(SEED);
    place(
        &mut store,
        &package,
        &hash,
        "local:house",
        "Matrix House",
        100.0,
    );

    // A plain v1 load followed by a re-save keeps every byte.
    let first_path = workspace.save("v1-first.json");
    let second_path = workspace.save("v1-second.json");
    let mut bridge = SimulatedBridge::with_world(store.clone());
    let first = bridge
        .save(&first_path, &base_active(), None)
        .expect("v1 save");
    assert_eq!(first.content_status, "absent");
    let before = std::fs::read(&first_path).expect("read v1");
    let loaded = bridge
        .load(
            std::str::from_utf8(&before).expect("v1 utf8"),
            &base_active(),
            false,
            None,
        )
        .expect("v1 load");
    assert_eq!(loaded.status, ContentIdentityStatus::Absent);
    bridge
        .save(&second_path, &base_active(), None)
        .expect("v1 re-save");
    let after = std::fs::read(&second_path).expect("read v1 re-save");
    assert_eq!(before, after);

    // A v2 save normalizes to v1 for the store parsers and is not rewritten.
    let construction = ConstructionStore::default();
    let v2 = compose_save(&store, &construction, None, &pack_active(), None).expect("v2 save");
    let v2_path = workspace.save("v2.json");
    std::fs::write(&v2_path, &v2.json).expect("write v2");
    let v2_before = std::fs::read(&v2_path).expect("read v2");
    let verified = verify_world_content(&v2.json, &pack_active()).expect("v2 match");
    let normalized = verified.normalized_json.expect("v2 normalizes");
    assert_ne!(normalized, v2.json);
    let normalized_value = value(&normalized);
    assert_eq!(normalized_value["format_version"], 1);
    assert!(normalized_value.get("content").is_none());
    assert_eq!(
        PlacementStore::from_json(&normalized)
            .expect("normalized store")
            .checksum(),
        store.checksum()
    );

    let mut bridge = SimulatedBridge::new();
    bridge
        .load(&v2.json, &pack_active(), false, None)
        .expect("v2 load");
    assert_eq!(bridge.store, store);
    assert_eq!(
        std::fs::read(&v2_path).expect("read v2 after load"),
        v2_before,
        "load must never rewrite the save"
    );

    // No silent downgrade: a base-only registry refuses to save the binding.
    let recorded = v2.identity.clone().expect("v2 identity");
    expect_verify_code(
        "v2 strict without pack",
        &v2.json,
        &base_active(),
        "content_pack_missing",
    );
    expect_code(
        "v2 save without pack",
        compose_save(&store, &construction, None, &base_active(), Some(&recorded)),
        "content_save_requires_pack",
    );
}
