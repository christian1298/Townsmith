//! Listen mode: accept protocol v0 sessions for one authoritative world and
//! optionally persist it.
//!
//! Connections run concurrently against one shared world. Each accepted socket
//! gets its own reader thread, writer thread, and bounded outbound queue; the
//! shared world is serialized per command under a mutex. An accepted `chat`
//! command is pushed as the existing `chat_sent` frame to every other live
//! session that completed `hello`; `chat_since` remains the reconnect
//! catch-up path.
//!
//! When a save path is configured, the world is saved on every non-empty
//! session end (disconnect, EOF, an idle timeout, or a failed response write)
//! and optionally every `autosave_ticks` accepted tick changes. A session that
//! read no command frame (a port probe or cancelled connect) writes no save.

use std::collections::BTreeMap;
use std::io;
#[cfg(test)]
use std::io::Read;
#[cfg(test)]
use std::io::Write;
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

use crate::protocol::{
    codes, configure_stream, decode_client_command, read_frame, read_json_frame, write_json_frame,
    ClientCommand, FrameError, ServerMessage, LOOPBACK_SEED, PROTOCOL_VERSION, SESSION_IO_TIMEOUT,
};
use crate::save::{temp_path, SaveReport};
use crate::world::ServerWorld;

// Bounded per-connection outbound queue drained by the session writer thread.
mod outbound;

use outbound::{drain, OutboundError, OutboundQueue};

/// Options for one listen run.
#[derive(Debug, Clone, Default)]
pub struct ListenConfig {
    /// Save path. `None` keeps the world in memory only.
    pub save_path: Option<PathBuf>,
    /// Save after this many accepted tick changes. `0` disables periodic saves.
    pub autosave_ticks: u64,
}

/// Summary of one listen run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenReport {
    /// Connections accepted.
    pub sessions: u64,
    /// Command frames read.
    pub commands: u64,
    /// Successful writes to the save path.
    pub saves: u64,
    /// Final authoritative tick.
    pub tick: u64,
    /// Final world checksum.
    pub checksum: String,
}

/// Accept concurrent sessions until `max_sessions` is reached.
///
/// The caller owns the bound listener so it can print the real address (useful
/// with port `0`). `max_sessions = None` serves until the process is stopped.
/// Every accepted socket gets a process-local connection id from
/// [`ServerWorld::open_connection`] and runs on its own session thread.
///
/// A session-level failure (socket configuration, a write to a peer that went
/// away, or a framing error) ends **only that session**; the accept loop keeps
/// serving. A save failure ends the loop with an error after active sessions
/// have drained; the previous save and its backup remain on disk.
pub fn run_listen(
    listener: TcpListener,
    world: ServerWorld,
    config: ListenConfig,
    max_sessions: Option<u64>,
) -> io::Result<ListenReport> {
    let last_saved_tick = world.tick;
    let shared = Arc::new(SharedServer {
        world: Mutex::new(world),
        registry: Mutex::new(SessionRegistry::default()),
        save: Mutex::new(SaveState {
            last_saved_tick,
            saves: 0,
        }),
        config,
    });

    let mut sessions = 0u64;
    let mut commands = 0u64;
    let mut handles: Vec<thread::JoinHandle<io::Result<u64>>> = Vec::new();
    let mut failure: Option<io::Error> = None;

    loop {
        if max_sessions.is_some_and(|limit| sessions >= limit) || failure.is_some() {
            break;
        }
        let (stream, _peer) = listener.accept()?;
        if let Err(error) = configure_stream(&stream) {
            eprintln!("SERVER_LISTEN status=error phase=configure error={error}");
            continue;
        }
        let writer_stream = match stream.try_clone() {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!(
                    "SERVER_SESSION status=end reason=writer_clone_error code={} message={}",
                    codes::FRAME_IO,
                    error
                );
                continue;
            }
        };
        let broadcast_stream = match stream.try_clone() {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!(
                    "SERVER_SESSION status=end reason=push_clone_error code={} message={}",
                    codes::FRAME_IO,
                    error
                );
                continue;
            }
        };

        sessions += 1;
        let (connection_id, queue) = {
            let mut world = shared.lock_world()?;
            let connection_id = world.open_connection();
            let queue = Arc::new(
                OutboundQueue::new(outbound::DEFAULT_CAPACITY)
                    .expect("default capacity is non-zero"),
            );
            (connection_id, queue)
        };
        shared
            .registry
            .lock()
            .map_err(|_| io::Error::other("listen registry mutex poisoned"))?
            .register(connection_id, Arc::clone(&queue), broadcast_stream);

        let shared = Arc::clone(&shared);
        handles.push(thread::spawn(move || {
            serve_connection(stream, writer_stream, &shared, connection_id, queue)
        }));
        reap_finished(&mut handles, &mut commands, &mut failure);
    }

    for handle in handles {
        match handle.join() {
            Ok(Ok(session_commands)) => {
                commands = commands.saturating_add(session_commands);
            }
            Ok(Err(error)) => {
                if failure.is_none() {
                    failure = Some(error);
                }
            }
            Err(_) => {
                if failure.is_none() {
                    failure = Some(io::Error::other("listen session thread panicked"));
                }
            }
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }

    let save = shared
        .save
        .lock()
        .map_err(|_| io::Error::other("listen save mutex poisoned"))?;
    let world = shared.lock_world()?;
    Ok(ListenReport {
        sessions,
        commands,
        saves: save.saves,
        tick: world.tick,
        checksum: world.checksum(),
    })
}

