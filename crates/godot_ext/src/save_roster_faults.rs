//! Local player-roster × world-save/load composition matrix.
//!
//! `world_save*`/`world_load*` are `#[func]` methods and need a Godot runtime,
//! so this module drives the same pure helpers in the production order and
//! keeps the bridge-local [`PlayerRegistry`] beside the simulated stores. The
//! point is the seam, not a second copy of the placement matrix:
//!
//! ```text
//! save: stamp refs (library only) -> placement JSON -> construction ->
//!       settlements -> stamp content identity -> sync library -> write
//! load: read JSON -> verify content identity -> normalize v2 to v1 ->
//!       parse store/construction/settlements -> resolve packages -> commit
//! ```
//!
//! The roster is deliberately *not* an argument to save or load. Production
//! only consults it in `local_players()` (which follows the open world seed),
//! never in `world_save_impl`/`world_load_impl`. These tests pin that:
//!
//! - 0, 1, and the 256-player limit produce the same world bytes;
//! - plain v1 and content-bound v2 saves never carry a roster or its checksum;
//! - a load never hydrates, resets, or otherwise mutates the local roster;
//! - settlements keep their own player ids even when the local roster is empty;
//! - three cycles preserve placements, construction, settlements, and
//!   checksums without accumulating optional fields;
//! - corrupt/missing libraries, corrupt references, content mismatches, and
//!   the read-only refusal fail closed and leave the roster untouched.
//!
//! Package paths stay legacy save values (D-027). Only references, content
//! identity, and the roster are required to be path-free.

use super::*;
use crate::content_identity::{ContentIdentityStatus, CONTENT_IDENTITY_FORMAT_VERSION};
use core_city::player::MAX_PLAYERS;
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
            "townsmith-save-roster-{}-{}-{label}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create roster workspace");
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

/// Deterministic 4 × 1 × 4 package with 16 solid voxels.
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

/// Non-empty construction so the save/load path actually carries a site.
fn construction_layer() -> ConstructionStore {
    let mut construction = ConstructionStore::new(2, 640);
    construction
        .add_priced_site(1, 16, 240.0)
        .expect("construction site");
    construction
}

/// Unowned, owned-with-members-and-tax, and owned-without-extra-fields.
///
/// The ids are world data, independent of the bridge-local roster. An empty
/// local roster must still round-trip these references unchanged.
fn settlement_layer() -> SettlementStore {
    let mut settlements = SettlementStore::new(SEED);
    settlements
        .found(&SettlementRequest {
            name: "Riverbend".to_string(),
            owner: "legacy-label".to_string(),
            center: [10, 20],
            radius_m: 64,
        })
        .expect("unowned settlement");
    let owner = PlayerId::new(1).expect("owner id");
    let northwatch = settlements
        .found_for_player(
            &SettlementRequest {
                name: "Northwatch".to_string(),
                owner: "Northwatch".to_string(),
                center: [400, -120],
                radius_m: 48,
            },
            owner,
        )
        .expect("owned settlement");
    settlements
        .invite(
            owner,
            northwatch.id,
            PlayerId::new(2).expect("manager id"),
            SettlementRole::Manager,
            northwatch.membership_revision,
        )
        .expect("manager invite");
    let northwatch_id = northwatch.id;
    let northwatch_revision = settlements
        .get(northwatch_id)
        .expect("northwatch")
        .membership_revision;
    settlements
        .invite(
            owner,
            northwatch_id,
            PlayerId::new(3).expect("member id"),
            SettlementRole::Member,
            northwatch_revision,
        )
        .expect("member invite");
    settlements
        .set_tax_rate(owner, northwatch_id, 12)
        .expect("tax rate");
    settlements
        .found_for_player(
            &SettlementRequest {
                name: "Southgate".to_string(),
                owner: "Southgate".to_string(),
                center: [-300, 500],
                radius_m: 32,
            },
            PlayerId::new(4).expect("second owner"),
        )
        .expect("owned settlement without extras");
    settlements
}

/// Deterministic local roster of `size` players. The bridge limit is 256.
fn roster_for(size: usize) -> PlayerRegistry {
    let mut registry = PlayerRegistry::new(SEED);
    for index in 0..size {
        let name = format!("Player {index:03}");
        players_register_in_store(&mut registry, &name).expect("register roster player");
    }
    registry
}

