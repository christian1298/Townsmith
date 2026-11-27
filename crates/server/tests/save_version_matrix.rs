//! Server save version/migration matrix (docs/80 §3.8).
//!
//! The server save is deliberately identity-free (D-034): base-only documents
//! stay outer `format_version: 1` with no `content` member, and the reader has
//! no v2 branch. The bridge owns the D-034 outer-v2 content envelope; its
//! round-trip and fail-closed cases live in
//! `crates/godot_ext/src/save_composition_matrix.rs` and
//! `crates/godot_ext/src/content_identity.rs` and are not duplicated here.
//!
//! This matrix covers the server boundary instead:
//!
//! - v1 base-only load/save is byte-identical and leaves no `.tmp`/`.bak`;
//! - an outer v2 with a plausible bridge `content` block fails closed with
//!   `save.unsupported_version` before any store validator runs;
//! - an unknown outer v3 fails closed with the stable code/message, does not
//!   fall back to `.bak`, and leaves the primary untouched;
//! - each optional store is exercised absent, valid, and with a malformed
//!   `format_version`, reporting its own stable `save.invalid_*` code;
//! - repeated load/save cycles are byte-stable and do not advance ids,
//!   membership revisions, or checksums;
//! - a v1 document with an unknown `content` field strips it and re-saves the
//!   canonical base-only bytes.
//!
//! Every fixture uses [`LOOPBACK_SEED`], a process-unique temporary directory,
//! and no wall-clock reads.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use core_city::construction::ConstructionStore;
use core_city::placement::{PlacementRequest, PlacementStore};
use core_city::player::PlayerRegistry;
use core_city::settlement::{SettlementRequest, SettlementRole, SettlementStore};

use townsmith_server::protocol::{ClientCommand, TimeAction, LOOPBACK_SEED, PROTOCOL_VERSION};
use townsmith_server::save::{
    backup_path, codes, temp_path, SaveError, SERVER_SAVE_FORMAT_VERSION,
};
use townsmith_server::world::{ClientRole, ServerWorld};

/// Optional stores in schema order.
const OPTIONAL_STORES: [(&str, &str); 4] = [
    ("placements", codes::INVALID_PLACEMENTS),
    ("construction", codes::INVALID_CONSTRUCTION),
    ("players", codes::INVALID_PLAYERS),
    ("settlements", codes::INVALID_SETTLEMENTS),
];

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let unique = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "townsmith-save-version-{label}-{}-{unique}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
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

/// Deterministic base world: one registered player, one road, a paused clock,
/// and non-default tax rates.
fn played_world() -> ServerWorld {
    let mut world = ServerWorld::with_role(LOOPBACK_SEED, ClientRole::Admin);
    world.apply(&hello());
    world.apply(&ClientCommand::SetTax {
        resident_rate: 12,
        business_rate: 8,
    });
    world.apply(&ClientCommand::PlaceRoad {
        start: [0, 0],
        end: [8, 0],
    });
    world.apply(&ClientCommand::TimeControl {
        action: TimeAction::Pause,
    });
    world
}

fn placement_store() -> PlacementStore {
    let request = PlacementRequest {
        blueprint_id: "townsmith:version_house".to_string(),
        blueprint_hash: format!("blake3:{}", "ab".repeat(32)),
        blueprint_path: format!("tsbp:blake3:{}", "ab".repeat(32)),
        name: "Version House".to_string(),
        bounds_m: [3.2, 3.2, 3.2],
        center_x: 0.0,
        center_z: 0.0,
        rotation_deg: 0,
        material_cost: 100.0,
        resident_capacity: 2,
        job_slots: 1,
        solid_voxels: 10,
        structure: None,
    };
    let mut placements = PlacementStore::new(LOOPBACK_SEED);
    placements
        .place(&request, |_, _| 40)
        .expect("placement record");
    placements
}

fn construction_store() -> ConstructionStore {
    let mut construction = ConstructionStore::default();
    construction
        .add_priced_site(1, 10, 100.0)
        .expect("construction site");
    construction
}

