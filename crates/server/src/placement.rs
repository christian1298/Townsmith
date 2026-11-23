//! Deterministic end-to-end smoke for authoritative blueprint placement.
//!
//! The smoke builds a tiny valid `.tsbp`, inserts it into a temporary
//! content-addressed library, configures a same-seed district policy registry,
//! and drives one [`crate::world::ServerWorld`] through every v1 outcome:
//! accepted placement, idempotent duplicate, policy denial, overlap,
//! insufficient funds, save, and load.
//!
//! It uses no network, no wall clock, and no fixed absolute paths: both temp
//! directories live under [`std::env::temp_dir`] with a unique name and are
//! removed on drop.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use core_blueprint::{save_tsbp_with_hash, BlueprintLibrary, BlueprintMetadata};
use core_city::policy::{DistrictPolicyRequest, DistrictPolicyStore};
use core_voxel::volume::{VolumeConfig, VolumePos, VoxelVolume};

use crate::protocol::{ClientCommand, ServerMessage, PROTOCOL_VERSION};
use crate::world::{ClientRole, ServerWorld};

/// Seed used by the deterministic placement smoke.
pub const PLACEMENT_SMOKE_SEED: u64 = 2026;

/// Blueprint id used by the deterministic placement smoke.
pub const PLACEMENT_SMOKE_BLUEPRINT_ID: &str = "local:server_smoke_house";

/// Blueprint material cost used by the deterministic placement smoke.
pub const PLACEMENT_SMOKE_COST: f64 = 1_000.0;

/// Centre of the accepted smoke placement.
pub const PLACEMENT_SMOKE_CENTER: [f64; 2] = [0.0, 0.0];

/// Centre inside the denying smoke policy.
pub const PLACEMENT_SMOKE_DENIED_CENTER: [f64; 2] = [256.0, 0.0];

/// Centre that overlaps the accepted smoke placement.
pub const PLACEMENT_SMOKE_OVERLAP_CENTER: [f64; 2] = [5.0, 0.0];

