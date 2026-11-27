//! Save/load composition fault matrix for the bridge.
//!
//! `world_save*` and `world_load*` are `#[func]` methods and need a Godot
//! runtime, so this module drives the same pure helper layer in the production
//! order and asserts the composition contract without constructing an engine
//! object:
//!
//! ```text
//! save: stamp refs (library only) -> placement JSON -> construction ->
//!       settlements -> stamp content identity -> sync library -> durable write
//! load: read JSON -> verify content identity -> normalize v2 to v1 ->
//!       parse store/construction/settlements -> resolve packages -> commit
//! ```
//!
//! The focus is the seam between the three save features: legacy placements,
//! content-addressed library references, D-034 content identity, and the
//! settlement layer. Every rejection pins a stable code and asserts that no
//! live store, identity, read-only guard, or resolution map moved. Package
//! paths stay legacy save values (D-027); only references, content identity,
//! and settlements are required to be path-free.

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

/// Self-cleaning workspace. Labels may contain spaces on purpose.
struct TempWorkspace {
    root: PathBuf,
}

impl TempWorkspace {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "townsmith-composition-faults-{}-{}-{label}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create composition workspace");
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

/// Deterministic 4 × 1 × 4 package.
fn write_package(path: &Path, id: &str, name: &str, fill: u16) -> String {
    let mut volume = VoxelVolume::new(VolumeConfig::new(1, 1, 1).expect("config")).expect("volume");
    volume
        .fill_box(VolumePos::new(0, 0, 0), VolumePos::new(4, 1, 4), fill)
        .expect("fill floor");
    let blueprint = Blueprint::new(BlueprintMetadata::new(id, name), volume);
    save_tsbp_with_hash(path, &blueprint).expect("save package")
}

/// Placement record whose metadata matches [`write_package`].
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

/// One v1 unowned and one v2 owned settlement, so the layer exercises both
/// serialization versions without a player registry.
fn settlement_layer() -> SettlementStore {
    let mut settlements = SettlementStore::new(SEED);
    settlements
        .found(&SettlementRequest {
            name: "Riverbend".to_string(),
            owner: "player".to_string(),
            center: [10, 20],
            radius_m: 64,
        })
        .expect("unowned settlement");
    settlements
        .found_for_player(
            &SettlementRequest {
                name: "Northwatch".to_string(),
                owner: "player".to_string(),
                center: [400, -120],
                radius_m: 48,
            },
            PlayerId::new(7).expect("player id"),
        )
        .expect("owned settlement");
    settlements
}

struct ComposedSave {
    /// Exactly what the bridge would write.
    json: String,
    /// The stamped clone that was committed after a durable write.
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
    settlements: Option<&SettlementStore>,
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
    let json = match settlements {
        Some(settlements) => {
            if settlements.seed != saved_store.seed {
                return Err((
                    "seed_mismatch",
                    "settlements belong to a different seed".to_string(),
                ));
            }
            attach_settlements(&json, settlements).map_err(|error| ("save_failed", error))?
        }
        None => json,
    };
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
    settlements: Option<SettlementStore>,
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
    let settlements = settlements_from_world_json(normalized)?;
    if let Some(settlements) = settlements.as_ref() {
        if settlements.seed != store.seed {
            return Err((
                "seed_mismatch",
                "settlements belong to a different seed".to_string(),
            ));
        }
    }
    let loaded = load_world_placement_packages(&store, library)?;
    let read_only = verified.read_only();
    let format_version = verified.format_version;
    let status = verified.status;
    let identity = verified.identity;
    Ok(LoadedSave {
        store,
        construction,
        settlements,
        loaded,
        status,
        identity,
        read_only,
        format_version,
    })
}

/// Bridge-like state: stores, recorded identity, guard, and resolution map.
struct SimulatedBridge {
    store: PlacementStore,
    construction: ConstructionStore,
    settlements: Option<SettlementStore>,
    recorded: Option<WorldContentIdentity>,
    read_only: bool,
    resolved_paths: BTreeMap<String, String>,
}