/// Every optional store populated: two registered players, one owned
/// settlement with a member and tax, one placement, and one construction site.
fn full_world() -> ServerWorld {
    let mut world = played_world();
    let owner = world.player_id().expect("hello assigns a player id");
    let builder = world.players.register("builder").expect("builder").id;

    let mut settlements = SettlementStore::new(LOOPBACK_SEED);
    let settlement = settlements
        .found_for_player(
            &SettlementRequest {
                name: "Version Matrix".to_string(),
                owner: "mayor".to_string(),
                center: [4, -4],
                radius_m: 64,
            },
            owner,
        )
        .expect("found settlement");
    settlements
        .invite(
            owner,
            settlement.id,
            builder,
            SettlementRole::Builder,
            settlement.membership_revision,
        )
        .expect("invite member");
    settlements
        .set_tax_rate(owner, settlement.id, 7)
        .expect("settlement tax");
    world.settlements = Some(settlements);

    world.placements = Some(placement_store());
    world.construction = Some(construction_store());
    world
}

fn parse(json: &str) -> serde_json::Value {
    serde_json::from_str(json).expect("json value")
}

fn render(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).expect("json")
}

fn assert_no_residue(path: &Path) {
    assert!(!temp_path(path).exists(), "{} left a .tmp", path.display());
    assert!(
        !backup_path(path).exists(),
        "{} left a .bak",
        path.display()
    );
}

#[test]
fn v1_base_only_load_save_is_byte_identical_without_residue() {
    let dir = TempDir::new("base");
    let path = dir.join("world.json");
    let mut world = played_world();
    // Base-only means no optional stores at all, including the player
    // registry that `hello` would populate.
    world.players = PlayerRegistry::new(LOOPBACK_SEED);
    let canonical = world.to_save_json_pretty().expect("canonical json");

    let value = parse(&canonical);
    assert_eq!(value["format_version"], SERVER_SAVE_FORMAT_VERSION);
    assert!(
        value.get("content").is_none(),
        "server save is identity-free"
    );
    for (store, _) in OPTIONAL_STORES {
        assert!(value.get(store).is_none(), "{store} must be absent");
    }

    fs::write(&path, &canonical).expect("write canonical v1");
    for cycle in 0..3 {
        let loaded = ServerWorld::load_from(&path).expect("load v1");
        assert!(!loaded.report.recovered_from_backup, "cycle {cycle}");
        assert_eq!(loaded.world.checksum(), world.checksum(), "cycle {cycle}");
        assert_eq!(
            loaded.world.to_save_json_pretty().expect("re-save"),
            canonical,
            "cycle {cycle} must re-save byte-identically"
        );

        let report = loaded.world.save_to(&path).expect("no-op save");
        assert!(!report.backup_rotated, "cycle {cycle}");
        assert_eq!(
            fs::read_to_string(&path).expect("bytes"),
            canonical,
            "cycle {cycle} changed the file"
        );
        assert_no_residue(&path);
    }
}

#[test]
fn outer_v2_with_content_identity_fails_closed_before_store_validation() {
    let world = full_world();
    let baseline = world.to_save_json_pretty().expect("baseline json");

    let mut value = parse(&baseline);
    value["format_version"] = 2.into();
    value["content"] = serde_json::json!({
        "format_version": 1,
        "pack_id": "example:copper_age",
        "pack_checksum": "ad4c645869a411ed",
        "registry_checksum": "330a538d07805b50",
    });
    // Every store carries an unsupported version as well. A reader that
    // validated stores before the outer version would report a store code.
    for (store, _) in OPTIONAL_STORES {
        value[store]["format_version"] = 99.into();
    }
    value["placements"]["next_id"] = 0.into();

    let error = ServerWorld::from_save_json(&render(&value)).expect_err("outer v2");
    assert_eq!(error.code(), codes::UNSUPPORTED_VERSION);
    assert!(matches!(error, SaveError::UnsupportedVersion(2)));
    assert!(
        error
            .to_string()
            .contains("unsupported server save format version 2"),
        "{error}"
    );
    assert!(error.to_string().contains("expected 1"), "{error}");

    // The server never writes the bridge `content` envelope, even when the
    // binary contains the D-034 bridge code.
    assert!(parse(&baseline).get("content").is_none());

    // A file load is equally fail-closed and leaves both the primary and a
    // possible backup untouched.
    let dir = TempDir::new("v2");
    let path = dir.join("world.json");
    let future = render(&value);
    fs::write(&path, &future).expect("write v2 primary");
    fs::write(backup_path(&path), &baseline).expect("write v1 backup");
    let error = ServerWorld::load_from(&path).expect_err("v2 file");
    assert_eq!(error.code(), codes::UNSUPPORTED_VERSION);
    assert_eq!(fs::read_to_string(&path).expect("primary"), future);
    assert_eq!(
        fs::read_to_string(backup_path(&path)).expect("backup"),
        baseline
    );
    assert!(!temp_path(&path).exists(), "rejected load left a .tmp");
}

