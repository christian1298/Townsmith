//! Deterministic concurrency and shutdown stress coverage for the listener.
//!
//! Every case drives the real [`run_listen`] accept loop and per-connection
//! writer over IPv4 loopback sockets. Ordering points are request/response
//! pairs or bounded readiness polls, never wall-clock sleeps, so a passing run
//! is reproducible.
//!
//! - Four concurrent clients complete `hello`, one chats, and every other
//!   client sees exactly one pushed `chat_sent` in sequence order; the accept
//!   loop then joins every writer thread.
//! - A client that stops reading is closed by outbound backpressure while the
//!   remaining clients keep complete request/response exchanges and the server
//!   still accepts one more connection afterwards.
//! - A closed queue rejects later pushes with `outbound.closed` and ends the
//!   session reader without a panic, whether the peer or the writer went away.
//! - A session-cap trigger under concurrent command load joins every thread
//!   within a bounded deadline and leaves the deterministic world checksum
//!   unchanged.
//! - `--sessions N` serves exactly N connections and never admits the next.
//!
//! Chat and ping never advance the authoritative tick, so every case ends at
//! tick 0 with the fresh-world checksum.

use std::collections::BTreeSet;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use super::outbound::{OutboundError, OutboundQueue};
use super::{
    run_listen, serve_connection, ListenConfig, ListenReport, SaveState, SessionRegistry,
    SharedServer,
};
use crate::protocol::{
    configure_stream, frame_to_io, read_json_frame, write_json_frame, ChatEntry, ClientCommand,
    FrameError, ServerMessage, LOOPBACK_SEED, MAX_CHAT_CHARS, PROTOCOL_VERSION, SESSION_IO_TIMEOUT,
};
use crate::world::ServerWorld;

/// Round-trip content hash accepted by every hello in the harness.
const CONTENT_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// Upper bound for a listener to join every session thread.
const JOIN_DEADLINE: Duration = Duration::from_secs(20);

/// Waves of chat senders used to fill a non-reading peer's transport buffers.
///
/// One wave is six fresh sessions that each spend their post-hello burst of
/// 40 chats. That is 240 accepted lines per wave and 12,000 frames in total,
/// well past the largest default Linux TCP send/receive window plus the
/// 256-frame outbound queue, so the slow peer's queue is guaranteed to reject a
/// push instead of depending on a single machine's buffer autotuning. Sender
/// names are reused across waves, so the registry stays small.
const WAVES: u64 = 50;

/// Chat senders per wave. Six keeps every healthy sender below the 256-frame
/// queue: it receives its own 40 responses plus five peers' 40 pushes (240).
/// Eight would exceed the queue and could drop a sender on a slower host.
const SENDERS: usize = 6;

/// Chat sends per sender per wave, at the session's 40-credit burst.
const CHATS_PER_SENDER: usize = 40;

/// Frames one sender receives per wave: its own responses plus every push.
const FRAMES_PER_SENDER: usize = SENDERS * CHATS_PER_SENDER;

/// Connections accepted by the backpressure case: the slow peer, every wave
/// sender, and one healthy probe after the slow peer is gone.
const BACKPRESSURE_SESSIONS: u64 = 1 + WAVES * SENDERS as u64 + 1;

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

fn ping(nonce: u64) -> ClientCommand {
    ClientCommand::Ping { nonce }
}

fn bind() -> (TcpListener, SocketAddr) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind loopback listener");
    let address = listener.local_addr().expect("listener address");
    (listener, address)
}

fn connect(address: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect_timeout(&address, SESSION_IO_TIMEOUT).expect("connect");
    configure_stream(&stream).expect("configure client stream");
    stream
}

fn exchange(stream: &mut TcpStream, command: &ClientCommand) -> io::Result<ServerMessage> {
    write_json_frame(stream, command).map_err(frame_to_io)?;
    read_json_frame(stream).map_err(frame_to_io)
}

fn read_message(stream: &mut TcpStream) -> ServerMessage {
    read_json_frame(stream).expect("read server message")
}

fn expect_chat_sent(message: ServerMessage) -> ChatEntry {
    match message {
        ServerMessage::ChatSent { entry, .. } => entry,
        other => panic!("expected chat_sent, got {other:?}"),
    }
}

/// Connect, complete `hello`, and require the fresh welcome at tick 0.
fn hello_client(address: SocketAddr, name: &str) -> TcpStream {
    let mut stream = connect(address);
    let welcome = exchange(&mut stream, &hello(name)).expect("hello exchange");
    assert!(
        matches!(
            welcome,
            ServerMessage::Welcome {
                tick: 0,
                resumed: false,
                ..
            }
        ),
        "expected a fresh welcome at tick 0 for `{name}`, got {welcome:?}"
    );
    stream
}