/// The world save must not contain the roster under any bridge spelling.
fn assert_roster_is_local(json: &str, roster: &PlayerRegistry) {
    let envelope = value(json);
    let object = envelope.as_object().expect("world save object");
    for key in ["players", "player_registry", "roster", "local_players"] {
        assert!(
            !object.contains_key(key),
            "world save unexpectedly carries top-level {key}"
        );
    }
    assert!(
        !json.contains(&format!("{:016x}", roster.checksum())),
        "world save leaked the roster checksum"
    );
    for player in roster.players() {
        assert!(
            !json.contains(&player.display_name),
            "world save leaked roster name {}",
            player.display_name
        );
    }
}

struct ComposedSave {
    json: String,
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

/// Bridge-like state with the local roster carried alongside, never inside,
/// the world stores.
struct RosterBridge {
    store: PlacementStore,
    construction: ConstructionStore,
    settlements: Option<SettlementStore>,
    roster: PlayerRegistry,
    recorded: Option<WorldContentIdentity>,
    read_only: bool,
    resolved_paths: BTreeMap<String, String>,
}

impl RosterBridge {
    fn with_world(
        store: PlacementStore,
        construction: ConstructionStore,
        settlements: Option<SettlementStore>,
        roster: PlayerRegistry,
    ) -> Self {
        Self {
            store,
            construction,
            settlements,
            roster,
            recorded: None,
            read_only: false,
            resolved_paths: BTreeMap::new(),
        }
    }

    fn roster_snapshot(&self) -> BridgePlayersSnapshot {
        players_snapshot(&self.roster)
    }

    /// Only the world stores move; the roster has no save/load argument.
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

