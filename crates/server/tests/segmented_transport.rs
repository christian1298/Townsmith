//! Deterministic segmented/fragmented remote-transport coverage.
//!
//! `docs/80 §3.4` calls for remote-load tuning coverage that is not a single
//! in-process loopback write. This test puts a byte-level proxy between one
//! client and the listener: every frame, in both directions, is re-written in a
//! fixed 1/2/3-byte cycle with a flush after each chunk. A second client talks
//! to the listener directly, so the same session mix covers a fragmented
//! remote-style path and a normal path at once.
//!
//! The proxy is deterministic: no randomness and no sleeps. Every wait is a
//! [`SESSION_IO_TIMEOUT`]-bounded socket read or a bounded join, never a
//! wall-clock assertion. The sequence exercises `hello`, `chat` (and its push
//! to the other session), `server_status`, `set_tax`, `set_interest` (snapshot +
//! `interest_view`), `disconnect`, and a `resume` through a fresh proxied
//! connection. Every decoded response must equal the reference world's frame,
//! and the listener's final tick and checksum must equal that reference.
//!
//! An incomplete frame (a declared payload length followed by fewer bytes and
//! then EOF) is sent on a fourth, direct connection. It must not be accepted as
//! a command: the listener closes it without a reply and the final authoritative
//! tick/checksum are unchanged.
//!
//! Limits: IPv4 loopback in one process, no TLS, no latency/jitter, no packet
//! reordering. This is framing/transport coverage, not a performance benchmark.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use townsmith_server::listen::{run_listen, ListenConfig, ListenReport};
use townsmith_server::protocol::{
    read_frame, read_json_frame, write_json_frame, ClientCommand, FrameError, ServerMessage,
    LOOPBACK_SEED, PROTOCOL_VERSION, SESSION_IO_TIMEOUT,
};
use townsmith_server::world::ServerWorld;

const CONTENT_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// Sessions accepted by the listener: proxied client, direct client, proxied
/// resume, and the direct client that sends an incomplete frame.
const SESSIONS: u64 = 4;

/// Valid command frames the listener must count. The incomplete frame is never
/// read as a command, so it contributes nothing.
const COMMANDS: u64 = 12;

/// Upper bound for the listener and proxy threads to join.
const JOIN_DEADLINE: Duration = Duration::from_secs(20);

/// Fixed re-chunking cycle for every frame written by the proxy.
const CHUNK_CYCLE: [usize; 3] = [1, 2, 3];

const RESIDENT_RATE: u16 = 7;
const BUSINESS_RATE: u16 = 5;
const INTEREST_CENTER: [f64; 2] = [16.0, 0.0];
const INTEREST_RADIUS_M: f64 = 32.0;

fn bind() -> (TcpListener, SocketAddr) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind loopback listener");
    let address = listener.local_addr().expect("listener address");
    (listener, address)
}

fn configure(stream: &TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(SESSION_IO_TIMEOUT))?;
    stream.set_write_timeout(Some(SESSION_IO_TIMEOUT))?;
    stream.set_nodelay(true)?;
    Ok(())
}

fn connect(address: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect_timeout(&address, SESSION_IO_TIMEOUT).expect("connect");
    configure(&stream).expect("configure stream");
    stream
}

fn send(stream: &mut TcpStream, command: &ClientCommand) {
    write_json_frame(stream, command).expect("write command");
}

fn recv(stream: &mut TcpStream) -> ServerMessage {
    read_json_frame(stream).expect("read server message")
}

fn hello(name: &str) -> ClientCommand {
    ClientCommand::Hello {
        protocol_version: PROTOCOL_VERSION,
        content_hash: CONTENT_HASH.to_string(),
        player_name: name.to_string(),
    }
}

fn session_ended(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::UnexpectedEof
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::WouldBlock
            | io::ErrorKind::TimedOut
    )
}

