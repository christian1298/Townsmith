//! Real-socket integration for the read-only player roster and settlement paging.
//!
//! Each case starts one authoritative listen loop on an ephemeral loopback
//! port, drives two sequential sessions, and joins the loop. The assertions
//! cover cursor walks, case-insensitive identity reuse, permission gates, and
//! the invariant that list reads never advance the tick or move the world
//! checksum.

use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::thread;

use townsmith_server::listen::{run_listen, ListenConfig, ListenReport};
use townsmith_server::protocol::{
    read_json_frame, write_json_frame, ClientCommand, ServerMessage, LOOPBACK_SEED,
    PROTOCOL_VERSION, SESSION_IO_TIMEOUT,
};
use townsmith_server::world::{ClientRole, ServerWorld};

const CONTENT_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn start_listen(max_sessions: u64) -> (SocketAddr, thread::JoinHandle<io::Result<ListenReport>>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
    let address = listener.local_addr().expect("address");
    let world = ServerWorld::with_role(LOOPBACK_SEED, ClientRole::Mayor);
    let handle = thread::spawn(move || {
        run_listen(listener, world, ListenConfig::default(), Some(max_sessions))
    });
    (address, handle)
}

fn connect(address: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect(address).expect("connect");
    stream
        .set_read_timeout(Some(SESSION_IO_TIMEOUT))
        .expect("read timeout");
    stream
        .set_write_timeout(Some(SESSION_IO_TIMEOUT))
        .expect("write timeout");
    stream
}

fn send(stream: &mut TcpStream, command: &ClientCommand) {
    write_json_frame(stream, command).expect("write command");
}

fn receive(stream: &mut TcpStream) -> ServerMessage {
    read_json_frame(stream).expect("read response")
}

fn hello(stream: &mut TcpStream, name: &str) -> ServerMessage {
    send(
        stream,
        &ClientCommand::Hello {
            protocol_version: PROTOCOL_VERSION,
            content_hash: CONTENT_HASH.to_string(),
            player_name: name.to_string(),
        },
    );
    receive(stream)
}

fn expect_welcome(message: ServerMessage) -> (u64, String) {
    match message {
        ServerMessage::Welcome {
            session_id,
            tick,
            resumed,
            ..
        } => {
            assert!(!resumed);
            (tick, session_id)
        }
        other => panic!("expected welcome, got {other:?}"),
    }
}

#[allow(clippy::type_complexity)]
fn expect_player_page(
    message: ServerMessage,
) -> (u64, u32, usize, bool, u64, String, Vec<(u64, String)>) {
    match message {
        ServerMessage::PlayerList {
            tick,
            count,
            matched,
            next_after,
            has_more,
            checksum,
            players,
        } => (
            tick,
            matched,
            count,
            has_more,
            next_after,
            checksum,
            players.into_iter().map(|p| (p.id, p.name)).collect(),
        ),
        other => panic!("expected player_list, got {other:?}"),
    }
}

#[allow(clippy::type_complexity)]
fn expect_settlement_page(
    message: ServerMessage,
) -> (
    u64,
    u32,
    usize,
    bool,
    u64,
    String,
    Vec<(u64, String, Option<u64>)>,
) {
    match message {
        ServerMessage::SettlementList {
            tick,
            count,
            matched,
            next_after,
            has_more,
            checksum,
            settlements,
        } => (
            tick,
            matched,
            count,
            has_more,
            next_after,
            checksum,
            settlements
                .into_iter()
                .map(|s| (s.id, s.name, s.owner_id))
                .collect(),
        ),
        other => panic!("expected settlement_list, got {other:?}"),
    }
}

fn expect_goodbye(stream: &mut TcpStream) {
    send(stream, &ClientCommand::Disconnect);
    match receive(stream) {
        ServerMessage::Goodbye => {}
        other => panic!("expected goodbye, got {other:?}"),
    }
}

