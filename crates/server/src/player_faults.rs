//! Deterministic identity and `player_list` fault corpus.
//!
//! The cases drive the authoritative [`ServerWorld`] and the transport-only
//! [`Session`] boundary. They pin the roster limits, case-insensitive identity
//! reuse, paging arithmetic, role gates, and the invariant that a read never
//! changes the tick, world checksum, or registry checksum.
//!
//! No network, filesystem, or wall clock is involved.

use crate::protocol::{
    codes, decode_client_command, ClientCommand, ServerMessage, Session, PLAYER_LIST_PAGE_CAP,
    PROTOCOL_VERSION,
};
use crate::world::{codes as world_codes, ClientRole, ServerWorld};
use core_city::player::{PlayerError, MAX_PLAYERS};

const SEED: u64 = 2026;
const CONTENT_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[derive(Debug, Clone, PartialEq)]
struct Page {
    tick: u64,
    count: usize,
    matched: u32,
    next_after: u64,
    has_more: bool,
    checksum: String,
    ids: Vec<u64>,
    names: Vec<String>,
}

fn hello_named(name: &str) -> ClientCommand {
    ClientCommand::Hello {
        protocol_version: PROTOCOL_VERSION,
        content_hash: CONTENT_HASH.to_string(),
        player_name: name.to_string(),
    }
}

#[track_caller]
fn welcome(world: &mut ServerWorld, name: &str) -> u64 {
    let message = world.apply(&hello_named(name));
    assert!(
        matches!(message, ServerMessage::Welcome { .. }),
        "expected welcome for {name}, got {message:?}"
    );
    world.player_id().expect("hello assigns an id").get()
}

#[track_caller]
fn fetch_page(world: &mut ServerWorld, after_id: Option<u64>, limit: Option<u32>) -> Page {
    let response = world.apply(&ClientCommand::PlayerList { after_id, limit });
    let ServerMessage::PlayerList {
        tick,
        count,
        matched,
        next_after,
        has_more,
        checksum,
        players,
    } = response
    else {
        panic!("expected player_list, got {response:?}");
    };
    Page {
        tick,
        count,
        matched,
        next_after,
        has_more,
        checksum,
        ids: players.iter().map(|player| player.id).collect(),
        names: players.into_iter().map(|player| player.name).collect(),
    }
}

#[track_caller]
fn reject_code(world: &mut ServerWorld, command: &ClientCommand) -> String {
    let messages = world.apply_all(command);
    match messages.as_slice() {
        [ServerMessage::Reject { code, .. }] => code.clone(),
        other => panic!("expected one reject, got {other:?}"),
    }
}

fn snapshot(world: &ServerWorld) -> (u64, String, u64) {
    (world.tick, world.checksum(), world.players.checksum())
}

#[test]
fn empty_single_and_full_registries_hold_their_limits() {
    let mut empty = ServerWorld::with_role(SEED, ClientRole::Admin);
    let initial = welcome(&mut empty, "mayor");
    assert_eq!(initial, 1);
    empty.players = core_city::player::PlayerRegistry::new(SEED);
    let page = fetch_page(&mut empty, None, None);
    assert_eq!(page.count, 0);
    assert_eq!(page.matched, 0);
    assert_eq!(page.next_after, 0);
    assert!(!page.has_more);
    assert_eq!(page.checksum, format!("{:016x}", empty.players.checksum()));

    let mut single = ServerWorld::with_role(SEED, ClientRole::Admin);
    welcome(&mut single, "solo");
    let page = fetch_page(&mut single, None, None);
    assert_eq!(page.ids, vec![1]);
    assert_eq!(page.names, vec!["solo"]);
    assert_eq!(page.count, 1);
    assert_eq!(page.matched, 1);
    assert_eq!(page.next_after, 1);
    assert!(!page.has_more);

    let mut full = ServerWorld::with_role(SEED, ClientRole::Admin);
    for index in 0..MAX_PLAYERS {
        full.players
            .register(&format!("player{index}"))
            .expect("registration below the cap");
    }
    assert_eq!(full.players.count(), MAX_PLAYERS);
    assert_eq!(
        full.players.register("overflow"),
        Err(PlayerError::TooMany),
        "the registry boundary reports TooMany"
    );
    assert_eq!(
        reject_code(&mut full, &hello_named("overflow")),
        world_codes::PLAYER_IDENTITY,
        "the authority maps a full registry to the stable hello code"
    );
    assert_eq!(full.players.count(), MAX_PLAYERS);
    welcome(&mut full, "player0");
    let page = fetch_page(&mut full, None, Some(PLAYER_LIST_PAGE_CAP));
    assert_eq!(page.count, PLAYER_LIST_PAGE_CAP as usize);
    assert_eq!(page.matched, MAX_PLAYERS as u32);
    assert!(!page.has_more, "one full page covers the whole registry");
    assert_eq!(page.ids, (1..=256).collect::<Vec<u64>>());
}