impl SimulatedBridge {
    fn new() -> Self {
        Self {
            store: PlacementStore::new(SEED),
            construction: ConstructionStore::default(),
            settlements: None,
            recorded: None,
            read_only: false,
            resolved_paths: BTreeMap::new(),
        }
    }

    fn with_world(store: PlacementStore, construction: ConstructionStore) -> Self {
        Self {
            store,
            construction,
            ..Self::new()
        }
    }

    /// Commit only after the whole load has succeeded. A plain load clears the
    /// resolution map, matching the bridge.
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
        self.settlements = loaded.settlements.clone();
        self.recorded = loaded.identity.clone();
        self.read_only = loaded.read_only;
        self.resolved_paths = loaded.loaded.resolved_paths.clone();
        Ok(loaded)
    }

    /// Read-only short-circuit, composition, durable write, then commit.
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
            self.settlements.as_ref(),
            library,
            active,
            self.recorded.as_ref(),
        )?;
        std::fs::write(path, &composed.json).map_err(|error| ("save_failed", error.to_string()))?;
        self.store = composed.saved_store.clone();
        self.recorded = composed.identity.clone();
        Ok(composed)
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

fn placement_refs(json: &str) -> Vec<String> {
    value(json)["placements"]
        .as_array()
        .expect("placements array")
        .iter()
        .filter_map(|placement| placement["blueprint_ref"].as_str().map(str::to_owned))
        .collect()
}

/// Remove every reference from a parsed save so the library JSON can be
/// compared field-for-field with the plain baseline.
fn without_references(json: &str) -> serde_json::Value {
    let mut envelope = value(json);
    if let Some(placements) = envelope["placements"].as_array_mut() {
        for placement in placements {
            placement
                .as_object_mut()
                .expect("placement object")
                .remove("blueprint_ref");
        }
    }
    envelope
}

#[test]
fn plain_and_library_saves_add_references_only_in_the_library_path() {
    let workspace = TempWorkspace::new("plain-vs-library");
    let construction = ConstructionStore::default();
    let settlements = settlement_layer();
    let mut store = PlacementStore::new(SEED);
    let package = workspace.package("house.tsbp");
    let hash = write_package(&package, "local:house", "Composition House", EDITOR_FLOOR);
    for (x, name) in [(100.0, "Composition House"), (200.0, "Composition House")] {
        place(&mut store, &package, &hash, "local:house", name, x);
    }

    // Zero placements: the library path has nothing to stamp or copy.
    let empty = PlacementStore::new(SEED);
    let plain_empty = compose_save(
        &empty,
        &construction,
        Some(&settlements),
        None,
        &base_active(),
        None,
    )
    .expect("plain empty save");
    let empty_library = BlueprintLibrary::new(workspace.library("empty library")).expect("library");
    let library_empty = compose_save(
        &empty,
        &construction,
        Some(&settlements),
        Some(&empty_library),
        &base_active(),
        None,
    )
    .expect("library empty save");
    assert_eq!(plain_empty.json, library_empty.json);
    assert_eq!(library_empty.library_sync.as_ref().expect("sync").unique, 0);
    assert!(placement_refs(&plain_empty.json).is_empty());
    assert!(placement_refs(&library_empty.json).is_empty());

    // One and many placements: the plain baseline is the pre-reference
    // composition byte for byte, and only the library path adds references.
    let library = BlueprintLibrary::new(workspace.library("library with spaces")).expect("library");
    let mut one = PlacementStore::new(SEED);
    place(
        &mut one,
        &package,
        &hash,
        "local:house",
        "Composition House",
        100.0,
    );
    for (label, candidate) in [("one", &one), ("many", &store)] {
        let plain = compose_save(
            candidate,
            &construction,
            Some(&settlements),
            None,
            &base_active(),
            None,
        )
        .expect("plain save");
        let baseline = attach_settlements(
            &attach_construction(
                &candidate.to_json_pretty().expect("store JSON"),
                &construction,
            )
            .expect("construction"),
            &settlements,
        )
        .expect("settlements");
        assert_eq!(plain.json, baseline, "{label}: plain save changed shape");
        assert_eq!(plain.content_status, "absent");
        assert!(!plain.content_bound);
        assert!(plain.library_sync.is_none());
        assert!(
            placement_refs(&plain.json).is_empty(),
            "{label}: plain save must not add references"
        );
        let envelope = value(&plain.json);
        assert_eq!(envelope["format_version"], 1);
        assert!(envelope.get("content").is_none());
        assert!(envelope.get("library").is_none());
        assert!(envelope["settlements"].is_object());

        let bound = compose_save(
            candidate,
            &construction,
            Some(&settlements),
            Some(&library),
            &base_active(),
            None,
        )
        .expect("library save");
        let refs = placement_refs(&bound.json);
        assert_eq!(
            refs.len(),
            candidate.placements.len(),
            "{label}: every record carries a reference"
        );
        for reference in &refs {
            assert_eq!(reference, &format!("tsbp:{hash}"));
            assert!(!reference.contains('/'));
            assert!(!reference.contains(workspace.root().to_string_lossy().as_ref()));
        }
        assert_eq!(
            without_references(&bound.json),
            value(&plain.json),
            "{label}: references are the only difference"
        );
        assert_eq!(
            bound.library_sync.as_ref().expect("sync").unique,
            usize::from(!candidate.placements.is_empty())
        );
        let replayed = compose_save(
            candidate,
            &construction,
            Some(&settlements),
            Some(&library),
            &base_active(),
            None,
        )
        .expect("second library save");
        assert_eq!(bound.json, replayed.json, "{label}: save is deterministic");
    }
}

