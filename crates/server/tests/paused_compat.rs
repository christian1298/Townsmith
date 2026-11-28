//! Wire compatibility corpus for the additive `paused` field.
//!
//! `snapshot` and `server_status` gained an additive `paused` boolean after the
//! first protocol-v1 payloads shipped. The field is optional on decode
//! (`#[serde(default)]`, so older payloads read as unpaused), always present on
//! encode, and strictly typed: `null`, strings, numbers, arrays, and objects
//! fail closed instead of being coerced. Unknown sibling fields stay ignored
//! for forward compatibility.
//!
//! Every case is in memory: no sockets, threads, sleeps, randomness, or wall
//! clock. Old and new payloads also pass through the real length-prefixed
//! frame reader (`write_frame` + `read_json_frame`), not only `serde_json`.

use std::io::Cursor;

use serde_json::Value;
use townsmith_server::protocol::{
    read_json_frame, write_frame, ClientCommand, ServerMessage, Session, TimeAction,
    PROTOCOL_VERSION,
};
use townsmith_server::world::{ClientRole, ServerWorld};

const SEED: u64 = 2026;
const CONTENT_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A protocol-v1 `snapshot` payload from before `paused` existed.
const OLD_SNAPSHOT: &str = r#"{"type":"snapshot","tick":7,"population":1200,"balance":250000,"checksum":"0123456789abcdef","interest_center":[1.0,2.0],"interest_radius_m":128.0,"visible_roads":3,"chat_seq":4}"#;

/// A protocol-v1 `server_status` payload from before `paused` existed.
const OLD_STATUS: &str = r#"{"type":"server_status","tick":7,"population":1200,"balance":250000,"roads":3,"buildings":2,"settlements":1,"placements":2,"online":1,"chat_seq":4,"locks":0,"checksum":"0123456789abcdef"}"#;

