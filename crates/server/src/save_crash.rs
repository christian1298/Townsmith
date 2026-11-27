//! Crash/restart matrix for the single-file server save path.
//!
//! These tests would be integration tests, but they need the crate-private
//! [`ServerWorld::restore`] path and the existing protocol fixtures, so they
//! live beside [`crate::save`] as a test-only module. Every case uses a unique
//! temporary directory, fixed seeds, and no wall-clock reads.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use core_city::construction::ConstructionStore;
use core_city::economy::DAY_SECONDS;
use core_city::placement::{PlacementRequest, PlacementStore};
use core_city::player::PlayerRegistry;
use core_city::settlement::{SettlementRequest, SettlementRole, SettlementStore};

use crate::protocol::{ClientCommand, TimeAction, LOOPBACK_SEED};
use crate::save::{backup_path, codes, temp_path, SaveError, SAVE_BACKUP_SUFFIX};
use crate::world::{ClientRole, ServerWorld};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "townsmith-server-save-crash-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("temp dir");
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
        protocol_version: crate::protocol::PROTOCOL_VERSION,
        content_hash: "0123456789abcdef".repeat(4),
        player_name: "mayor".to_string(),
    }
}

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

fn placement_and_site_stores() -> (PlacementStore, ConstructionStore) {
    let hash = format!("blake3:{}", "ab".repeat(32));
    let request = PlacementRequest {
        blueprint_id: "townsmith:save_house".to_string(),
        blueprint_hash: hash.clone(),
        blueprint_path: format!("tsbp:blake3:{}", "ab".repeat(32)),
        name: "Save House".to_string(),
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
    let mut construction = ConstructionStore::default();
    construction
        .add_priced_site(1, 10, 100.0)
        .expect("construction site");
    (placements, construction)
}

/// Every optional store populated: two registered players, one owned
/// settlement with a member and tax, one placement, and one construction site.
fn crash_world() -> ServerWorld {
    let mut world = played_world();
    let owner = world.player_id().expect("hello assigns a player id");
    let builder = world.players.register("builder").expect("builder").id;

    let mut settlements = SettlementStore::new(LOOPBACK_SEED);
    let settlement = settlements
        .found_for_player(
            &SettlementRequest {
                name: "Crash Test".to_string(),
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

    let (placements, construction) = placement_and_site_stores();
    world.placements = Some(placements);
    world.construction = Some(construction);
    world
}

fn save_json(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).expect("json")
}

fn backup_count(directory: &Path) -> usize {
    fs::read_dir(directory)
        .expect("read temp dir")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .ends_with(SAVE_BACKUP_SUFFIX)
        })
        .count()
}

#[test]
fn valid_primary_loads_and_a_corrupt_primary_recovers_the_backup() {
    let dir = TempDir::new();
    let path = dir.join("world.json");

    let mut world = ServerWorld::new(LOOPBACK_SEED);
    world.save_to(&path).expect("first save");
    world.advance_day(DAY_SECONDS);
    world.save_to(&path).expect("second save");
    let primary_bytes = world.to_save_json_pretty().expect("primary json");
    assert_eq!(fs::read(&path).expect("primary"), primary_bytes.as_bytes());

    let primary = ServerWorld::load_from(&path).expect("valid primary");
    assert!(!primary.report.recovered_from_backup);
    assert_eq!(primary.world.tick, world.tick);

    fs::write(&path, b"{ not json").expect("corrupt primary");
    let recovered = ServerWorld::load_from(&path).expect("backup fallback");
    assert!(recovered.report.recovered_from_backup);
    assert_eq!(recovered.world.tick, world.tick - 1);

    // Both files unusable: one stable code, with the path and both failure
    // reasons reported in Display.
    fs::write(&path, b"{").expect("corrupt primary");
    fs::write(backup_path(&path), b"[]").expect("corrupt backup");
    let primary_bytes = fs::read(&path).expect("primary bytes");
    let backup_bytes = fs::read(backup_path(&path)).expect("backup bytes");

    let error = ServerWorld::load_from(&path).expect_err("both corrupt");
    assert_eq!(error.code(), codes::CORRUPT);
    match &error {
        SaveError::Corrupt {
            path: reported,
            reason,
        } => {
            assert_eq!(reported, &path);
            assert!(reason.contains("primary:"), "{reason}");
            assert!(reason.contains("backup:"), "{reason}");
        }
        other => panic!("expected corrupt save, got {other:?}"),
    }
    assert_eq!(fs::read(&path).expect("primary untouched"), primary_bytes);
    assert_eq!(
        fs::read(backup_path(&path)).expect("backup untouched"),
        backup_bytes
    );
}

