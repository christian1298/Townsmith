//! Deterministic, bounded latency-injection transport coverage.
//!
//! `docs/80 §3.4` leaves remote-load tuning with injected latency as the last
//! open listener case. This test injects latency without sleeps: a frame-level
//! proxy reads a whole frame and then **holds** it until the harness advances a
//! barrier. Every barrier is advanced only after a protocol event proves the
//! earlier step finished (a welcome, a status response, a stopped-frame
//! notification), so the outcome never depends on thread scheduling or elapsed
//! time. At most one frame is held per direction, so the injected delay is a
//! fixed, bounded number of read/write steps.
//!
//! Cases:
//!
//! - A held `hello` is not admitted until released: the direct client polls
//!   `server_status` while the frame is withheld and must see the held session
//!   absent from the online roster. After release, chat and status match a
//!   direct-client reference world, and a rejected `set_tax` leaves the world
//!   unchanged.
//! - A held reader does not block another client: the proxied welcome (and
//!   everything queued behind it) is withheld while a direct client completes
//!   `hello`, `server_status`, `set_tax`, `place_road`, and a chat push. The
//!   released reader then observes the same frames in order.
//!
//! A truncated-frame drop and the server read timeout stay covered by the
//! segmented-transport and shutdown paths; this harness adds no wall-clock
//! sleep. Every wait is bounded by an event or join deadline with an explicit
//! failure code.
//!
//! Limits: IPv4 loopback in one process, no TLS, no jitter, no reordering, and
//! no packet loss. This is transport-state coverage, not a latency benchmark.

use std::io::{self, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use townsmith_server::listen::{run_listen, ListenConfig, ListenReport};
use townsmith_server::protocol::{
    read_frame, read_json_frame, write_json_frame, ChatEntry, ClientCommand, FrameError,
    ServerMessage, LOOPBACK_SEED, MAX_TAX_RATE, PROTOCOL_VERSION, SESSION_IO_TIMEOUT,
};
use townsmith_server::save::temp_path;
use townsmith_server::world::ServerWorld;

/// Content hash accepted by every `hello` in the harness.
const CONTENT_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// Upper bound for the listener, a proxy, and every helper thread to join.
const JOIN_DEADLINE: Duration = Duration::from_secs(20);

/// Upper bound for one proxy/harness step handshake. A held frame that is
/// never released fails here instead of hanging the suite.
const STEP_DEADLINE: Duration = Duration::from_secs(20);

/// Proxy socket deadline. It must be longer than [`SESSION_IO_TIMEOUT`] so a
/// close observed by the proxy is the server's timeout, never the proxy's own
/// socket deadline; a proxy deadline is reported as [`ProxyEvent::Failed`].
const PROXY_IO_TIMEOUT: Duration = Duration::from_secs(15);

/// Client socket deadline. It only bounds a broken test; responses in the
/// scripted exchanges arrive immediately.
const CLIENT_IO_TIMEOUT: Duration = Duration::from_secs(10);

/// Rates used by every accepted `set_tax` in the harness.
const RESIDENT_RATE: u16 = 7;
const BUSINESS_RATE: u16 = 5;

/// Road placed while a reader's response is withheld.
const ROAD_START: [i32; 2] = [0, 0];
const ROAD_END: [i32; 2] = [8, 0];

// ---------------------------------------------------------------------------
// Step-count proxy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Up,
    Down,
}

/// What the proxy does with one frame in one direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Forward the frame immediately.
    Pass,
    /// Read the frame, tell the harness, and wait for a permit before
    /// forwarding it.
    Hold,
}

/// Per-direction frame script. Frames past the script pass through unchanged.
#[derive(Debug, Clone, Default)]
struct PumpScript {
    steps: Vec<Step>,
}

impl PumpScript {
    fn new(steps: impl IntoIterator<Item = Step>) -> Self {
        Self {
            steps: steps.into_iter().collect(),
        }
    }

    fn at(&self, frame: usize) -> Step {
        self.steps.get(frame).copied().unwrap_or(Step::Pass)
    }
}

/// One proxied connection. Only one frame per direction is ever held here.
#[derive(Debug, Clone, Default)]
struct ProxyPlan {
    up: PumpScript,
    down: PumpScript,
}

