//! Stalled watcher isolation over real loopback sockets.
//!
//! One watcher completes `hello`, sends `server_status`, and then never reads
//! its response again. Three healthy watchers poll `server_status` while two
//! senders interleave `chat` and deterministic `set_tax` mutations. All six
//! sessions stay live on the server for the whole run; the healthy exchanges
//! are serialized only so the expected tick and checksum are exact. Every wait
//! is a `SESSION_IO_TIMEOUT`-bounded socket read, so the run only completes
//! while the stalled peer is not holding the shared world or any other session.
//!
//! The stalled peer is queued 17 frames (its one status response plus one chat
//! push per sender per round), well under the listener's 256-frame outbound
//! cap. That is the listener's documented keep-open path: the peer is not
//! dropped for backpressure, it stays connected while the healthy sessions run,
//! and the harness then closes it with a normal socket shutdown. The final
//! `ListenReport` must show every accepted session and a checksum equal to a
//! reference [`ServerWorld`] that applied the same mutation sequence, which
//! proves the stalled socket and the read-only polling left the authoritative
//! world untouched.
//!
//! Limits: IPv4 loopback in one process, not remote latency or TLS. A peer that
//! stays stalled long enough to overflow the outbound cap is covered by the
//! `outbound.backpressure` case in `src/listen/stress.rs`, not here.

use std::io;
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use townsmith_server::listen::{run_listen, ListenConfig, ListenReport};
use townsmith_server::protocol::{
    read_json_frame, write_json_frame, ClientCommand, ServerMessage, LOOPBACK_SEED,
    PROTOCOL_VERSION, SESSION_IO_TIMEOUT,
};
use townsmith_server::world::{ClientRole, ServerWorld};

const CONTENT_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// Healthy watchers polling status alongside the stalled peer.
const WATCHERS: usize = 3;
/// Senders chatting and mutating the world.
const SENDERS: usize = 2;
/// The stalled watcher plus every healthy client.
const CLIENTS: usize = 1 + WATCHERS + SENDERS;

/// Chat plus mutation rounds. One sender applies `set_tax` per round, so the
/// authoritative tick ends at [`ROUNDS`].
const ROUNDS: usize = 8;

/// Extra status polls each healthy watcher makes after the last mutation.
const FINAL_POLLS: usize = 2;

/// Frames queued for the stalled peer: one status response plus one chat push
/// per sender per round. Compile-time guarded so a future count change fails
/// here instead of silently crossing the listener's backpressure cap.
const STALLED_FRAMES: usize = 1 + SENDERS * ROUNDS;
const _: () = assert!(STALLED_FRAMES < 256);

/// Mutation applied once per round by `sender-0`.
const RESIDENT_RATE: u16 = 7;
const BUSINESS_RATE: u16 = 5;

/// Upper bound for the accept loop to join every session thread.
const JOIN_DEADLINE: Duration = Duration::from_secs(20);

fn bind() -> (TcpListener, SocketAddr) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind loopback listener");
    let address = listener.local_addr().expect("listener address");
    (listener, address)
}

fn connect(address: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect_timeout(&address, SESSION_IO_TIMEOUT).expect("connect");
    stream
        .set_read_timeout(Some(SESSION_IO_TIMEOUT))
        .expect("read timeout");
    stream
        .set_write_timeout(Some(SESSION_IO_TIMEOUT))
        .expect("write timeout");
    stream
}

fn write(stream: &mut TcpStream, command: &ClientCommand) {
    write_json_frame(stream, command).expect("write command");
}

fn read(stream: &mut TcpStream) -> ServerMessage {
    read_json_frame(stream).expect("read server message")
}

fn hello_command(name: &str) -> ClientCommand {
    ClientCommand::Hello {
        protocol_version: PROTOCOL_VERSION,
        content_hash: CONTENT_HASH.to_string(),
        player_name: name.to_string(),
    }
}