/// World, session registry, and save bookkeeping shared by every connection.
struct SharedServer {
    world: Mutex<ServerWorld>,
    registry: Mutex<SessionRegistry>,
    save: Mutex<SaveState>,
    config: ListenConfig,
}

impl SharedServer {
    fn lock_world(&self) -> io::Result<MutexGuard<'_, ServerWorld>> {
        self.world
            .lock()
            .map_err(|_| io::Error::other("listen world mutex poisoned"))
    }
}

/// Save path state shared by concurrent sessions.
struct SaveState {
    last_saved_tick: u64,
    saves: u64,
}

/// One live connection that can receive a pushed `chat_sent`.
struct LiveSession {
    queue: Arc<OutboundQueue>,
    stream: TcpStream,
    /// Set once this connection's `hello`/`resume` was accepted.
    hello: bool,
}

/// Live connections for one world, ordered by connection id.
///
/// The registry belongs to a single [`SharedServer`], so every entry is on the
/// same world. Parked sessions (disconnected or resumable) and connections that
/// have not completed `hello` are never eligible for a push.
#[derive(Default)]
struct SessionRegistry {
    sessions: BTreeMap<u64, LiveSession>,
}

impl SessionRegistry {
    fn register(&mut self, connection_id: u64, queue: Arc<OutboundQueue>, stream: TcpStream) {
        self.sessions.insert(
            connection_id,
            LiveSession {
                queue,
                stream,
                hello: false,
            },
        );
    }

    fn set_hello(&mut self, connection_id: u64, hello: bool) {
        if let Some(entry) = self.sessions.get_mut(&connection_id) {
            entry.hello = hello;
        }
    }

    fn remove(&mut self, connection_id: u64) {
        self.sessions.remove(&connection_id);
    }

    /// Push one encoded frame to every other hello-complete connection in
    /// ascending connection-id order.
    ///
    /// A rejected push closes that recipient only: the queue is closed and the
    /// socket is shut down through the same path a writer error uses, which
    /// wakes the recipient reader and ends its session. The sender and the
    /// accepted chat entry are untouched.
    fn broadcast_chat(&mut self, sender: u64, frame: &[u8]) {
        let mut closed = Vec::new();
        for (connection_id, entry) in &self.sessions {
            if *connection_id == sender || !entry.hello {
                continue;
            }
            if let Err(error) = entry.queue.push(frame.to_vec()) {
                eprintln!(
                    "SERVER_CHAT status=drop sender={sender} recipient={connection_id} code={} message={error}",
                    error.code()
                );
                entry.queue.close();
                let _ = entry.stream.shutdown(Shutdown::Both);
                closed.push(*connection_id);
            }
        }
        for connection_id in closed {
            self.sessions.remove(&connection_id);
        }
    }
}

/// What one decoded command produced for the shared world.
struct Applied {
    responses: Vec<ServerMessage>,
    closed: bool,
    /// `Some` when the command changed the connection's hello state.
    hello: Option<bool>,
    advanced: bool,
}

