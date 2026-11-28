//! Pause persistence and restart semantics.
//!
//! `ServerSave` v1 carries `paused`, while `ServerWorld::checksum` deliberately
//! excludes it: pause is envelope/clock state, not world-state identity. These
//! tests pin the resulting behavior across a save/load cycle and a resumed
//! economy tick: a paused world reloads paused at the same tick and checksum,
//! a paused `advance_day` is a no-op, and resuming advances exactly one day.

use std::path::{Path, PathBuf};

use core_city::economy::DAY_SECONDS;
use townsmith_server::protocol::{ClientCommand, ServerMessage, LOOPBACK_SEED, PROTOCOL_VERSION};
use townsmith_server::world::ServerWorld;

fn temp_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "townsmith-pause-restart-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create temp dir");
    path
}

fn snapshot_paused(world: &ServerWorld) -> bool {
    match world.snapshot() {
        ServerMessage::Snapshot { paused, .. } => paused,
        other => panic!("expected snapshot, got {other:?}"),
    }
}

fn status_paused(world: &mut ServerWorld) -> bool {
    let hello = ClientCommand::Hello {
        protocol_version: PROTOCOL_VERSION,
        content_hash: "0".repeat(64),
        player_name: "pause-status".to_string(),
    };
    let _ = world.apply(&hello);
    match world.apply(&ClientCommand::ServerStatus) {
        ServerMessage::ServerStatus { paused, .. } => paused,
        other => panic!("expected server_status, got {other:?}"),
    }
}

#[test]
fn paused_world_reloads_paused_at_the_same_tick_and_checksum() {
    let mut world = ServerWorld::new(LOOPBACK_SEED);
    world.paused = true;
    let checksum = world.checksum();
    let tick = world.tick;

    let json = world.to_save_json_pretty().expect("serialize paused world");
    assert!(
        json.contains("\"paused\": true"),
        "pause is written: {json}"
    );

    let mut loaded = ServerWorld::from_save_json(&json).expect("load paused world");
    assert!(loaded.paused, "pause survives load");
    assert_eq!(loaded.tick, tick, "tick is unchanged by load");
    assert_eq!(
        loaded.checksum(),
        checksum,
        "pause is excluded from identity"
    );
    assert!(
        snapshot_paused(&loaded),
        "snapshot reports paused after load"
    );
    assert!(
        status_paused(&mut loaded),
        "server_status reports paused after load"
    );
}

#[test]
fn paused_restart_does_not_advance_or_skip_an_economy_day() {
    let mut paused = ServerWorld::new(LOOPBACK_SEED);
    paused.paused = true;
    let paused_tick = paused.tick;
    let paused_checksum = paused.checksum();
    paused.advance_day(DAY_SECONDS);
    assert_eq!(paused.tick, paused_tick, "paused advance_day is a no-op");
    assert_eq!(
        paused.checksum(),
        paused_checksum,
        "no-op keeps the checksum"
    );

    let mut resumed = ServerWorld::new(LOOPBACK_SEED);
    resumed.paused = false;
    resumed.advance_day(DAY_SECONDS);
    assert_eq!(
        resumed.tick,
        paused_tick + 1,
        "resume advances exactly one day"
    );
    assert_ne!(
        resumed.checksum(),
        paused_checksum,
        "a real day moves the world"
    );
}

#[test]
fn paused_save_cycles_are_byte_identical_and_leave_no_temp_file() {
    let directory = temp_dir("cycles");
    let first = directory.join("world.json");
    let second = directory.join("world-2.json");

    let mut world = ServerWorld::new(LOOPBACK_SEED);
    world.paused = true;
    world.save_to(&first).expect("first save");

    let loaded = ServerWorld::load_from(&first).expect("first load").world;
    assert!(loaded.paused, "first load keeps pause");
    loaded.save_to(&second).expect("second save");

    let first_bytes = std::fs::read(&first).expect("read first save");
    let second_bytes = std::fs::read(&second).expect("read second save");
    assert_eq!(
        first_bytes, second_bytes,
        "paused save is stable across a load/save cycle"
    );
    assert!(
        !Path::new(&format!("{}.tmp", first.display())).exists(),
        "no temp file"
    );
    assert!(
        !Path::new(&format!("{}.tmp", second.display())).exists(),
        "no temp file"
    );

    std::fs::remove_dir_all(&directory).expect("cleanup");
}