/// Forward length-prefixed frames, re-chunking each wire frame into the fixed
/// [`CHUNK_CYCLE`] and flushing after every chunk.
fn pump_frames(mut from: TcpStream, mut to: TcpStream) -> io::Result<()> {
    let mut cycle = CHUNK_CYCLE.iter().copied().cycle();
    loop {
        let payload = match read_frame(&mut from) {
            Ok(payload) => payload,
            // Clean EOF at a frame boundary: half-close the other direction so
            // the peer also observes EOF instead of waiting for a frame.
            Err(FrameError::Truncated { actual: 0, .. }) => {
                let _ = to.shutdown(Shutdown::Write);
                return Ok(());
            }
            Err(FrameError::Io(error)) if session_ended(&error) => {
                let _ = to.shutdown(Shutdown::Both);
                return Ok(());
            }
            Err(error) => {
                let _ = to.shutdown(Shutdown::Both);
                return Err(io::Error::other(error.to_string()));
            }
        };

        let length = u32::try_from(payload.len()).expect("payload fits a u32 length");
        let mut wire = Vec::with_capacity(payload.len() + 4);
        wire.extend_from_slice(&length.to_be_bytes());
        wire.extend_from_slice(&payload);

        let mut offset = 0;
        while offset < wire.len() {
            let size = cycle.next().expect("chunk cycle").min(wire.len() - offset);
            to.write_all(&wire[offset..offset + size])?;
            to.flush()?;
            offset += size;
        }
    }
}

/// Serve `connections` sequential proxied clients against the same listener.
///
/// Each accepted client gets one upstream connection and two frame pumps. The
/// next client is accepted only after both pumps for the previous one finish,
/// which keeps the re-chunking deterministic and independent per connection.
fn proxy_serve(listener: TcpListener, upstream: SocketAddr, connections: usize) -> io::Result<()> {
    for _ in 0..connections {
        let (client, _) = listener.accept()?;
        configure(&client)?;
        let server = TcpStream::connect_timeout(&upstream, SESSION_IO_TIMEOUT)?;
        configure(&server)?;

        let up_client = client.try_clone()?;
        let up_server = server.try_clone()?;
        let up = thread::spawn(move || pump_frames(up_client, up_server));

        let down_server = server.try_clone()?;
        let down_client = client.try_clone()?;
        let down = thread::spawn(move || pump_frames(down_server, down_client));

        up.join()
            .map_err(|_| io::Error::other("proxy client-to-server pump panicked"))??;
        down.join()
            .map_err(|_| io::Error::other("proxy server-to-client pump panicked"))??;
    }
    Ok(())
}

fn start_proxy(
    upstream: SocketAddr,
    connections: usize,
) -> (SocketAddr, thread::JoinHandle<io::Result<()>>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind proxy listener");
    let address = listener.local_addr().expect("proxy address");
    let handle = thread::spawn(move || proxy_serve(listener, upstream, connections));
    (address, handle)
}

fn join_listen(server: thread::JoinHandle<io::Result<ListenReport>>) -> ListenReport {
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

fn join_proxy(proxy: thread::JoinHandle<io::Result<()>>) {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(proxy.join());
    });
    match receiver.recv_timeout(JOIN_DEADLINE) {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => panic!("proxy failed: {error}"),
        Ok(Err(_)) => panic!("proxy thread panicked"),
        Err(_) => panic!("proxy did not finish within {JOIN_DEADLINE:?}"),
    }
}

/// Assert the two status frames agree on every authoritative field except the
/// per-process online roster, which differs because the reference has one
/// session.
fn assert_status_matches_reference(actual: &ServerMessage, reference: &ServerMessage) {
    match (actual, reference) {
        (
            ServerMessage::ServerStatus {
                tick,
                population,
                balance,
                roads,
                buildings,
                settlements,
                placements,
                online,
                chat_seq,
                locks,
                checksum,
                ..
            },
            ServerMessage::ServerStatus {
                tick: ref_tick,
                population: ref_population,
                balance: ref_balance,
                roads: ref_roads,
                buildings: ref_buildings,
                settlements: ref_settlements,
                placements: ref_placements,
                online: ref_online,
                chat_seq: ref_chat_seq,
                locks: ref_locks,
                checksum: ref_checksum,
                ..
            },
        ) => {
            assert_eq!(tick, ref_tick);
            assert_eq!(population, ref_population);
            assert_eq!(balance, ref_balance);
            assert_eq!(roads, ref_roads);
            assert_eq!(buildings, ref_buildings);
            assert_eq!(settlements, ref_settlements);
            assert_eq!(placements, ref_placements);
            assert_eq!(chat_seq, ref_chat_seq);
            assert_eq!(locks, ref_locks);
            assert_eq!(checksum, ref_checksum);
            assert_eq!(*online, 2, "both hello-complete sessions are online");
            assert_eq!(*ref_online, 1, "the reference world has one session");
        }
        (actual, reference) => {
            panic!("expected server_status frames, got {actual:?} and {reference:?}")
        }
    }
}