#[test]
fn an_interrupted_temp_file_is_ignored_by_primary_and_recovery() {
    let dir = TempDir::new();
    let path = dir.join("world.json");
    let temp = temp_path(&path);

    let mut world = ServerWorld::new(LOOPBACK_SEED);
    world.save_to(&path).expect("first save");
    world.advance_day(DAY_SECONDS);
    world.save_to(&path).expect("second save");
    let primary_tick = world.tick;

    // A crash between the temp write and the rename leaves a valid but newer
    // document in `.tmp`; it must never be read as the primary.
    world.advance_day(DAY_SECONDS);
    let interrupted = world.to_save_json_pretty().expect("interrupted json");
    fs::write(&temp, interrupted.as_bytes()).expect("temp file");

    let loaded = ServerWorld::load_from(&path).expect("primary");
    assert!(!loaded.report.recovered_from_backup);
    assert_eq!(loaded.world.tick, primary_tick);
    assert_eq!(
        fs::read(&temp).expect("temp untouched"),
        interrupted.as_bytes()
    );

    let backup = ServerWorld::load_from(backup_path(&path)).expect("backup");
    assert_eq!(backup.world.tick, primary_tick - 1);

    // Even when the primary is corrupt, recovery goes to `.bak`, not the
    // newer `.tmp`.
    fs::write(&path, b"{").expect("corrupt primary");
    let recovered = ServerWorld::load_from(&path).expect("backup fallback");
    assert!(recovered.report.recovered_from_backup);
    assert_eq!(recovered.world.tick, primary_tick - 1);
    assert_eq!(
        fs::read(&temp).expect("temp still untouched"),
        interrupted.as_bytes()
    );
}

#[test]
fn successful_saves_replace_the_primary_and_keep_exactly_one_backup() {
    let dir = TempDir::new();
    let path = dir.join("world.json");

    let mut world = ServerWorld::new(LOOPBACK_SEED);
    let first_bytes = world.to_save_json_pretty().expect("first json");
    let first = world.save_to(&path).expect("first save");
    assert!(!first.backup_rotated);
    assert_eq!(fs::read(&path).expect("primary"), first_bytes.as_bytes());
    assert!(!backup_path(&path).exists());

    world.advance_day(DAY_SECONDS);
    let second_bytes = world.to_save_json_pretty().expect("second json");
    let second = world.save_to(&path).expect("second save");
    assert!(second.backup_rotated);
    assert_eq!(fs::read(&path).expect("primary"), second_bytes.as_bytes());
    assert_eq!(
        fs::read(backup_path(&path)).expect("backup"),
        first_bytes.as_bytes()
    );
    assert_eq!(backup_count(&dir.0), 1);

    world.advance_day(DAY_SECONDS);
    let third_bytes = world.to_save_json_pretty().expect("third json");
    let third = world.save_to(&path).expect("third save");
    assert!(third.backup_rotated);
    assert_eq!(fs::read(&path).expect("primary"), third_bytes.as_bytes());
    assert_eq!(
        fs::read(backup_path(&path)).expect("backup"),
        second_bytes.as_bytes()
    );
    assert_eq!(backup_count(&dir.0), 1);

    // Re-saving identical bytes is a no-op for the backup generation.
    for _ in 0..2 {
        let repeated = world.save_to(&path).expect("repeat save");
        assert!(!repeated.backup_rotated);
        assert_eq!(
            fs::read(backup_path(&path)).expect("backup"),
            second_bytes.as_bytes()
        );
        assert_eq!(backup_count(&dir.0), 1);
    }
    assert!(!temp_path(&path).exists());
}

