//! Deterministic decode and authority corpus for the newer protocol-v1
//! commands.
//!
//! The cases pin the wire contract for `player_list`, `settlement_list`,
//! `settlement_at`, `server_status`, `audit_log`, the advisory blueprint lock
//! commands, `chat`/`chat_since`, and the settlement mutation commands:
//!
//! * malformed shapes fail at the JSON or field layer with a stable code;
//! * well-formed extremes decode and are clamped or answered as reads;
//! * authority rejections keep the tick, world checksum, player registry,
//!   settlement store, chat log, and lock table untouched.
//!
//! Decode cases go through `serde_json::from_str::<ClientCommand>` and the
//! server decoder (for the stable rejection code). Authority cases go through
//! [`ServerWorld::apply`]; transport-only cases use [`Session`]. No network,
//! filesystem, or wall clock is involved.

use crate::protocol::{
    codes, decode_client_command, ClientCommand, ServerMessage, Session, PLAYER_LIST_PAGE_CAP,
    PROTOCOL_VERSION, SETTLEMENT_LIST_PAGE_CAP,
};
use crate::world::{
    codes as world_codes, ClientRole, ServerWorld, AUDIT_ROLE_FILTER_MAX_LEN,
    AUDIT_TAG_FILTER_MAX_LEN,
};
use core_city::player::MAX_PLAYERS;
use core_city::settlement::{SettlementRequest, SettlementStore};

const SEED: u64 = 2026;
const CONTENT_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const HASH_BARE: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    tick: u64,
    world_checksum: String,
    players_checksum: u64,
    settlements_checksum: Option<u64>,
    chat_seq: u64,
    locks: usize,
}

fn snapshot(world: &ServerWorld) -> Snapshot {
    Snapshot {
        tick: world.tick,
        world_checksum: world.checksum(),
        players_checksum: world.players.checksum(),
        settlements_checksum: world.settlements.as_ref().map(SettlementStore::checksum),
        chat_seq: world.chat_seq(),
        locks: world.blueprint_locks.len(),
    }
}

fn hello(name: &str) -> ClientCommand {
    ClientCommand::Hello {
        protocol_version: PROTOCOL_VERSION,
        content_hash: CONTENT_HASH.to_string(),
        player_name: name.to_string(),
    }
}

#[track_caller]
fn hello_world(role: ClientRole, name: &str) -> ServerWorld {
    let mut world = ServerWorld::with_role(SEED, role);
    let response = world.apply(&hello(name));
    assert!(
        matches!(response, ServerMessage::Welcome { .. }),
        "expected welcome for {name} as {role:?}, got {response:?}"
    );
    world
}

#[track_caller]
fn assert_invalid_fields(payload: &str) {
    assert!(
        serde_json::from_str::<ClientCommand>(payload).is_err(),
        "serde accepted invalid payload `{payload}`"
    );
    let error = decode_client_command(payload.as_bytes())
        .expect_err("server decoder accepted an invalid payload");
    assert_eq!(
        error.code,
        codes::COMMAND_INVALID_FIELDS,
        "payload `{payload}`: {error:?}"
    );
}

#[track_caller]
fn assert_invalid_json(payload: &str) {
    assert!(serde_json::from_str::<ClientCommand>(payload).is_err());
    let error = decode_client_command(payload.as_bytes()).expect_err("decoder accepted bad JSON");
    assert_eq!(
        error.code,
        codes::COMMAND_INVALID_JSON,
        "payload `{payload}`: {error:?}"
    );
}

#[track_caller]
fn assert_unknown_tag(payload: &str) {
    assert!(serde_json::from_str::<ClientCommand>(payload).is_err());
    let error =
        decode_client_command(payload.as_bytes()).expect_err("decoder accepted unknown tag");
    assert_eq!(
        error.code,
        codes::COMMAND_UNKNOWN_TAG,
        "payload `{payload}`: {error:?}"
    );
}

#[track_caller]
fn decode_ok(payload: &str) -> ClientCommand {
    serde_json::from_str::<ClientCommand>(payload)
        .unwrap_or_else(|error| panic!("payload `{payload}` should decode: {error}"))
}

#[track_caller]
fn assert_rejected(world: &mut ServerWorld, command: ClientCommand, code: &str) {
    let before = snapshot(world);
    let response = world.apply(&command);
    match response {
        ServerMessage::Reject { code: actual, .. } => {
            assert_eq!(actual, code, "{command:?}");
        }
        other => panic!("expected reject {code} for {command:?}, got {other:?}"),
    }
    assert_eq!(
        snapshot(world),
        before,
        "rejection mutated world state for {command:?}"
    );
}

#[track_caller]
fn player_page(
    world: &mut ServerWorld,
    after_id: Option<u64>,
    limit: Option<u32>,
) -> (usize, u32, u64, bool) {
    match world.apply(&ClientCommand::PlayerList { after_id, limit }) {
        ServerMessage::PlayerList {
            count,
            matched,
            next_after,
            has_more,
            ..
        } => (count, matched, next_after, has_more),
        other => panic!("expected player_list, got {other:?}"),
    }
}

#[track_caller]
fn settlement_page(
    world: &mut ServerWorld,
    after_id: Option<u64>,
    limit: Option<u32>,
) -> (usize, u32, u64, bool) {
    match world.apply(&ClientCommand::SettlementList { after_id, limit }) {
        ServerMessage::SettlementList {
            count,
            matched,
            next_after,
            has_more,
            ..
        } => (count, matched, next_after, has_more),
        other => panic!("expected settlement_list, got {other:?}"),
    }
}

