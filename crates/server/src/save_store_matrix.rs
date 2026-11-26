//! Presence matrix for the optional [`ServerSave`](crate::save::ServerSave)
//! stores.
//!
//! Test-only. Pins the 16 present/absent combinations of `players`,
//! `settlements`, `placements`, and `construction`, including present-but-empty
//! stores, per-store seed and validation rejections, forward-compatible unknown
//! fields, outer-version behavior, and byte identity across load/re-save
//! cycles. Every fixture uses [`LOOPBACK_SEED`] and no wall-clock reads.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use core_city::construction::ConstructionStore;
use core_city::placement::{PlacementRequest, PlacementStore};
use core_city::player::PlayerRegistry;
use core_city::settlement::{SettlementRequest, SettlementStore};

use crate::protocol::{ClientCommand, TimeAction, LOOPBACK_SEED};
use crate::save::{backup_path, codes, temp_path, SaveError};
use crate::world::{ClientRole, ServerWorld};

const PLAYERS: u8 = 1 << 0;
const SETTLEMENTS: u8 = 1 << 1;
const PLACEMENTS: u8 = 1 << 2;
const CONSTRUCTION: u8 = 1 << 3;
const ALL_STORES: u8 = PLAYERS | SETTLEMENTS | PLACEMENTS | CONSTRUCTION;

/// Optional keys in schema/declaration order.
const OPTIONAL_KEYS: [&str; 4] = ["placements", "construction", "players", "settlements"];

/// Required keys in schema/declaration order.
const REQUIRED_KEYS: [&str; 9] = [
    "format_version",
    "seed",
    "tick",
    "population",
    "employed",
    "buildings",
    "paused",
    "economy",
    "roads",
];

/// Optional key and its presence bit.
const STORE_BITS: [(&str, u8); 4] = [
    ("placements", PLACEMENTS),
    ("construction", CONSTRUCTION),
    ("players", PLAYERS),
    ("settlements", SETTLEMENTS),
];

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "townsmith-server-store-matrix-{}-{unique}",
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

fn settlement_store() -> SettlementStore {
    let mut players = PlayerRegistry::new(LOOPBACK_SEED);
    let owner = players.register("mayor").expect("owner").id;
    let mut settlements = SettlementStore::new(LOOPBACK_SEED);
    settlements
        .found_for_player(
            &SettlementRequest {
                name: "Matrix".to_string(),
                owner: "mayor".to_string(),
                center: [4, -4],
                radius_m: 64,
            },
            owner,
        )
        .expect("found settlement");
    settlements
}

fn placement_store() -> PlacementStore {
    let request = PlacementRequest {
        blueprint_id: "townsmith:matrix_house".to_string(),
        blueprint_hash: format!("blake3:{}", "ab".repeat(32)),
        blueprint_path: format!("tsbp:blake3:{}", "ab".repeat(32)),
        name: "Matrix House".to_string(),
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

/// World for a presence bitmask. `players` is non-empty only when its bit is
/// set; the other stores are populated `Some` values only when set.
fn world_for_mask(mask: u8) -> ServerWorld {
    let mut world = played_world();
    if mask & PLAYERS == 0 {
        world.players = PlayerRegistry::new(LOOPBACK_SEED);
    } else {
        world.players.register("builder").expect("second player");
    }
    world.settlements = (mask & SETTLEMENTS != 0).then(settlement_store);
    world.placements = (mask & PLACEMENTS != 0).then(placement_store);
    world.construction = (mask & CONSTRUCTION != 0).then(construction_store);
    world
}

fn parse(json: &str) -> serde_json::Value {
    serde_json::from_str(json).expect("json value")
}

fn render(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).expect("json")
}

fn label(mask: u8) -> String {
    format!("{mask:04b}")
}

fn has_store(mask: u8, key: &str) -> bool {
    let (_, bit) = STORE_BITS
        .iter()
        .find(|(name, _)| *name == key)
        .expect("known optional key");
    mask & bit != 0
}

fn expected_keys(mask: u8) -> Vec<&'static str> {
    let mut keys = REQUIRED_KEYS.to_vec();
    keys.extend(
        OPTIONAL_KEYS
            .iter()
            .copied()
            .filter(|key| has_store(mask, key)),
    );
    keys
}