/// Progress reported by the proxy pumps.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ProxyEvent {
    /// The pump read the frame and is waiting for a permit.
    Holding { direction: Direction, frame: usize },
    /// The peer closed its write side; the opposite socket is half-closed.
    PeerClosed { direction: Direction },
    /// A pump read its own socket deadline or saw an unexpected frame error.
    Failed {
        direction: Direction,
        message: String,
    },
}

struct ProxyHandle {
    address: SocketAddr,
    events: Receiver<ProxyEvent>,
    up_release: Sender<()>,
    down_release: Sender<()>,
    join: thread::JoinHandle<io::Result<()>>,
}

impl ProxyHandle {
    /// Wait for the next pump event. A `Failed` event or a missing event is a
    /// test failure with a stable code.
    fn next_event(&self) -> ProxyEvent {
        match self.events.recv_timeout(STEP_DEADLINE) {
            Ok(ProxyEvent::Failed { direction, message }) => {
                panic!("latency.event.failed direction={direction:?} message={message}")
            }
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => {
                panic!("latency.event.deadline: no proxy step within {STEP_DEADLINE:?}")
            }
            Err(RecvTimeoutError::Disconnected) => panic!("latency.event.disconnected"),
        }
    }

    fn release(&self, direction: Direction) {
        let release = match direction {
            Direction::Up => &self.up_release,
            Direction::Down => &self.down_release,
        };
        release
            .send(())
            .unwrap_or_else(|_| panic!("latency.release.closed direction={direction:?}"));
    }
}

/// True only for a real peer close, never for the proxy's own read deadline.
fn peer_closed(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::UnexpectedEof
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
    )
}

fn configure(stream: &TcpStream, read_timeout: Duration) -> io::Result<()> {
    stream.set_read_timeout(Some(read_timeout))?;
    stream.set_write_timeout(Some(read_timeout))?;
    stream.set_nodelay(true)?;
    Ok(())
}

fn encode_wire(payload: &[u8]) -> Vec<u8> {
    let length = u32::try_from(payload.len()).expect("frame payload fits a u32");
    let mut wire = Vec::with_capacity(payload.len() + 4);
    wire.extend_from_slice(&length.to_be_bytes());
    wire.extend_from_slice(payload);
    wire
}

/// Forward frames until the peer closes or the per-direction script stalls.
fn pump(
    mut from: TcpStream,
    mut to: TcpStream,
    direction: Direction,
    script: PumpScript,
    events: Sender<ProxyEvent>,
    release: Receiver<()>,
) -> io::Result<()> {
    let mut frame = 0usize;
    loop {
        let payload = match read_frame(&mut from) {
            Ok(payload) => payload,
            Err(FrameError::Truncated { actual: 0, .. }) => {
                let _ = to.shutdown(Shutdown::Write);
                let _ = events.send(ProxyEvent::PeerClosed { direction });
                return Ok(());
            }
            Err(FrameError::Io(error)) if peer_closed(&error) => {
                let _ = to.shutdown(Shutdown::Write);
                let _ = events.send(ProxyEvent::PeerClosed { direction });
                return Ok(());
            }
            Err(FrameError::Io(error)) => {
                // A read/socket deadline is the proxy's own timeout, not a
                // peer close: fail instead of reporting a clean end.
                let message = format!("peer read failed at frame {frame}: {error}");
                let _ = events.send(ProxyEvent::Failed {
                    direction,
                    message: message.clone(),
                });
                return Err(io::Error::other(message));
            }
            Err(error) => {
                let message = format!("frame error at frame {frame}: {error:?}");
                let _ = events.send(ProxyEvent::Failed {
                    direction,
                    message: message.clone(),
                });
                return Err(io::Error::other(message));
            }
        };

        let wire = encode_wire(&payload);
        match script.at(frame) {
            Step::Pass => {
                to.write_all(&wire)?;
                to.flush()?;
            }
            Step::Hold => {
                let _ = events.send(ProxyEvent::Holding { direction, frame });
                if release.recv_timeout(STEP_DEADLINE).is_err() {
                    let message = format!("hold release deadline at frame {frame}");
                    let _ = events.send(ProxyEvent::Failed {
                        direction,
                        message: message.clone(),
                    });
                    return Err(io::Error::other(message));
                }
                to.write_all(&wire)?;
                to.flush()?;
            }
        }
        frame += 1;
    }
}