/// Serve one connection with a dedicated writer thread.
///
/// The reader keeps the accepted `TcpStream`; the writer owns a clone, so a
/// blocking write cannot stall command decoding. Responses are encoded once and
/// handed to a bounded [`OutboundQueue`] in the executor's order. The reader is
/// the only producer for its queue, and the writer is the only thread that
/// writes the socket.
///
/// Lifecycle:
///
/// - The writer drains until the queue is closed and empty, then flushes and
///   exits.
/// - When the reader ends (EOF, `disconnect`, a fault, or a save failure) it
///   leaves the registry, closes the queue, so the writer flushes the accepted
///   backlog and exits before the session is reported as done.
/// - A writer error closes the queue and shuts down the socket, which wakes the
///   reader out of `read_frame`; the error ends only this session.
/// - A full or closed queue means the peer is not draining or the writer is
///   gone. The reader stops accepting commands, the queue is closed, and the
///   socket is shut down. The rejected frame is reported with the
///   `outbound.backpressure` / `outbound.closed` code instead of being dropped
///   silently; frames already accepted are written unless the peer is gone.
/// - A chat push that cannot be queued for a recipient closes only that
///   recipient, never the sender.
fn serve_connection(
    mut reader: TcpStream,
    writer_stream: TcpStream,
    shared: &Arc<SharedServer>,
    connection_id: u64,
    queue: Arc<OutboundQueue>,
) -> io::Result<u64> {
    let writer_queue = Arc::clone(&queue);
    let writer = thread::spawn(move || {
        let mut writer = writer_stream;
        let result = drain(&mut writer, &writer_queue);
        if result.is_err() {
            // The reader may be blocked in `read_frame`; shutting down the
            // shared socket makes that read return and ends the session.
            let _ = writer.shutdown(Shutdown::Both);
        }
        writer_queue.close();
        result
    });

    let mut session_commands = 0u64;
    let mut outbound: Option<OutboundError> = None;
    let mut fatal: Option<io::Error> = None;

    loop {
        let payload = match read_frame(&mut reader) {
            Ok(payload) => payload,
            // A clean EOF at the frame boundary: the client closed without
            // `disconnect`.
            Err(FrameError::Truncated { actual: 0, .. }) => break,
            Err(FrameError::Io(error)) if session_ended(&error) => break,
            Err(error) => {
                eprintln!(
                    "SERVER_PROTOCOL status=error code={} message={}",
                    error.code(),
                    error.message()
                );
                break;
            }
        };
        session_commands += 1;

        let applied = match decode_client_command(&payload) {
            Ok(command) => {
                let mut world = match shared.lock_world() {
                    Ok(world) => world,
                    Err(error) => {
                        fatal = Some(error);
                        break;
                    }
                };
                let tick_before = world.tick;
                let responses = world.apply_session(connection_id, &command);
                Applied {
                    responses,
                    closed: world.is_closed(),
                    hello: Some(world.hello_accepted()),
                    advanced: world.tick > tick_before,
                }
            }
            Err(rejection) => Applied {
                responses: vec![rejection.into_message()],
                closed: false,
                hello: None,
                advanced: false,
            },
        };

        if let Some(hello) = applied.hello {
            match shared.registry.lock() {
                Ok(mut registry) => registry.set_hello(connection_id, hello),
                Err(_) => {
                    eprintln!(
                        "SERVER_SESSION status=end reason=registry_poisoned connection={connection_id}"
                    );
                    fatal = Some(io::Error::other("listen registry mutex poisoned"));
                    break;
                }
            }
        }

        let mut sink_error: Option<FrameSinkError> = None;
        for response in &applied.responses {
            let mut frame = Vec::new();
            if let Err(error) = write_json_frame(&mut frame, response) {
                sink_error = Some(FrameSinkError::Write(error));
                break;
            }
            if let Err(error) = queue.push(frame.clone()) {
                sink_error = Some(FrameSinkError::Outbound(error));
                break;
            }
            if matches!(response, ServerMessage::ChatSent { .. }) {
                match shared.registry.lock() {
                    Ok(mut registry) => registry.broadcast_chat(connection_id, &frame),
                    Err(_) => {
                        eprintln!(
                            "SERVER_CHAT status=drop sender={connection_id} code=registry.poisoned"
                        );
                    }
                }
            }
        }
        if let Some(error) = sink_error {
            outbound = match &error {
                FrameSinkError::Outbound(error) => Some(*error),
                FrameSinkError::Write(_) => None,
            };
            eprintln!(
                "SERVER_SESSION status=end reason={} code={} message={}",
                if outbound.is_some() {
                    "outbound_error"
                } else {
                    "write_error"
                },
                error.code(),
                error.message()
            );
            break;
        }

        if applied.closed {
            break;
        }

        if applied.advanced {
            if let Err(error) = autosave_world(shared) {
                fatal = Some(error);
                break;
            }
        }
    }

    // No new chat pushes reach this connection once it leaves the registry.
    if let Ok(mut registry) = shared.registry.lock() {
        registry.remove(connection_id);
    }
    // No more responses can be produced, whatever ended the session.
    queue.close();

    if outbound.is_some() || fatal.is_some() {
        // Do not wait indefinitely for a peer that is not reading: stop both
        // directions. Frames already queued may be lost here, which is exactly
        // the documented backpressure close path.
        let _ = reader.shutdown(Shutdown::Both);
    }

    match writer.join() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            eprintln!(
                "SERVER_SESSION status=end reason=write_error code={} message={}",
                codes::FRAME_IO,
                error
            );
        }
        Err(_) => {
            eprintln!(
                "SERVER_SESSION status=end reason=writer_panic message=outbound writer thread panicked"
            );
        }
    }

    match shared.lock_world() {
        Ok(mut world) => world.close_connection(connection_id),
        Err(error) => {
            if fatal.is_none() {
                fatal = Some(error);
            }
        }
    }

    // A port probe (or any connect with no command frame) has nothing to
    // persist. Saving it would rotate the backup and rewrite an identical file
    // for every health check.
    if session_commands > 0 && fatal.is_none() {
        if let Err(error) = save_world(shared, "disconnect") {
            fatal = Some(error);
        }
    }

    if let Some(error) = fatal {
        return Err(error);
    }
    Ok(session_commands)
}