fn start_server(listener: TcpListener, limit: u64) -> thread::JoinHandle<io::Result<ListenReport>> {
    thread::spawn(move || {
        run_listen(
            listener,
            ServerWorld::new(LOOPBACK_SEED),
            ListenConfig::default(),
            Some(limit),
        )
    })
}

/// Join the accept loop through a channel so a deadlock fails the test instead
/// of hanging the suite.
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

fn fresh_checksum() -> String {
    ServerWorld::new(LOOPBACK_SEED).checksum()
}

fn assert_untouched_world(report: &ListenReport) {
    assert_eq!(report.tick, 0, "chat and ping must not advance the tick");
    assert_eq!(
        report.saves, 0,
        "the stress harness never configures a save"
    );
    assert_eq!(
        report.checksum,
        fresh_checksum(),
        "chat/ping load must not move the world checksum"
    );
}

#[test]
fn four_clients_receive_one_ordered_chat_push_each() {
    let (listener, address) = bind();
    let server = start_server(listener, 4);

    let names = ["alice", "bob", "carol", "dave"];
    let mut streams: Vec<TcpStream> = names
        .iter()
        .map(|name| hello_client(address, name))
        .collect();

    // Serialize the sends so every client can assert the pushed sequence
    // without tolerating reordering: sender i answers with chat_sent i, every
    // other client reads exactly that frame before the next send starts.
    for (index, name) in names.iter().enumerate() {
        let text = format!("line from {name}");
        let sent = exchange(&mut streams[index], &chat(&text)).expect("chat exchange");
        let entry = expect_chat_sent(sent);
        assert_eq!(entry.seq, index as u64 + 1, "sender sequence");
        assert_eq!(entry.player, *name);
        assert_eq!(entry.text, text);

        for (other, stream) in streams.iter_mut().enumerate() {
            if other == index {
                continue;
            }
            let pushed = expect_chat_sent(read_message(stream));
            assert_eq!(
                pushed, entry,
                "client {} must see `{name}`'s line exactly once and in order",
                names[other]
            );
        }
    }

    for stream in &mut streams {
        assert_eq!(
            exchange(stream, &ClientCommand::Disconnect).expect("disconnect"),
            ServerMessage::Goodbye
        );
    }

    let report = join_within(server);
    assert_eq!(report.sessions, 4);
    assert_eq!(
        report.commands, 12,
        "four hello, four chat, and four disconnect frames"
    );
    assert_untouched_world(&report);
}

#[test]
fn a_closed_queue_rejects_pushes_and_stops_the_reader() {
    // The peer drops: the reader ends, closes the queue, and joins the writer.
    {
        let (listener, address) = bind();
        let mut client = connect(address);
        let (reader, _) = listener.accept().expect("accept");
        configure_stream(&reader).expect("configure server stream");
        let writer = reader.try_clone().expect("clone writer stream");
        let shared = Arc::new(test_shared());
        let connection = shared.lock_world().expect("world").open_connection();
        let queue = Arc::new(OutboundQueue::new(8).expect("queue"));

        let session_queue = Arc::clone(&queue);
        let session_shared = Arc::clone(&shared);
        let handle = thread::spawn(move || {
            serve_connection(reader, writer, &session_shared, connection, session_queue)
        });

        assert!(matches!(
            exchange(&mut client, &hello("leaver")).expect("hello"),
            ServerMessage::Welcome { tick: 0, .. }
        ));
        drop(client);

        let commands = handle.join().expect("session thread").expect("session");
        assert_eq!(commands, 1, "the session stops reading after the drop");
        assert!(queue.is_closed(), "the reader closes the queue on exit");

        let error = queue
            .push(b"late frame".to_vec())
            .expect_err("a closed queue must reject a later push");
        assert_eq!(error, OutboundError::Closed);
        assert_eq!(error.code(), "outbound.closed");
        assert!(queue.is_empty());
    }

    // The writer drops: an externally closed queue makes the next response
    // unqueueable, so the reader stops before a further command.
    {
        let (listener, address) = bind();
        let mut client = connect(address);
        let (reader, _) = listener.accept().expect("accept");
        configure_stream(&reader).expect("configure server stream");
        let writer = reader.try_clone().expect("clone writer stream");
        let shared = Arc::new(test_shared());
        let connection = shared.lock_world().expect("world").open_connection();
        let queue = Arc::new(OutboundQueue::new(8).expect("queue"));

        let session_queue = Arc::clone(&queue);
        let session_shared = Arc::clone(&shared);
        let handle = thread::spawn(move || {
            serve_connection(reader, writer, &session_shared, connection, session_queue)
        });

        assert!(matches!(
            exchange(&mut client, &hello("orphan")).expect("hello"),
            ServerMessage::Welcome { tick: 0, .. }
        ));
        // Simulate the writer thread going away after the welcome.
        queue.close();
        write_json_frame(&mut client, &ping(9)).expect("write ping");
        drop(client);

        let commands = handle.join().expect("session thread").expect("session");
        assert_eq!(
            commands, 2,
            "the ping is read and applied, then the rejected response ends the session"
        );
        assert!(queue.is_closed());

        let world = shared.lock_world().expect("world");
        assert_eq!(world.commands(), 2);
        assert!(
            !world.hello_accepted(),
            "the session was closed, not resumed"
        );
    }
}