#[test]
fn three_restarts_preserve_stores_checksum_ids_and_resave_bytes() {
    let dir = TempDir::new();
    let path = dir.join("world.json");
    let world = crash_world();

    let expected_players = world.players.clone();
    let expected_settlements = world.settlements.clone();
    let expected_placements = world.placements.clone();
    let expected_construction = world.construction.clone();
    let expected_checksum = world.checksum();

    assert_eq!(expected_players.count(), 2);
    assert_eq!(expected_players.next_id, 3);
    let settlement = &expected_settlements
        .as_ref()
        .expect("settlements")
        .settlements[0];
    assert_eq!(settlement.members.len(), 2);
    assert_eq!(settlement.membership_revision, 2);
    assert_eq!(settlement.tax_rate, 7);
    assert_eq!(expected_placements.as_ref().expect("placements").next_id, 2);
    assert_eq!(
        expected_construction
            .as_ref()
            .expect("construction")
            .site_count(),
        1
    );

    let first = world.save_to(&path).expect("initial save");
    assert!(!first.backup_rotated);
    let normalized = fs::read(&path).expect("normalized bytes");
    assert_eq!(
        normalized,
        world
            .to_save_json_pretty()
            .expect("normalized json")
            .into_bytes()
    );

    for restart in 0..3 {
        let loaded = ServerWorld::load_from(&path).expect("restart load");
        assert!(
            !loaded.report.recovered_from_backup,
            "restart {restart} unexpectedly used the backup"
        );
        assert_eq!(loaded.world.players, expected_players, "restart {restart}");
        assert_eq!(
            loaded.world.settlements, expected_settlements,
            "restart {restart}"
        );
        assert_eq!(
            loaded.world.placements, expected_placements,
            "restart {restart}"
        );
        assert_eq!(
            loaded.world.construction, expected_construction,
            "restart {restart}"
        );
        assert_eq!(
            loaded.world.checksum(),
            expected_checksum,
            "restart {restart}"
        );
        assert_eq!(loaded.world.players.next_id, 3, "restart {restart}");
        assert_eq!(
            loaded
                .world
                .placements
                .as_ref()
                .expect("placements")
                .next_id,
            2,
            "restart {restart}"
        );
        let restored = &loaded
            .world
            .settlements
            .as_ref()
            .expect("settlements")
            .settlements[0];
        assert_eq!(restored.membership_revision, 2, "restart {restart}");
        assert_eq!(restored.tax_rate, 7, "restart {restart}");

        let re_save = loaded.world.save_to(&path).expect("re-save");
        assert!(!re_save.backup_rotated, "restart {restart}");
        assert_eq!(
            fs::read(&path).expect("re-saved bytes"),
            normalized,
            "restart {restart} normalized bytes drifted"
        );
    }
    assert!(!backup_path(&path).exists());
}

#[test]
fn foreign_seed_optional_stores_are_rejected_before_a_world_is_built() {
    let foreign = LOOPBACK_SEED + 1;
    let base = ServerWorld::new(LOOPBACK_SEED)
        .to_save_json_pretty()
        .expect("base json");
    let stores = [
        (
            "placements",
            serde_json::to_value(PlacementStore::new(foreign)).expect("placements"),
        ),
        (
            "players",
            serde_json::to_value(PlayerRegistry::new(foreign)).expect("players"),
        ),
        (
            "settlements",
            serde_json::to_value(SettlementStore::new(foreign)).expect("settlements"),
        ),
    ];

    for (field, store) in stores {
        let mut value: serde_json::Value = serde_json::from_str(&base).expect("value");
        value[field] = store;
        let error = ServerWorld::from_save_json(&save_json(&value)).expect_err(field);
        assert_eq!(error.code(), codes::SEED_MISMATCH, "{field}");
        assert!(
            !error.code().contains('/') && !error.code().contains('\\'),
            "{field}: {}",
            error.code()
        );
    }

    // The unmodified document still builds a world: a rejected attempt has no
    // side effect on the caller's state.
    let untouched = ServerWorld::from_save_json(&base).expect("base still valid");
    assert!(untouched.players.is_empty());
    assert!(untouched.placements.is_none());
    assert!(untouched.settlements.is_none());
}