#[test]
fn names_are_trimmed_case_insensitive_and_keep_the_first_label() {
    let mut world = ServerWorld::with_role(SEED, ClientRole::Admin);
    world.players.register("Alice").expect("seed alice");
    let id = welcome(&mut world, "  aLiCe  ");
    assert_eq!(id, 1);
    assert_eq!(world.players.count(), 1);
    assert_eq!(world.players.players()[0].display_name, "Alice");

    let mut second = ServerWorld::with_role(SEED, ClientRole::Admin);
    second.players.register("Alice").expect("seed alice");
    let bob = welcome(&mut second, "bob");
    assert_ne!(bob, id);
    let page = fetch_page(&mut second, None, None);
    assert_eq!(page.ids, vec![id, bob]);
    assert_eq!(page.names, vec!["Alice", "bob"]);
}

#[test]
fn invalid_names_do_not_mutate_the_registry() {
    for (name, expected) in [
        ("", codes::PLAYER_NAME_EMPTY),
        ("   ", codes::PLAYER_NAME_EMPTY),
        ("bad\nname", codes::PLAYER_NAME_INVALID),
        ("bad\u{7f}name", codes::PLAYER_NAME_INVALID),
    ] {
        let mut world = ServerWorld::with_role(SEED, ClientRole::Admin);
        let before = snapshot(&world);
        assert_eq!(reject_code(&mut world, &hello_named(name)), expected);
        assert_eq!(snapshot(&world), before, "rejected name mutated the world");
    }
    let mut world = ServerWorld::with_role(SEED, ClientRole::Admin);
    let before = snapshot(&world);
    let overlong = "x".repeat(33);
    assert_eq!(
        reject_code(&mut world, &hello_named(&overlong)),
        codes::PLAYER_NAME_TOO_LONG
    );
    assert_eq!(snapshot(&world), before);
}

#[test]
fn paging_cursors_and_clamps_are_exact() {
    let mut world = ServerWorld::with_role(SEED, ClientRole::Admin);
    welcome(&mut world, "p0");
    for index in 1..8 {
        world
            .players
            .register(&format!("p{index}"))
            .expect("seed roster");
    }
    let all = fetch_page(&mut world, None, None);
    assert_eq!(all.ids, (1..=8).collect::<Vec<u64>>());
    assert_eq!(all.matched, 8);
    assert_eq!(all.next_after, 8);
    assert!(!all.has_more);

    let first = fetch_page(&mut world, None, Some(3));
    assert_eq!(first.ids, vec![1, 2, 3]);
    assert_eq!(first.matched, 8);
    assert_eq!(first.next_after, 3);
    assert!(first.has_more);

    let second = fetch_page(&mut world, Some(3), Some(3));
    assert_eq!(second.ids, vec![4, 5, 6]);
    assert_eq!(second.matched, 5);
    assert_eq!(second.next_after, 6);
    assert!(second.has_more);

    let last = fetch_page(&mut world, Some(6), Some(3));
    assert_eq!(last.ids, vec![7, 8]);
    assert_eq!(last.matched, 2);
    assert_eq!(last.next_after, 8);
    assert!(!last.has_more);

    let empty = fetch_page(&mut world, Some(99), Some(3));
    assert_eq!(empty.ids, Vec::<u64>::new());
    assert_eq!(empty.count, 0);
    assert_eq!(empty.matched, 0);
    assert_eq!(empty.next_after, 99);
    assert!(!empty.has_more);

    assert_eq!(fetch_page(&mut world, None, Some(0)).ids, vec![1]);
    assert_eq!(fetch_page(&mut world, None, Some(257)).count, 8);
    assert_eq!(
        fetch_page(&mut world, Some(0), Some(PLAYER_LIST_PAGE_CAP)).count,
        8
    );
}