#[test]
fn content_bound_saves_refuse_base_content_and_read_only_until_a_match() {
    let workspace = TempWorkspace::new("content-bound");
    let package = workspace.package("house.tsbp");
    let hash = write_package(&package, "local:house", "Content House", EDITOR_FLOOR);
    let mut store = PlacementStore::new(SEED);
    place(
        &mut store,
        &package,
        &hash,
        "local:house",
        "Content House",
        100.0,
    );
    let construction = ConstructionStore::default();
    let settlements = settlement_layer();

    let bound = compose_save(
        &store,
        &construction,
        Some(&settlements),
        None,
        &pack_active(),
        None,
    )
    .expect("content-bound save");
    assert!(bound.content_bound);
    assert_eq!(bound.content_status, "new");
    assert_eq!(bound.identity, Some(recorded_identity()));
    let envelope = value(&bound.json);
    assert_eq!(envelope["format_version"], 2);
    assert_eq!(envelope["content"]["pack_id"], PACK_ID);
    assert!(envelope["settlements"].is_object());
    let recorded = bound.identity.clone().expect("recorded identity");

    // Base-only active content cannot write the binding back.
    expect_code(
        "base-only save keeps the binding",
        compose_save(
            &store,
            &construction,
            Some(&settlements),
            None,
            &base_active(),
            Some(&recorded),
        ),
        "content_save_requires_pack",
    );

    // A different pack or checksum is a stable, distinct mismatch.
    expect_code(
        "different pack save",
        compose_save(
            &store,
            &construction,
            Some(&settlements),
            None,
            &other_pack_active(),
            Some(&recorded),
        ),
        "content_save_pack_mismatch",
    );
    expect_code(
        "pack checksum save",
        compose_save(
            &store,
            &construction,
            Some(&settlements),
            None,
            &pack_checksum_mismatch(),
            Some(&recorded),
        ),
        "content_pack_checksum_mismatch",
    );
    expect_code(
        "registry checksum save",
        compose_save(
            &store,
            &construction,
            Some(&settlements),
            None,
            &registry_checksum_mismatch(),
            Some(&recorded),
        ),
        "content_registry_checksum_mismatch",
    );
    expect_verify_code(
        "different pack load",
        &bound.json,
        &other_pack_active(),
        "content_pack_mismatch",
    );
    expect_verify_code(
        "pack checksum load",
        &bound.json,
        &pack_checksum_mismatch(),
        "content_pack_checksum_mismatch",
    );
    expect_verify_code(
        "registry checksum load",
        &bound.json,
        &registry_checksum_mismatch(),
        "content_registry_checksum_mismatch",
    );

    // A read-only inspection refuses every save before touching a file and
    // keeps the guard, stores, identity, and settlement layer unchanged.
    let mut bridge = SimulatedBridge::with_world(store.clone(), construction.clone());
    bridge.settlements = Some(settlements.clone());
    let inspected = bridge
        .load(&bound.json, &base_active(), true, None)
        .expect("missing-pack inspection");
    assert_eq!(inspected.status, ContentIdentityStatus::Missing);
    assert_eq!(inspected.format_version, recorded_identity().format_version);
    assert!(inspected.read_only);
    assert_eq!(inspected.settlements, Some(settlements.clone()));
    assert_eq!(bridge.recorded, Some(recorded_identity()));
    let store_before = bridge.store.checksum();
    let settlements_before = bridge.settlements.as_ref().map(SettlementStore::checksum);
    let save_path = workspace.save("refused.json");
    expect_code(
        "read-only save",
        bridge.save(&save_path, &base_active(), None),
        "content_read_only",
    );
    assert!(!save_path.exists(), "the refusal must precede any write");
    assert!(bridge.read_only);
    assert_eq!(bridge.store.checksum(), store_before);
    assert_eq!(
        bridge.settlements.as_ref().map(SettlementStore::checksum),
        settlements_before
    );
    assert_eq!(bridge.recorded, Some(recorded_identity()));

    // The exact pack restores write mode and the save stays v2 with identity.
    let matched = bridge
        .load(&bound.json, &pack_active(), false, None)
        .expect("matching load");
    assert_eq!(matched.status, ContentIdentityStatus::Match);
    assert!(!matched.read_only);
    let written = bridge
        .save(&save_path, &pack_active(), None)
        .expect("save after match");
    assert_eq!(written.content_status, "match");
    assert_eq!(written.identity, Some(recorded_identity()));
    let written_value = value(&written.json);
    assert_eq!(written_value["format_version"], 2);
    assert_eq!(written_value["content"]["pack_id"], PACK_ID);
    assert_eq!(
        written_value["settlements"],
        value(&bound.json)["settlements"]
    );
}