/// Bind a proxy that accepts exactly one client and connects it to `upstream`.
fn start_proxy(upstream: SocketAddr, plan: ProxyPlan) -> ProxyHandle {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind latency proxy");
    let address = listener.local_addr().expect("latency proxy address");
    let (event_sender, event_receiver) = mpsc::channel();
    let (up_release, up_gate) = mpsc::channel();
    let (down_release, down_gate) = mpsc::channel();

    let join = thread::spawn(move || {
        let (client, _) = listener.accept()?;
        configure(&client, PROXY_IO_TIMEOUT)?;
        let server = TcpStream::connect_timeout(&upstream, SESSION_IO_TIMEOUT)?;
        configure(&server, PROXY_IO_TIMEOUT)?;

        let up_events = event_sender.clone();
        let up = {
            let from = client.try_clone()?;
            let to = server.try_clone()?;
            thread::spawn(move || pump(from, to, Direction::Up, plan.up, up_events, up_gate))
        };
        let down_events = event_sender.clone();
        let down = {
            let from = server.try_clone()?;
            let to = client.try_clone()?;
            thread::spawn(move || {
                pump(from, to, Direction::Down, plan.down, down_events, down_gate)
            })
        };

        up.join()
            .map_err(|_| io::Error::other("latency proxy up pump panicked"))??;
        down.join()
            .map_err(|_| io::Error::other("latency proxy down pump panicked"))??;
        Ok(())
    });

    ProxyHandle {
        address,
        events: event_receiver,
        up_release,
        down_release,
        join,
    }
}

fn join_proxy(proxy: ProxyHandle) {
    let (sender, receiver) = mpsc::channel();
    let join = proxy.join;
    thread::spawn(move || {
        let _ = sender.send(join.join());
    });
    match receiver.recv_timeout(JOIN_DEADLINE) {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => panic!("latency.proxy.failed: {error}"),
        Ok(Err(_)) => panic!("latency.proxy.panicked"),
        Err(_) => panic!("latency.proxy.deadline: proxy did not join within {JOIN_DEADLINE:?}"),
    }
}

// ---------------------------------------------------------------------------
// Client helpers
// ---------------------------------------------------------------------------

fn bind() -> (TcpListener, SocketAddr) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind loopback listener");
    let address = listener.local_addr().expect("listener address");
    (listener, address)
}