#[test]
fn viewer_mayor_and_admin_can_read_the_roster() {
    for role in [ClientRole::Viewer, ClientRole::Mayor, ClientRole::Admin] {
        let mut world = ServerWorld::with_role(SEED, role);
        welcome(&mut world, "reader");
        let response = world.apply(&ClientCommand::PlayerList {
            after_id: None,
            limit: None,
        });
        assert!(
            matches!(response, ServerMessage::PlayerList { .. }),
            "{role:?} should read the roster"
        );
    }
}

#[test]
fn transport_session_rejects_player_list_without_state_change() {
    let mut transport = Session::new(SEED);
    let handshake = transport.handle(hello_named("transport"));
    assert!(matches!(
        handshake.as_slice(),
        [ServerMessage::Welcome { .. }]
    ));
    let messages = transport.handle(ClientCommand::PlayerList {
        after_id: None,
        limit: None,
    });
    let code = match messages.as_slice() {
        [ServerMessage::Reject { code, .. }] => code.clone(),
        other => panic!("expected transport reject, got {other:?}"),
    };
    assert_eq!(code, codes::PLAYER_UNAVAILABLE);
}

#[test]
fn reads_do_not_move_the_tick_or_checksums() {
    let mut world = ServerWorld::with_role(SEED, ClientRole::Admin);
    welcome(&mut world, "mayor");
    for index in 0..5 {
        world
            .players
            .register(&format!("p{index}"))
            .expect("seed roster");
    }
    let before = snapshot(&world);
    let first = fetch_page(&mut world, None, Some(2));
    let second = fetch_page(&mut world, Some(2), Some(2));
    let tail = fetch_page(&mut world, Some(4), Some(2));
    assert_eq!(snapshot(&world), before);
    assert_eq!(first.checksum, second.checksum);
    assert_eq!(second.checksum, tail.checksum);
    assert_eq!(first.tick, before.0);
}

#[test]
fn resume_keeps_the_player_id_and_roster() {
    let mut world = ServerWorld::with_role(SEED, ClientRole::Admin);
    let id = welcome(&mut world, "alice");
    let token = world.session_token().to_string();
    world.apply(&ClientCommand::Disconnect);
    assert!(world.is_resumable());
    world.begin_connection();

    let messages = world.apply_all(&ClientCommand::Resume {
        session_id: token,
        last_tick: 0,
    });
    assert!(
        matches!(
            messages.first(),
            Some(ServerMessage::Welcome { resumed: true, .. })
        ),
        "resume messages: {messages:?}"
    );
    assert_eq!(world.player_id().map(|player| player.get()), Some(id));
    let page = fetch_page(&mut world, None, None);
    assert_eq!(page.ids, vec![id]);
}

#[test]
fn save_and_load_preserve_ids_names_and_paging() {
    let mut world = ServerWorld::with_role(SEED, ClientRole::Admin);
    welcome(&mut world, "alice");
    world.players.register("bob").expect("bob");
    world.players.register("carol").expect("carol");
    let json = world.to_save_json_pretty().expect("save");
    let mut loaded = ServerWorld::from_save_json(&json).expect("load");
    assert_eq!(loaded.players, world.players);
    assert_eq!(loaded.players.checksum(), world.players.checksum());
    welcome(&mut loaded, "dave");
    let page = fetch_page(&mut loaded, None, Some(2));
    assert_eq!(page.ids, vec![1, 2]);
    assert!(page.has_more);
    assert_eq!(page.names, vec!["alice", "bob"]);
}

#[test]
fn malformed_player_list_payloads_do_not_mutate() {
    let mut world = ServerWorld::with_role(SEED, ClientRole::Admin);
    welcome(&mut world, "mayor");
    let before = snapshot(&world);
    for raw in [
        br#"{"type":"player_list","after_id":"x"}"#.as_slice(),
        br#"{"type":"player_list","limit":-1}"#.as_slice(),
        br#"{"type":"player_list","after_id":1.5}"#.as_slice(),
    ] {
        let error = decode_client_command(raw).expect_err("malformed payload");
        assert_eq!(error.code, codes::COMMAND_INVALID_FIELDS);
        assert_eq!(snapshot(&world), before);
    }
    let unknown = decode_client_command(br#"{"type":"no_such_command"}"#).expect_err("unknown tag");
    assert_eq!(unknown.code, codes::COMMAND_UNKNOWN_TAG);
    assert_eq!(snapshot(&world), before);
}