#[test]
fn library_references_and_content_identity_resolve_first_and_fail_closed() {
    let workspace = TempWorkspace::new("refs with spaces");
    let first_package = workspace.package("first package with spaces.tsbp");
    let second_package = workspace.package("second.tsbp");
    let first_hash = write_package(
        &first_package,
        "local:first",
        "First Composition",
        EDITOR_FLOOR,
    );
    let second_hash = write_package(
        &second_package,
        "local:second",
        "Second Composition",
        EDITOR_WALL,
    );
    let mut store = PlacementStore::new(SEED);
    place(
        &mut store,
        &first_package,
        &first_hash,
        "local:first",
        "First Composition",
        100.0,
    );
    place(
        &mut store,
        &first_package,
        &first_hash,
        "local:first",
        "First Composition",
        150.0,
    );
    place(
        &mut store,
        &second_package,
        &second_hash,
        "local:second",
        "Second Composition",
        300.0,
    );
    let construction = ConstructionStore::default();
    let settlements = settlement_layer();
    let library = BlueprintLibrary::new(workspace.library("library with spaces")).expect("library");

    let bound = compose_save(
        &store,
        &construction,
        Some(&settlements),
        Some(&library),
        &pack_active(),
        None,
    )
    .expect("combined save");
    assert_eq!(bound.content_status, "new");
    assert_eq!(placement_refs(&bound.json).len(), 3);
    assert_eq!(bound.library_sync.as_ref().expect("sync").unique, 2);
    let library_root = workspace
        .library("library with spaces")
        .to_string_lossy()
        .into_owned();
    assert!(!bound.json.contains(&format!("\"{library_root}\"")));
    for reference in placement_refs(&bound.json) {
        assert!(reference.starts_with("tsbp:blake3:"));
        assert!(!reference.contains('/'));
    }

    // The source packages are gone; the library is the only package source.
    std::fs::remove_file(&first_package).expect("remove first");
    std::fs::remove_file(&second_package).expect("remove second");

    let loaded = load_save(&bound.json, &pack_active(), false, Some(&library))
        .expect("portable reference-first load");
    assert_eq!(loaded.status, ContentIdentityStatus::Match);
    assert!(!loaded.read_only);
    assert_eq!(loaded.loaded.resolved_references, 3);
    assert_eq!(loaded.loaded.legacy_path_fallbacks, 0);
    assert_eq!(loaded.store, bound.saved_store);
    assert_eq!(loaded.settlements, Some(settlements.clone()));
    assert_eq!(loaded.loaded.solid_by_id.get(&1), Some(&16));
    assert_eq!(loaded.loaded.solid_by_id.get(&2), Some(&16));
    assert_eq!(loaded.loaded.solid_by_id.get(&3), Some(&16));
    for path in loaded.loaded.resolved_paths.values() {
        assert!(path.contains(&library_root), "{path}");
    }

    // Inspecting without the pack stays read-only but still resolves refs.
    let inspected = load_save(&bound.json, &base_active(), true, Some(&library))
        .expect("read-only portable load");
    assert_eq!(inspected.status, ContentIdentityStatus::Missing);
    assert!(inspected.read_only);
    assert_eq!(inspected.loaded.resolved_references, 3);
    assert_eq!(inspected.settlements, Some(settlements.clone()));

    // Strict base content rejects before package resolution.
    expect_code(
        "strict base content",
        load_save(&bound.json, &base_active(), false, Some(&library)),
        "content_pack_missing",
    );

    // A reference without a library may use the recorded path only when that
    // path works; a failed path fails closed with the stable code.
    let mut bridge = SimulatedBridge::with_world(store.clone(), construction.clone());
    bridge.settlements = Some(settlements.clone());
    let store_before = bridge.store.checksum();
    let settlements_before = bridge.settlements.as_ref().map(SettlementStore::checksum);
    expect_code(
        "reference without library and missing source",
        bridge.load(&bound.json, &base_active(), true, None),
        "blueprint_reference_requires_library",
    );
    assert_eq!(bridge.store.checksum(), store_before);
    assert_eq!(
        bridge.settlements.as_ref().map(SettlementStore::checksum),
        settlements_before
    );
    assert_eq!(bridge.recorded, None);
    assert!(!bridge.read_only);
    assert!(bridge.resolved_paths.is_empty());

    // Restore the sources: the legacy recorded paths are allowed to serve a
    // reference only when no library is configured.
    std::fs::copy(
        library.resolve(&first_hash).expect("first entry"),
        &first_package,
    )
    .expect("restore first");
    std::fs::copy(
        library.resolve(&second_hash).expect("second entry"),
        &second_package,
    )
    .expect("restore second");
    let fallback = bridge
        .load(&bound.json, &base_active(), true, None)
        .expect("legacy path fallback");
    assert!(fallback.read_only);
    assert_eq!(fallback.loaded.resolved_references, 0);
    assert_eq!(fallback.loaded.legacy_path_fallbacks, 3);
    assert_eq!(fallback.loaded.solid_by_id.len(), 3);
    assert!(bridge.resolved_paths.is_empty());
    let post_fallback = bridge.store.checksum();

    // A configured library must satisfy the reference; it never falls back to
    // the recorded path when the entry is missing.
    let empty = BlueprintLibrary::new(workspace.library("empty library")).expect("empty library");
    expect_code(
        "missing referenced entry",
        load_save(&bound.json, &pack_active(), false, Some(&empty)),
        "blueprint_library_missing_file",
    );
    assert!(first_package.exists());
    assert!(second_package.exists());

    // Corrupt reference bytes are rejected by the record validator before any
    // package is opened.
    let mut corrupt = value(&bound.json);
    corrupt["placements"][0]["blueprint_ref"] = serde_json::json!("tsbp:blake3:not-a-content-hash");
    let corrupt_json = serde_json::to_string_pretty(&corrupt).expect("corrupt JSON");
    expect_code(
        "corrupt reference JSON",
        load_save(&corrupt_json, &pack_active(), false, Some(&library)),
        "invalid_placements",
    );
    let mut truncated = value(&bound.json);
    truncated["placements"][1]["blueprint_ref"] =
        serde_json::json!(format!("tsbp:blake3:{}", "a".repeat(63)));
    expect_code(
        "wrong reference hash length",
        load_save(
            &serde_json::to_string_pretty(&truncated).expect("truncated JSON"),
            &pack_active(),
            false,
            Some(&library),
        ),
        "invalid_placements",
    );
    assert_eq!(bridge.store.checksum(), post_fallback);

    // The pure resolver keeps the distinct in-memory codes.
    stamp_world_blueprint_references(&mut store);
    let mut malformed = store.clone();
    malformed.placements[0].blueprint_ref = Some("blake3:not-a-reference".to_string());
    expect_code(
        "malformed in-memory reference",
        load_world_placement_packages(&malformed, Some(&library)),
        "invalid_blueprint_reference",
    );
    malformed.placements[0].blueprint_ref = Some(format!("tsbp:blake3:{}", "0".repeat(64)));
    expect_code(
        "mismatched in-memory reference",
        load_world_placement_packages(&malformed, Some(&library)),
        "blueprint_hash_mismatch",
    );
    assert_eq!(bridge.store.checksum(), post_fallback);
}