/// Reap finished session threads without blocking the accept loop.
fn reap_finished(
    handles: &mut Vec<thread::JoinHandle<io::Result<u64>>>,
    commands: &mut u64,
    failure: &mut Option<io::Error>,
) {
    let mut index = 0;
    while index < handles.len() {
        if !handles[index].is_finished() {
            index += 1;
            continue;
        }
        let handle = handles.swap_remove(index);
        match handle.join() {
            Ok(Ok(session_commands)) => {
                *commands = commands.saturating_add(session_commands);
            }
            Ok(Err(error)) => {
                if failure.is_none() {
                    *failure = Some(error);
                }
            }
            Err(_) => {
                if failure.is_none() {
                    *failure = Some(io::Error::other("listen session thread panicked"));
                }
            }
        }
    }
}

/// Persist the shared world for a completed non-empty session.
fn save_world(shared: &SharedServer, reason: &str) -> io::Result<()> {
    let Some(path) = shared.config.save_path.as_deref() else {
        return Ok(());
    };
    let mut save = shared
        .save
        .lock()
        .map_err(|_| io::Error::other("listen save mutex poisoned"))?;
    let world = shared.lock_world()?;
    persist(&world, path, reason)?;
    save.last_saved_tick = world.tick;
    save.saves = save.saves.saturating_add(1);
    Ok(())
}

/// Run the configured periodic save when enough accepted ticks have elapsed.
fn autosave_world(shared: &SharedServer) -> io::Result<()> {
    if shared.config.autosave_ticks == 0 {
        return Ok(());
    }
    let Some(path) = shared.config.save_path.as_deref() else {
        return Ok(());
    };
    let mut save = shared
        .save
        .lock()
        .map_err(|_| io::Error::other("listen save mutex poisoned"))?;
    let world = shared.lock_world()?;
    if world.tick.saturating_sub(save.last_saved_tick) >= shared.config.autosave_ticks {
        persist(&world, path, "ticks")?;
        save.last_saved_tick = world.tick;
        save.saves = save.saves.saturating_add(1);
    }
    Ok(())
}

/// Why one response frame was not handed to the transport.
#[derive(Debug)]
enum FrameSinkError {
    /// The frame could not be encoded or written.
    Write(FrameError),
    /// The bounded queue rejected the frame.
    Outbound(OutboundError),
}

impl FrameSinkError {
    fn code(&self) -> &'static str {
        match self {
            Self::Write(error) => error.code(),
            Self::Outbound(error) => error.code(),
        }
    }

    fn message(&self) -> String {
        match self {
            Self::Write(error) => error.message(),
            Self::Outbound(error) => error.to_string(),
        }
    }
}

/// What one read/execute/write pass produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
struct ServeOutcome {
    /// Command frames read.
    commands: u64,
    /// Set when the queue rejected a response, which ends the session.
    outbound: Option<OutboundError>,
}

/// Read and answer command frames until the peer leaves or the session closes.
///
/// Returns the number of command frames read. The only `Err` is a save failure:
/// a read error ends the session quietly and a response-write or queue error
/// ends it too, without taking down the accept loop.
#[cfg(test)]
fn serve_frames<R, F>(
    reader: &mut R,
    mut send: F,
    world: &mut ServerWorld,
    config: &ListenConfig,
    last_saved_tick: &mut u64,
    saves: &mut u64,
) -> io::Result<ServeOutcome>
where
    R: Read,
    F: FnMut(&ServerMessage) -> Result<(), FrameSinkError>,
{
    let mut session_commands = 0u64;
    loop {
        let payload = match read_frame(&mut *reader) {
            Ok(payload) => payload,
            // A clean EOF at the frame boundary: the client closed without
            // `disconnect`.
            Err(FrameError::Truncated { actual: 0, .. }) => {
                return Ok(ServeOutcome {
                    commands: session_commands,
                    outbound: None,
                });
            }
            Err(FrameError::Io(error)) if session_ended(&error) => {
                return Ok(ServeOutcome {
                    commands: session_commands,
                    outbound: None,
                });
            }
            Err(error) => {
                eprintln!(
                    "SERVER_PROTOCOL status=error code={} message={}",
                    error.code(),
                    error.message()
                );
                return Ok(ServeOutcome {
                    commands: session_commands,
                    outbound: None,
                });
            }
        };
        session_commands += 1;

        let tick_before = world.tick;
        let responses = match decode_client_command(&payload) {
            Ok(command) => world.apply_all(&command),
            Err(rejection) => vec![rejection.into_message()],
        };
        for response in &responses {
            if let Err(error) = send(response) {
                let outbound = match &error {
                    FrameSinkError::Outbound(error) => Some(*error),
                    FrameSinkError::Write(_) => None,
                };
                eprintln!(
                    "SERVER_SESSION status=end reason={} code={} message={}",
                    if outbound.is_some() {
                        "outbound_error"
                    } else {
                        "write_error"
                    },
                    error.code(),
                    error.message()
                );
                return Ok(ServeOutcome {
                    commands: session_commands,
                    outbound,
                });
            }
        }

        if world.is_closed() {
            return Ok(ServeOutcome {
                commands: session_commands,
                outbound: None,
            });
        }

        if config.autosave_ticks > 0
            && world.tick > tick_before
            && world.tick.saturating_sub(*last_saved_tick) >= config.autosave_ticks
        {
            if let Some(path) = &config.save_path {
                persist(world, path, "ticks")?;
                *last_saved_tick = world.tick;
                *saves += 1;
            }
        }
    }
}