    /// Read-only refusal, composition, durable write, then store commit.
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

fn settlement_layer_value(json: &str) -> serde_json::Value {
    value(json)["settlements"].clone()
}

fn sample_world(workspace: &TempWorkspace) -> (PlacementStore, ConstructionStore, SettlementStore) {
    let package = workspace.package("roster-house.tsbp");
    let hash = write_package(&package, "local:roster_house", "Roster House", EDITOR_FLOOR);
    let mut store = PlacementStore::new(SEED);
    place(
        &mut store,
        &package,
        &hash,
        "local:roster_house",
        "Roster House",
        100.0,
    );
    (store, construction_layer(), settlement_layer())
}

#[test]
fn roster_sizes_do_not_change_the_plain_world_save() {
    let workspace = TempWorkspace::new("plain-size-matrix");
    let (store, construction, settlements) = sample_world(&workspace);
    let mut baseline: Option<String> = None;
    let mut rosters = Vec::new();

    for size in [0, 1, MAX_PLAYERS] {
        let roster = roster_for(size);
        assert_eq!(roster.count(), size);
        let bridge = RosterBridge::with_world(
            store.clone(),
            construction.clone(),
            Some(settlements.clone()),
            roster.clone(),
        );
        let composed = compose_save(
            &bridge.store,
            &bridge.construction,
            bridge.settlements.as_ref(),
            None,
            &base_active(),
            None,
        )
        .expect("plain save composes");

        assert_eq!(composed.content_status, "absent");
        assert!(!composed.content_bound);
        assert!(composed.identity.is_none());
        assert!(composed.library_sync.is_none());
        assert_eq!(value(&composed.json)["format_version"], 1);
        assert!(value(&composed.json).get("content").is_none());
        assert_roster_is_local(&composed.json, &bridge.roster);
        assert_eq!(bridge.roster_snapshot(), players_snapshot(&roster));
        if let Some(expected) = baseline.as_ref() {
            assert_eq!(&composed.json, expected, "roster size {size} changed bytes");
        } else {
            baseline = Some(composed.json);
        }
        rosters.push(roster);
    }

    let mut full = rosters.pop().expect("full roster");
    let (code, _) = players_register_in_store(&mut full, "Overflow").expect_err("257th player");
    assert_eq!(code, "player.limit");
    assert_eq!(full.count(), MAX_PLAYERS);
    assert_eq!(rosters[0].checksum(), PlayerRegistry::new(SEED).checksum());
}

#[test]
fn plain_round_trip_preserves_layers_for_each_roster_size() {
    let workspace = TempWorkspace::new("plain-round-trip-matrix");
    let (store, construction, settlements) = sample_world(&workspace);

    for size in [0, 1, MAX_PLAYERS] {
        let roster = roster_for(size);
        let roster_before = players_snapshot(&roster);
        let mut bridge = RosterBridge::with_world(
            store.clone(),
            construction.clone(),
            Some(settlements.clone()),
            roster,
        );
        let path = workspace.save(&format!("plain-{size}.json"));
        let saved = bridge
            .save(&path, &base_active(), None)
            .expect("plain save");
        assert_eq!(saved.content_status, "absent");
        assert_eq!(bridge.roster_snapshot(), roster_before, "save moved roster");

        let bytes = std::fs::read(&path).expect("read save");
        let loaded = bridge
            .load(
                std::str::from_utf8(&bytes).expect("save utf8"),
                &base_active(),
                false,
                None,
            )
            .expect("plain load");
        assert_eq!(loaded.status, ContentIdentityStatus::Absent);
        assert_eq!(loaded.format_version, 0);
        assert!(loaded.identity.is_none());
        assert!(!loaded.read_only);
        assert_eq!(loaded.store, store);
        assert_eq!(loaded.store.checksum(), store.checksum());
        assert_eq!(loaded.construction.checksum(), construction.checksum());
        assert_eq!(loaded.construction.site_count(), 1);
        assert_eq!(
            loaded.settlements.as_ref().expect("settlements").checksum(),
            settlements.checksum()
        );
        assert_eq!(loaded.settlements.as_ref(), Some(&settlements));
        assert_eq!(loaded.loaded.solid_by_id.get(&1), Some(&16));
        assert_eq!(loaded.loaded.resolved_references, 0);
        assert_eq!(loaded.loaded.legacy_path_fallbacks, 0);
        assert!(bridge.resolved_paths.is_empty());
        assert_eq!(bridge.store.checksum(), store.checksum());
        assert_eq!(
            bridge.construction.checksum(),
            construction.checksum(),
            "load lost construction for roster size {size}"
        );
        assert_eq!(bridge.roster_snapshot(), roster_before, "load moved roster");
        assert_eq!(
            std::fs::read(&path).expect("read after load"),
            bytes,
            "load rewrote the save"
        );
    }
}

#[test]
fn full_local_roster_is_never_written_and_reload_keeps_it_unchanged() {
    let workspace = TempWorkspace::new("full-roster-local");
    let (store, construction, settlements) = sample_world(&workspace);
    let roster = roster_for(MAX_PLAYERS);
    let roster_before = players_snapshot(&roster);
    let roster_json = roster.to_json_pretty().expect("roster JSON");
    let mut bridge = RosterBridge::with_world(
        store.clone(),
        construction,
        Some(settlements),
        roster.clone(),
    );

    let path = workspace.save("full-roster.json");
    let saved = bridge
        .save(&path, &base_active(), None)
        .expect("full-roster save");
    assert_roster_is_local(&saved.json, &roster);
    assert!(!saved.json.contains("local_players()"));
    assert_eq!(bridge.roster_snapshot(), roster_before);

    let loaded = bridge
        .load(&saved.json, &base_active(), false, None)
        .expect("full-roster load");
    assert_eq!(loaded.store, store);
    assert_eq!(bridge.roster, roster);
    assert_eq!(bridge.roster_snapshot(), roster_before);

    // The local roster's own persistence is independent of the world save.
    let mut restored = PlayerRegistry::from_json(&roster_json).expect("restore roster");
    assert_eq!(restored, roster);
    let first = restored.players()[0].id;
    restored.rename(first, "Renamed Local").expect("rename");
    assert_eq!(bridge.roster, roster, "restored roster is not independent");
    assert_eq!(bridge.roster.players()[0].display_name, "Player 000");
}

#[test]
fn library_save_resolves_references_for_each_roster_size() {
    let workspace = TempWorkspace::new("library-size-matrix");
    let (store, construction, settlements) = sample_world(&workspace);
    let library = BlueprintLibrary::new(workspace.library("library")).expect("library");
    let mut baseline: Option<String> = None;

    for size in [0, 1, MAX_PLAYERS] {
        let roster = roster_for(size);
        let roster_before = players_snapshot(&roster);
        let mut bridge = RosterBridge::with_world(
            store.clone(),
            construction.clone(),
            Some(settlements.clone()),
            roster.clone(),
        );
        let path = workspace.save(&format!("library-{size}.json"));
        let saved = bridge
            .save(&path, &base_active(), Some(&library))
            .expect("library save");

        assert_eq!(saved.library_sync.as_ref().expect("sync").unique, 1);
        assert_eq!(saved.content_status, "absent");
        assert_eq!(placement_refs(&saved.json).len(), 1);
        assert!(
            placement_refs(&saved.json)[0].starts_with("tsbp:blake3:"),
            "reference is not canonical"
        );
        assert!(!saved.json.contains("library_root"));
        assert!(!saved.json.contains("resolved_blueprint_path"));
        assert_roster_is_local(&saved.json, &roster);
        assert_eq!(bridge.roster_snapshot(), roster_before);
        if let Some(expected) = baseline.as_ref() {
            assert_eq!(
                &saved.json, expected,
                "roster size {size} changed library bytes"
            );
        } else {
            baseline = Some(saved.json.clone());
        }

        let loaded = bridge
            .load(&saved.json, &base_active(), false, Some(&library))
            .expect("library load");
        assert_eq!(loaded.loaded.resolved_references, 1);
        assert_eq!(loaded.loaded.legacy_path_fallbacks, 0);
        assert_eq!(loaded.store, saved.saved_store);
        assert_eq!(loaded.loaded.solid_by_id.get(&1), Some(&16));
        assert_eq!(
            loaded.settlements.as_ref().expect("settlements").checksum(),
            settlements.checksum()
        );
        assert_eq!(bridge.roster, roster);
        assert_eq!(bridge.roster_snapshot(), roster_before);
        assert_eq!(bridge.resolved_paths.len(), 1);
    }
}

#[test]
fn library_failures_leave_the_roster_untouched() {
    let workspace = TempWorkspace::new("library-failures");
    let (store, construction, settlements) = sample_world(&workspace);
    let library = BlueprintLibrary::new(workspace.library("library")).expect("library");
    let roster = roster_for(1);
    let roster_before = players_snapshot(&roster);
    let mut bridge = RosterBridge::with_world(
        store,
        construction,
        Some(settlements.clone()),
        roster.clone(),
    );
    let path = workspace.save("portable.json");
    let saved = bridge
        .save(&path, &base_active(), Some(&library))
        .expect("portable save");
    let hash = saved.saved_store.placements[0].blueprint_hash.clone();
    let source = PathBuf::from(&saved.saved_store.placements[0].blueprint_path);
    assert!(source.exists());

    // Reference-first resolution ignores a working recorded path.
    let first = bridge
        .load(&saved.json, &base_active(), false, Some(&library))
        .expect("reference-first load");
    assert_eq!(first.loaded.resolved_references, 1);
    assert_eq!(first.loaded.legacy_path_fallbacks, 0);

    // Without a library the still-working legacy path is the fallback.
    let fallback = bridge
        .load(&saved.json, &base_active(), false, None)
        .expect("legacy fallback");
    assert_eq!(fallback.loaded.resolved_references, 0);
    assert_eq!(fallback.loaded.legacy_path_fallbacks, 1);
    assert_eq!(fallback.loaded.solid_by_id.get(&1), Some(&16));

    // Removing the source forces the library or a clean refusal.
    std::fs::remove_file(&source).expect("remove source");
    expect_code(
        "missing library and source",
        bridge.load(&saved.json, &base_active(), false, None),
        "blueprint_reference_requires_library",
    );
    let empty = BlueprintLibrary::new(workspace.library("empty")).expect("empty library");
    expect_code(
        "missing library entry",
        bridge.load(&saved.json, &base_active(), false, Some(&empty)),
        "blueprint_library_missing_file",
    );

    // Corrupting the stored package fails closed as an invalid package.
    let stored = library.resolve(&hash).expect("stored package");
    std::fs::write(&stored, b"not a zip").expect("corrupt stored package");
    expect_code(
        "corrupt stored package",
        bridge.load(&saved.json, &base_active(), false, Some(&library)),
        "blueprint_library_invalid_package",
    );

    // A missing library root is refused before any save/load work.
    let missing = workspace.root().join("absent-library");
    let (code, _) =
        blueprint_library_open(&missing.to_string_lossy()).expect_err("missing library root");
    assert_eq!(code, "blueprint_library_root_missing");

    assert_eq!(bridge.roster, roster);
    assert_eq!(bridge.roster_snapshot(), roster_before);
    assert_eq!(bridge.store.checksum(), saved.saved_store.checksum());
    assert_eq!(
        bridge.settlements.as_ref().map(SettlementStore::checksum),
        Some(settlements.checksum())
    );
    assert_eq!(
        std::fs::read(&path).expect("read portable save"),
        saved.json.as_bytes(),
        "failed loads must not rewrite the save"
    );
}

#[test]
fn content_bound_v2_keeps_roster_local_and_settlements_intact() {
    let workspace = TempWorkspace::new("content-bound-roster");
    let (store, construction, settlements) = sample_world(&workspace);
    let roster = roster_for(MAX_PLAYERS);
    let roster_before = players_snapshot(&roster);
    let mut bridge = RosterBridge::with_world(
        store.clone(),
        construction.clone(),
        Some(settlements.clone()),
        roster.clone(),
    );

    let path = workspace.save("content-v2.json");
    let saved = bridge
        .save(&path, &pack_active(), None)
        .expect("content-bound save");
    assert!(saved.content_bound);
    assert_eq!(saved.content_status, "new");
    assert_eq!(saved.identity, Some(recorded_identity()));
    let envelope = value(&saved.json);
    assert_eq!(envelope["format_version"], 2);
    assert_eq!(envelope["content"]["pack_id"], PACK_ID);
    assert_roster_is_local(&saved.json, &roster);

    let loaded = bridge
        .load(&saved.json, &pack_active(), false, None)
        .expect("content-bound load");
    assert_eq!(loaded.status, ContentIdentityStatus::Match);
    assert_eq!(loaded.format_version, CONTENT_IDENTITY_FORMAT_VERSION);
    assert_eq!(loaded.identity, Some(recorded_identity()));
    assert!(!loaded.read_only);
    assert_eq!(loaded.store, store);
    assert_eq!(loaded.construction.checksum(), construction.checksum());
    assert_eq!(
        loaded.settlements.as_ref().expect("settlements").checksum(),
        settlements.checksum()
    );
    assert_eq!(bridge.roster, roster);
    assert_eq!(bridge.roster_snapshot(), roster_before);
}

#[test]
fn three_save_load_cycles_preserve_checksums_and_roster() {
    let workspace = TempWorkspace::new("three-cycles-roster");
    let (store, construction, settlements) = sample_world(&workspace);
    let library = BlueprintLibrary::new(workspace.library("library")).expect("library");
    let roster = roster_for(MAX_PLAYERS);
    let roster_before = players_snapshot(&roster);
    let mut bridge = RosterBridge::with_world(
        store.clone(),
        construction.clone(),
        Some(settlements.clone()),
        roster.clone(),
    );
    let settlement_checksum = settlements.checksum();
    let settlement_value = settlement_layer_value(
        &compose_save(
            &store,
            &construction,
            Some(&settlements),
            None,
            &base_active(),
            None,
        )
        .expect("baseline")
        .json,
    );
    let mut first_bytes: Option<Vec<u8>> = None;

    for cycle in 0..3 {
        let path = workspace.save(&format!("cycle-{cycle}.json"));
        let saved = bridge
            .save(&path, &pack_active(), Some(&library))
            .expect("cycle save");
        assert_eq!(
            saved.content_status,
            if cycle == 0 { "new" } else { "match" }
        );
        assert_eq!(saved.identity, Some(recorded_identity()));
        assert_eq!(placement_refs(&saved.json).len(), 1);
        assert_roster_is_local(&saved.json, &roster);
        assert_eq!(bridge.roster_snapshot(), roster_before);
        assert_eq!(
            settlement_layer_value(&saved.json),
            settlement_value,
            "cycle {cycle}: settlement payload drifted"
        );

        let bytes = std::fs::read(&path).expect("read cycle save");
        if let Some(first) = first_bytes.as_ref() {
            assert_eq!(first, &bytes, "cycle {cycle}: save bytes drifted");
        } else {
            first_bytes = Some(bytes.clone());
            // From now on the library is the only package source.
            let source = PathBuf::from(&saved.saved_store.placements[0].blueprint_path);
            std::fs::remove_file(source).expect("remove source");
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
        assert_eq!(loaded.format_version, CONTENT_IDENTITY_FORMAT_VERSION);
        assert_eq!(loaded.identity, Some(recorded_identity()));
        assert_eq!(loaded.store.checksum(), saved.saved_store.checksum());
        assert_eq!(loaded.construction.checksum(), construction.checksum());
        assert_eq!(
            loaded.settlements.as_ref().expect("settlements").checksum(),
            settlement_checksum
        );
        assert_eq!(
            settlement_layer_value(&saved.json),
            settlement_layer_value(
                &compose_save(
                    &loaded.store,
                    &loaded.construction,
                    loaded.settlements.as_ref(),
                    Some(&library),
                    &pack_active(),
                    loaded.identity.as_ref(),
                )
                .expect("recomposed")
                .json
            ),
            "cycle {cycle}: optional fields accumulated"
        );
        assert_eq!(loaded.loaded.resolved_references, 1);
        assert_eq!(loaded.loaded.legacy_path_fallbacks, 0);
        assert_eq!(bridge.roster, roster);
        assert_eq!(bridge.roster_snapshot(), roster_before);
        assert_eq!(
            std::fs::read(&path).expect("read after load"),
            bytes,
            "cycle {cycle}: load rewrote the save"
        );
    }
    assert!(first_bytes.is_some());
}

#[test]
fn three_cycles_do_not_accumulate_optional_settlement_fields() {
    let workspace = TempWorkspace::new("settlement-optional-fields");
    let (store, construction, settlements) = sample_world(&workspace);
    let roster = roster_for(1);
    let mut bridge = RosterBridge::with_world(
        store,
        construction,
        Some(settlements.clone()),
        roster.clone(),
    );
    let path = workspace.save("optional.json");
    let saved = bridge
        .save(&path, &base_active(), None)
        .expect("optional save");
    let expected_layer = settlement_layer_value(&saved.json);
    let records = expected_layer["settlements"]
        .as_array()
        .expect("settlement records");

    let unowned = records
        .iter()
        .find(|record| record["id"] == 1)
        .expect("unowned record");
    let unowned_keys: BTreeSet<&str> = unowned
        .as_object()
        .expect("unowned object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        unowned_keys,
        BTreeSet::from(["center", "id", "name", "owner", "radius_m"]),
        "unowned settlement gained optional fields"
    );

    let members = records
        .iter()
        .find(|record| record["id"] == 2)
        .expect("member record");
    assert_eq!(members["owner_id"], 1);
    assert_eq!(members["tax_rate"], 12);
    assert_eq!(members["membership_revision"], 3);
    assert_eq!(
        members["members"].as_array().expect("members").len(),
        3,
        "owner + manager + member"
    );

    let simple = records
        .iter()
        .find(|record| record["id"] == 3)
        .expect("simple record");
    assert_eq!(simple["owner_id"], 4);
    assert_eq!(simple["members"].as_array().expect("owner member").len(), 1);
    assert!(
        simple.get("tax_rate").is_none(),
        "zero tax must stay serde-skipped"
    );

    for cycle in 0..3 {
        let next_path = workspace.save(&format!("optional-{cycle}.json"));
        let next = bridge
            .save(&next_path, &base_active(), None)
            .expect("cycle save");
        assert_eq!(
            settlement_layer_value(&next.json),
            expected_layer,
            "cycle {cycle}: settlement keys/values drifted"
        );
        let loaded = bridge
            .load(&next.json, &base_active(), false, None)
            .expect("cycle load");
        assert_eq!(
            loaded.settlements.as_ref().expect("settlements").checksum(),
            settlements.checksum()
        );
        assert_eq!(bridge.roster, roster);
    }
}

#[test]
fn empty_world_with_a_roster_is_byte_identical_v1_and_path_free() {
    let workspace = TempWorkspace::new("empty-world-roster");
    let store = PlacementStore::new(SEED);
    let construction = ConstructionStore::default();
    let expected = attach_construction(
        &store.to_json_pretty().expect("empty store JSON"),
        &construction,
    )
    .expect("legacy empty composition");
    let mut baseline: Option<String> = None;

    for size in [0, 1, MAX_PLAYERS] {
        let roster = roster_for(size);
        let bridge =
            RosterBridge::with_world(store.clone(), construction.clone(), None, roster.clone());
        let composed = compose_save(
            &bridge.store,
            &bridge.construction,
            None,
            None,
            &base_active(),
            None,
        )
        .expect("empty save");
        assert_eq!(composed.content_status, "absent");
        assert!(!composed.content_bound);
        assert_eq!(composed.json, expected);
        let envelope = value(&composed.json);
        assert_eq!(envelope["format_version"], 1);
        assert!(envelope.get("content").is_none());
        assert!(envelope.get("settlements").is_none());
        assert!(envelope.get("players").is_none());
        assert!(!composed
            .json
            .contains(workspace.root().to_string_lossy().as_ref()));
        assert!(!composed.json.contains("resolved_blueprint_path"));
        assert_roster_is_local(&composed.json, &roster);
        if let Some(first) = baseline.as_ref() {
            assert_eq!(
                &composed.json, first,
                "roster size {size} changed empty bytes"
            );
        } else {
            baseline = Some(composed.json);
        }
    }
}

#[test]
fn corrupt_reference_fails_closed_without_touching_the_roster() {
    let workspace = TempWorkspace::new("corrupt-reference-roster");
    let (store, construction, settlements) = sample_world(&workspace);
    let library = BlueprintLibrary::new(workspace.library("library")).expect("library");
    let roster = roster_for(1);
    let roster_before = players_snapshot(&roster);
    let mut bridge =
        RosterBridge::with_world(store, construction, Some(settlements), roster.clone());
    let path = workspace.save("corrupt-ref.json");
    let saved = bridge
        .save(&path, &base_active(), Some(&library))
        .expect("reference save");
    let store_before = bridge.store.checksum();
    let settlements_before = bridge.settlements.as_ref().map(SettlementStore::checksum);

    let mut malformed = value(&saved.json);
    malformed["placements"][0]["blueprint_ref"] =
        serde_json::json!("tsbp:blake3:not-a-content-hash");
    let malformed_json = serde_json::to_string_pretty(&malformed).expect("malformed JSON");
    expect_code(
        "corrupt reference",
        bridge.load(&malformed_json, &base_active(), false, Some(&library)),
        "invalid_placements",
    );

    let mut truncated = value(&saved.json);
    truncated["placements"][0]["blueprint_ref"] =
        serde_json::json!(format!("tsbp:blake3:{}", "a".repeat(63)));
    expect_code(
        "truncated reference",
        bridge.load(
            &serde_json::to_string_pretty(&truncated).expect("truncated JSON"),
            &base_active(),
            false,
            Some(&library),
        ),
        "invalid_placements",
    );

    let mut in_memory = saved.saved_store.clone();
    in_memory.placements[0].blueprint_ref = Some("blake3:not-a-reference".to_string());
    expect_code(
        "malformed in-memory reference",
        load_world_placement_packages(&in_memory, Some(&library)),
        "invalid_blueprint_reference",
    );

    assert_eq!(bridge.store.checksum(), store_before);
    assert_eq!(
        bridge.settlements.as_ref().map(SettlementStore::checksum),
        settlements_before
    );
    assert_eq!(bridge.recorded, None);
    assert!(!bridge.read_only);
    assert!(bridge.resolved_paths.is_empty());
    assert_eq!(bridge.roster, roster);
    assert_eq!(bridge.roster_snapshot(), roster_before);
    assert_eq!(
        std::fs::read(&path).expect("read after failures"),
        saved.json.as_bytes()
    );
}

#[test]
fn content_mismatch_and_read_only_refusal_keep_roster_local() {
    let workspace = TempWorkspace::new("content-mismatch-roster");
    let (store, construction, settlements) = sample_world(&workspace);
    let roster = roster_for(MAX_PLAYERS);
    let roster_before = players_snapshot(&roster);
    let mut bridge = RosterBridge::with_world(
        store.clone(),
        construction.clone(),
        Some(settlements.clone()),
        roster.clone(),
    );
    let path = workspace.save("bound.json");
    let saved = bridge
        .save(&path, &pack_active(), None)
        .expect("bound save");
    assert_eq!(saved.identity, Some(recorded_identity()));

    expect_code(
        "base content against v2",
        bridge.load(&saved.json, &base_active(), false, None),
        "content_pack_missing",
    );
    expect_code(
        "different pack against v2",
        bridge.load(&saved.json, &other_pack_active(), false, None),
        "content_pack_mismatch",
    );
    assert_eq!(bridge.roster, roster);
    assert_eq!(bridge.roster_snapshot(), roster_before);

    // Inspecting without the pack is allowed but permanently read-only.
    let inspected = bridge
        .load(&saved.json, &base_active(), true, None)
        .expect("inspecting load");
    assert_eq!(inspected.status, ContentIdentityStatus::Missing);
    assert!(inspected.read_only);
    assert_eq!(inspected.identity, Some(recorded_identity()));
    assert_eq!(inspected.store, store);
    assert_eq!(
        inspected
            .settlements
            .as_ref()
            .expect("settlements")
            .checksum(),
        settlements.checksum()
    );
    assert_eq!(bridge.roster, roster);
    assert_eq!(bridge.roster_snapshot(), roster_before);

    let blocked = workspace.save("blocked.json");
    expect_code(
        "read-only save",
        bridge.save(&blocked, &base_active(), None),
        "content_read_only",
    );
    assert!(!blocked.exists(), "read-only refusal wrote a file");
    expect_code(
        "read-only save with the exact pack",
        bridge.save(&blocked, &pack_active(), None),
        "content_read_only",
    );
    assert!(!blocked.exists());
    assert_eq!(bridge.recorded, Some(recorded_identity()));
    assert_eq!(bridge.roster, roster);
    assert_eq!(bridge.roster_snapshot(), roster_before);

    // Reloading the exact pack restores read-write and keeps the roster.
    let restored = bridge
        .load(&saved.json, &pack_active(), false, None)
        .expect("exact pack load");
    assert_eq!(restored.status, ContentIdentityStatus::Match);
    assert!(!restored.read_only);
    let resumed = workspace.save("resumed.json");
    let resaved = bridge
        .save(&resumed, &pack_active(), None)
        .expect("read-write save");
    assert_eq!(resaved.content_status, "match");
    assert_eq!(
        std::fs::read(&resumed).expect("read resumed"),
        saved.json.as_bytes(),
        "exact-pack reload changed bytes"
    );
    assert_eq!(bridge.roster, roster);
    assert_eq!(bridge.roster_snapshot(), roster_before);
}

#[test]
fn deterministic_json_and_checksums_across_independent_runs() {
    let workspace = TempWorkspace::new("deterministic-roster");
    let (store, construction, settlements) = sample_world(&workspace);

    let first_bridge = RosterBridge::with_world(
        store.clone(),
        construction.clone(),
        Some(settlements.clone()),
        roster_for(MAX_PLAYERS),
    );
    let second_bridge = RosterBridge::with_world(
        store.clone(),
        construction.clone(),
        Some(settlements.clone()),
        roster_for(MAX_PLAYERS),
    );
    let first = compose_save(
        &first_bridge.store,
        &first_bridge.construction,
        first_bridge.settlements.as_ref(),
        None,
        &base_active(),
        None,
    )
    .expect("first composition");
    let second = compose_save(
        &second_bridge.store,
        &second_bridge.construction,
        second_bridge.settlements.as_ref(),
        None,
        &base_active(),
        None,
    )
    .expect("second composition");
    assert_eq!(first.json, second.json);
    assert_eq!(first.saved_store.checksum(), second.saved_store.checksum());
    assert_eq!(settlements.checksum(), settlement_layer().checksum());
    assert_eq!(construction.checksum(), construction_layer().checksum());
    assert_eq!(first_bridge.roster, second_bridge.roster);
    assert_eq!(
        first_bridge.roster.to_json_pretty().expect("roster JSON"),
        second_bridge.roster.to_json_pretty().expect("roster JSON")
    );
    assert_eq!(
        players_snapshot(&first_bridge.roster),
        players_snapshot(&second_bridge.roster)
    );
    assert_eq!(
        format!("{:016x}", first_bridge.roster.checksum()),
        format!("{:016x}", second_bridge.roster.checksum())
    );
    assert_eq!(
        format!("{:016x}", first.saved_store.checksum()),
        format!("{:016x}", store.checksum())
    );
    assert_eq!(
        format!("{:016x}", settlements.checksum()),
        settlement_layer_value(&first.json)["settlements"]
            .as_array()
            .map_or_else(
                || format!("{:016x}", settlement_layer().checksum()),
                |_| format!("{:016x}", settlement_layer().checksum())
            )
    );
}

#[test]
fn reloaded_world_never_hydrates_or_mutates_the_local_roster() {
    let workspace = TempWorkspace::new("roster-hydration");
    let (store, construction, settlements) = sample_world(&workspace);

    // The settlement layer references players 1..=4 even with no local roster.
    let empty_roster = PlayerRegistry::new(SEED);
    assert!(empty_roster.is_empty());
    let mut empty_bridge = RosterBridge::with_world(
        store.clone(),
        construction.clone(),
        Some(settlements.clone()),
        empty_roster.clone(),
    );
    let empty_path = workspace.save("empty-roster.json");
    let saved = empty_bridge
        .save(&empty_path, &base_active(), None)
        .expect("empty roster save");
    assert_roster_is_local(&saved.json, &empty_roster);
    let loaded = empty_bridge
        .load(&saved.json, &base_active(), false, None)
        .expect("empty roster load");
    assert!(loaded.settlements.is_some());
    assert_eq!(loaded.settlements.as_ref().expect("settlements").count(), 3);
    assert!(
        empty_bridge.roster.is_empty(),
        "settlement owners/members hydrated the local roster"
    );
    assert_eq!(empty_bridge.roster.checksum(), empty_roster.checksum());

    // A full roster is also untouched; the world and roster persist separately.
    let full = roster_for(MAX_PLAYERS);
    let full_before = players_snapshot(&full);
    let mut full_bridge =
        RosterBridge::with_world(store, construction, Some(settlements), full.clone());
    let full_path = workspace.save("full-roster.json");
    let saved = full_bridge
        .save(&full_path, &base_active(), None)
        .expect("full roster save");
    full_bridge
        .load(&saved.json, &base_active(), false, None)
        .expect("full roster load");
    assert_eq!(full_bridge.roster, full);
    assert_eq!(full_bridge.roster_snapshot(), full_before);

    // The registry is seed-scoped: the same seed is a no-op, a different seed
    // drops names and restarts ids. The original snapshot stays independent.
    assert!(!player_registry_sync_seed(&mut full_bridge.roster, SEED));
    assert_eq!(full_bridge.roster, full);
    let preserved = full_bridge.roster.clone();
    assert!(player_registry_sync_seed(&mut full_bridge.roster, SEED + 1));
    assert!(full_bridge.roster.is_empty());
    assert_eq!(
        full_bridge.roster.checksum(),
        PlayerRegistry::new(SEED + 1).checksum()
    );
    assert_eq!(players_snapshot(&preserved), full_before);
    assert_ne!(preserved.checksum(), full_bridge.roster.checksum());
}
