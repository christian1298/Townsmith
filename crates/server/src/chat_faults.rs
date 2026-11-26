//! Deterministic malformed/invalid chat command corpus.
//!
//! Every rejection asserts the stable code and that the tick, world checksum,
//! chat sequence, and retained log are unchanged. No network or wall clock is
//! involved; two-connection cases use the same deterministic session path as
//! the authoritative executor.

use crate::protocol::{
    ClientCommand, ServerMessage, Session, CHAT_LOG_CAPACITY, MAX_CHAT_CHARS, PROTOCOL_VERSION,
};
use crate::world::{ClientRole, ServerWorld};

const SEED: u64 = 2026;
const CONTENT_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    tick: u64,
    world_checksum: String,
    chat_seq: u64,
    chat_len: usize,
    last: Option<(u64, String, String)>,
}

fn snapshot(world: &ServerWorld) -> Snapshot {
    Snapshot {
        tick: world.tick,
        world_checksum: world.checksum(),
        chat_seq: world.chat_seq(),
        chat_len: world.chat.len(),
        last: world
            .chat
            .back()
            .map(|entry| (entry.seq, entry.player.clone(), entry.text.clone())),
    }
}

fn hello(name: &str) -> ClientCommand {
    ClientCommand::Hello {
        protocol_version: PROTOCOL_VERSION,
        content_hash: CONTENT_HASH.to_string(),
        player_name: name.to_string(),
    }
}

fn chat(text: &str) -> ClientCommand {
    ClientCommand::Chat {
        text: text.to_string(),
    }
}

fn fresh_world() -> ServerWorld {
    let mut world = ServerWorld::with_role(SEED, ClientRole::Viewer);
    let response = world.apply(&hello("mayor"));
    assert!(
        matches!(response, ServerMessage::Welcome { .. }),
        "{response:?}"
    );
    world
}

fn accept_chat(world: &mut ServerWorld, text: &str) -> u64 {
    let response = world.apply(&chat(text));
    match response {
        ServerMessage::ChatSent { entry, .. } => entry.seq,
        other => panic!("expected chat_sent, got {other:?}"),
    }
}

fn assert_rejected(world: &mut ServerWorld, command: ClientCommand, code: &str) {
    let before = snapshot(world);
    let response = world.apply(&command);
    match response {
        ServerMessage::Reject { code: actual, .. } => assert_eq!(actual, code, "{command:?}"),
        other => panic!("expected reject {code}, got {other:?} for {command:?}"),
    }
    assert_eq!(
        snapshot(world),
        before,
        "rejection mutated chat state for {command:?}"
    );
}

#[test]
fn chat_rejects_empty_whitespace_and_control_text() {
    let mut world = fresh_world();
    for text in [
        "",
        "   ",
        "\t\n",
        "\u{2003}\u{3000}",
        "a\0b",
        "a\nb",
        "a\tb",
        "a\u{7f}b",
    ] {
        let code = match text {
            "" | "   " | "\u{2003}\u{3000}" | "\t\n" => "chat.empty",
            _ => "chat.invalid",
        };
        assert_rejected(&mut world, chat(text), code);
    }
}

#[test]
fn chat_accepts_boundary_lengths_and_trims() {
    let mut world = fresh_world();
    let exactly = "x".repeat(MAX_CHAT_CHARS);
    let seq = accept_chat(&mut world, &format!("  {exactly}  "));
    let entry = world.chat.back().expect("entry");
    assert_eq!(entry.seq, seq);
    assert_eq!(entry.text, exactly);
    assert_eq!(world.chat.len(), 1);

    let unicode = "é".repeat(MAX_CHAT_CHARS);
    assert_eq!(accept_chat(&mut world, &unicode), 2);
    assert_eq!(world.chat.back().expect("entry").text, unicode);
}

#[test]
fn chat_rejects_overlong_ascii_and_unicode() {
    let mut world = fresh_world();
    assert_rejected(
        &mut world,
        chat(&"x".repeat(MAX_CHAT_CHARS + 1)),
        "chat.too_long",
    );
    assert_rejected(
        &mut world,
        chat(&"😀".repeat(MAX_CHAT_CHARS + 1)),
        "chat.too_long",
    );
    assert_rejected(
        &mut world,
        chat(&"é".repeat(MAX_CHAT_CHARS + 1)),
        "chat.too_long",
    );
}

#[test]
fn chat_requires_hello_and_a_registry_identity() {
    let mut world = ServerWorld::with_role(SEED, ClientRole::Viewer);
    assert_rejected(&mut world, chat("hello"), "not_hello");

    let mut world = fresh_world();
    world.players = core_city::player::PlayerRegistry::new(SEED);
    assert_rejected(&mut world, chat("hello"), "chat.identity");
}