fn hello(stream: &mut TcpStream, name: &str) {
    write(stream, &hello_command(name));
    match read(stream) {
        ServerMessage::Welcome {
            tick: 0,
            resumed: false,
            ..
        } => {}
        other => panic!("expected a fresh welcome for `{name}`, got {other:?}"),
    }
}

/// Send `server_status` and return `(tick, checksum)`, skipping pushed chats.
fn poll_status(stream: &mut TcpStream) -> (u64, String) {
    write(stream, &ClientCommand::ServerStatus);
    loop {
        match read(stream) {
            ServerMessage::ServerStatus { tick, checksum, .. } => return (tick, checksum),
            ServerMessage::ChatSent { .. } => {}
            other => panic!("expected server_status, got {other:?}"),
        }
    }
}

/// Send `set_tax` and return the resulting `(tick, checksum)`, skipping pushes.
fn poll_snapshot(stream: &mut TcpStream) -> (u64, String) {
    write(
        stream,
        &ClientCommand::SetTax {
            resident_rate: RESIDENT_RATE,
            business_rate: BUSINESS_RATE,
        },
    );
    loop {
        match read(stream) {
            ServerMessage::Snapshot { tick, checksum, .. } => return (tick, checksum),
            ServerMessage::ChatSent { .. } => {}
            other => panic!("expected a set_tax snapshot, got {other:?}"),
        }
    }
}

/// Send one chat and wait for its own `chat_sent` acknowledgement.
fn send_chat(stream: &mut TcpStream, player: &str, text: &str) {
    write(
        stream,
        &ClientCommand::Chat {
            text: text.to_string(),
        },
    );
    loop {
        match read(stream) {
            ServerMessage::ChatSent { entry, .. } if entry.player == player => {
                assert_eq!(entry.text, text, "chat acknowledgement text");
                return;
            }
            ServerMessage::ChatSent { .. } => {}
            other => panic!("expected chat_sent, got {other:?}"),
        }
    }
}

fn disconnect(stream: &mut TcpStream) {
    write(stream, &ClientCommand::Disconnect);
    loop {
        match read(stream) {
            ServerMessage::Goodbye => return,
            ServerMessage::ChatSent { .. } => {}
            other => panic!("expected goodbye, got {other:?}"),
        }
    }
}

fn join_within(server: thread::JoinHandle<io::Result<ListenReport>>) -> ListenReport {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(server.join());
    });
    match receiver.recv_timeout(JOIN_DEADLINE) {
        Ok(Ok(Ok(report))) => report,
        Ok(Ok(Err(error))) => panic!("listen run failed: {error}"),
        Ok(Err(_)) => panic!("listen thread panicked"),
        Err(_) => panic!("listen did not join every session within {JOIN_DEADLINE:?}"),
    }
}