#[test]
fn unknown_optional_fields_are_ignored_and_stripped_on_re_save() {
    let world = crash_world();
    let original = world.to_save_json_pretty().expect("original json");
    let mut value: serde_json::Value = serde_json::from_str(&original).expect("value");
    value["future_layer"] = serde_json::json!({
        "format_version": 99,
        "records": [1, 2, 3]
    });
    value["players"]["future_field"] = serde_json::json!(true);
    value["placements"]["future_field"] = serde_json::json!("ignored");

    let loaded = ServerWorld::from_save_json(&save_json(&value)).expect("forward compatible");
    assert_eq!(loaded.to_save(), world.to_save());
    assert_eq!(loaded.checksum(), world.checksum());
    assert_eq!(
        loaded.to_save_json_pretty().expect("re-save"),
        original,
        "unknown fields must not survive a re-save"
    );
}

#[test]
fn a_future_version_primary_fails_closed_instead_of_recovering_the_backup() {
    let dir = TempDir::new();
    let path = dir.join("world.json");

    // Two valid v1 generations: the first rotates to `.bak`, so a naive
    // recovery path would happily load it.
    let mut world = ServerWorld::new(LOOPBACK_SEED);
    world.save_to(&path).expect("first save");
    let backup_bytes = fs::read(&path).expect("backup bytes");
    world.advance_day(DAY_SECONDS);
    world.save_to(&path).expect("second save");
    assert!(backup_path(&path).is_file());

    // A structurally valid document from a future server version.
    let mut future = world.to_save();
    future.format_version = 3;
    let future_json = serde_json::to_string_pretty(&future).expect("future json");
    fs::write(&path, future_json.as_bytes()).expect("future primary");

    let error = ServerWorld::load_from(&path).expect_err("future version must fail closed");
    assert_eq!(error.code(), codes::UNSUPPORTED_VERSION);
    assert!(matches!(error, SaveError::UnsupportedVersion(3)));
    assert!(
        error.to_string().contains("expected 1"),
        "stable message names the supported version: {error}"
    );

    // No downgrade and no side effects: both files stay byte-identical and no
    // temp file is created.
    assert_eq!(
        fs::read(&path).expect("primary untouched"),
        future_json.as_bytes()
    );
    assert_eq!(
        fs::read(backup_path(&path)).expect("backup untouched"),
        backup_bytes
    );
    assert!(!temp_path(&path).exists());
}

#[test]
fn a_legacy_save_without_optional_stores_is_byte_identical_on_a_noop_save() {
    let dir = TempDir::new();
    let path = dir.join("legacy.json");

    let mut legacy = played_world().to_save();
    legacy.players = None;
    legacy.settlements = None;
    legacy.placements = None;
    legacy.construction = None;
    let legacy_json = serde_json::to_string_pretty(&legacy).expect("legacy json");
    let legacy_bytes = legacy_json.as_bytes().to_vec();
    fs::write(&path, &legacy_bytes).expect("legacy file");

    let loaded = ServerWorld::load_from(&path).expect("legacy load");
    assert!(loaded.world.players.is_empty());
    assert!(loaded.world.settlements.is_none());
    assert!(loaded.world.placements.is_none());
    assert!(loaded.world.construction.is_none());

    let report = loaded.world.save_to(&path).expect("no-op save");
    assert!(!report.backup_rotated);
    assert_eq!(fs::read(&path).expect("no-op bytes"), legacy_bytes);
    assert!(!backup_path(&path).exists());
}