#[test]
fn chat_burst_is_bounded_at_48_per_session() {
    let mut world = fresh_world();
    let mut accepted = 0;
    let mut rejection = None;
    for index in 0..60 {
        match world.apply(&chat(&format!("line {index}"))) {
            ServerMessage::ChatSent { .. } => accepted += 1,
            ServerMessage::Reject { code, .. } => {
                rejection = Some(code);
                break;
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(accepted, 48);
    assert_eq!(rejection.as_deref(), Some("rate.limited"));
    assert_eq!(world.chat_seq(), 48);
    assert_eq!(world.chat.len(), 48);
}

#[test]
fn chat_log_caps_at_64_and_evicts_oldest() {
    let mut world = fresh_world();
    for index in 0..48 {
        assert_eq!(
            accept_chat(&mut world, &format!("first {index}")),
            index + 1
        );
    }
    for _ in 0..5 {
        world.tick += 1;
        for index in 0..16 {
            accept_chat(&mut world, &format!("later {index}"));
        }
    }
    assert_eq!(world.chat_seq(), 128);
    assert_eq!(world.chat.len(), CHAT_LOG_CAPACITY);
    assert_eq!(world.chat.front().expect("front").seq, 65);
    assert_eq!(world.chat.back().expect("back").seq, 128);
}

#[test]
fn chat_since_is_read_only_and_bounded() {
    let mut world = fresh_world();
    for index in 0..3 {
        accept_chat(&mut world, &format!("line {index}"));
    }
    let before = snapshot(&world);
    let response = world.apply(&ClientCommand::ChatSince { after: 0 });
    match response {
        ServerMessage::ChatLog { entries, .. } => {
            assert_eq!(entries.len(), 3);
            assert_eq!(entries[0].seq, 1);
            assert_eq!(entries[2].seq, 3);
        }
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(snapshot(&world), before);

    for after in [1, 3, 4, u64::MAX] {
        let response = world.apply(&ClientCommand::ChatSince { after });
        let expected = match after {
            1 => 2,
            3 => 0,
            _ => 0,
        };
        match response {
            ServerMessage::ChatLog { entries, .. } => assert_eq!(entries.len(), expected),
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(snapshot(&world), before);
}

#[test]
fn chat_since_decode_rejects_missing_negative_and_overflow() {
    for json in [
        r#"{"type":"chat_since"}"#,
        r#"{"type":"chat_since","after":-1}"#,
        r#"{"type":"chat_since","after":1.5}"#,
        r#"{"type":"chat_since","after":18446744073709551616}"#,
        r#"{"type":"chat_since","after":"1"}"#,
    ] {
        assert!(
            serde_json::from_str::<ClientCommand>(json).is_err(),
            "accepted {json}"
        );
    }
}

#[test]
fn two_connections_share_the_log_without_duplicates() {
    let mut world = ServerWorld::with_role(SEED, ClientRole::Viewer);
    let alice = world.open_connection();
    assert!(matches!(
        world.apply_session(alice, &hello("alice"))[0],
        ServerMessage::Welcome { .. }
    ));
    let bob = world.open_connection();
    assert!(matches!(
        world.apply_session(bob, &hello("bob"))[0],
        ServerMessage::Welcome { .. }
    ));

    assert!(matches!(
        world.apply_session(alice, &chat("from alice"))[0],
        ServerMessage::ChatSent { .. }
    ));
    let bob_view = world.apply_session(bob, &ClientCommand::ChatSince { after: 0 });
    match &bob_view[0] {
        ServerMessage::ChatLog { entries, .. } => {
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].text, "from alice");
        }
        other => panic!("unexpected {other:?}"),
    }

    assert!(matches!(
        world.apply_session(bob, &chat("from bob"))[0],
        ServerMessage::ChatSent { .. }
    ));
    let alice_view = world.apply_session(alice, &ClientCommand::ChatSince { after: 1 });
    match &alice_view[0] {
        ServerMessage::ChatLog { entries, .. } => {
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].seq, 2);
            assert_eq!(entries[0].text, "from bob");
        }
        other => panic!("unexpected {other:?}"),
    }

    // A rejection from one session does not append or disturb the other.
    assert!(matches!(
        &world.apply_session(bob, &chat("   "))[0],
        ServerMessage::Reject { code, .. } if code == "chat.empty"
    ));
    let still_two = world.apply_session(alice, &ClientCommand::ChatSince { after: 0 });
    match still_two[0] {
        ServerMessage::ChatLog { ref entries, .. } => assert_eq!(entries.len(), 2),
        ref other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn chat_is_not_persisted_and_restarts_empty() {
    let mut world = fresh_world();
    accept_chat(&mut world, "before save");
    let json = world.to_save_json_pretty().expect("save");
    assert!(!json.contains("before save"));
    let loaded = ServerWorld::from_save_json(&json).expect("load");
    assert!(loaded.chat.is_empty());
    assert_eq!(loaded.chat_seq(), 0);

    let mut loaded = loaded;
    let response = loaded.apply(&hello("mayor"));
    assert!(matches!(response, ServerMessage::Welcome { .. }));
    assert_eq!(accept_chat(&mut loaded, "after load"), 1);
}

#[test]
fn transport_session_rejects_chat_paths() {
    let mut session = Session::new(SEED);
    let pre = session.handle(chat("hello"));
    match pre.as_slice() {
        [ServerMessage::Reject { code, .. }] => assert_eq!(code, "hello.required"),
        other => panic!("unexpected {other:?}"),
    }
    session.handle(hello("mayor"));
    let post = session.handle(chat("hello"));
    match post.as_slice() {
        [ServerMessage::Reject { code, .. }] => assert_eq!(code, "chat.unavailable"),
        other => panic!("unexpected {other:?}"),
    }
    let since = session.handle(ClientCommand::ChatSince { after: 0 });
    match since.as_slice() {
        [ServerMessage::Reject { code, .. }] => assert_eq!(code, "chat.unavailable"),
        other => panic!("unexpected {other:?}"),
    }
}