#[test]
fn a_stalled_status_watcher_does_not_block_or_corrupt_the_world() {
    let (listener, address) = bind();

    // Pre-connect every client before the accept loop starts so the kernel
    // completes the loopback handshakes in the listen backlog.
    let mut stalled = connect(address);
    let mut streams: Vec<TcpStream> = (1..CLIENTS).map(|_| connect(address)).collect();

    let server = thread::spawn(move || {
        run_listen(
            listener,
            ServerWorld::with_role(LOOPBACK_SEED, ClientRole::Mayor),
            ListenConfig::default(),
            Some(CLIENTS as u64),
        )
    });

    hello(&mut stalled, "stalled-watcher");
    let (watcher_streams, sender_streams) = streams.split_at_mut(WATCHERS);
    for (index, stream) in watcher_streams.iter_mut().enumerate() {
        hello(stream, &format!("watcher-{index}"));
    }
    for (index, stream) in sender_streams.iter_mut().enumerate() {
        hello(stream, &format!("sender-{index}"));
    }

    // Independent reference: the same chats and mutations in the same order.
    let mut reference = ServerWorld::with_role(LOOPBACK_SEED, ClientRole::Mayor);
    match reference.apply(&hello_command("reference")) {
        ServerMessage::Welcome { tick: 0, .. } => {}
        other => panic!("reference hello expected welcome at tick 0, got {other:?}"),
    }

    // The stalled watcher asks for status once and never reads again. Its one
    // response and the later chat pushes fit in the socket buffers and in the
    // 256-frame outbound cap, so the listener keeps the session open until the
    // harness closes the socket below.
    write(&mut stalled, &ClientCommand::ServerStatus);

    let mut expected_tick = 0u64;
    let mut expected_checksum = reference.checksum();

    for round in 0..ROUNDS {
        // Both senders chat; chat never advances the tick or the checksum.
        for (index, stream) in sender_streams.iter_mut().enumerate() {
            let player = format!("sender-{index}");
            let text = format!("{player}-round-{round}");
            send_chat(stream, &player, &text);
            let _ = reference.apply(&ClientCommand::Chat { text });
        }

        // `sender-0` mutates once per round; the reference applies the same
        // command in the same position.
        let (tick, checksum) = match reference.apply(&ClientCommand::SetTax {
            resident_rate: RESIDENT_RATE,
            business_rate: BUSINESS_RATE,
        }) {
            ServerMessage::Snapshot { tick, checksum, .. } => (tick, checksum),
            other => panic!("reference set_tax expected a snapshot, got {other:?}"),
        };
        assert_eq!(tick, expected_tick + 1, "round {round} mutation tick");
        expected_tick = tick;
        expected_checksum = checksum;

        let (tick, checksum) = poll_snapshot(&mut sender_streams[0]);
        assert_eq!(
            (tick, checksum.clone()),
            (expected_tick, expected_checksum.clone()),
            "sender-0 round {round} snapshot"
        );

        // Every healthy watcher must observe the mutation while the stalled
        // peer sits on its unread response.
        for (index, stream) in watcher_streams.iter_mut().enumerate() {
            let (tick, checksum) = poll_status(stream);
            assert_eq!(
                tick, expected_tick,
                "watcher {index} round {round} must see the current tick"
            );
            assert_eq!(
                checksum, expected_checksum,
                "watcher {index} round {round} must see the reference checksum"
            );
        }
    }

    for (index, stream) in watcher_streams.iter_mut().enumerate() {
        for poll in 0..FINAL_POLLS {
            let (tick, checksum) = poll_status(stream);
            assert_eq!(
                tick, expected_tick,
                "watcher {index} final poll {poll} tick"
            );
            assert_eq!(
                checksum, expected_checksum,
                "watcher {index} final poll {poll} checksum"
            );
        }
    }

    // Close the stalled socket without ever reading its backlog. Under the cap
    // the listener was never allowed to drop it, so this is the clean path: the
    // session sees EOF (or the writer's pending frame fails), and the accept
    // loop still joins every session below.
    stalled
        .shutdown(Shutdown::Both)
        .expect("shutdown stalled watcher");
    drop(stalled);

    for stream in watcher_streams.iter_mut() {
        disconnect(stream);
    }
    for stream in sender_streams.iter_mut() {
        disconnect(stream);
    }

    let report = join_within(server);
    assert_eq!(report.sessions, CLIENTS as u64, "accepted sessions");
    let expected_commands = CLIENTS as u64
        + 1 // stalled server_status
        + ROUNDS as u64 * (SENDERS as u64 + 1 + WATCHERS as u64)
        + WATCHERS as u64 * FINAL_POLLS as u64
        + (WATCHERS + SENDERS) as u64;
    assert_eq!(
        report.commands, expected_commands,
        "every hello, status poll, chat, mutation, and disconnect frame is counted once"
    );
    assert_eq!(report.saves, 0, "the harness never configures a save");
    assert_eq!(
        report.tick, ROUNDS as u64,
        "only one sender mutation per round advances the tick"
    );
    assert_eq!(
        report.checksum,
        reference.checksum(),
        "the stalled peer and read-only polling must not change the final checksum"
    );
}