#[test]
fn malformed_truncated_non_utf8_and_directory_saves_have_stable_errors() {
    let dir = TempDir::new();
    let cases: [(&str, Vec<u8>); 3] = [
        ("malformed", b"{ not json".to_vec()),
        (
            "truncated",
            br#"{"format_version":1,"seed":42,"tick":"#.to_vec(),
        ),
        ("non_utf8", vec![0xff, 0xfe, 0x7b, 0x00]),
    ];

    for (name, bytes) in &cases {
        let path = dir.join(&format!("{name}.json"));
        fs::write(&path, bytes).expect("bad save file");
        let error = ServerWorld::load_from(&path).expect_err(name);
        // A primary that cannot be parsed and has no backup is reported through
        // the recovery boundary, not as a bare parse error.
        assert_eq!(error.code(), codes::CORRUPT, "{name}: {error}");
        assert!(!error.code().contains('/') && !error.code().contains('\\'));
        assert_eq!(fs::read(&path).expect("untouched"), *bytes, "{name}");
    }

    // Parsing without a file keeps the precise code.
    assert_eq!(
        ServerWorld::from_save_json("{").expect_err("json").code(),
        codes::INVALID_JSON
    );
    assert_eq!(
        ServerWorld::from_save_json(r#"{"format_version":1"#)
            .expect_err("schema")
            .code(),
        codes::INVALID_JSON
    );

    let directory_path = dir.join("directory.json");
    fs::create_dir(&directory_path).expect("directory save path");
    let load_error = ServerWorld::load_from(&directory_path).expect_err("directory load");
    assert_eq!(load_error.code(), codes::CORRUPT, "{load_error}");
    let save_error = ServerWorld::new(LOOPBACK_SEED)
        .save_to(&directory_path)
        .expect_err("directory save");
    assert_eq!(save_error.code(), codes::IO, "{save_error}");
    assert!(!temp_path(&directory_path).exists());
}

#[test]
fn stable_error_codes_never_embed_absolute_paths() {
    let dir = TempDir::new();
    let absolute = dir.0.to_str().expect("utf-8 temp dir");

    let missing = ServerWorld::load_from(dir.join("absent.json")).expect_err("missing");
    let corrupt_path = dir.join("both-corrupt.json");
    fs::write(&corrupt_path, b"{").expect("corrupt primary");
    fs::write(backup_path(&corrupt_path), b"[").expect("corrupt backup");
    let corrupt = ServerWorld::load_from(&corrupt_path).expect_err("both corrupt");

    let directory_path = dir.join("directory.json");
    fs::create_dir(&directory_path).expect("directory save path");
    let io = ServerWorld::new(LOOPBACK_SEED)
        .save_to(&directory_path)
        .expect_err("directory save");

    let parse = ServerWorld::from_save_json("{").expect_err("json");
    let unsupported = ServerWorld::from_save_json(&save_json(&serde_json::json!({
        "format_version": 99,
        "seed": LOOPBACK_SEED,
        "tick": 0,
        "population": 1,
        "employed": 0,
        "buildings": 1,
        "paused": false,
        "economy": serde_json::to_value(ServerWorld::new(LOOPBACK_SEED).economy)
            .expect("economy"),
        "roads": serde_json::to_value(ServerWorld::new(LOOPBACK_SEED).roads)
            .expect("roads"),
    })))
    .expect_err("version");

    for error in [&missing, &corrupt, &io, &parse, &unsupported] {
        let code = error.code();
        assert!(!code.contains('/'), "{code}");
        assert!(!code.contains('\\'), "{code}");
        assert!(!code.contains(absolute), "{error}");
    }

    // Paths stay in Display where the variants already document them.
    assert!(missing.to_string().contains(absolute));
    assert!(corrupt.to_string().contains(absolute));
    assert!(!parse.to_string().contains(absolute));
}