#[test]
fn settlement_layer_round_trips_with_and_without_placements_and_stays_path_free() {
    let workspace = TempWorkspace::new("settlements");
    let package = workspace.package("house.tsbp");
    let hash = write_package(&package, "local:house", "Settlement House", EDITOR_FLOOR);
    let mut store = PlacementStore::new(SEED);
    place(
        &mut store,
        &package,
        &hash,
        "local:house",
        "Settlement House",
        100.0,
    );
    let construction = ConstructionStore::default();
    let settlements = settlement_layer();
    let settlement_checksum = settlements.checksum();
    assert_eq!(settlements.count(), 2);

    // Absent settlement layer stays absent and adds no key.
    let absent = compose_save(&store, &construction, None, None, &base_active(), None)
        .expect("absent settlement save");
    assert!(value(&absent.json).get("settlements").is_none());
    let absent_loaded =
        load_save(&absent.json, &base_active(), false, None).expect("absent settlement load");
    assert!(absent_loaded.settlements.is_none());

    // Present with placements, both plain v1 and content-bound v2.
    let plain = compose_save(
        &store,
        &construction,
        Some(&settlements),
        None,
        &base_active(),
        None,
    )
    .expect("plain settlement save");
    let bound = compose_save(
        &store,
        &construction,
        Some(&settlements),
        None,
        &pack_active(),
        None,
    )
    .expect("bound settlement save");
    for composed in [&plain, &bound] {
        let envelope = value(&composed.json);
        let layer = envelope["settlements"]
            .as_object()
            .expect("settlement layer");
        assert_eq!(layer["seed"], SEED);
        assert_eq!(layer["settlements"].as_array().expect("records").len(), 2);
        let layer_text = serde_json::to_string(layer).expect("layer text");
        assert!(!layer_text.contains(workspace.root().to_string_lossy().as_ref()));
        assert!(!layer_text.contains("resolved_blueprint_path"));
        assert!(!composed.json.contains("resolved_blueprint_path"));
        assert!(!composed.json.contains("library_root"));
    }
    let replay = compose_save(
        &store,
        &construction,
        Some(&settlements),
        None,
        &base_active(),
        None,
    )
    .expect("plain settlement replay");
    assert_eq!(
        plain.json, replay.json,
        "settlements serialize deterministically"
    );

    let plain_loaded =
        load_save(&plain.json, &base_active(), false, None).expect("plain settlement load");
    assert_eq!(plain_loaded.store, store);
    assert_eq!(plain_loaded.settlements.as_ref(), Some(&settlements));
    assert_eq!(
        plain_loaded
            .settlements
            .as_ref()
            .expect("settlements")
            .checksum(),
        settlement_checksum
    );
    assert_eq!(plain_loaded.loaded.solid_by_id.get(&1), Some(&16));

    let bound_loaded =
        load_save(&bound.json, &pack_active(), false, None).expect("bound settlement load");
    assert_eq!(bound_loaded.status, ContentIdentityStatus::Match);
    assert_eq!(bound_loaded.settlements.as_ref(), Some(&settlements));
    assert_eq!(
        bound_loaded
            .settlements
            .as_ref()
            .expect("settlements")
            .checksum(),
        settlement_checksum
    );

    // Malformed settlements fail with the layer code and no commit.
    let mut malformed = value(&plain.json);
    malformed["settlements"]["format_version"] = serde_json::json!(99);
    let malformed_json = serde_json::to_string_pretty(&malformed).expect("malformed JSON");
    let mut bridge = SimulatedBridge::with_world(store.clone(), construction.clone());
    bridge.settlements = Some(settlements.clone());
    let store_before = bridge.store.checksum();
    let settlements_before = bridge.settlements.as_ref().map(SettlementStore::checksum);
    expect_code(
        "unsupported settlement version",
        bridge.load(&malformed_json, &base_active(), false, None),
        "invalid_settlements",
    );
    assert_eq!(bridge.store.checksum(), store_before);
    assert_eq!(
        bridge.settlements.as_ref().map(SettlementStore::checksum),
        settlements_before
    );
}