#[track_caller]
fn audit(
    world: &mut ServerWorld,
    limit: Option<u32>,
    tag: Option<&str>,
    accepted: Option<bool>,
    role: Option<&str>,
) -> (usize, u32) {
    match world.apply(&ClientCommand::AuditLog {
        limit,
        tag: tag.map(str::to_string),
        accepted,
        role: role.map(str::to_string),
    }) {
        ServerMessage::AuditLog {
            matched, entries, ..
        } => (entries.len(), matched),
        other => panic!("expected audit_log, got {other:?}"),
    }
}

#[test]
fn command_envelope_faults_are_stable() {
    for payload in [
        r#"[{"type":"player_list"}]"#,
        r#"{"after_id":0}"#,
        r#"{"type":null}"#,
        r#"{"type":7}"#,
        r#"{"type":["player_list"]}"#,
        r#"{"type":"player_lists"}"#,
        r#"{"type":"not_a_command"}"#,
    ] {
        if payload.contains("not_a_command") || payload.contains("player_lists") {
            assert_unknown_tag(payload);
        } else {
            assert_invalid_fields(payload);
        }
    }
    for payload in ["{", "not json", ""] {
        assert_invalid_json(payload);
    }
}

#[test]
fn player_list_wire_faults_and_tolerated_extras() {
    for payload in [
        r#"{"type":"player_list","after_id":-1}"#,
        r#"{"type":"player_list","after_id":1.5}"#,
        r#"{"type":"player_list","after_id":"1"}"#,
        r#"{"type":"player_list","after_id":true}"#,
        r#"{"type":"player_list","after_id":[]}"#,
        r#"{"type":"player_list","after_id":{}}"#,
        r#"{"type":"player_list","after_id":18446744073709551616}"#,
        r#"{"type":"player_list","limit":-1}"#,
        r#"{"type":"player_list","limit":1.5}"#,
        r#"{"type":"player_list","limit":"1"}"#,
        r#"{"type":"player_list","limit":true}"#,
        r#"{"type":"player_list","limit":4294967296}"#,
    ] {
        assert_invalid_fields(payload);
    }

    let absent = ClientCommand::PlayerList {
        after_id: None,
        limit: None,
    };
    assert_eq!(decode_ok(r#"{"type":"player_list"}"#), absent);
    assert_eq!(
        decode_ok(r#"{"type":"player_list","after_id":null,"limit":null}"#),
        absent
    );
    assert_eq!(
        decode_ok(r#"{"type":"player_list","extra":{"ignored":[1,2]},"limit":1}"#),
        ClientCommand::PlayerList {
            after_id: None,
            limit: Some(1),
        }
    );
    for limit in [0, 1, 256, 257, u32::MAX] {
        assert_eq!(
            decode_ok(&format!(r#"{{"type":"player_list","limit":{limit}}}"#)),
            ClientCommand::PlayerList {
                after_id: None,
                limit: Some(limit),
            }
        );
    }
    assert_eq!(
        decode_ok(r#"{"type":"player_list","after_id":18446744073709551615,"limit":4294967295}"#),
        ClientCommand::PlayerList {
            after_id: Some(u64::MAX),
            limit: Some(u32::MAX),
        }
    );
}

#[test]
fn player_list_authority_clamps_and_reads_without_mutation() {
    let mut world = ServerWorld::with_role(SEED, ClientRole::Viewer);
    for index in 0..MAX_PLAYERS {
        world
            .players
            .register(&format!("p{index:03}"))
            .expect("register below the cap");
    }
    assert!(matches!(
        world.apply(&hello("p000")),
        ServerMessage::Welcome { .. }
    ));
    assert_eq!(world.players.count(), MAX_PLAYERS);
    let before = snapshot(&world);

    assert_eq!(
        player_page(&mut world, None, None),
        (MAX_PLAYERS, MAX_PLAYERS as u32, MAX_PLAYERS as u64, false)
    );
    assert_eq!(
        player_page(&mut world, Some(0), Some(0)),
        (1, MAX_PLAYERS as u32, 1, true)
    );
    assert_eq!(
        player_page(&mut world, Some(0), Some(1)),
        (1, MAX_PLAYERS as u32, 1, true)
    );
    assert_eq!(
        player_page(&mut world, Some(0), Some(PLAYER_LIST_PAGE_CAP)),
        (
            PLAYER_LIST_PAGE_CAP as usize,
            MAX_PLAYERS as u32,
            u64::from(PLAYER_LIST_PAGE_CAP),
            false
        )
    );
    assert_eq!(
        player_page(&mut world, Some(0), Some(PLAYER_LIST_PAGE_CAP + 1)),
        (
            PLAYER_LIST_PAGE_CAP as usize,
            MAX_PLAYERS as u32,
            u64::from(PLAYER_LIST_PAGE_CAP),
            false
        )
    );
    assert_eq!(
        player_page(&mut world, Some(250), Some(u32::MAX)),
        (6, 6, MAX_PLAYERS as u64, false)
    );
    assert_eq!(
        player_page(&mut world, Some(u64::MAX), Some(u32::MAX)),
        (0, 0, u64::MAX, false)
    );
    assert_eq!(snapshot(&world), before);
}

#[test]
fn reads_require_hello_before_authority_and_payload_checks() {
    let mut world = ServerWorld::with_role(SEED, ClientRole::Admin);
    for command in [
        ClientCommand::PlayerList {
            after_id: None,
            limit: None,
        },
        ClientCommand::SettlementList {
            after_id: None,
            limit: None,
        },
        ClientCommand::SettlementAt { x: 0, z: 0 },
        ClientCommand::ServerStatus,
        ClientCommand::BlueprintLocks,
        ClientCommand::ChatSince { after: 0 },
    ] {
        assert_rejected(&mut world, command, world_codes::NOT_HELLO);
    }
}

#[test]
fn settlement_list_wire_faults_and_extras() {
    for payload in [
        r#"{"type":"settlement_list","after_id":-1}"#,
        r#"{"type":"settlement_list","after_id":1.5}"#,
        r#"{"type":"settlement_list","after_id":"1"}"#,
        r#"{"type":"settlement_list","after_id":18446744073709551616}"#,
        r#"{"type":"settlement_list","limit":-1}"#,
        r#"{"type":"settlement_list","limit":1.5}"#,
        r#"{"type":"settlement_list","limit":"1"}"#,
        r#"{"type":"settlement_list","limit":4294967296}"#,
    ] {
        assert_invalid_fields(payload);
    }
    let absent = ClientCommand::SettlementList {
        after_id: None,
        limit: None,
    };
    assert_eq!(decode_ok(r#"{"type":"settlement_list"}"#), absent);
    assert_eq!(
        decode_ok(r#"{"type":"settlement_list","after_id":null,"limit":null}"#),
        absent
    );
    assert_eq!(
        decode_ok(r#"{"type":"settlement_list","extra":true,"limit":256}"#),
        ClientCommand::SettlementList {
            after_id: None,
            limit: Some(256),
        }
    );
    assert_eq!(
        decode_ok(r#"{"type":"settlement_list","after_id":18446744073709551615}"#),
        ClientCommand::SettlementList {
            after_id: Some(u64::MAX),
            limit: None,
        }
    );
}

fn seeded_settlements(count: usize) -> (ServerWorld, u64) {
    let mut world = ServerWorld::with_role(SEED, ClientRole::Viewer);
    let mut store = SettlementStore::new(SEED);
    for index in 0..count {
        store
            .found(&SettlementRequest {
                name: format!("S{index:04}"),
                owner: "nobody".to_string(),
                center: [i32::try_from(index).expect("small index"), 0],
                radius_m: 64,
            })
            .expect("seed settlement");
    }
    let checksum = store.checksum();
    world.settlements = Some(store);
    assert!(matches!(
        world.apply(&hello("viewer")),
        ServerMessage::Welcome { .. }
    ));
    (world, checksum)
}

#[test]
fn settlement_list_authority_paging_and_clamps() {
    let (mut world, store_checksum) = seeded_settlements(257);
    let before = snapshot(&world);
    assert_eq!(before.settlements_checksum, Some(store_checksum));

    assert_eq!(
        settlement_page(&mut world, None, None),
        (257, 257, 257, false)
    );
    assert_eq!(
        settlement_page(&mut world, Some(0), Some(0)),
        (1, 257, 1, true)
    );
    assert_eq!(
        settlement_page(&mut world, Some(0), Some(1)),
        (1, 257, 1, true)
    );
    assert_eq!(
        settlement_page(&mut world, Some(0), Some(SETTLEMENT_LIST_PAGE_CAP)),
        (
            SETTLEMENT_LIST_PAGE_CAP as usize,
            257,
            u64::from(SETTLEMENT_LIST_PAGE_CAP),
            true
        )
    );
    assert_eq!(
        settlement_page(&mut world, Some(0), Some(SETTLEMENT_LIST_PAGE_CAP + 1)),
        (
            SETTLEMENT_LIST_PAGE_CAP as usize,
            257,
            u64::from(SETTLEMENT_LIST_PAGE_CAP),
            true
        )
    );
    assert_eq!(
        settlement_page(&mut world, Some(255), Some(u32::MAX)),
        (2, 2, 257, false)
    );
    assert_eq!(
        settlement_page(&mut world, Some(u64::MAX), Some(u32::MAX)),
        (0, 0, u64::MAX, false)
    );
    assert_eq!(snapshot(&world), before);
}

#[test]
fn settlement_at_wire_types_and_read_only_ranges() {
    for payload in [
        r#"{"type":"settlement_at"}"#,
        r#"{"type":"settlement_at","x":0}"#,
        r#"{"type":"settlement_at","z":0}"#,
        r#"{"type":"settlement_at","x":null,"z":0}"#,
        r#"{"type":"settlement_at","x":1.5,"z":0}"#,
        r#"{"type":"settlement_at","x":"0","z":0}"#,
        r#"{"type":"settlement_at","x":0,"z":true}"#,
        r#"{"type":"settlement_at","x":2147483648,"z":0}"#,
        r#"{"type":"settlement_at","x":0,"z":-2147483649}"#,
    ] {
        assert_invalid_fields(payload);
    }
    for payload in [
        r#"{"type":"settlement_at","x":NaN,"z":0}"#,
        r#"{"type":"settlement_at","x":0,"z":Infinity}"#,
        r#"{"type":"settlement_at","x":0,"z":1e999}"#,
    ] {
        assert_invalid_json(payload);
    }

    let mut store = SettlementStore::new(SEED);
    store
        .found(&SettlementRequest {
            name: "At".to_string(),
            owner: "nobody".to_string(),
            center: [10, -20],
            radius_m: 64,
        })
        .expect("seed settlement");
    let mut world = ServerWorld::with_role(SEED, ClientRole::Viewer);
    world.settlements = Some(store);
    assert!(matches!(
        world.apply(&hello("viewer")),
        ServerMessage::Welcome { .. }
    ));
    let before = snapshot(&world);

    for (x, z, found) in [
        (10, -20, true),
        (0, 0, true),
        (1000, 1000, false),
        (i32::MIN, i32::MAX, false),
    ] {
        match world.apply(&ClientCommand::SettlementAt { x, z }) {
            ServerMessage::SettlementAt {
                found: actual,
                settlement,
                ..
            } => {
                assert_eq!(actual, found, "at ({x},{z})");
                assert_eq!(settlement.is_some(), found, "at ({x},{z})");
            }
            other => panic!("expected settlement_at, got {other:?}"),
        }
    }
    assert_eq!(snapshot(&world), before);
    assert_eq!(
        decode_ok(r#"{"type":"settlement_at","x":-20,"z":10,"extra":1}"#),
        ClientCommand::SettlementAt { x: -20, z: 10 }
    );
}

#[test]
fn server_status_extra_fields_and_roles() {
    assert_eq!(
        decode_ok(r#"{"type":"server_status"}"#),
        ClientCommand::ServerStatus
    );
    for payload in [
        r#"{"type":"server_status","extra":1}"#,
        r#"{"type":"server_status","payload":{"unexpected":true}}"#,
        r#"{"type":"server_status","tick":"ignored-extra"}"#,
    ] {
        assert_eq!(decode_ok(payload), ClientCommand::ServerStatus);
    }
    assert_eq!(
        decode_ok(r#"{"type":"server_status","type2":1,"x":null}"#),
        ClientCommand::ServerStatus
    );
    assert_invalid_fields(r#"{"type":7}"#);
    assert_unknown_tag(r#"{"type":"server_statuss"}"#);

    for role in ClientRole::ALL {
        let mut world = hello_world(role, "status");
        let before = snapshot(&world);
        match world.apply(&ClientCommand::ServerStatus) {
            ServerMessage::ServerStatus {
                tick,
                checksum,
                online,
                chat_seq,
                ..
            } => {
                assert_eq!(tick, before.tick);
                assert_eq!(checksum, before.world_checksum);
                assert_eq!(chat_seq, before.chat_seq);
                assert_eq!(online, 1);
            }
            other => panic!("expected server_status for {role:?}, got {other:?}"),
        }
        assert_eq!(snapshot(&world), before);
    }

    let mut transport = Session::new(SEED);
    assert!(matches!(
        transport.handle(hello("transport")).as_slice(),
        [ServerMessage::Welcome { .. }]
    ));
    let before = (transport.tick(), transport.checksum());
    match transport.handle(ClientCommand::ServerStatus).as_slice() {
        [ServerMessage::Reject { code, .. }] => assert_eq!(code, codes::STATUS_UNAVAILABLE),
        other => panic!("expected transport status reject, got {other:?}"),
    }
    assert_eq!((transport.tick(), transport.checksum()), before);
}

#[test]
fn audit_log_wire_faults_and_filter_validation() {
    for payload in [
        r#"{"type":"audit_log","limit":-1}"#,
        r#"{"type":"audit_log","limit":1.5}"#,
        r#"{"type":"audit_log","limit":"1"}"#,
        r#"{"type":"audit_log","limit":4294967296}"#,
        r#"{"type":"audit_log","tag":1}"#,
        r#"{"type":"audit_log","tag":[]}"#,
        r#"{"type":"audit_log","role":true}"#,
        r#"{"type":"audit_log","accepted":"true"}"#,
        r#"{"type":"audit_log","accepted":1}"#,
        r#"{"type":"audit_log","accepted":null,"role":null,"tag":null,"limit":null}"#,
    ] {
        if payload.contains(r#""accepted":null"#) {
            assert_eq!(
                decode_ok(payload),
                ClientCommand::AuditLog {
                    limit: None,
                    tag: None,
                    accepted: None,
                    role: None,
                }
            );
        } else {
            assert_invalid_fields(payload);
        }
    }
    assert_eq!(
        decode_ok(r#"{"type":"audit_log","extra":1,"limit":0}"#),
        ClientCommand::AuditLog {
            limit: Some(0),
            tag: None,
            accepted: None,
            role: None,
        }
    );

    let mut quiet = hello_world(ClientRole::Admin, "quiet");
    let quiet_before = snapshot(&quiet);
    for _ in 0..30 {
        quiet.apply(&ClientCommand::Ping { nonce: 1 });
    }
    assert_eq!(
        snapshot(&quiet),
        quiet_before,
        "pings must not move world state"
    );

    let mut world = hello_world(ClientRole::Admin, "admin");
    for _ in 0..30 {
        world.apply(&ClientCommand::Ping { nonce: 1 });
    }
    let before = snapshot(&world);

    assert!(audit(&mut world, None, None, None, None).0 >= 31);
    assert_eq!(audit(&mut world, Some(0), None, None, None).0, 1);
    assert_eq!(audit(&mut world, Some(1), None, None, None).0, 1);
    assert!(audit(&mut world, Some(256), None, None, None).0 >= 30);
    assert!(audit(&mut world, Some(u32::MAX), None, None, None).0 >= 30);
    let (entries, matched) = audit(&mut world, Some(u32::MAX), Some("ping"), None, None);
    assert_eq!(entries, 30);
    assert_eq!(matched, 30);
    assert!(matches!(
        world.apply(&ClientCommand::AuditLog {
            limit: Some(5),
            tag: Some("ping".to_string()),
            accepted: Some(false),
            role: Some("admin".to_string()),
        }),
        ServerMessage::AuditLog { ref entries, .. }
            if entries.iter().all(|entry| entry.tag == "ping"
                && entry.role == "admin"
                && !entry.accepted)
    ));
    assert_eq!(audit(&mut world, Some(64), Some("zzz"), None, None), (0, 0));
    assert_eq!(
        audit(&mut world, Some(64), Some(" ping "), None, None).0,
        30
    );
    assert_eq!(
        audit(
            &mut world,
            Some(64),
            Some(&"a".repeat(AUDIT_TAG_FILTER_MAX_LEN)),
            None,
            None
        ),
        (0, 0)
    );
    assert_eq!(
        audit(
            &mut world,
            Some(64),
            None,
            None,
            Some(&"a".repeat(AUDIT_ROLE_FILTER_MAX_LEN))
        ),
        (0, 0)
    );
    assert_eq!(
        snapshot(&world),
        before,
        "audit reads must not move world state"
    );

    for command in [
        ClientCommand::AuditLog {
            limit: None,
            tag: Some("PING".to_string()),
            accepted: None,
            role: None,
        },
        ClientCommand::AuditLog {
            limit: None,
            tag: Some(String::new()),
            accepted: None,
            role: None,
        },
        ClientCommand::AuditLog {
            limit: None,
            tag: Some("ping-command".to_string()),
            accepted: None,
            role: None,
        },
        ClientCommand::AuditLog {
            limit: None,
            tag: Some("a".repeat(AUDIT_TAG_FILTER_MAX_LEN + 1)),
            accepted: None,
            role: None,
        },
        ClientCommand::AuditLog {
            limit: None,
            tag: None,
            accepted: None,
            role: Some("Admin".to_string()),
        },
        ClientCommand::AuditLog {
            limit: None,
            tag: None,
            accepted: None,
            role: Some("a".repeat(AUDIT_ROLE_FILTER_MAX_LEN + 1)),
        },
        ClientCommand::AuditLog {
            limit: None,
            tag: None,
            accepted: None,
            role: Some("role name".to_string()),
        },
    ] {
        assert_rejected(&mut world, command, codes::AUDIT_FILTER_INVALID);
    }
}

#[test]
fn audit_log_requires_admin_and_transport_support() {
    for role in [ClientRole::Viewer, ClientRole::Mayor] {
        let mut world = hello_world(role, "reader");
        assert_rejected(
            &mut world,
            ClientCommand::AuditLog {
                limit: None,
                tag: None,
                accepted: None,
                role: None,
            },
            world_codes::FORBIDDEN,
        );
    }

    let mut transport = Session::new(SEED);
    assert!(matches!(
        transport.handle(hello("transport")).as_slice(),
        [ServerMessage::Welcome { .. }]
    ));
    let before = (transport.tick(), transport.checksum());
    match transport
        .handle(ClientCommand::AuditLog {
            limit: None,
            tag: None,
            accepted: None,
            role: None,
        })
        .as_slice()
    {
        [ServerMessage::Reject { code, .. }] => assert_eq!(code, codes::AUDIT_UNAVAILABLE),
        other => panic!("expected transport audit reject, got {other:?}"),
    }
    assert_eq!((transport.tick(), transport.checksum()), before);
}

#[test]
fn blueprint_lock_hash_and_type_faults() {
    for payload in [
        r#"{"type":"blueprint_lock"}"#,
        r#"{"type":"blueprint_lock","hash":null}"#,
        r#"{"type":"blueprint_lock","hash":1}"#,
        r#"{"type":"blueprint_lock","hash":[]}"#,
        r#"{"type":"blueprint_lock","hash":{}}"#,
        r#"{"type":"blueprint_lock","hash":true}"#,
        r#"{"type":"blueprint_unlock"}"#,
        r#"{"type":"blueprint_unlock","hash":null}"#,
        r#"{"type":"blueprint_unlock","hash":1}"#,
        r#"{"type":"blueprint_unlock","hash":[]}"#,
    ] {
        assert_invalid_fields(payload);
    }
    assert_eq!(
        decode_ok(&format!(
            r#"{{"type":"blueprint_lock","hash":"{HASH_BARE}"}}"#
        )),
        ClientCommand::BlueprintLock {
            hash: HASH_BARE.to_string()
        }
    );
    assert_eq!(
        decode_ok(&format!(
            r#"{{"type":"blueprint_unlock","hash":"blake3:{HASH_BARE}"}}"#
        )),
        ClientCommand::BlueprintUnlock {
            hash: format!("blake3:{HASH_BARE}")
        }
    );
    assert_eq!(
        decode_ok(r#"{"type":"blueprint_locks","extra":1}"#),
        ClientCommand::BlueprintLocks
    );

    let mut mayor = hello_world(ClientRole::Mayor, "mayor");
    let canonical = format!("blake3:{HASH_BARE}");
    let before = snapshot(&mayor);
    for hash in [
        HASH_BARE.to_ascii_uppercase(),
        "abc".to_string(),
        format!("blake3:{}", HASH_BARE.to_ascii_uppercase()),
        format!("/tmp/{HASH_BARE}"),
        format!("{HASH_BARE}0"),
    ] {
        assert_rejected(
            &mut mayor,
            ClientCommand::BlueprintLock { hash },
            codes::PLACEMENT_HASH_INVALID,
        );
    }
    assert_rejected(
        &mut mayor,
        ClientCommand::BlueprintUnlock {
            hash: "short".to_string(),
        },
        codes::PLACEMENT_HASH_INVALID,
    );

    match mayor.apply(&ClientCommand::BlueprintLock {
        hash: HASH_BARE.to_string(),
    }) {
        ServerMessage::BlueprintLockResult { lock, .. } => assert_eq!(lock.hash, canonical),
        other => panic!("expected lock result, got {other:?}"),
    }
    assert_eq!(mayor.blueprint_locks.len(), 1);
    let locked = snapshot(&mayor);
    assert_rejected(
        &mut mayor,
        ClientCommand::BlueprintUnlock {
            hash: HASH_BARE.to_ascii_uppercase(),
        },
        codes::PLACEMENT_HASH_INVALID,
    );
    assert_rejected(
        &mut mayor,
        ClientCommand::BlueprintUnlock {
            hash: format!("blake3:{}", "f".repeat(64)),
        },
        codes::BLUEPRINT_LOCK_UNKNOWN,
    );
    assert_eq!(snapshot(&mayor), locked);

    let mut viewer = hello_world(ClientRole::Viewer, "viewer");
    let before_viewer = snapshot(&viewer);
    assert_rejected(
        &mut viewer,
        ClientCommand::BlueprintLock {
            hash: HASH_BARE.to_string(),
        },
        world_codes::FORBIDDEN,
    );
    assert_rejected(
        &mut viewer,
        ClientCommand::BlueprintUnlock {
            hash: HASH_BARE.to_string(),
        },
        world_codes::FORBIDDEN,
    );
    match viewer.apply(&ClientCommand::BlueprintLocks) {
        ServerMessage::BlueprintLockList { count, locks, .. } => {
            assert_eq!(count, 0);
            assert!(locks.is_empty());
        }
        other => panic!("expected lock list, got {other:?}"),
    }
    assert_eq!(snapshot(&viewer), before_viewer);
    assert_eq!(before.locks, 0);
}

#[test]
fn chat_and_chat_since_wire_faults() {
    for payload in [
        r#"{"type":"chat"}"#,
        r#"{"type":"chat","text":null}"#,
        r#"{"type":"chat","text":1}"#,
        r#"{"type":"chat","text":[]}"#,
        r#"{"type":"chat","text":{}}"#,
        r#"{"type":"chat","text":true}"#,
        r#"{"type":"chat_since"}"#,
        r#"{"type":"chat_since","after":null}"#,
        r#"{"type":"chat_since","after":-1}"#,
        r#"{"type":"chat_since","after":1.5}"#,
        r#"{"type":"chat_since","after":"1"}"#,
        r#"{"type":"chat_since","after":true}"#,
        r#"{"type":"chat_since","after":18446744073709551616}"#,
    ] {
        assert_invalid_fields(payload);
    }
    assert_eq!(
        decode_ok(r#"{"type":"chat","text":"hello","extra":1}"#),
        ClientCommand::Chat {
            text: "hello".to_string()
        }
    );
    assert_eq!(
        decode_ok(r#"{"type":"chat_since","after":18446744073709551615}"#),
        ClientCommand::ChatSince { after: u64::MAX }
    );

    let mut world = hello_world(ClientRole::Viewer, "viewer");
    let before = snapshot(&world);
    match world.apply(&ClientCommand::Chat {
        text: "  hello  ".to_string(),
    }) {
        ServerMessage::ChatSent { entry, .. } => {
            assert_eq!(entry.seq, 1);
            assert_eq!(entry.text, "hello");
        }
        other => panic!("expected chat_sent, got {other:?}"),
    }
    assert_eq!(world.chat_seq(), 1);
    assert_eq!(world.tick, before.tick);
    assert_eq!(world.checksum(), before.world_checksum);

    for (after, expected) in [(0, 1), (1, 0), (u64::MAX, 0)] {
        match world.apply(&ClientCommand::ChatSince { after }) {
            ServerMessage::ChatLog { entries, .. } => assert_eq!(entries.len(), expected),
            other => panic!("expected chat_log, got {other:?}"),
        }
    }
    assert_eq!(world.tick, before.tick);
    assert_eq!(world.checksum(), before.world_checksum);
}

#[test]
fn found_settlement_wire_faults_and_authority_mapping() {
    for payload in [
        r#"{"type":"found_settlement"}"#,
        r#"{"type":"found_settlement","name":"A","x":0,"z":0}"#,
        r#"{"type":"found_settlement","name":null,"x":0,"z":0,"radius_m":64}"#,
        r#"{"type":"found_settlement","name":"A","x":0.5,"z":0,"radius_m":64}"#,
        r#"{"type":"found_settlement","name":"A","x":"0","z":0,"radius_m":64}"#,
        r#"{"type":"found_settlement","name":"A","x":0,"z":null,"radius_m":64}"#,
        r#"{"type":"found_settlement","name":"A","x":0,"z":0,"radius_m":"64"}"#,
        r#"{"type":"found_settlement","name":"A","x":0,"z":0,"radius_m":-1}"#,
        r#"{"type":"found_settlement","name":"A","x":2147483648,"z":0,"radius_m":64}"#,
    ] {
        assert_invalid_fields(payload);
    }
    assert_eq!(
        decode_ok(
            r#"{"type":"found_settlement","name":"A","x":1,"z":-2,"radius_m":16,"extra":true}"#
        ),
        ClientCommand::FoundSettlement {
            name: "A".to_string(),
            x: 1,
            z: -2,
            radius_m: 16,
        }
    );

    let mut viewer = hello_world(ClientRole::Viewer, "viewer");
    assert_rejected(
        &mut viewer,
        ClientCommand::FoundSettlement {
            name: "A".to_string(),
            x: 0,
            z: 0,
            radius_m: 64,
        },
        world_codes::FORBIDDEN,
    );
    assert!(viewer.settlements.is_none());

    let mut mayor = hello_world(ClientRole::Mayor, "mayor");
    for (command, expected) in [
        (
            ClientCommand::FoundSettlement {
                name: "   ".to_string(),
                x: 0,
                z: 0,
                radius_m: 64,
            },
            "empty_name",
        ),
        (
            ClientCommand::FoundSettlement {
                name: "A".to_string(),
                x: 0,
                z: 0,
                radius_m: 15,
            },
            "invalid_radius",
        ),
        (
            ClientCommand::FoundSettlement {
                name: "A".to_string(),
                x: 0,
                z: 0,
                radius_m: 2_001,
            },
            "invalid_radius",
        ),
    ] {
        assert_eq!(mayor.tick, 0);
        match mayor.apply(&command) {
            ServerMessage::Reject { code, .. } => assert_eq!(code, expected),
            other => panic!("expected {expected}, got {other:?}"),
        }
        assert_eq!(mayor.tick, 0);
        assert!(
            mayor
                .settlements
                .as_ref()
                .expect("found creates the store")
                .settlements
                .is_empty(),
            "a rejected found must not create a record"
        );
    }
}

#[test]
fn settlement_mutation_payload_faults() {
    for payload in [
        r#"{"type":"settlement_invite"}"#,
        r#"{"type":"settlement_invite","settlement_id":1,"player_id":1,"role":"member"}"#,
        r#"{"type":"settlement_invite","settlement_id":"1","player_id":1,"role":"member","expected_revision":0}"#,
        r#"{"type":"settlement_invite","settlement_id":1,"player_id":1,"role":"member","expected_revision":-1}"#,
        r#"{"type":"settlement_set_role"}"#,
        r#"{"type":"settlement_set_role","settlement_id":1,"player_id":1,"expected_revision":0}"#,
        r#"{"type":"settlement_set_role","settlement_id":1,"player_id":1,"role":1,"expected_revision":0}"#,
        r#"{"type":"settlement_remove_member"}"#,
        r#"{"type":"settlement_remove_member","settlement_id":1,"player_id":1}"#,
        r#"{"type":"settlement_remove_member","settlement_id":1,"player_id":null,"expected_revision":0}"#,
        r#"{"type":"settlement_claim"}"#,
        r#"{"type":"settlement_claim","settlement_id":1}"#,
        r#"{"type":"settlement_claim","settlement_id":"1","expected_revision":0}"#,
        r#"{"type":"settlement_rename"}"#,
        r#"{"type":"settlement_rename","settlement_id":1,"name":null,"expected_revision":0}"#,
        r#"{"type":"settlement_transfer"}"#,
        r#"{"type":"settlement_transfer","settlement_id":1,"player_id":1,"expected_revision":1.5}"#,
        r#"{"type":"settlement_set_tax"}"#,
        r#"{"type":"settlement_set_tax","settlement_id":1}"#,
        r#"{"type":"settlement_set_tax","settlement_id":1,"rate":"10"}"#,
        r#"{"type":"settlement_release"}"#,
        r#"{"type":"settlement_release","settlement_id":1}"#,
        r#"{"type":"settlement_release","settlement_id":1,"expected_revision":4294967296}"#,
    ] {
        assert_invalid_fields(payload);
    }
    assert_eq!(
        decode_ok(
            r#"{"type":"settlement_claim","settlement_id":7,"expected_revision":2,"extra":true}"#
        ),
        ClientCommand::SettlementClaim {
            settlement_id: 7,
            expected_revision: 2,
        }
    );
    assert_eq!(
        decode_ok(
            r#"{"type":"settlement_rename","settlement_id":1,"name":"A","expected_revision":0}"#
        ),
        ClientCommand::SettlementRename {
            settlement_id: 1,
            name: "A".to_string(),
            expected_revision: 0,
        }
    );
    assert_eq!(
        decode_ok(
            r#"{"type":"settlement_transfer","settlement_id":1,"player_id":1,"expected_revision":0}"#
        ),
        ClientCommand::SettlementTransfer {
            settlement_id: 1,
            player_id: 1,
            expected_revision: 0,
        }
    );
}

#[test]
fn settlement_mutations_gate_roles_and_reject_unknown_ids_without_state() {
    let mut viewer = hello_world(ClientRole::Viewer, "viewer");
    let commands = vec![
        ClientCommand::SettlementInvite {
            settlement_id: 1,
            player_id: 1,
            role: "member".to_string(),
            expected_revision: 0,
        },
        ClientCommand::SettlementSetRole {
            settlement_id: 1,
            player_id: 1,
            role: "member".to_string(),
            expected_revision: 0,
        },
        ClientCommand::SettlementRemoveMember {
            settlement_id: 1,
            player_id: 1,
            expected_revision: 0,
        },
        ClientCommand::SettlementClaim {
            settlement_id: 1,
            expected_revision: 0,
        },
        ClientCommand::SettlementRename {
            settlement_id: 1,
            name: "A".to_string(),
            expected_revision: 0,
        },
        ClientCommand::SettlementTransfer {
            settlement_id: 1,
            player_id: 1,
            expected_revision: 0,
        },
        ClientCommand::SettlementSetTax {
            settlement_id: 1,
            rate: 10,
        },
        ClientCommand::FoundSettlement {
            name: "A".to_string(),
            x: 0,
            z: 0,
            radius_m: 64,
        },
    ];
    for command in commands {
        assert_rejected(&mut viewer, command, world_codes::FORBIDDEN);
    }
    assert!(viewer.settlements.is_none());

    let mut mayor = hello_world(ClientRole::Mayor, "mayor");
    let before = snapshot(&mayor);
    for command in [
        ClientCommand::SettlementInvite {
            settlement_id: 99,
            player_id: 1,
            role: "member".to_string(),
            expected_revision: 0,
        },
        ClientCommand::SettlementSetRole {
            settlement_id: 99,
            player_id: 1,
            role: "member".to_string(),
            expected_revision: 0,
        },
        ClientCommand::SettlementRemoveMember {
            settlement_id: 99,
            player_id: 1,
            expected_revision: 0,
        },
        ClientCommand::SettlementClaim {
            settlement_id: 99,
            expected_revision: 0,
        },
        ClientCommand::SettlementRename {
            settlement_id: 99,
            name: "A".to_string(),
            expected_revision: 0,
        },
        ClientCommand::SettlementTransfer {
            settlement_id: 99,
            player_id: 1,
            expected_revision: 0,
        },
        ClientCommand::SettlementSetTax {
            settlement_id: 99,
            rate: 10,
        },
    ] {
        assert_rejected(&mut mayor, command, world_codes::SETTLEMENT_UNKNOWN);
    }
    assert!(mayor.settlements.is_none());
    assert_eq!(snapshot(&mayor), before);

    let mut admin = hello_world(ClientRole::Admin, "admin");
    assert_rejected(
        &mut admin,
        ClientCommand::SettlementRelease {
            settlement_id: 99,
            expected_revision: 0,
        },
        world_codes::SETTLEMENT_UNKNOWN,
    );
    assert!(admin.settlements.is_none());
}

#[test]
fn newer_commands_are_unavailable_in_the_transport_session() {
    let mut transport = Session::new(SEED);
    assert!(matches!(
        transport.handle(hello("transport")).as_slice(),
        [ServerMessage::Welcome { .. }]
    ));
    let before = (transport.tick(), transport.checksum());
    for (command, expected) in [
        (
            ClientCommand::PlayerList {
                after_id: None,
                limit: None,
            },
            codes::PLAYER_UNAVAILABLE,
        ),
        (
            ClientCommand::SettlementList {
                after_id: None,
                limit: None,
            },
            codes::SETTLEMENT_UNAVAILABLE,
        ),
        (
            ClientCommand::SettlementAt { x: 0, z: 0 },
            codes::SETTLEMENT_UNAVAILABLE,
        ),
        (
            ClientCommand::FoundSettlement {
                name: "A".to_string(),
                x: 0,
                z: 0,
                radius_m: 64,
            },
            codes::SETTLEMENT_UNAVAILABLE,
        ),
        (ClientCommand::BlueprintLocks, codes::BLUEPRINT_UNAVAILABLE),
        (
            ClientCommand::BlueprintLock {
                hash: HASH_BARE.to_string(),
            },
            codes::BLUEPRINT_UNAVAILABLE,
        ),
        (
            ClientCommand::BlueprintUnlock {
                hash: HASH_BARE.to_string(),
            },
            codes::BLUEPRINT_UNAVAILABLE,
        ),
        (
            ClientCommand::AuditLog {
                limit: None,
                tag: None,
                accepted: None,
                role: None,
            },
            codes::AUDIT_UNAVAILABLE,
        ),
        (
            ClientCommand::Chat {
                text: "hello".to_string(),
            },
            codes::CHAT_UNAVAILABLE,
        ),
        (
            ClientCommand::ChatSince { after: 0 },
            codes::CHAT_UNAVAILABLE,
        ),
    ] {
        match transport.handle(command).as_slice() {
            [ServerMessage::Reject { code, .. }] => assert_eq!(code, expected),
            other => panic!("expected {expected} from transport, got {other:?}"),
        }
    }
    assert_eq!((transport.tick(), transport.checksum()), before);
}