/// Centre used by the insufficient-funds check; clear of the first placement.
pub const PLACEMENT_SMOKE_FUNDS_CENTER: [f64; 2] = [64.0, 0.0];

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Outcome of [`run_placement_smoke`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementSmokeReport {
    /// One accepted authoritative placement.
    pub placed: bool,
    /// Repeating the request returned the same id without a charge.
    pub duplicate: bool,
    /// The denying policy blocked a placement without mutating state.
    pub policy: bool,
    /// An unaffordable installment was rejected without mutating state.
    pub funds: bool,
    /// An overlapping footprint was rejected without mutating state.
    pub overlap: bool,
    /// The world saved with a placement and construction site.
    pub save: bool,
    /// The save reloaded with the same checksum and stores.
    pub reload: bool,
    /// Final world checksum after the reload.
    pub checksum: String,
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Result<Self, String> {
        let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "townsmith-server-placement-{label}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path).map_err(|error| error.to_string())?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn hello() -> ClientCommand {
    ClientCommand::Hello {
        protocol_version: PROTOCOL_VERSION,
        content_hash: "0123456789abcdef".repeat(4),
        player_name: "mayor".to_string(),
    }
}

fn place(hash: &str, center: [f64; 2]) -> ClientCommand {
    ClientCommand::PlaceBlueprint {
        blueprint_hash: hash.to_string(),
        x: center[0],
        z: center[1],
        rotation: 0,
    }
}

fn rejection_code(message: &ServerMessage) -> Option<&str> {
    match message {
        ServerMessage::Reject { code, .. } => Some(code.as_str()),
        _ => None,
    }
}

fn placement_result(message: &ServerMessage) -> Option<(u64, String)> {
    match message {
        ServerMessage::PlaceBlueprintResult {
            placement_id,
            world_checksum,
            ..
        } => Some((*placement_id, world_checksum.clone())),
        _ => None,
    }
}

/// Write the tiny smoke package and insert it into `library_root`.
fn seed_library(library_root: &Path) -> Result<String, String> {
    let mut volume = VoxelVolume::new(
        VolumeConfig::new(2, 1, 2).map_err(|error| format!("volume config: {error}"))?,
    )
    .map_err(|error| format!("volume: {error}"))?;
    volume
        .fill_box(VolumePos::new(0, 0, 0), VolumePos::new(4, 1, 4), 1)
        .map_err(|error| format!("fill: {error}"))?;

    let mut metadata = BlueprintMetadata::new(PLACEMENT_SMOKE_BLUEPRINT_ID, "Server Smoke House");
    metadata.stats.resident_capacity = 2;
    metadata.stats.job_slots = 1;
    metadata.stats.material_cost = PLACEMENT_SMOKE_COST;
    let blueprint = core_blueprint::Blueprint::new(metadata, volume);

    let package = library_root.join("server_smoke_house.tsbp");
    let hash = save_tsbp_with_hash(&package, &blueprint)
        .map_err(|error| format!("save package: {error}"))?;
    let library =
        BlueprintLibrary::new(library_root).map_err(|error| format!("open library: {error}"))?;
    library
        .insert_file(&package)
        .map_err(|error| format!("insert package: {error}"))?;
    Ok(hash)
}

fn smoke_policies() -> Result<DistrictPolicyStore, String> {
    let mut store = DistrictPolicyStore::new(PLACEMENT_SMOKE_SEED);
    let mut allowed = DistrictPolicyRequest {
        name: "Central".to_string(),
        center: [0.0, 0.0],
        radius_m: 128.0,
        tax_multiplier: 1.0,
        service_funding: 1.0,
        construction_allowed: true,
        blueprint_allowlist: BTreeSet::new(),
    };
    allowed
        .blueprint_allowlist
        .insert(PLACEMENT_SMOKE_BLUEPRINT_ID.to_string());
    store
        .create(&allowed)
        .map_err(|error| format!("create allow policy: {error}"))?;

    let denied = DistrictPolicyRequest {
        name: "No Build".to_string(),
        center: [
            PLACEMENT_SMOKE_DENIED_CENTER[0] as f32,
            PLACEMENT_SMOKE_DENIED_CENTER[1] as f32,
        ],
        radius_m: 64.0,
        tax_multiplier: 1.0,
        service_funding: 1.0,
        construction_allowed: false,
        blueprint_allowlist: BTreeSet::new(),
    };
    store
        .create(&denied)
        .map_err(|error| format!("create deny policy: {error}"))?;
    Ok(store)
}

/// Run the deterministic authoritative placement smoke.
///
/// # Errors
///
/// Returns a human-readable failure for the first broken expectation or
/// filesystem error. Every temporary directory is removed on the way out.
pub fn run_placement_smoke() -> Result<PlacementSmokeReport, String> {
    let library_dir = TempDir::new("library")?;
    let save_dir = TempDir::new("save")?;
    let hash = seed_library(library_dir.path())?;

    let mut world = ServerWorld::with_role(PLACEMENT_SMOKE_SEED, ClientRole::Mayor);
    world
        .set_blueprint_library(library_dir.path().to_path_buf())
        .map_err(|error| format!("configure library: {error}"))?;
    world
        .set_policy_registry(smoke_policies()?)
        .map_err(|error| format!("configure policies: {error}"))?;
    match world.apply(&hello()) {
        ServerMessage::Welcome { .. } => {}
        other => return Err(format!("hello was rejected: {other:?}")),
    }

    let baseline_checksum = world.checksum();
    let first = world.apply(&place(&hash, PLACEMENT_SMOKE_CENTER));
    let Some((placement_id, _)) = placement_result(&first) else {
        return Err(format!("first placement was rejected: {first:?}"));
    };
    let after_first = world.checksum();
    if after_first == baseline_checksum {
        return Err("an accepted placement did not move the world checksum".to_string());
    }
    let balance_after_first = world.economy.balance;
    let tick_after_first = world.tick;
    let placed = world
        .placements
        .as_ref()
        .is_some_and(|store| store.placements.len() == 1);
    let site_created = world
        .construction
        .as_ref()
        .is_some_and(|store| store.site(placement_id).is_some());
    if !placed || !site_created {
        return Err("the accepted placement did not store a record and site".to_string());
    }

    let duplicate = world.apply(&place(&hash, PLACEMENT_SMOKE_CENTER));
    let duplicate = match placement_result(&duplicate) {
        Some((duplicate_id, duplicate_checksum)) => {
            duplicate_id == placement_id
                && duplicate_checksum == after_first
                && world.economy.balance == balance_after_first
                && world.tick == tick_after_first
                && world
                    .placements
                    .as_ref()
                    .is_some_and(|store| store.placements.len() == 1)
        }
        None => false,
    };
    if !duplicate {
        return Err("repeating the placement was not a no-op".to_string());
    }

    let denied = world.apply(&place(&hash, PLACEMENT_SMOKE_DENIED_CENTER));
    let policy = rejection_code(&denied) == Some("PLACEMENT_POLICY_DENIED")
        && world.checksum() == after_first;
    if !policy {
        return Err(format!("policy denial failed: {denied:?}"));
    }

    let overlap = world.apply(&place(&hash, PLACEMENT_SMOKE_OVERLAP_CENTER));
    let overlap =
        rejection_code(&overlap) == Some("PLACEMENT_OVERLAP") && world.checksum() == after_first;
    if !overlap {
        return Err(format!("overlap rejection failed: {overlap:?}"));
    }

    world.economy.balance = 1.0;
    let funds_rejection = world.apply(&place(&hash, PLACEMENT_SMOKE_FUNDS_CENTER));
    let funds_rejected = rejection_code(&funds_rejection) == Some("PLACEMENT_INSUFFICIENT_FUNDS");
    world.economy.balance = balance_after_first;
    let funds = funds_rejected && world.checksum() == after_first;
    if !funds {
        return Err(format!(
            "insufficient-funds rejection failed: {funds_rejection:?}"
        ));
    }
    if world.checksum() != after_first {
        return Err("the funds probe left the world mutated".to_string());
    }

    let save_path = save_dir.path().join("world.json");
    world
        .save_to(&save_path)
        .map_err(|error| format!("save: {error}"))?;
    let save_json =
        std::fs::read_to_string(&save_path).map_err(|error| format!("read save: {error}"))?;
    let library_root = library_dir.path().to_string_lossy().to_string();
    if save_json.contains(&library_root)
        || save_json.contains("/tmp")
        || save_json.contains("/media")
        || save_json.contains("\"blueprint_path\": \"/")
    {
        return Err("the server save leaked a filesystem path".to_string());
    }
    let save = save_path.is_file()
        && world
            .to_save()
            .placements
            .as_ref()
            .is_some_and(|store| store.placements.len() == 1)
        && world
            .to_save()
            .construction
            .as_ref()
            .is_some_and(|store| store.site_count() == 1);
    if !save {
        return Err("the save did not carry the placement and site".to_string());
    }

    let loaded = ServerWorld::load_from(&save_path).map_err(|error| format!("load: {error}"))?;
    let mut loaded_world = loaded.world;
    let reload = loaded_world.checksum() == after_first
        && loaded_world
            .placements
            .as_ref()
            .is_some_and(|store| store.placements.len() == 1)
        && loaded_world
            .construction
            .as_ref()
            .is_some_and(|store| store.site_count() == 1)
        && loaded_world.blueprint_library.is_none()
        && loaded_world.policies.is_none();
    if !reload {
        return Err("the reloaded world does not match the saved state".to_string());
    }

    // A restored world has no process configuration: placement must reject
    // until the operator reconfigures the library.
    match loaded_world.apply(&hello()) {
        ServerMessage::Welcome { .. } => {}
        other => return Err(format!("hello after load was rejected: {other:?}")),
    }
    let unavailable = loaded_world.apply(&place(&hash, PLACEMENT_SMOKE_CENTER));
    if rejection_code(&unavailable) != Some("PLACEMENT_LIBRARY_UNAVAILABLE") {
        return Err(format!(
            "a restored world without a library accepted placement: {unavailable:?}"
        ));
    }
    loaded_world
        .set_blueprint_library(library_dir.path().to_path_buf())
        .map_err(|error| format!("reconfigure library: {error}"))?;
    let reconfigured = loaded_world.apply(&place(&hash, PLACEMENT_SMOKE_CENTER));
    if placement_result(&reconfigured).is_none_or(|(id, _)| id != placement_id) {
        return Err(format!(
            "the reloaded world lost idempotent placement: {reconfigured:?}"
        ));
    }

    let checksum = loaded_world.checksum();
    Ok(PlacementSmokeReport {
        placed,
        duplicate,
        policy,
        funds,
        overlap,
        save,
        reload,
        checksum,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_smoke_is_deterministic() {
        let first = run_placement_smoke().expect("first smoke");
        assert!(first.placed && first.duplicate && first.policy);
        assert!(first.funds && first.overlap && first.save && first.reload);
        assert_eq!(first.checksum.len(), 16);
        assert!(first.checksum.bytes().all(|byte| byte.is_ascii_hexdigit()));
        let second = run_placement_smoke().expect("second smoke");
        // The record stores a content-addressed reference, so two runs in
        // different temp library roots report the identical checksum.
        assert_eq!(first, second);
    }
}