#[test]
fn three_save_load_cycles_preserve_refs_content_and_settlements() {
    let workspace = TempWorkspace::new("three-cycles");
    let package = workspace.package("house.tsbp");
    let hash = write_package(&package, "local:house", "Cycle House", EDITOR_FLOOR);
    let mut store = PlacementStore::new(SEED);
    place(
        &mut store,
        &package,
        &hash,
        "local:house",
        "Cycle House",
        100.0,
    );
    let construction = ConstructionStore::default();
    let settlements = settlement_layer();
    let library = BlueprintLibrary::new(workspace.library("library")).expect("library");

    let mut bridge = SimulatedBridge::with_world(store.clone(), construction.clone());
    bridge.settlements = Some(settlements.clone());

    let mut written: Vec<Vec<u8>> = Vec::new();
    for cycle in 0..3 {
        let path = workspace.save(&format!("cycle-{cycle}.json"));
        let saved = bridge
            .save(&path, &pack_active(), Some(&library))
            .expect("cycle save");
        assert_eq!(
            saved.content_status,
            if cycle == 0 { "new" } else { "match" },
            "cycle {cycle}: identity status"
        );
        assert_eq!(saved.identity, Some(recorded_identity()));
        assert_eq!(placement_refs(&saved.json).len(), 1);
        if cycle == 0 {
            // Only the first save has a source package to copy.
            std::fs::remove_file(&package).expect("remove source");
        }
        let bytes = std::fs::read(&path).expect("read cycle save");
        if let Some(first) = written.first() {
            assert_eq!(first, &bytes, "cycle {cycle}: save bytes drifted");
        }
        let loaded = bridge
            .load(
                std::str::from_utf8(&bytes).expect("cycle utf8"),
                &pack_active(),
                false,
                Some(&library),
            )
            .expect("cycle load");
        assert_eq!(loaded.status, ContentIdentityStatus::Match);
        assert!(!loaded.read_only);
        assert_eq!(loaded.format_version, recorded_identity().format_version);
        assert_eq!(loaded.identity, Some(recorded_identity()));
        assert_eq!(loaded.loaded.resolved_references, 1);
        assert_eq!(loaded.loaded.legacy_path_fallbacks, 0);
        assert_eq!(loaded.store, saved.saved_store);
        assert_eq!(loaded.settlements.as_ref(), Some(&settlements));
        assert_eq!(
            loaded.settlements.as_ref().expect("settlements").checksum(),
            settlements.checksum()
        );
        assert_eq!(bridge.recorded, Some(recorded_identity()));
        assert_eq!(bridge.settlements.as_ref(), Some(&settlements));
        assert_eq!(
            std::fs::read(&path).expect("read after load"),
            bytes,
            "cycle {cycle}: load must not rewrite the save"
        );
        written.push(bytes);
    }
    assert_eq!(written.len(), 3);
    assert_eq!(written[0], written[1]);
    assert_eq!(written[1], written[2]);
}