fn connect(address: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect_timeout(&address, SESSION_IO_TIMEOUT).expect("connect");
    configure(&stream, CLIENT_IO_TIMEOUT).expect("configure client stream");
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

fn expect_welcome(message: ServerMessage, tick: u64, resumed: bool) -> String {
    match message {
        ServerMessage::Welcome {
            session_token,
            tick: actual_tick,
            resumed: actual_resumed,
            ..
        } => {
            assert_eq!(actual_tick, tick, "welcome tick");
            assert_eq!(actual_resumed, resumed, "welcome resumed");
            session_token
        }
        other => panic!("latency.expected.welcome: expected a welcome, got {other:?}"),
    }
}

fn expect_chat(message: ServerMessage, seq: u64, player: &str, text: &str) -> ChatEntry {
    match message {
        ServerMessage::ChatSent { entry, .. } => {
            assert_eq!(entry.seq, seq, "chat sequence");
            assert_eq!(entry.player, player, "chat player");
            assert_eq!(entry.text, text, "chat text");
            entry
        }
        other => panic!("latency.expected.chat: expected chat_sent, got {other:?}"),
    }
}

fn status_fields(message: ServerMessage) -> (u64, u32, u64, String) {
    match message {
        ServerMessage::ServerStatus {
            tick,
            online,
            chat_seq,
            checksum,
            ..
        } => (tick, online, chat_seq, checksum),
        other => panic!("latency.expected.status: expected server_status, got {other:?}"),
    }
}

fn assert_status(message: ServerMessage, tick: u64, online: u32, chat_seq: u64, checksum: &str) {
    let (actual_tick, actual_online, actual_chat_seq, actual_checksum) = status_fields(message);
    assert_eq!(actual_tick, tick, "server_status tick");
    assert_eq!(actual_online, online, "server_status online");
    assert_eq!(actual_chat_seq, chat_seq, "server_status chat_seq");
    assert_eq!(actual_checksum, checksum, "server_status checksum");
}

fn assert_status_matches_reference(
    actual: ServerMessage,
    reference: ServerMessage,
    expected_online: u32,
    expected_chat_seq: u64,
) {
    let (tick, online, chat_seq, checksum) = status_fields(actual);
    let (reference_tick, reference_online, reference_chat_seq, reference_checksum) =
        status_fields(reference);
    assert_eq!(tick, reference_tick, "status tick");
    assert_eq!(checksum, reference_checksum, "status checksum");
    assert_eq!(chat_seq, expected_chat_seq, "status chat sequence");
    assert_eq!(
        reference_chat_seq, expected_chat_seq,
        "reference chat sequence"
    );
    assert_eq!(online, expected_online, "online sessions");
    assert_eq!(reference_online, 1, "the reference world has one session");
}

fn expect_eof(stream: &mut TcpStream, code: &str) {
    match read_frame(stream) {
        Err(FrameError::Truncated { actual: 0, .. }) => {}
        Err(FrameError::Io(error)) if peer_closed(&error) => {}
        other => panic!("{code}: expected end of stream, got {other:?}"),
    }
}

fn fresh_checksum() -> String {
    ServerWorld::new(LOOPBACK_SEED).checksum()
}

fn join_listen(server: thread::JoinHandle<io::Result<ListenReport>>) -> ListenReport {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(server.join());
    });
    match receiver.recv_timeout(JOIN_DEADLINE) {
        Ok(Ok(Ok(report))) => report,
        Ok(Ok(Err(error))) => panic!("latency.listen.failed: {error}"),
        Ok(Err(_)) => panic!("latency.listen.panicked"),
        Err(_) => panic!("latency.listen.deadline: listener did not join within {JOIN_DEADLINE:?}"),
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("townsmith-latency-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("latency temp dir");
    path
}

/// Prove the saved world is the authoritative final world and the atomic save
/// left no temp file behind.
fn assert_saved_world(path: &PathBuf, report: &ListenReport) {
    assert!(path.is_file(), "the configured save path was written");
    assert!(
        !temp_path(path).exists(),
        "the atomic save must not leave a .tmp file"
    );
    let loaded = ServerWorld::load_from(path).expect("load the final save");
    assert_eq!(loaded.world.tick, report.tick, "saved tick");
    assert_eq!(loaded.world.checksum(), report.checksum, "saved checksum");
}

// ---------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------

#[test]
fn a_held_hello_joins_on_release_and_matches_the_direct_reference() {
    let directory = temp_dir("held-hello");
    let save_path = directory.join("world.json");

    let (listener, server_address) = bind();
    let config = ListenConfig {
        save_path: Some(save_path.clone()),
        autosave_ticks: 0,
        shutdown_save: false,
    };
    let server = thread::spawn(move || {
        run_listen(listener, ServerWorld::new(LOOPBACK_SEED), config, Some(2))
    });

    let proxy = start_proxy(
        server_address,
        ProxyPlan {
            up: PumpScript::new([Step::Hold]),
            ..ProxyPlan::default()
        },
    );

    // The direct client is accepted and completes first, so the accept order
    // and every state assertion below are script-determined.
    let mut direct = connect(server_address);
    send(&mut direct, &hello("direct-observer"));
    let direct_token = expect_welcome(recv(&mut direct), 0, false);

    let mut held = connect(proxy.address);
    send(&mut held, &hello("held-hello"));
    assert_eq!(
        proxy.next_event(),
        ProxyEvent::Holding {
            direction: Direction::Up,
            frame: 0,
        },
        "the proxy must read and hold the hello before forwarding it"
    );

    // While the hello is held, the session is not on the online roster.
    send(&mut direct, &ClientCommand::ServerStatus);
    assert_status(recv(&mut direct), 0, 1, 0, &fresh_checksum());

    // A rejected command must not mutate the held-adjacent world either.
    send(
        &mut direct,
        &ClientCommand::SetTax {
            resident_rate: MAX_TAX_RATE + 1,
            business_rate: BUSINESS_RATE,
        },
    );
    match recv(&mut direct) {
        ServerMessage::Reject { code, .. } => assert_eq!(code, "tax.rate_out_of_range"),
        other => panic!("latency.expected.reject: expected a tax rejection, got {other:?}"),
    }
    send(&mut direct, &ClientCommand::ServerStatus);
    assert_status(recv(&mut direct), 0, 1, 0, &fresh_checksum());

    // Advance the barrier: exactly one held step, then the hello is forwarded.
    proxy.release(Direction::Up);
    let held_token = expect_welcome(recv(&mut held), 0, false);
    assert_ne!(
        held_token, direct_token,
        "session tokens are per connection"
    );

    // Chat both ways through the delayed path, frame-exact.
    send(
        &mut held,
        &ClientCommand::Chat {
            text: "over a held hello".to_string(),
        },
    );
    let held_echo = expect_chat(recv(&mut held), 1, "held-hello", "over a held hello");
    let direct_push = expect_chat(recv(&mut direct), 1, "held-hello", "over a held hello");
    assert_eq!(held_echo, direct_push);

    send(
        &mut direct,
        &ClientCommand::Chat {
            text: "direct line".to_string(),
        },
    );
    let direct_echo = expect_chat(recv(&mut direct), 2, "direct-observer", "direct line");
    let held_push = expect_chat(recv(&mut held), 2, "direct-observer", "direct line");
    assert_eq!(direct_echo, held_push);

    send(&mut held, &ClientCommand::ServerStatus);
    assert_status(recv(&mut held), 0, 2, 2, &fresh_checksum());

    // One accepted mutation; the snapshot drives the reference comparison.
    send(
        &mut held,
        &ClientCommand::SetTax {
            resident_rate: RESIDENT_RATE,
            business_rate: BUSINESS_RATE,
        },
    );
    let held_tax = recv(&mut held);
    assert!(
        matches!(held_tax, ServerMessage::Snapshot { tick: 1, .. }),
        "the accepted set_tax must answer with the tick-1 snapshot, got {held_tax:?}"
    );

    send(&mut direct, &ClientCommand::ServerStatus);
    let final_status = recv(&mut direct);

    // Direct-client reference: the same accepted commands in wire order.
    let mut reference = ServerWorld::new(LOOPBACK_SEED);
    let _ = reference.apply(&hello("reference-observer"));
    let _ = reference.apply(&ClientCommand::SetTax {
        resident_rate: MAX_TAX_RATE + 1,
        business_rate: BUSINESS_RATE,
    });
    let _ = reference.apply(&ClientCommand::Chat {
        text: "over a held hello".to_string(),
    });
    let _ = reference.apply(&ClientCommand::Chat {
        text: "direct line".to_string(),
    });
    let reference_tax = reference.apply(&ClientCommand::SetTax {
        resident_rate: RESIDENT_RATE,
        business_rate: BUSINESS_RATE,
    });
    let reference_status = reference.apply(&ClientCommand::ServerStatus);

    assert_eq!(held_tax, reference_tax, "the delayed set_tax snapshot");
    assert_status_matches_reference(final_status, reference_status, 2, 2);

    send(&mut held, &ClientCommand::Disconnect);
    assert_eq!(recv(&mut held), ServerMessage::Goodbye);
    expect_eof(&mut held, "latency.held.eof");
    drop(held);

    send(&mut direct, &ClientCommand::Disconnect);
    assert_eq!(recv(&mut direct), ServerMessage::Goodbye);
    drop(direct);

    let report = join_listen(server);
    join_proxy(proxy);

    assert_eq!(report.sessions, 2, "accepted sessions");
    assert_eq!(
        report.commands, 12,
        "held path: hello, chat, status, set_tax, disconnect; direct: hello, two status, reject, chat, status, disconnect"
    );
    assert_eq!(
        report.tick, 1,
        "only the accepted set_tax advances the tick"
    );
    assert_eq!(report.saves, 2, "one disconnect save per session");
    assert_eq!(
        report.checksum,
        reference.checksum(),
        "the delayed transport must not change the final world"
    );
    assert_saved_world(&save_path, &report);

    std::fs::remove_dir_all(&directory).expect("cleanup");
}

#[test]
fn a_held_reader_does_not_block_another_client() {
    let (listener, server_address) = bind();
    let server = thread::spawn(move || {
        run_listen(
            listener,
            ServerWorld::new(LOOPBACK_SEED),
            ListenConfig::default(),
            Some(2),
        )
    });

    let proxy = start_proxy(
        server_address,
        ProxyPlan {
            down: PumpScript::new([Step::Hold]),
            ..ProxyPlan::default()
        },
    );

    let mut reader = connect(proxy.address);
    send(&mut reader, &hello("slow-reader"));
    assert_eq!(
        proxy.next_event(),
        ProxyEvent::Holding {
            direction: Direction::Down,
            frame: 0,
        },
        "the proxy must read and hold the welcome before writing it back"
    );

    // The server already accepted this hello; only the response is withheld.
    let mut direct = connect(server_address);
    send(&mut direct, &hello("fast-client"));
    let _ = expect_welcome(recv(&mut direct), 0, false);

    send(&mut direct, &ClientCommand::ServerStatus);
    assert_status(recv(&mut direct), 0, 2, 0, &fresh_checksum());

    // Two mutations and a chat complete while the reader's response waits.
    send(
        &mut direct,
        &ClientCommand::SetTax {
            resident_rate: RESIDENT_RATE,
            business_rate: BUSINESS_RATE,
        },
    );
    assert!(matches!(
        recv(&mut direct),
        ServerMessage::Snapshot { tick: 1, .. }
    ));
    send(
        &mut direct,
        &ClientCommand::PlaceRoad {
            start: ROAD_START,
            end: ROAD_END,
        },
    );
    assert!(matches!(
        recv(&mut direct),
        ServerMessage::Snapshot { tick: 2, .. }
    ));
    send(
        &mut direct,
        &ClientCommand::Chat {
            text: "queued behind the welcome".to_string(),
        },
    );
    let direct_echo = expect_chat(
        recv(&mut direct),
        1,
        "fast-client",
        "queued behind the welcome",
    );

    // Release the reader: the welcome comes first, then the queued push, in
    // the order the server produced them.
    proxy.release(Direction::Down);
    let _ = expect_welcome(recv(&mut reader), 0, false);
    let queued_push = expect_chat(
        recv(&mut reader),
        1,
        "fast-client",
        "queued behind the welcome",
    );
    assert_eq!(queued_push, direct_echo);

    send(&mut reader, &ClientCommand::ServerStatus);
    let reader_status = recv(&mut reader);

    let mut reference = ServerWorld::new(LOOPBACK_SEED);
    let _ = reference.apply(&hello("reference-client"));
    let _ = reference.apply(&ClientCommand::SetTax {
        resident_rate: RESIDENT_RATE,
        business_rate: BUSINESS_RATE,
    });
    let _ = reference.apply(&ClientCommand::PlaceRoad {
        start: ROAD_START,
        end: ROAD_END,
    });
    let _ = reference.apply(&ClientCommand::Chat {
        text: "queued behind the welcome".to_string(),
    });
    let reference_status = reference.apply(&ClientCommand::ServerStatus);

    assert_status_matches_reference(reader_status, reference_status, 2, 1);

    send(
        &mut reader,
        &ClientCommand::Chat {
            text: "released reader".to_string(),
        },
    );
    let reader_echo = expect_chat(recv(&mut reader), 2, "slow-reader", "released reader");
    let direct_push = expect_chat(recv(&mut direct), 2, "slow-reader", "released reader");
    assert_eq!(reader_echo, direct_push);

    send(&mut reader, &ClientCommand::Disconnect);
    assert_eq!(recv(&mut reader), ServerMessage::Goodbye);
    expect_eof(&mut reader, "latency.reader.eof");
    drop(reader);

    send(&mut direct, &ClientCommand::Disconnect);
    assert_eq!(recv(&mut direct), ServerMessage::Goodbye);
    drop(direct);

    let report = join_listen(server);
    join_proxy(proxy);

    assert_eq!(report.sessions, 2, "accepted sessions");
    assert_eq!(
        report.commands, 10,
        "reader: hello, status, two chats, disconnect; direct: hello, status, set_tax, place_road, chat, disconnect"
    );
    assert_eq!(
        report.tick, 2,
        "the held reader must not block the two mutations"
    );
    assert_eq!(report.saves, 0, "this case configures no save path");
    assert_eq!(
        report.checksum,
        reference.checksum(),
        "the held reader must leave the final world untouched"
    );
}