#[test]
fn unknown_outer_v3_fails_closed_without_partial_store_or_mutation() {
    let world = full_world();
    let baseline = world.to_save_json_pretty().expect("baseline json");

    let mut value = parse(&baseline);
    value["format_version"] = 3.into();
    // A future save may carry a content identity and newer store versions;
    // none of that may be partially applied by a v1 reader.
    value["content"] = serde_json::json!({
        "format_version": 1,
        "pack_id": "example:copper_age",
        "pack_checksum": "ad4c645869a411ed",
        "registry_checksum": "330a538d07805b50",
    });
    value["settlements"]["format_version"] = 99.into();
    let future = render(&value);

    let error = ServerWorld::from_save_json(&future).expect_err("outer v3");
    assert_eq!(error.code(), codes::UNSUPPORTED_VERSION);
    assert!(matches!(error, SaveError::UnsupportedVersion(3)));
    assert!(
        error
            .to_string()
            .contains("unsupported server save format version 3"),
        "{error}"
    );

    // A structurally valid v1 backup must not be used: a future save fails
    // closed rather than silently downgrading to stale data.
    let dir = TempDir::new("v3");
    let path = dir.join("world.json");
    fs::write(&path, &future).expect("write v3 primary");
    fs::write(backup_path(&path), &baseline).expect("write v1 backup");

    let error = ServerWorld::load_from(&path).expect_err("v3 file");
    assert_eq!(error.code(), codes::UNSUPPORTED_VERSION);
    assert!(matches!(error, SaveError::UnsupportedVersion(3)));
    assert_eq!(fs::read_to_string(&path).expect("primary"), future);
    assert_eq!(
        fs::read_to_string(backup_path(&path)).expect("backup"),
        baseline
    );
    assert!(!temp_path(&path).exists(), "rejected load left a .tmp");
}

#[test]
fn optional_store_version_matrix_covers_absent_and_valid() {
    let world = full_world();
    let baseline = world.to_save_json_pretty().expect("baseline json");

    // Valid present: every supported store loads and re-saves identically.
    let loaded = ServerWorld::from_save_json(&baseline).expect("valid full save");
    assert_eq!(loaded.checksum(), world.checksum());
    assert_eq!(loaded.to_save_json_pretty().expect("re-save"), baseline);

    let value = parse(&baseline);
    for (store, _) in OPTIONAL_STORES {
        // Absent: the optional key is omitted and stays omitted on re-save.
        let mut absent = value.clone();
        absent.as_object_mut().expect("save object").remove(store);
        let loaded = ServerWorld::from_save_json(&render(&absent))
            .unwrap_or_else(|error| panic!("{store} absent: {error}"));
        let resaved = loaded.to_save_json_pretty().expect("re-save absent");
        assert!(
            parse(&resaved).get(store).is_none(),
            "{store} must stay omitted"
        );
    }
}

#[test]
fn optional_store_version_matrix_rejects_malformed_versions() {
    let world = full_world();
    let value = parse(&world.to_save_json_pretty().expect("baseline json"));

    for (store, expected_code) in OPTIONAL_STORES {
        for version in [0u16, 99] {
            let mut broken = value.clone();
            broken[store]["format_version"] = version.into();
            let json = render(&broken);

            // The version gate runs before the store validator, so an unknown
            // store version surfaces as that store's stable invalid code.
            let error = ServerWorld::from_save_json(&json)
                .expect_err(&format!("{store} format_version {version}"));
            assert_eq!(
                error.code(),
                expected_code,
                "{store} format_version {version}: {error}"
            );
            assert!(
                error.to_string().contains("version") || error.to_string().contains("unsupported"),
                "{store} {version}: {error}"
            );
        }
    }
}

