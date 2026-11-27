//! Concurrent `server_status` watcher coverage over real loopback sockets.
//!
//! Four watchers poll `server_status` while two senders interleave `chat`
//! commands. After the watchers finish their bounded polls, one sender applies a
//! deterministic `set_tax` mutation and every watcher polls once more.
//!
//! The assertions are response-driven, never wall-clock driven: every wait is a
//! `SESSION_IO_TIMEOUT`-bounded socket read or a bounded join. Pre-mutation
//! polls must all report the fresh world checksum at tick 0; post-mutation polls
//! must report the reference world's tick 1 checksum. The final listener report
//! must equal a reference [`ServerWorld`] that applied the same mutation, which
//! proves the read-only status polling and the chats left the authoritative
//! world untouched.
//!
//! Limits: this is IPv4 loopback in one process, not remote latency or TLS. A
//! stalled/non-reading watcher is covered separately by
//! `tests/stalled_watcher.rs`, which keeps its workload under the outbound cap.

use std::collections::BTreeSet;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::{mpsc, Barrier};
use std::thread;
use std::time::Duration;

use townsmith_server::listen::{run_listen, ListenConfig, ListenReport};
use townsmith_server::protocol::{
    read_json_frame, write_json_frame, ClientCommand, ServerMessage, LOOPBACK_SEED,
    PROTOCOL_VERSION, SESSION_IO_TIMEOUT,
};
use townsmith_server::world::{ClientRole, ServerWorld};

const CONTENT_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

const WATCHERS: usize = 4;
const SENDERS: usize = 2;
const CLIENTS: usize = WATCHERS + SENDERS;

/// Status polls per watcher. The first [`CHAT_ROUNDS`] polls are released in
/// lockstep with one chat per sender, so status reads and chat writes overlap.
const POLLS: usize = 20;
const CHAT_ROUNDS: usize = 10;

/// Mutation applied after the watchers finish.
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

fn hello(stream: &mut TcpStream, name: &str) {
    write(
        stream,
        &ClientCommand::Hello {
            protocol_version: PROTOCOL_VERSION,
            content_hash: CONTENT_HASH.to_string(),
            player_name: name.to_string(),
        },
    );
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
fn status_watchers_see_monotonic_ticks_and_a_stable_checksum() {
    let (listener, address) = bind();

    // Pre-connect every client before the accept loop starts so the kernel
    // completes the loopback handshakes in the listen backlog.
    let mut streams: Vec<TcpStream> = (0..CLIENTS).map(|_| connect(address)).collect();

    let world = ServerWorld::with_role(LOOPBACK_SEED, ClientRole::Mayor);
    let server = thread::spawn(move || {
        run_listen(
            listener,
            world,
            ListenConfig::default(),
            Some(CLIENTS as u64),
        )
    });

    for (index, stream) in streams.iter_mut().enumerate() {
        if index < WATCHERS {
            hello(stream, &format!("watcher-{index}"));
        } else {
            hello(stream, &format!("sender-{}", index - WATCHERS));
        }
    }

    // Independent reference: only the deterministic mutation moves the world.
    let mut reference = ServerWorld::with_role(LOOPBACK_SEED, ClientRole::Mayor);
    match reference.apply(&ClientCommand::Hello {
        protocol_version: PROTOCOL_VERSION,
        content_hash: CONTENT_HASH.to_string(),
        player_name: "reference".to_string(),
    }) {
        ServerMessage::Welcome { tick: 0, .. } => {}
        other => panic!("reference hello expected welcome at tick 0, got {other:?}"),
    }
    let pre_checksum = reference.checksum();
    let post_checksum = match reference.apply(&ClientCommand::SetTax {
        resident_rate: RESIDENT_RATE,
        business_rate: BUSINESS_RATE,
    }) {
        ServerMessage::Snapshot {
            tick: 1, checksum, ..
        } => checksum,
        other => panic!("reference set_tax expected a tick 1 snapshot, got {other:?}"),
    };

    let barrier = Barrier::new(CLIENTS);
    let (watcher_streams, sender_streams) = streams.split_at_mut(WATCHERS);

    let watcher_results = thread::scope(|scope| {
        let watcher_handles: Vec<_> = watcher_streams
            .iter_mut()
            .map(|stream| {
                let barrier = &barrier;
                scope.spawn(move || {
                    let mut ticks = Vec::with_capacity(POLLS);
                    let mut checksums = BTreeSet::new();
                    for round in 0..POLLS {
                        if round < CHAT_ROUNDS {
                            barrier.wait();
                        }
                        let (tick, checksum) = poll_status(stream);
                        ticks.push(tick);
                        checksums.insert(checksum);
                    }
                    (ticks, checksums)
                })
            })
            .collect();

        let sender_handles: Vec<_> = sender_streams
            .iter_mut()
            .enumerate()
            .map(|(index, stream)| {
                let barrier = &barrier;
                scope.spawn(move || {
                    let player = format!("sender-{index}");
                    for round in 0..CHAT_ROUNDS {
                        barrier.wait();
                        send_chat(stream, &player, &format!("{player}-chat-{round}"));
                    }
                })
            })
            .collect();

        for handle in sender_handles {
            handle.join().expect("sender thread");
        }
        watcher_handles
            .into_iter()
            .map(|handle| handle.join().expect("watcher thread"))
            .collect::<Vec<_>>()
    });

    for (index, (ticks, checksums)) in watcher_results.iter().enumerate() {
        assert_eq!(ticks.len(), POLLS, "watcher {index} poll count");
        assert!(
            ticks.windows(2).all(|pair| pair[0] <= pair[1]),
            "watcher {index} saw non-monotonic ticks: {ticks:?}"
        );
        assert_eq!(
            ticks.last(),
            Some(&0),
            "chat and status must not advance the tick for watcher {index}"
        );
        assert_eq!(
            checksums,
            &BTreeSet::from([pre_checksum.clone()]),
            "read-only status polling must keep the pre-mutation checksum for watcher {index}"
        );
    }

    // One deterministic Mayor mutation after every watcher has finished.
    write(
        &mut sender_streams[0],
        &ClientCommand::SetTax {
            resident_rate: RESIDENT_RATE,
            business_rate: BUSINESS_RATE,
        },
    );
    loop {
        match read(&mut sender_streams[0]) {
            ServerMessage::Snapshot {
                tick: 1, checksum, ..
            } => {
                assert_eq!(
                    checksum, post_checksum,
                    "listener mutation checksum must match the reference world"
                );
                break;
            }
            ServerMessage::ChatSent { .. } => {}
            other => panic!("expected a tick 1 snapshot after set_tax, got {other:?}"),
        }
    }

    for (index, stream) in watcher_streams.iter_mut().enumerate() {
        let (tick, checksum) = poll_status(stream);
        assert_eq!(tick, 1, "watcher {index} post-mutation tick");
        assert_eq!(
            checksum, post_checksum,
            "watcher {index} post-mutation checksum"
        );
    }

    for stream in &mut streams {
        disconnect(stream);
    }

    let report = join_within(server);
    assert_eq!(report.sessions, CLIENTS as u64, "accepted sessions");
    assert_eq!(report.saves, 0, "status_watch_load never configures a save");
    assert_eq!(
        report.tick, 1,
        "only the set_tax mutation advances the tick"
    );
    assert_eq!(
        report.checksum, post_checksum,
        "read-only status polling must not change the final authoritative checksum"
    );
}