/// Top-level keys in serialized order. Top-level fields are indented two
/// spaces; nested fields are indented four or more.
fn top_level_keys(json: &str) -> Vec<&str> {
    json.lines()
        .filter_map(|line| line.strip_prefix("  \""))
        .filter_map(|rest| rest.split_once("\": ").map(|(key, _)| key))
        .collect()
}

fn assert_absent(value: &serde_json::Value, mask: u8) {
    for key in OPTIONAL_KEYS {
        if has_store(mask, key) {
            assert!(
                value[key].is_object(),
                "{}: {key} must be an object",
                label(mask)
            );
            assert!(
                !value[key].is_null(),
                "{}: {key} must not be null",
                label(mask)
            );
        } else {
            assert!(
                value.get(key).is_none(),
                "{}: {key} must be omitted",
                label(mask)
            );
        }
    }
}

fn assert_path_free(json: &str) {
    for needle in ["/tmp", "/media", "\\\\", "C:\\"] {
        assert!(
            !json.contains(needle),
            "save JSON contains path fragment {needle}"
        );
    }
}

#[test]
fn all_sixteen_store_combinations_validate_round_trip_and_resave_identically() {
    for mask in 0..=ALL_STORES {
        let world = world_for_mask(mask);
        let save = world.to_save();
        save.validate()
            .unwrap_or_else(|error| panic!("mask {}: {error}", label(mask)));

        let json = world
            .to_save_json_pretty()
            .unwrap_or_else(|error| panic!("mask {}: {error}", label(mask)));
        assert_eq!(
            top_level_keys(&json),
            expected_keys(mask),
            "mask {}",
            label(mask)
        );
        assert_eq!(json.lines().next(), Some("{"));
        assert!(
            json.starts_with("{\n  \"format_version\": 1,"),
            "v1 is canonical"
        );

        let value = parse(&json);
        assert_eq!(value["format_version"], 1, "mask {}", label(mask));
        assert_eq!(value["seed"], LOOPBACK_SEED);
        assert!(value.get("content").is_none(), "server save is v1-only");
        assert_absent(&value, mask);
        assert_path_free(&json);

        if mask & PLACEMENTS != 0 {
            let stored = value["placements"]["placements"][0]["blueprint_path"]
                .as_str()
                .expect("stored blueprint reference");
            assert!(stored.starts_with("tsbp:blake3:"));
            assert!(!stored.contains('/'), "stored reference is not a path");
        }

        let loaded = ServerWorld::from_save_json(&json).expect("load");
        assert_eq!(loaded.to_save(), save, "mask {}", label(mask));
        assert_eq!(loaded.checksum(), world.checksum(), "mask {}", label(mask));
        assert_eq!(
            loaded.players.is_empty(),
            mask & PLAYERS == 0,
            "mask {}",
            label(mask)
        );
        assert_eq!(
            loaded.settlements.is_some(),
            mask & SETTLEMENTS != 0,
            "mask {}",
            label(mask)
        );
        assert_eq!(
            loaded.placements.is_some(),
            mask & PLACEMENTS != 0,
            "mask {}",
            label(mask)
        );
        assert_eq!(
            loaded.construction.is_some(),
            mask & CONSTRUCTION != 0,
            "mask {}",
            label(mask)
        );

        let resaved = loaded.to_save_json_pretty().expect("re-save");
        assert_eq!(
            resaved,
            json,
            "mask {} must re-save byte-identically",
            label(mask)
        );
        assert_eq!(top_level_keys(&resaved), expected_keys(mask));

        // A fresh build of the same fixture is byte-identical: no iteration
        // order, hash map, or timestamp leaks into the document.
        let fresh = world_for_mask(mask).to_save_json_pretty().expect("fresh");
        assert_eq!(fresh, json, "mask {} must be deterministic", label(mask));
    }
}