#[test]
fn a_non_reading_peer_is_dropped_without_hurting_other_clients() {
    let (listener, address) = bind();
    let server = start_server(listener, BACKPRESSURE_SESSIONS);

    // The slow peer completes hello (so it is push-eligible) and then never
    // reads again. A ping per wave keeps its server-side read timeout fresh;
    // the responses it refuses to read still go through the bounded queue.
    let mut slow = hello_client(address, "slow-peer");
    slow.set_read_timeout(Some(Duration::from_secs(2)))
        .expect("slow read timeout");

    let text = "x".repeat(MAX_CHAT_CHARS);
    for wave in 0..WAVES {
        // A failed write means the slow peer was already dropped; later waves
        // then only exercise the healthy senders.
        let _ = write_json_frame(&mut slow, &ping(wave));

        let mut senders: Vec<TcpStream> = (0..SENDERS)
            .map(|sender| hello_client(address, &format!("sender-{sender}")))
            .collect();

        let mut batch = Vec::new();
        for _ in 0..CHATS_PER_SENDER {
            write_json_frame(&mut batch, &chat(&text)).expect("encode chat");
        }
        for stream in &mut senders {
            use std::io::Write;
            stream.write_all(&batch).expect("write chat burst");
            stream.flush().expect("flush chat burst");
        }

        let base = wave * FRAMES_PER_SENDER as u64;
        let expected: BTreeSet<u64> = (base + 1..=base + FRAMES_PER_SENDER as u64).collect();
        for (sender, stream) in senders.iter_mut().enumerate() {
            let mut seqs = BTreeSet::new();
            let mut own = 0;
            for _ in 0..FRAMES_PER_SENDER {
                let entry = expect_chat_sent(read_message(stream));
                assert!(
                    seqs.insert(entry.seq),
                    "wave {wave} sender {sender} saw sequence {} twice",
                    entry.seq
                );
                if entry.player == format!("sender-{sender}") {
                    own += 1;
                }
                assert_eq!(entry.text, text);
            }
            assert_eq!(seqs, expected, "wave {wave} sender {sender} sequences");
            assert_eq!(own, CHATS_PER_SENDER, "sender {sender} lost its own line");
        }

        for stream in &mut senders {
            assert_eq!(
                exchange(stream, &ClientCommand::Disconnect).expect("disconnect"),
                ServerMessage::Goodbye
            );
        }
    }

    // The slow peer must have been closed by a rejected push, not by the
    // harness: drain the buffered backlog and require end-of-stream. A read
    // timeout here means its queue was never allowed to overflow.
    let mut delivered = 0u64;
    loop {
        match read_json_frame::<_, ServerMessage>(&mut slow) {
            Ok(_) => delivered += 1,
            Err(FrameError::Truncated { .. }) => break,
            Err(FrameError::Io(error))
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                panic!(
                    "the non-reading peer was still open after {} chat pushes (read {} frames)",
                    WAVES * FRAMES_PER_SENDER as u64,
                    delivered
                );
            }
            Err(FrameError::Io(_)) => break,
            Err(other) => panic!("slow peer did not close cleanly: {other:?}"),
        }
    }
    assert!(
        delivered > 0,
        "the slow peer should have received part of its backlog before the close"
    );

    // The server still accepts and serves a new connection.
    let mut probe = hello_client(address, "after-drop");
    assert_eq!(
        exchange(&mut probe, &ping(7)).expect("probe ping"),
        ServerMessage::Pong { nonce: 7 }
    );
    assert_eq!(
        exchange(&mut probe, &ClientCommand::Disconnect).expect("probe disconnect"),
        ServerMessage::Goodbye
    );

    let report = join_within(server);
    assert_eq!(report.sessions, BACKPRESSURE_SESSIONS);
    // Every wave sender frame is accounted for; the slow peer's accepted pings
    // depend on how many writes reached the server before its queue closed.
    let sender_commands = WAVES * (SENDERS as u64 * (2 + CHATS_PER_SENDER as u64));
    assert!(
        report.commands >= sender_commands + 4 && report.commands <= sender_commands + 4 + WAVES,
        "unexpected command count {} for {sender_commands} sender commands",
        report.commands
    );
    assert_untouched_world(&report);
}