/// Inline adapter used by the fault harness: writes each response frame
/// directly to `writer`.
#[cfg(test)]
fn serve_io<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    world: &mut ServerWorld,
    config: &ListenConfig,
    last_saved_tick: &mut u64,
    saves: &mut u64,
) -> io::Result<u64> {
    let send = |response: &ServerMessage| {
        write_json_frame(&mut *writer, response).map_err(FrameSinkError::Write)
    };
    let outcome = serve_frames(reader, send, world, config, last_saved_tick, saves)?;
    Ok(outcome.commands)
}

/// Listen-path adapter: encodes each response once and queues the exact bytes
/// the writer will put on the socket.
#[cfg(test)]
fn serve_io_outbound<R: Read>(
    reader: &mut R,
    queue: &OutboundQueue,
    world: &mut ServerWorld,
    config: &ListenConfig,
    last_saved_tick: &mut u64,
    saves: &mut u64,
) -> io::Result<ServeOutcome> {
    let send = |response: &ServerMessage| {
        let mut frame = Vec::new();
        write_json_frame(&mut frame, response).map_err(FrameSinkError::Write)?;
        queue.push(frame).map_err(FrameSinkError::Outbound)
    };
    serve_frames(reader, send, world, config, last_saved_tick, saves)
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

fn persist(world: &ServerWorld, path: &Path, reason: &str) -> io::Result<SaveReport> {
    let report = world
        .save_to(path)
        .map_err(|error| io::Error::other(error.to_string()))?;
    println!(
        "SERVER_SAVE status=ok action=save reason={reason} path={} bytes={} tick={} checksum={} backup_rotated={}",
        path.display(),
        report.bytes,
        world.tick,
        world.checksum(),
        report.backup_rotated
    );
    Ok(report)
}

/// Report from the in-process listen/save loopback smoke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveSmokeReport {
    /// Final save file size.
    pub bytes: u64,
    /// Tick restored from disk.
    pub tick: u64,
    /// Checksum restored from disk.
    pub checksum: String,
    /// Backing file used by the smoke (already removed).
    pub path: PathBuf,
}