#[test]
fn three_file_cycles_preserve_every_combination_without_field_growth() {
    let dir = TempDir::new();
    for mask in 0..=ALL_STORES {
        let mut current = world_for_mask(mask);
        let baseline = current.to_save_json_pretty().expect("baseline");
        let keys = expected_keys(mask);
        let path = dir.join(&format!("mask-{mask:02}.json"));

        for cycle in 0..3 {
            let report = current.save_to(&path).expect("save");
            assert_eq!(report.bytes, baseline.len() as u64);
            assert!(
                !report.backup_rotated,
                "identical save must not rotate a backup"
            );
            assert!(!temp_path(&path).exists());
            assert!(!backup_path(&path).exists());

            let loaded = ServerWorld::load_from(&path).expect("load");
            assert!(!loaded.report.recovered_from_backup);
            assert_eq!(loaded.world.checksum(), current.checksum());
            assert_eq!(loaded.world.to_save(), current.to_save());

            let resaved = loaded.world.to_save_json_pretty().expect("re-save");
            assert_eq!(resaved, baseline, "mask {} cycle {cycle}", label(mask));
            assert_eq!(
                top_level_keys(&resaved),
                keys,
                "mask {} cycle {cycle}",
                label(mask)
            );
            assert_eq!(
                parse(&resaved)["format_version"],
                1,
                "mask {} cycle {cycle}",
                label(mask)
            );
            current = loaded.world;
        }
    }
}