#[test]
fn segmented_transport_reassembles_exact_frames_across_proxy_and_direct_peers() {
    let (listener, server_address) = bind();
    let server = thread::spawn(move || {
        run_listen(
            listener,
            ServerWorld::new(LOOPBACK_SEED),
            ListenConfig::default(),
            Some(SESSIONS),
        )
    });
    let (proxy_address, proxy) = start_proxy(server_address, 2);

    // Two real clients: one fragmented through the proxy, one direct.
    let mut over_proxy = connect(proxy_address);
    let mut direct = connect(server_address);

    send(&mut over_proxy, &hello("proxy-player"));
    let proxy_token = match recv(&mut over_proxy) {
        ServerMessage::Welcome {
            session_token,
            tick: 0,
            resumed: false,
            ..
        } => session_token,
        other => panic!("expected a fresh proxied welcome, got {other:?}"),
    };

    send(&mut direct, &hello("direct-player"));
    match recv(&mut direct) {
        ServerMessage::Welcome {
            session_token,
            tick: 0,
            resumed: false,
            ..
        } => assert_ne!(
            session_token, proxy_token,
            "session tokens are per connection"
        ),
        other => panic!("expected a fresh direct welcome, got {other:?}"),
    }

    // Chat from the proxied session is pushed, frame-exact, to the direct one.
    send(
        &mut over_proxy,
        &ClientCommand::Chat {
            text: "over a segmented proxy".to_string(),
        },
    );
    let proxy_echo = recv(&mut over_proxy);
    let direct_push = recv(&mut direct);
    assert_eq!(proxy_echo, direct_push);
    match &proxy_echo {
        ServerMessage::ChatSent { entry, .. } => {
            assert_eq!(entry.seq, 1);
            assert_eq!(entry.text, "over a segmented proxy");
        }
        other => panic!("expected the proxied chat echo, got {other:?}"),
    }

    // Chat from the direct session is pushed back through the proxy.
    send(
        &mut direct,
        &ClientCommand::Chat {
            text: "direct line".to_string(),
        },
    );
    let direct_echo = recv(&mut direct);
    let proxy_push = recv(&mut over_proxy);
    assert_eq!(direct_echo, proxy_push);
    match &direct_echo {
        ServerMessage::ChatSent { entry, .. } => {
            assert_eq!(entry.seq, 2);
            assert_eq!(entry.text, "direct line");
        }
        other => panic!("expected the direct chat echo, got {other:?}"),
    }

    // One authoritative mutation and a viewport change, both through the proxy.
    send(
        &mut over_proxy,
        &ClientCommand::SetTax {
            resident_rate: RESIDENT_RATE,
            business_rate: BUSINESS_RATE,
        },
    );
    let proxy_tax = recv(&mut over_proxy);
    assert!(matches!(proxy_tax, ServerMessage::Snapshot { tick: 1, .. }));

    send(
        &mut over_proxy,
        &ClientCommand::SetInterest {
            center_x: INTEREST_CENTER[0],
            center_z: INTEREST_CENTER[1],
            radius_m: INTEREST_RADIUS_M,
        },
    );
    let interest_snapshot = recv(&mut over_proxy);
    let interest_view = recv(&mut over_proxy);

    // Read-only status from the direct client, with both sessions online.
    send(&mut direct, &ClientCommand::ServerStatus);
    let direct_status = recv(&mut direct);

    // Reference: one deterministic world applying the same accepted commands.
    let mut reference = ServerWorld::new(LOOPBACK_SEED);
    assert!(matches!(
        reference.apply(&hello("reference")),
        ServerMessage::Welcome { tick: 0, .. }
    ));
    let reference_chat_a = reference.apply(&ClientCommand::Chat {
        text: "over a segmented proxy".to_string(),
    });
    let reference_chat_b = reference.apply(&ClientCommand::Chat {
        text: "direct line".to_string(),
    });
    let reference_tax = reference.apply(&ClientCommand::SetTax {
        resident_rate: RESIDENT_RATE,
        business_rate: BUSINESS_RATE,
    });
    let reference_interest = reference.apply_all(&ClientCommand::SetInterest {
        center_x: INTEREST_CENTER[0],
        center_z: INTEREST_CENTER[1],
        radius_m: INTEREST_RADIUS_M,
    });
    let reference_status = reference.apply(&ClientCommand::ServerStatus);
    let expected_checksum = reference.checksum();
    assert_eq!(reference.checksum().len(), 16);

    // The fragmented path must decode to exactly the reference frames.
    assert_eq!(proxy_tax, reference_tax);
    assert!(matches!(reference_chat_a, ServerMessage::ChatSent { .. }));
    assert!(matches!(reference_chat_b, ServerMessage::ChatSent { .. }));
    assert_eq!(
        vec![interest_snapshot, interest_view],
        reference_interest,
        "set_interest must return the same snapshot + interest_view batch"
    );
    assert_status_matches_reference(&direct_status, &reference_status);

    // Disconnect the proxied session, then prove the connection closed before
    // the resume: the proxy half-closes its write side after the server EOF.
    send(&mut over_proxy, &ClientCommand::Disconnect);
    assert_eq!(recv(&mut over_proxy), ServerMessage::Goodbye);
    assert!(
        matches!(
            read_frame(&mut over_proxy),
            Err(FrameError::Truncated { actual: 0, .. })
        ),
        "the proxied connection must report a clean EOF, not a partial frame"
    );
    drop(over_proxy);

    // Resume through a fresh proxied connection. The parked session state,
    // including the viewport, must come back exactly.
    let mut resumed = connect(proxy_address);
    send(
        &mut resumed,
        &ClientCommand::Resume {
            session_id: proxy_token.clone(),
            last_tick: 1,
        },
    );
    match recv(&mut resumed) {
        ServerMessage::Welcome {
            session_id,
            session_token,
            tick: 1,
            resumed: true,
            ..
        } => {
            assert_eq!(session_token, proxy_token);
            assert_eq!(session_id, proxy_token);
        }
        other => panic!("expected a resumed welcome through the proxy, got {other:?}"),
    }
    let resumed_snapshot = recv(&mut resumed);
    assert_eq!(
        resumed_snapshot, reference_interest[0],
        "resume must restore the same snapshot, viewport included"
    );

    send(&mut direct, &ClientCommand::ServerStatus);
    let status_after_resume = recv(&mut direct);
    assert_status_matches_reference(&status_after_resume, &reference_status);

    // An incomplete frame followed by EOF: a declared 64-byte payload with 22
    // bytes and a half-close. The listener must not answer or apply anything.
    let mut incomplete = connect(server_address);
    incomplete
        .write_all(&64_u32.to_be_bytes())
        .expect("declare a 64-byte payload");
    incomplete
        .write_all(b"{\"type\":\"server_status\"")
        .expect("write a partial payload");
    incomplete.flush().expect("flush the partial frame");
    incomplete
        .shutdown(Shutdown::Write)
        .expect("half-close after the partial frame");
    let mut scratch = [0_u8; 16];
    assert_eq!(
        incomplete
            .read(&mut scratch)
            .expect("read after partial-frame EOF"),
        0,
        "the listener must close a partial frame without a reply"
    );
    drop(incomplete);

    send(&mut resumed, &ClientCommand::Disconnect);
    assert_eq!(recv(&mut resumed), ServerMessage::Goodbye);
    assert!(matches!(
        read_frame(&mut resumed),
        Err(FrameError::Truncated { actual: 0, .. })
    ));
    drop(resumed);

    send(&mut direct, &ClientCommand::Disconnect);
    assert_eq!(recv(&mut direct), ServerMessage::Goodbye);
    drop(direct);

    let report = join_listen(server);
    join_proxy(proxy);

    assert_eq!(report.sessions, SESSIONS, "accepted sessions");
    assert_eq!(
        report.commands, COMMANDS,
        "only complete frames are commands; the partial frame is not applied"
    );
    assert_eq!(report.saves, 0, "this test configures no save path");
    assert_eq!(
        report.tick, 1,
        "only the single set_tax mutation advances the tick"
    );
    assert_eq!(
        report.checksum, expected_checksum,
        "fragmented and incomplete frames must not change the world checksum"
    );
}