#[test]
fn repeated_load_save_cycles_are_byte_stable_and_do_not_drift() {
    let dir = TempDir::new("idempotent");
    let path = dir.join("world.json");
    let world = full_world();
    let normalized = world.to_save_json_pretty().expect("normalized json");
    let expected_checksum = world.checksum();

    let players_next = world.players.next_id;
    let placements_next = world.placements.as_ref().expect("placements").next_id;
    let settlement = &world.settlements.as_ref().expect("settlements").settlements[0];
    let expected_revision = settlement.membership_revision;
    let expected_tax = settlement.tax_rate;
    assert_eq!(world.players.count(), 2);
    assert_eq!(expected_revision, 2);
    assert_eq!(expected_tax, 7);

    fs::write(&path, &normalized).expect("write baseline");
    for cycle in 0..3 {
        let loaded = ServerWorld::load_from(&path).expect("load");
        assert!(!loaded.report.recovered_from_backup, "cycle {cycle}");
        assert_eq!(loaded.world.checksum(), expected_checksum, "cycle {cycle}");
        assert_eq!(loaded.world.players.next_id, players_next, "cycle {cycle}");
        assert_eq!(
            loaded
                .world
                .placements
                .as_ref()
                .expect("placements")
                .next_id,
            placements_next,
            "cycle {cycle}"
        );
        let restored = &loaded
            .world
            .settlements
            .as_ref()
            .expect("settlements")
            .settlements[0];
        assert_eq!(
            restored.membership_revision, expected_revision,
            "cycle {cycle}"
        );
        assert_eq!(restored.tax_rate, expected_tax, "cycle {cycle}");

        let resaved = loaded.world.to_save_json_pretty().expect("re-save");
        assert_eq!(resaved, normalized, "cycle {cycle} bytes drifted");

        let report = loaded.world.save_to(&path).expect("save");
        assert!(!report.backup_rotated, "cycle {cycle}");
        assert_eq!(
            fs::read_to_string(&path).expect("file"),
            normalized,
            "cycle {cycle} file drifted"
        );
        assert_no_residue(&path);
    }
}

#[test]
fn v1_with_an_unknown_content_field_is_stripped_and_resaves_canonical_bytes() {
    let world = played_world();
    let canonical = world.to_save_json_pretty().expect("canonical json");

    let mut value = parse(&canonical);
    value["content"] = serde_json::json!({
        "format_version": 1,
        "pack_id": "example:copper_age",
        "pack_checksum": "ad4c645869a411ed",
        "registry_checksum": "330a538d07805b50",
    });
    value["future_layer"] = serde_json::json!({ "format_version": 99, "records": [1, 2, 3] });

    // On a v1 server save `content` is an unknown field, not an identity:
    // it is ignored on load and stripped on re-save.
    let loaded = ServerWorld::from_save_json(&render(&value)).expect("v1 unknown content");
    assert_eq!(loaded.checksum(), world.checksum());
    let resaved = loaded.to_save_json_pretty().expect("re-save");
    assert_eq!(
        resaved, canonical,
        "base-only v1 must keep its canonical bytes"
    );
    assert!(!resaved.contains("\"content\""), "content survived");
    assert!(!resaved.contains("future_layer"), "unknown field survived");
    assert_eq!(parse(&resaved)["format_version"], 1);
}

#[test]
fn rejected_store_loads_leave_the_file_and_directory_untouched() {
    let dir = TempDir::new("rejected-store");
    let path = dir.join("world.json");
    let world = full_world();
    let baseline = world.to_save_json_pretty().expect("baseline json");

    let mut broken = parse(&baseline);
    broken["settlements"]["format_version"] = 99.into();
    let broken = render(&broken);
    fs::write(&path, &broken).expect("write malformed store");

    // The file boundary reports corruption for an unusable primary with no
    // backup, but it must not rewrite the file or leave a temp sibling.
    let error = ServerWorld::load_from(&path).expect_err("malformed store file");
    assert_eq!(error.code(), codes::CORRUPT, "{error}");
    assert_eq!(fs::read_to_string(&path).expect("primary"), broken);
    assert_no_residue(&path);

    // Direct parsing keeps the precise store code and has no side effects.
    let error = ServerWorld::from_save_json(&broken).expect_err("malformed store parse");
    assert_eq!(error.code(), codes::INVALID_SETTLEMENTS);
    assert_eq!(fs::read_to_string(&path).expect("primary"), broken);
    assert_no_residue(&path);
}