#[test]
fn empty_registries_are_omitted_but_present_empty_stores_are_kept() {
    let mut world = played_world();
    world.players = PlayerRegistry::new(LOOPBACK_SEED);
    world.settlements = Some(SettlementStore::new(LOOPBACK_SEED));
    world.placements = Some(PlacementStore::new(LOOPBACK_SEED));
    world.construction = Some(ConstructionStore::default());

    let json = world.to_save_json_pretty().expect("json");
    let value = parse(&json);
    assert!(value.get("players").is_none(), "empty registry is omitted");
    assert_eq!(
        value["settlements"]["settlements"].as_array().map(Vec::len),
        Some(0)
    );
    assert_eq!(
        value["placements"]["placements"].as_array().map(Vec::len),
        Some(0)
    );
    assert_eq!(
        value["construction"]["sites"]
            .as_object()
            .map(::serde_json::Map::len),
        Some(0)
    );

    let loaded = ServerWorld::from_save_json(&json).expect("load");
    assert!(loaded.players.is_empty());
    assert!(loaded
        .settlements
        .as_ref()
        .is_some_and(SettlementStore::is_empty));
    assert!(loaded
        .placements
        .as_ref()
        .is_some_and(|store| store.placements.is_empty()));
    assert_eq!(
        loaded
            .construction
            .as_ref()
            .map(ConstructionStore::site_count),
        Some(0)
    );
    assert_eq!(loaded.checksum(), world.checksum());
    assert_eq!(loaded.to_save_json_pretty().expect("re-save"), json);

    // Present-but-empty is distinct from absent for the `Option` stores.
    let absent = ServerWorld::new(LOOPBACK_SEED);
    let absent_json = absent.to_save_json_pretty().expect("absent");
    let mut absent_value = parse(&absent_json);
    for key in ["players", "settlements", "placements", "construction"] {
        assert!(absent_value.get(key).is_none(), "{key}");
    }
    absent_value["future"] = 1.into();
    assert_ne!(json, absent_json);

    // Player registries round-trip for one and for several players.
    let mut one = played_world();
    let one_json = one.to_save_json_pretty().expect("one player");
    assert_eq!(
        parse(&one_json)["players"]["players"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    let one_loaded = ServerWorld::from_save_json(&one_json).expect("load one");
    assert_eq!(one_loaded.players, one.players);

    one.players.register("builder").expect("second player");
    let two_json = one.to_save_json_pretty().expect("two players");
    assert_eq!(
        parse(&two_json)["players"]["players"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    let two_loaded = ServerWorld::from_save_json(&two_json).expect("load two");
    assert_eq!(two_loaded.players, one.players);
    assert_eq!(two_loaded.checksum(), one.checksum());
}

#[test]
fn foreign_seed_is_rejected_with_the_offending_store_name() {
    let world = world_for_mask(ALL_STORES);
    let baseline = world.to_save_json_pretty().expect("json");
    let foreign = LOOPBACK_SEED + 1;

    let cases = vec![
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

    for (field, store) in cases {
        let mut value = parse(&baseline);
        value[field] = store;
        let error = ServerWorld::from_save_json(&render(&value)).expect_err(field);
        match error {
            SaveError::SeedMismatch {
                field: reported,
                expected,
                found,
            } => {
                assert_eq!(reported, field);
                assert_eq!(expected, LOOPBACK_SEED);
                assert_eq!(found, foreign);
            }
            other => panic!("{field}: expected seed mismatch, got {other:?}"),
        }
    }

    // Construction carries no seed. A `seed` key is an unknown field: it is
    // ignored on load and stripped on re-save rather than treated as a seed.
    let mut value = parse(&baseline);
    value["construction"]["seed"] = foreign.into();
    let loaded = ServerWorld::from_save_json(&render(&value)).expect("construction has no seed");
    assert!(loaded.construction.is_some());
    let resaved = loaded.to_save_json_pretty().expect("re-save");
    assert!(parse(&resaved)["construction"].get("seed").is_none());
    assert_eq!(resaved, baseline);
}

#[test]
fn malformed_optional_stores_report_their_stable_codes() {
    let clean = ServerWorld::new(LOOPBACK_SEED)
        .to_save_json_pretty()
        .expect("json");

    let mut placements = serde_json::to_value(PlacementStore::new(LOOPBACK_SEED)).expect("store");
    placements["next_id"] = 0.into();
    let mut construction =
        serde_json::to_value(ConstructionStore::default()).expect("construction");
    construction["crew_capacity"] = 65.into();
    let mut players = serde_json::to_value(PlayerRegistry::new(LOOPBACK_SEED)).expect("players");
    players["next_id"] = 0.into();
    let mut settlements =
        serde_json::to_value(SettlementStore::new(LOOPBACK_SEED)).expect("settlements");
    settlements["next_id"] = 0.into();

    let cases = [
        ("placements", codes::INVALID_PLACEMENTS, placements),
        ("construction", codes::INVALID_CONSTRUCTION, construction),
        ("players", codes::INVALID_PLAYERS, players),
        ("settlements", codes::INVALID_SETTLEMENTS, settlements),
    ];
    for (field, expected, store) in cases {
        // Validate each store even when every other optional store is absent.
        let mut value = parse(&clean);
        value[field] = store;
        let error = ServerWorld::from_save_json(&render(&value)).expect_err(field);
        assert_eq!(error.code(), expected, "{field}");
    }

    // A store of the wrong JSON shape fails at the schema layer.
    for (field, store) in [
        ("placements", serde_json::json!("not-a-store")),
        ("construction", serde_json::json!(true)),
        ("players", serde_json::json!([1, 2])),
        ("settlements", serde_json::json!(42)),
    ] {
        let mut value = parse(&clean);
        value[field] = store;
        let error = ServerWorld::from_save_json(&render(&value)).expect_err(field);
        assert_eq!(error.code(), codes::INVALID_JSON, "{field}");
    }
}

#[test]
fn unknown_fields_are_ignored_and_stripped_but_future_versions_fail_closed() {
    let world = world_for_mask(ALL_STORES);
    let baseline = world.to_save_json_pretty().expect("json");

    let mut value = parse(&baseline);
    value["future_layer"] = serde_json::json!({ "format_version": 99, "records": [1, 2, 3] });
    value["future_scalar"] = 7.into();
    value["economy"]["future_field"] = true.into();
    value["roads"]["future_field"] = "ignored".into();
    for key in OPTIONAL_KEYS {
        value[key]["future_field"] = serde_json::json!([1, 2, 3]);
    }
    value["players"]["players"][0]["future_identity"] = "ignored".into();
    value["placements"]["placements"][0]["future_footprint"] = 1.25.into();

    let loaded = ServerWorld::from_save_json(&render(&value)).expect("forward compatible");
    assert_eq!(loaded.to_save(), world.to_save());
    assert_eq!(loaded.checksum(), world.checksum());
    let resaved = loaded.to_save_json_pretty().expect("re-save");
    assert_eq!(
        resaved, baseline,
        "unknown fields must not survive a re-save"
    );
    for needle in [
        "future_layer",
        "future_scalar",
        "future_field",
        "future_identity",
    ] {
        assert!(!resaved.contains(needle), "{needle} survived a re-save");
    }

    // The server save is v1-only: no content identity, no outer v2.
    let value = parse(&baseline);
    assert_eq!(value["format_version"], 1);
    assert!(value.get("content").is_none());

    // A v1 reader must reject an unknown future outer version even when the
    // document carries a plausible content identity.
    let mut future = parse(&baseline);
    future["format_version"] = 2.into();
    future["content"] = serde_json::json!({
        "pack_id": "base",
        "pack_checksum": "0123456789abcdef",
        "registry_checksum": "fedcba9876543210",
        "content_version": 1,
    });
    let error = ServerWorld::from_save_json(&render(&future)).expect_err("outer v2");
    assert!(matches!(error, SaveError::UnsupportedVersion(2)));
    assert!(error.to_string().contains("expected 1"));

    // On a v1 document an unknown `content` key is just an unknown field: it
    // is ignored and stripped, not interpreted as an identity.
    let mut v1_with_content = parse(&baseline);
    v1_with_content["content"] = serde_json::json!({ "pack_id": "base" });
    let loaded = ServerWorld::from_save_json(&render(&v1_with_content)).expect("v1 content");
    assert_eq!(loaded.to_save(), world.to_save());
    assert_eq!(loaded.to_save_json_pretty().expect("re-save"), baseline);
}

#[test]
fn legacy_baseline_without_optional_stores_is_byte_identical_after_load_and_resave() {
    let source = played_world();
    let mut legacy = source.to_save();
    legacy.players = None;
    legacy.settlements = None;
    legacy.placements = None;
    legacy.construction = None;
    let baseline = serde_json::to_string_pretty(&legacy).expect("legacy json");

    assert_eq!(top_level_keys(&baseline), REQUIRED_KEYS.to_vec());
    for key in OPTIONAL_KEYS {
        assert!(parse(&baseline).get(key).is_none(), "{key} must be absent");
    }
    assert_path_free(&baseline);

    let loaded = ServerWorld::from_save_json(&baseline).expect("legacy load");
    assert!(loaded.players.is_empty());
    assert!(loaded.settlements.is_none());
    assert!(loaded.placements.is_none());
    assert!(loaded.construction.is_none());
    assert_eq!(loaded.to_save(), legacy);
    assert_eq!(loaded.checksum(), source.checksum());
    assert_eq!(loaded.to_save_json_pretty().expect("re-save"), baseline);

    // A no-op file save of the loaded legacy world leaves the bytes untouched
    // and does not rotate a backup.
    let dir = TempDir::new();
    let path = dir.join("legacy.json");
    fs::write(&path, &baseline).expect("write legacy baseline");
    let loaded = ServerWorld::load_from(&path).expect("file load");
    loaded.world.save_to(&path).expect("no-op save");
    assert_eq!(fs::read_to_string(&path).expect("read back"), baseline);
    assert!(!backup_path(&path).exists());
    assert!(!temp_path(&path).exists());
}