/// Exercise the real listen path over TCP loopback and then reload the file.
///
/// The server binds `127.0.0.1:0`, accepts one session, applies
/// hello → set_tax → place_road → disconnect, saves on disconnect, and exits.
/// The client then loads the save and requires the same tick and checksum. The
/// temp directory is removed before returning.
pub fn run_save_load_smoke() -> io::Result<SaveSmokeReport> {
    let directory =
        std::env::temp_dir().join(format!("townsmith-save-loopback-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory)?;
    let path = directory.join("world.json");

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let address = listener.local_addr()?;
    let config = ListenConfig {
        save_path: Some(path.clone()),
        autosave_ticks: 0,
    };

    let server = thread::spawn(move || {
        run_listen(listener, ServerWorld::new(LOOPBACK_SEED), config, Some(1))
    });
    let client_result = run_smoke_client(address);
    let server_result = server
        .join()
        .map_err(|_| io::Error::other("listen save smoke server thread panicked"))?;

    client_result?;
    let report = server_result?;

    let loaded =
        ServerWorld::load_from(&path).map_err(|error| io::Error::other(error.to_string()))?;
    if loaded.world.tick != report.tick || loaded.world.checksum() != report.checksum {
        return Err(io::Error::other(format!(
            "save smoke mismatch: server tick={} checksum={}, disk tick={} checksum={}",
            report.tick,
            report.checksum,
            loaded.world.tick,
            loaded.world.checksum()
        )));
    }
    if temp_path(&path).exists() {
        return Err(io::Error::other("save smoke left a .tmp file"));
    }

    let bytes = loaded.report.bytes;
    let checksum = loaded.world.checksum();
    let tick = loaded.world.tick;
    std::fs::remove_dir_all(&directory)?;
    Ok(SaveSmokeReport {
        bytes,
        tick,
        checksum,
        path,
    })
}

fn run_smoke_client(address: SocketAddr) -> io::Result<()> {
    let mut stream = TcpStream::connect_timeout(&address, SESSION_IO_TIMEOUT)?;
    configure_stream(&stream)?;

    let mut reference = ServerWorld::new(LOOPBACK_SEED);
    let commands = [
        ClientCommand::Hello {
            protocol_version: PROTOCOL_VERSION,
            content_hash: "0123456789abcdef".repeat(4),
            player_name: "save-smoke".to_string(),
        },
        ClientCommand::SetTax {
            resident_rate: 12,
            business_rate: 8,
        },
        ClientCommand::PlaceRoad {
            start: [0, 0],
            end: [8, 0],
        },
        ClientCommand::Disconnect,
    ];

    for command in commands {
        let expected = reference.apply(&command);
        write_json_frame(&mut stream, &command)
            .map_err(|error| io::Error::other(error.to_string()))?;
        let actual: ServerMessage =
            read_json_frame(&mut stream).map_err(|error| io::Error::other(error.to_string()))?;
        if actual != expected {
            return Err(io::Error::other(format!(
                "save smoke mismatch for `{}`: expected {expected:?}, received {actual:?}",
                command.tag()
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod network_faults;

#[cfg(test)]
mod stress;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("townsmith-listen-{}-{unique}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temp dir");
        path
    }

    fn client_exchange(
        address: SocketAddr,
        commands: &[ClientCommand],
    ) -> io::Result<Vec<ServerMessage>> {
        let mut stream = TcpStream::connect_timeout(&address, SESSION_IO_TIMEOUT)?;
        configure_stream(&stream)?;
        let mut responses = Vec::with_capacity(commands.len());
        for command in commands {
            write_json_frame(&mut stream, command)
                .map_err(|error| io::Error::other(error.to_string()))?;
            responses.push(
                read_json_frame(&mut stream)
                    .map_err(|error| io::Error::other(error.to_string()))?,
            );
        }
        Ok(responses)
    }

    fn hello() -> ClientCommand {
        ClientCommand::Hello {
            protocol_version: PROTOCOL_VERSION,
            content_hash: "0123456789abcdef".repeat(4),
            player_name: "tester".to_string(),
        }
    }

    #[test]
    fn disconnect_saves_and_reload_resumes_the_tick() {
        let directory = temp_dir();
        let path = directory.join("world.json");
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let address = listener.local_addr().expect("address");
        let config = ListenConfig {
            save_path: Some(path.clone()),
            autosave_ticks: 0,
        };

        let server = thread::spawn(move || {
            run_listen(listener, ServerWorld::new(LOOPBACK_SEED), config, Some(1))
        });

        let responses = client_exchange(
            address,
            &[
                hello(),
                ClientCommand::SetTax {
                    resident_rate: 12,
                    business_rate: 8,
                },
                ClientCommand::PlaceRoad {
                    start: [0, 0],
                    end: [8, 0],
                },
                ClientCommand::Disconnect,
            ],
        )
        .expect("exchange");
        assert!(matches!(
            responses[0],
            ServerMessage::Welcome { tick: 0, .. }
        ));
        assert!(matches!(
            responses[1],
            ServerMessage::Snapshot { tick: 1, .. }
        ));
        assert!(matches!(
            responses[2],
            ServerMessage::Snapshot { tick: 2, .. }
        ));
        assert_eq!(responses[3], ServerMessage::Goodbye);

        let report = server.join().expect("server thread").expect("listen");
        assert_eq!(report.sessions, 1);
        assert_eq!(report.commands, 4);
        assert_eq!(report.saves, 1);
        assert_eq!(report.tick, 2);

        let loaded = ServerWorld::load_from(&path).expect("load");
        assert_eq!(loaded.world.tick, 2);
        assert_eq!(loaded.world.checksum(), report.checksum);
        assert_eq!(loaded.world.roads.roads.len(), 1);
        assert!(!temp_path(&path).exists());
        fs::remove_dir_all(&directory).expect("cleanup");
    }

    #[test]
    fn autosave_writes_once_per_configured_tick_window() {
        let directory = temp_dir();
        let path = directory.join("world.json");
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let address = listener.local_addr().expect("address");
        let config = ListenConfig {
            save_path: Some(path.clone()),
            autosave_ticks: 1,
        };

        let server = thread::spawn(move || {
            run_listen(listener, ServerWorld::new(LOOPBACK_SEED), config, Some(1))
        });

        // Only `set_tax` changes the tick; `ping` does not trigger an autosave.
        client_exchange(
            address,
            &[
                hello(),
                ClientCommand::SetTax {
                    resident_rate: 12,
                    business_rate: 8,
                },
                ClientCommand::Ping { nonce: 3 },
                ClientCommand::Disconnect,
            ],
        )
        .expect("exchange");

        let report = server.join().expect("server thread").expect("listen");
        assert_eq!(
            report.saves, 2,
            "one periodic save plus the disconnect save"
        );
        assert_eq!(report.tick, 1);
        assert!(path.is_file());
        fs::remove_dir_all(&directory).expect("cleanup");
    }

    #[test]
    fn sessions_cap_serves_a_reconnect_on_the_same_world() {
        use crate::world::ClientRole;

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let address = listener.local_addr().expect("address");
        let config = ListenConfig::default();

        let server = thread::spawn(move || {
            run_listen(
                listener,
                ServerWorld::with_role(LOOPBACK_SEED, ClientRole::Mayor),
                config,
                Some(2),
            )
        });

        let first = client_exchange(
            address,
            &[
                hello(),
                ClientCommand::PlaceRoad {
                    start: [0, 0],
                    end: [8, 0],
                },
                ClientCommand::Disconnect,
            ],
        )
        .expect("first session");
        let token = match &first[0] {
            ServerMessage::Welcome {
                session_token,
                tick: 0,
                resumed: false,
                ..
            } => session_token.clone(),
            other => panic!("expected a fresh welcome, got {other:?}"),
        };
        assert!(matches!(first[1], ServerMessage::Snapshot { tick: 1, .. }));
        assert_eq!(first[2], ServerMessage::Goodbye);

        // The resume window left by `disconnect` is process-local and survives
        // into the next accepted connection.
        let mut second = TcpStream::connect_timeout(&address, SESSION_IO_TIMEOUT).expect("connect");
        configure_stream(&second).expect("configure");
        write_json_frame(
            &mut second,
            &ClientCommand::Resume {
                session_id: token.clone(),
                last_tick: 1,
            },
        )
        .expect("resume");
        let resumed: ServerMessage = read_json_frame(&mut second).expect("resume welcome");
        match &resumed {
            ServerMessage::Welcome {
                session_token,
                resumed: true,
                tick: 1,
                ..
            } => assert_eq!(session_token, &token),
            other => panic!("expected a resumed welcome, got {other:?}"),
        }
        let snapshot: ServerMessage = read_json_frame(&mut second).expect("resume snapshot");
        assert!(matches!(snapshot, ServerMessage::Snapshot { tick: 1, .. }));

        write_json_frame(&mut second, &ClientCommand::Ping { nonce: 7 }).expect("ping");
        let pong: ServerMessage = read_json_frame(&mut second).expect("pong");
        assert_eq!(pong, ServerMessage::Pong { nonce: 7 });

        write_json_frame(&mut second, &ClientCommand::Disconnect).expect("disconnect");
        let goodbye: ServerMessage = read_json_frame(&mut second).expect("goodbye");
        assert_eq!(goodbye, ServerMessage::Goodbye);

        let report = server.join().expect("server thread").expect("listen");
        assert_eq!(report.sessions, 2);
        // Both the fresh session (hello/place_road/disconnect) and the resumed
        // session (resume/ping/disconnect) contribute three frames.
        assert_eq!(report.commands, 6);
        assert_eq!(report.tick, 1);
        assert!(report.saves == 0);
    }

    #[test]
    fn writer_thread_preserves_multi_frame_order_over_tcp() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            run_listen(
                listener,
                ServerWorld::new(LOOPBACK_SEED),
                ListenConfig::default(),
                Some(1),
            )
        });

        let mut stream = TcpStream::connect_timeout(&address, SESSION_IO_TIMEOUT).expect("connect");
        configure_stream(&stream).expect("configure");

        // Coalesce four commands into one client write. The server must answer
        // them in the same order through the queue and writer thread.
        let mut encoded = Vec::new();
        write_json_frame(&mut encoded, &hello()).expect("hello");
        for nonce in [1, 2, 3] {
            write_json_frame(&mut encoded, &ClientCommand::Ping { nonce }).expect("ping");
        }
        stream.write_all(&encoded).expect("coalesced commands");
        stream.flush().expect("flush commands");

        let welcome: ServerMessage = read_json_frame(&mut stream).expect("welcome");
        assert!(matches!(welcome, ServerMessage::Welcome { tick: 0, .. }));
        for nonce in [1, 2, 3] {
            let pong: ServerMessage = read_json_frame(&mut stream).expect("pong");
            assert_eq!(pong, ServerMessage::Pong { nonce });
        }

        write_json_frame(&mut stream, &ClientCommand::Disconnect).expect("disconnect");
        let goodbye: ServerMessage = read_json_frame(&mut stream).expect("goodbye");
        assert_eq!(goodbye, ServerMessage::Goodbye);

        let report = server.join().expect("server thread").expect("listen");
        assert_eq!(report.sessions, 1);
        assert_eq!(report.commands, 5);
        assert_eq!(report.tick, 0);
    }

    #[test]
    fn reader_eof_shuts_the_outbound_writer_down() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            run_listen(
                listener,
                ServerWorld::new(LOOPBACK_SEED),
                ListenConfig::default(),
                Some(1),
            )
        });

        let mut stream = TcpStream::connect_timeout(&address, SESSION_IO_TIMEOUT).expect("connect");
        configure_stream(&stream).expect("configure");
        write_json_frame(&mut stream, &hello()).expect("hello");
        let welcome: ServerMessage = read_json_frame(&mut stream).expect("welcome");
        assert!(matches!(welcome, ServerMessage::Welcome { .. }));

        // Dropping the client sends EOF. The server closes the queue, joins the
        // writer, and returns from `run_listen`; a leaked writer thread would
        // hang this test instead.
        drop(stream);

        let report = server.join().expect("server thread").expect("listen");
        assert_eq!(report.sessions, 1);
        assert_eq!(report.commands, 1);
        assert_eq!(report.saves, 0);
    }

    #[test]
    fn save_load_smoke_is_deterministic() {
        let first = run_save_load_smoke().expect("first smoke");
        let second = run_save_load_smoke().expect("second smoke");
        assert_eq!(first.bytes, second.bytes);
        assert_eq!(first.tick, 2);
        assert_eq!(first.checksum, second.checksum);
        assert_eq!(first.path, second.path);
        assert_eq!(first.checksum.len(), 16);
    }
    fn hello_named(name: &str) -> ClientCommand {
        ClientCommand::Hello {
            protocol_version: PROTOCOL_VERSION,
            content_hash: "0123456789abcdef".repeat(4),
            player_name: name.to_string(),
        }
    }

    fn connect_client(address: SocketAddr) -> io::Result<TcpStream> {
        let stream = TcpStream::connect_timeout(&address, SESSION_IO_TIMEOUT)?;
        configure_stream(&stream)?;
        Ok(stream)
    }

    fn exchange_one(stream: &mut TcpStream, command: &ClientCommand) -> io::Result<ServerMessage> {
        write_json_frame(stream, command).map_err(|error| io::Error::other(error.to_string()))?;
        read_json_frame(stream).map_err(|error| io::Error::other(error.to_string()))
    }

    #[test]
    fn chat_is_pushed_to_the_other_live_session_once() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            run_listen(
                listener,
                ServerWorld::new(LOOPBACK_SEED),
                ListenConfig::default(),
                Some(2),
            )
        });

        let mut alice = connect_client(address).expect("alice");
        assert!(matches!(
            exchange_one(&mut alice, &hello_named("alice")).expect("alice hello"),
            ServerMessage::Welcome { .. }
        ));
        let mut bob = connect_client(address).expect("bob");
        assert!(matches!(
            exchange_one(&mut bob, &hello_named("bob")).expect("bob hello"),
            ServerMessage::Welcome { .. }
        ));

        let alice_chat = exchange_one(
            &mut alice,
            &ClientCommand::Chat {
                text: "hello".to_string(),
            },
        )
        .expect("alice chat");
        let alice_entry = match alice_chat {
            ServerMessage::ChatSent { entry, .. } => entry,
            other => panic!("expected alice chat_sent, got {other:?}"),
        };
        let bob_chat = read_json_frame::<_, ServerMessage>(&mut bob)
            .map_err(|error| io::Error::other(format!("bob push: {error}")))
            .expect("bob push");
        let bob_entry = match bob_chat {
            ServerMessage::ChatSent { entry, .. } => entry,
            other => panic!("expected bob chat_sent, got {other:?}"),
        };
        assert_eq!(alice_entry.seq, bob_entry.seq);
        assert_eq!(alice_entry.text, "hello");
        assert_eq!(bob_entry.text, "hello");
        assert_eq!(alice_entry.player_id, bob_entry.player_id);

        assert_eq!(
            exchange_one(&mut alice, &ClientCommand::Disconnect).expect("alice goodbye"),
            ServerMessage::Goodbye
        );
        assert_eq!(
            exchange_one(&mut bob, &ClientCommand::Disconnect).expect("bob goodbye"),
            ServerMessage::Goodbye
        );
        let report = server.join().expect("server thread").expect("listen");
        assert_eq!(report.sessions, 2);
        assert_eq!(report.commands, 5);
    }
}