#[test]
fn roster_pages_walk_and_reuse_case_insensitive_identity() {
    let (address, handle) = start_listen(2);

    let mut first = connect(address);
    let (tick, _session) = expect_welcome(hello(&mut first, "alice"));
    assert_eq!(tick, 0);
    send(
        &mut first,
        &ClientCommand::PlayerList {
            after_id: None,
            limit: None,
        },
    );
    let (tick, matched, count, has_more, next_after, checksum, players) =
        expect_player_page(receive(&mut first));
    assert_eq!(tick, 0);
    assert_eq!((matched, count, has_more, next_after), (1, 1, false, 1));
    assert_eq!(players, vec![(1, "alice".to_string())]);
    assert_eq!(checksum.len(), 16);

    send(
        &mut first,
        &ClientCommand::PlayerList {
            after_id: Some(1),
            limit: Some(1),
        },
    );
    let (_, matched, count, has_more, next_after, _, players) =
        expect_player_page(receive(&mut first));
    assert_eq!((matched, count, has_more, next_after), (0, 0, false, 1));
    assert!(players.is_empty());

    expect_goodbye(&mut first);
    drop(first);

    let mut second = connect(address);
    let (tick, _session) = expect_welcome(hello(&mut second, "Alice"));
    assert_eq!(tick, 0, "hello does not advance the tick");
    send(
        &mut second,
        &ClientCommand::PlayerList {
            after_id: None,
            limit: None,
        },
    );
    let (_, matched, count, _, _, checksum_after, players) =
        expect_player_page(receive(&mut second));
    assert_eq!((matched, count), (1, 1), "case variants share one identity");
    assert_eq!(players, vec![(1, "alice".to_string())]);
    assert_eq!(checksum, checksum_after, "roster checksum is read-only");
    expect_goodbye(&mut second);

    let report = handle.join().expect("listen thread").expect("listen");
    assert_eq!(report.tick, 0);
    assert_eq!(report.sessions, 2);
}

#[test]
fn settlement_pages_walk_in_ascending_id_order() {
    let (address, handle) = start_listen(1);

    let mut stream = connect(address);
    expect_welcome(hello(&mut stream, "mayor"));

    for (name, x) in [("Alpha", 0_i32), ("Beta", 64)] {
        send(
            &mut stream,
            &ClientCommand::FoundSettlement {
                name: name.to_string(),
                x,
                z: 0,
                radius_m: 64,
            },
        );
        match receive(&mut stream) {
            ServerMessage::SettlementFound { settlement, .. } => {
                assert_eq!(settlement.name, name);
            }
            other => panic!("expected settlement_found, got {other:?}"),
        }
    }

    let mut cursor = 0_u64;
    let mut seen = Vec::new();
    loop {
        send(
            &mut stream,
            &ClientCommand::SettlementList {
                after_id: Some(cursor),
                limit: Some(1),
            },
        );
        let (tick, matched, count, has_more, next_after, checksum, settlements) =
            expect_settlement_page(receive(&mut stream));
        assert_eq!(tick, 0);
        assert_eq!(matched as usize, 2 - seen.len());
        assert_eq!(count, settlements.len());
        assert_eq!(settlements.len(), 1);
        assert!(settlements[0].0 > cursor);
        assert_eq!(checksum.len(), 16);
        seen.push(settlements[0].clone());
        cursor = next_after;
        if !has_more {
            break;
        }
    }
    assert_eq!(seen.len(), 2);
    assert!(seen[0].0 < seen[1].0);
    assert_eq!(seen[0].1, "Alpha");
    assert_eq!(seen[1].1, "Beta");

    send(
        &mut stream,
        &ClientCommand::SettlementList {
            after_id: Some(cursor),
            limit: Some(8),
        },
    );
    let (_, matched, count, has_more, next_after, _, settlements) =
        expect_settlement_page(receive(&mut stream));
    assert_eq!(
        (matched, count, has_more, next_after),
        (0, 0, false, cursor)
    );
    assert!(settlements.is_empty());

    expect_goodbye(&mut stream);
    let report = handle.join().expect("listen thread").expect("listen");
    assert_eq!(report.tick, 0, "list reads do not advance the tick");
}

#[test]
fn viewer_cannot_found_but_can_list() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
    let address = listener.local_addr().expect("address");
    let world = ServerWorld::with_role(LOOPBACK_SEED, ClientRole::Viewer);
    let handle =
        thread::spawn(move || run_listen(listener, world, ListenConfig::default(), Some(1)));

    let mut stream = connect(address);
    expect_welcome(hello(&mut stream, "viewer"));
    send(
        &mut stream,
        &ClientCommand::PlayerList {
            after_id: None,
            limit: None,
        },
    );
    let (_, matched, _, _, _, _, players) = expect_player_page(receive(&mut stream));
    assert_eq!(matched, 1);
    assert_eq!(players[0].1, "viewer");

    send(
        &mut stream,
        &ClientCommand::FoundSettlement {
            name: "Nope".to_string(),
            x: 0,
            z: 0,
            radius_m: 64,
        },
    );
    match receive(&mut stream) {
        ServerMessage::Reject { code, .. } => assert_eq!(code, "role.forbidden"),
        other => panic!("expected role reject, got {other:?}"),
    }
    expect_goodbye(&mut stream);
    let report = handle.join().expect("listen thread").expect("listen");
    assert_eq!(report.tick, 0);
}