#[test]
fn v1_stays_byte_identical_and_v2_keeps_its_identity_across_save_load() {
    let workspace = TempWorkspace::new("migration");
    let package = workspace.package("house.tsbp");
    let hash = write_package(&package, "local:house", "Migration House", EDITOR_FLOOR);
    let mut store = PlacementStore::new(SEED);
    place(
        &mut store,
        &package,
        &hash,
        "local:house",
        "Migration House",
        100.0,
    );
    let construction = ConstructionStore::default();
    let settlements = settlement_layer();

    // Base-only: v1 in, v1 out, no identity, byte-identical re-save.
    let mut bridge = SimulatedBridge::with_world(store.clone(), construction.clone());
    bridge.settlements = Some(settlements.clone());
    let v1_path = workspace.save("v1-first.json");
    let v1_second = workspace.save("v1-second.json");
    let v1 = bridge
        .save(&v1_path, &base_active(), None)
        .expect("v1 save");
    assert_eq!(v1.content_status, "absent");
    assert!(!v1.content_bound);
    assert_eq!(value(&v1.json)["format_version"], 1);
    assert!(value(&v1.json).get("content").is_none());
    let v1_before = std::fs::read(&v1_path).expect("read v1");
    let v1_loaded = bridge
        .load(
            std::str::from_utf8(&v1_before).expect("v1 utf8"),
            &base_active(),
            false,
            None,
        )
        .expect("v1 load");
    assert_eq!(v1_loaded.status, ContentIdentityStatus::Absent);
    assert_eq!(v1_loaded.format_version, 0);
    assert_eq!(v1_loaded.settlements.as_ref(), Some(&settlements));
    assert!(!bridge.read_only);
    bridge
        .save(&v1_second, &base_active(), None)
        .expect("v1 re-save");
    assert_eq!(
        std::fs::read(&v1_second).expect("read v1 re-save"),
        v1_before,
        "base-only saves stay v1 byte for byte"
    );

    // Content-bound: v2 in, v2 out, identity retained, byte-identical re-save.
    let v2_path = workspace.save("v2-first.json");
    let v2_second = workspace.save("v2-second.json");
    let v2 = bridge
        .save(&v2_path, &pack_active(), None)
        .expect("v2 save");
    assert!(v2.content_bound);
    assert_eq!(v2.content_status, "new");
    assert_eq!(v2.identity, Some(recorded_identity()));
    let v2_before = std::fs::read(&v2_path).expect("read v2");
    let v2_loaded = bridge
        .load(
            std::str::from_utf8(&v2_before).expect("v2 utf8"),
            &pack_active(),
            false,
            None,
        )
        .expect("v2 load");
    assert_eq!(v2_loaded.status, ContentIdentityStatus::Match);
    assert_eq!(v2_loaded.format_version, recorded_identity().format_version);
    assert_eq!(v2_loaded.identity, Some(recorded_identity()));
    assert_eq!(v2_loaded.settlements.as_ref(), Some(&settlements));
    assert!(!bridge.read_only);
    let v2_again = bridge
        .save(&v2_second, &pack_active(), None)
        .expect("v2 re-save");
    assert_eq!(v2_again.content_status, "match");
    assert_eq!(v2_again.identity, Some(recorded_identity()));
    assert_eq!(
        std::fs::read(&v2_second).expect("read v2 re-save"),
        v2_before,
        "content-bound saves stay v2 with the same identity"
    );

    // There is no silent downgrade: base content cannot save over the binding
    // and the v2 file on disk is untouched.
    let blocked = workspace.save("blocked-v1.json");
    expect_code(
        "v2 downgrade",
        bridge.save(&blocked, &base_active(), None),
        "content_save_requires_pack",
    );
    assert!(!blocked.exists());
    assert_eq!(
        std::fs::read(&v2_second).expect("read v2 after refusal"),
        v2_before
    );
    assert_eq!(bridge.recorded, Some(recorded_identity()));
    assert!(!bridge.read_only);
    assert_eq!(bridge.settlements.as_ref(), Some(&settlements));
}