#[test]
fn session_cap_trigger_joins_the_load_within_the_deadline() {
    const CLIENTS: usize = 8;
    let (listener, address) = bind();
    // The accept loop stops as soon as the eighth connection is accepted,
    // while all eight sessions are still active.
    let server = start_server(listener, CLIENTS as u64);

    let names: Vec<String> = (0..CLIENTS)
        .map(|index| format!("client-{index}"))
        .collect();
    let mut streams: Vec<TcpStream> = names
        .iter()
        .map(|name| hello_client(address, name))
        .collect();

    // Each session pipelines 25 pings and one chat without waiting for the
    // accept loop, which is already joining the live sessions.
    let mut ping_batch = Vec::new();
    for nonce in 1..=25 {
        write_json_frame(&mut ping_batch, &ping(nonce)).expect("encode ping");
    }
    for (index, stream) in streams.iter_mut().enumerate() {
        let mut batch = ping_batch.clone();
        write_json_frame(&mut batch, &chat(&format!("load-{index}"))).expect("encode chat");
        use std::io::Write;
        stream.write_all(&batch).expect("write load batch");
        stream.flush().expect("flush load batch");
    }

    for (index, stream) in streams.iter_mut().enumerate() {
        let mut pongs = BTreeSet::new();
        let mut seqs = BTreeSet::new();
        let mut players = BTreeSet::new();
        let mut own = 0;
        for _ in 0..25 + CLIENTS {
            match read_message(stream) {
                ServerMessage::Pong { nonce } => {
                    assert!(pongs.insert(nonce), "duplicate pong {nonce}");
                }
                ServerMessage::ChatSent { entry, .. } => {
                    assert!(seqs.insert(entry.seq), "duplicate chat sequence");
                    if entry.text == format!("load-{index}") {
                        assert_eq!(entry.player, names[index]);
                        own += 1;
                    }
                    players.insert(entry.player);
                }
                other => panic!("unexpected load response: {other:?}"),
            }
        }
        assert_eq!(
            pongs,
            (1..=25).collect::<BTreeSet<_>>(),
            "client {index} must see exactly its 25 pongs"
        );
        assert_eq!(seqs, (1..=CLIENTS as u64).collect::<BTreeSet<_>>());
        assert_eq!(players.len(), CLIENTS, "every chat must be delivered once");
        assert_eq!(own, 1, "the sender sees its own line exactly once");
    }

    for stream in &mut streams {
        assert_eq!(
            exchange(stream, &ClientCommand::Disconnect).expect("disconnect"),
            ServerMessage::Goodbye
        );
    }

    let report = join_within(server);
    assert_eq!(report.sessions, CLIENTS as u64);
    assert_eq!(
        report.commands,
        CLIENTS as u64 * (2 + 25 + 1),
        "hello, 25 pings, one chat, and disconnect per session"
    );
    assert_untouched_world(&report);
}

#[test]
fn the_session_cap_never_admits_the_next_connection() {
    let (listener, address) = bind();
    let server = start_server(listener, 2);

    let mut first = hello_client(address, "first");
    let mut second = hello_client(address, "second");

    // The accept loop already stopped at the cap. A third TCP connection may
    // sit in the backlog, but it must never receive a welcome or a response.
    let mut third = connect(address);
    let _ = write_json_frame(&mut third, &hello("third"));

    // The two accepted sessions stay intact while the extra connection waits.
    assert_eq!(
        exchange(&mut first, &ping(1)).expect("first ping"),
        ServerMessage::Pong { nonce: 1 }
    );
    assert_eq!(
        exchange(&mut second, &ping(2)).expect("second ping"),
        ServerMessage::Pong { nonce: 2 }
    );

    for stream in [&mut first, &mut second] {
        assert_eq!(
            exchange(stream, &ClientCommand::Disconnect).expect("disconnect"),
            ServerMessage::Goodbye
        );
    }
    let report = join_within(server);
    assert_eq!(report.sessions, 2);
    assert_eq!(
        report.commands, 6,
        "hello, ping, and disconnect per session"
    );
    assert_untouched_world(&report);

    // With the listener dropped, the backlogged connection is reset. The only
    // acceptable outcome is an error: any framed message would mean the server
    // admitted a third session past `--sessions 2`.
    third
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("third read timeout");
    match read_json_frame::<_, ServerMessage>(&mut third) {
        Err(FrameError::Truncated { .. }) | Err(FrameError::Io(_)) => {}
        Ok(message) => panic!("the N+1th connection was served: {message:?}"),
        Err(other) => panic!("the N+1th connection failed unexpectedly: {other:?}"),
    }
}

fn test_shared() -> SharedServer {
    SharedServer {
        world: Mutex::new(ServerWorld::new(LOOPBACK_SEED)),
        registry: Mutex::new(SessionRegistry::default()),
        save: Mutex::new(SaveState {
            last_saved_tick: 0,
            saves: 0,
        }),
        config: ListenConfig::default(),
    }
}