/// Build a current `snapshot` payload, optionally with `paused` and extras.
fn snapshot_payload(paused: Option<&str>, extra: &str) -> String {
    let paused = paused.map_or_else(String::new, |value| format!(r#","paused":{value}"#));
    format!(
        r#"{{"type":"snapshot","tick":7,"population":1200,"balance":250000,"checksum":"0123456789abcdef","interest_center":[1.0,2.0],"interest_radius_m":128.0,"visible_roads":3,"chat_seq":4{paused}{extra}}}"#
    )
}

/// Build a current `server_status` payload, optionally with `paused` and extras.
fn status_payload(paused: Option<&str>, extra: &str) -> String {
    let paused = paused.map_or_else(String::new, |value| format!(r#","paused":{value}"#));
    format!(
        r#"{{"type":"server_status","tick":7,"population":1200,"balance":250000,"roads":3,"buildings":2,"settlements":1,"placements":2,"online":1,"chat_seq":4,"locks":0,"checksum":"0123456789abcdef"{paused}{extra}}}"#
    )
}

fn snapshot(paused: bool) -> ServerMessage {
    ServerMessage::Snapshot {
        tick: 7,
        population: 1200,
        balance: 250_000,
        checksum: "0123456789abcdef".to_string(),
        interest_center: [1.0, 2.0],
        interest_radius_m: 128.0,
        visible_roads: 3,
        chat_seq: 4,
        paused,
    }
}

fn status(paused: bool) -> ServerMessage {
    ServerMessage::ServerStatus {
        tick: 7,
        population: 1200,
        balance: 250_000,
        roads: 3,
        buildings: 2,
        settlements: 1,
        placements: 2,
        online: 1,
        chat_seq: 4,
        locks: 0,
        paused,
        checksum: "0123456789abcdef".to_string(),
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
fn decode(payload: &str) -> ServerMessage {
    serde_json::from_str(payload)
        .unwrap_or_else(|error| panic!("payload should decode: {error}\n{payload}"))
}

#[track_caller]
fn decode_frame(payload: &str) -> ServerMessage {
    let mut framed = Vec::new();
    write_frame(&mut framed, payload.as_bytes()).expect("frame payload");
    read_json_frame::<_, ServerMessage>(&mut Cursor::new(framed))
        .unwrap_or_else(|error| panic!("frame should decode: {error}\n{payload}"))
}

#[track_caller]
fn assert_frame_rejects(payload: &str) {
    let mut framed = Vec::new();
    write_frame(&mut framed, payload.as_bytes()).expect("frame payload");
    assert!(
        read_json_frame::<_, ServerMessage>(&mut Cursor::new(framed)).is_err(),
        "frame reader accepted invalid payload: {payload}"
    );
}

#[track_caller]
fn paused(message: &ServerMessage) -> bool {
    match message {
        ServerMessage::Snapshot { paused, .. } | ServerMessage::ServerStatus { paused, .. } => {
            *paused
        }
        other => panic!("expected snapshot or server_status, got {other:?}"),
    }
}

#[track_caller]
fn checksum(message: &ServerMessage) -> &str {
    match message {
        ServerMessage::Snapshot { checksum, .. } | ServerMessage::ServerStatus { checksum, .. } => {
            checksum
        }
        other => panic!("expected snapshot or server_status, got {other:?}"),
    }
}

#[test]
fn old_payloads_without_paused_decode_unpaused() {
    assert_eq!(decode(OLD_SNAPSHOT), snapshot(false));
    assert_eq!(decode(OLD_STATUS), status(false));
    assert_eq!(decode_frame(OLD_SNAPSHOT), snapshot(false));
    assert_eq!(decode_frame(OLD_STATUS), status(false));
    assert_eq!(snapshot_payload(None, ""), OLD_SNAPSHOT);
    assert_eq!(status_payload(None, ""), OLD_STATUS);
}

#[test]
fn paused_round_trips_and_is_always_present_on_encode() {
    for (message, expected) in [
        (snapshot(true), true),
        (snapshot(false), false),
        (status(true), true),
        (status(false), false),
    ] {
        let encoded = serde_json::to_value(&message).expect("message serializes");
        assert_eq!(
            encoded.get("paused"),
            Some(&Value::Bool(expected)),
            "paused must be present on encode, including `false`: {encoded}"
        );
        let bytes = serde_json::to_vec(&message).expect("message serializes");
        assert_eq!(
            serde_json::from_slice::<ServerMessage>(&bytes).expect("re-decode"),
            message
        );
        assert_eq!(
            decode_frame(&String::from_utf8(bytes).expect("UTF-8")),
            message
        );
    }

    for (payload, expected) in [
        (snapshot_payload(Some("true"), ""), true),
        (snapshot_payload(Some("false"), ""), false),
        (status_payload(Some("true"), ""), true),
        (status_payload(Some("false"), ""), false),
    ] {
        assert_eq!(paused(&decode(&payload)), expected, "{payload}");
        assert_eq!(paused(&decode_frame(&payload)), expected, "{payload}");
    }
}

#[test]
fn unknown_fields_alongside_paused_are_ignored() {
    let extra = r#","future_field":{"nested":[1,2,3]},"another":"ignored""#;
    for (payload, expected) in [
        (snapshot_payload(Some("true"), extra), snapshot(true)),
        (snapshot_payload(Some("false"), extra), snapshot(false)),
        (status_payload(Some("true"), extra), status(true)),
        (status_payload(Some("false"), extra), status(false)),
        // Extras are ignored when `paused` is absent too.
        (snapshot_payload(None, extra), snapshot(false)),
        (status_payload(None, extra), status(false)),
    ] {
        assert_eq!(decode(&payload), expected, "{payload}");
        assert_eq!(decode_frame(&payload), expected, "{payload}");
    }
}

#[test]
fn non_boolean_paused_fails_closed_at_the_typed_decode() {
    for invalid in [
        "null",
        r#""true""#,
        r#""false""#,
        "1",
        "0",
        "-1",
        "1.5",
        "[]",
        "{}",
    ] {
        for payload in [
            snapshot_payload(Some(invalid), ""),
            status_payload(Some(invalid), ""),
        ] {
            assert!(
                serde_json::from_str::<ServerMessage>(&payload).is_err(),
                "serde accepted `paused: {invalid}`: {payload}"
            );
            assert_frame_rejects(&payload);
        }
    }
}

#[test]
fn transport_session_snapshot_is_always_unpaused() {
    let mut session = Session::new(SEED);
    assert!(matches!(
        session.handle(hello("transport")).as_slice(),
        [ServerMessage::Welcome { .. }]
    ));

    for command in [
        ClientCommand::TimeControl {
            action: TimeAction::Pause,
        },
        ClientCommand::SetTax {
            resident_rate: 12,
            business_rate: 8,
        },
    ] {
        match session.handle(command).as_slice() {
            [snapshot @ ServerMessage::Snapshot { .. }] => {
                assert!(
                    !paused(snapshot),
                    "the transport-only session has no time control"
                );
            }
            other => panic!("expected one snapshot, got {other:?}"),
        }
    }
}

#[test]
fn authoritative_pause_flag_is_envelope_state_not_world_checksum_state() {
    let mut world = ServerWorld::with_role(SEED, ClientRole::Admin);
    assert!(matches!(
        world.apply(&hello("admin")),
        ServerMessage::Welcome { .. }
    ));

    let unpaused_status = world.apply(&ClientCommand::ServerStatus);
    let unpaused_snapshot = world.snapshot();
    assert!(!paused(&unpaused_status));
    assert!(!paused(&unpaused_snapshot));

    let pause_result = world.apply(&ClientCommand::TimeControl {
        action: TimeAction::Pause,
    });
    assert!(paused(&pause_result));
    let paused_status = world.apply(&ClientCommand::ServerStatus);
    let paused_snapshot = world.snapshot();
    assert!(paused(&paused_status));
    assert!(paused(&paused_snapshot));

    // Deliberate: `ServerWorld::checksum` excludes the pause flag, so a
    // status/snapshot pair that differs only in `paused` keeps one world
    // checksum. This pins that documented choice (see `docs/05` §4 and the
    // `four_player_admin` integration test) rather than inventing a second
    // checksum. The transport-only `Session::checksum` does mix the flag in;
    // its unit test in `protocol.rs` covers that path.
    assert_eq!(checksum(&paused_status), checksum(&unpaused_status));
    assert_eq!(checksum(&paused_snapshot), checksum(&unpaused_snapshot));
    assert_eq!(
        checksum(&paused_status),
        checksum(&paused_snapshot),
        "status and snapshot report the same world checksum"
    );
}
