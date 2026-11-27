//! Deterministic concurrent listener + chat push load smoke.
//!
//! The harness binds an ephemeral IPv4 loopback port, pre-connects `N` real TCP
//! clients, and serves them through the production [`run_listen`] loop with a
//! temporary in-memory world. Every client completes `hello` with a unique
//! name, the clients release together through an abortable barrier, and each
//! sends `M` chat messages. Every send is followed by a `chat_since` poll, and
//! every client drains pushed `chat_sent` frames until it has seen all `N × M`
//! logical messages and the expected `(N - 1) × M` pushes.
//!
//! The chat log retains only [`CHAT_LOG_CAPACITY`] entries, so a client cannot
//! recover all `N × M` messages from `chat_since` after the fact when the
//! default workload exceeds that cap. The live push path is the completeness
//! witness: each client observes its own acknowledgements plus every other
//! client's pushes, and the final `chat_since` polls are the retained catch-up
//! check. The content checksum is deliberately order-independent (FNV-1a over
//! the observed `(player, text)` pairs sorted by player then text) because the
//! commit order across concurrent senders is not stable. Counts, the chat
//! checksum, the final tick, and the unchanged world checksum are stable
//! across runs; wall-clock `elapsed_ms` is advisory and never pinned.
//!
//! No save path is configured, the user's `--listen` port is not bound, and
//! the harness does not change the protocol, the world, or persistence.
//!
//! [`run_listen`]: crate::listen::run_listen
//! [`CHAT_LOG_CAPACITY`]: crate::protocol::CHAT_LOG_CAPACITY

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io;
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::listen::{run_listen, ListenConfig};
use crate::protocol::{
    configure_stream, read_json_frame, write_json_frame, ChatEntry, ClientCommand, ServerMessage,
    LOOPBACK_SEED, PROTOCOL_VERSION, SESSION_IO_TIMEOUT,
};
use crate::world::{ClientRole, ServerWorld};

/// Clients used when `--clients` is omitted.
pub const DEFAULT_CLIENTS: usize = 8;
/// Messages per client used when `--messages` is omitted.
pub const DEFAULT_MESSAGES: usize = 16;
/// Upper bound on harness clients (one socket and one thread per client).
pub const MAX_CLIENTS: usize = 64;
/// A fresh session accepts 48 chat commands after `hello` before its token
/// bucket rejects one. `--messages` must stay at or below that burst.
pub const MAX_MESSAGES_PER_CLIENT: usize = 48;
/// Default per-connection outbound queue bound in the listener.
///
/// Mirrors `listen/outbound.rs::DEFAULT_CAPACITY`; the listener module keeps
/// that constant private, so the smoke reports the documented value here.
pub const LISTEN_QUEUE_CAPACITY: usize = 256;

/// Whole-run liveness backstop. It is not a correctness input: every wait is
/// also driven by a response or by the listener's socket read timeout.
const RUN_DEADLINE: Duration = Duration::from_secs(60);
/// Hard bound on `chat_since` response polls per client.
const MAX_POLLS: u64 = 100_000;

/// Round-trip content hash carried by every hello in the smoke.
const CONTENT_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Stable error codes for the CLI and test output.
pub mod codes {
    /// Client, message, or overflow argument is invalid.
    pub const ARGS: &str = "load.args";
    /// The ephemeral listener could not bind.
    pub const BIND: &str = "load.bind";
    /// A client could not connect or configure its socket.
    pub const CONNECT: &str = "load.connect";
    /// `hello`/`welcome` failed.
    pub const HELLO: &str = "load.hello";
    /// A protocol frame was malformed, truncated, or unexpected.
    pub const FRAME: &str = "load.frame";
    /// The server rejected a chat command.
    pub const REJECT: &str = "load.reject";
    /// A bounded wait (barrier, poll budget, or run deadline) expired.
    pub const TIMEOUT: &str = "load.timeout";
    /// Another client aborted the shared start/round barrier.
    pub const GATE: &str = "load.gate";
    /// The sent/received message counts do not match the workload.
    pub const MESSAGES: &str = "load.messages";
    /// The push count or push content does not match the topology.
    pub const PUSHES: &str = "load.pushes";
    /// The listener report or final world state is inconsistent.
    pub const SERVER: &str = "load.server";
    /// A client or server thread panicked.
    pub const PANIC: &str = "load.panic";
}

/// A load-smoke failure with a stable code and a human-readable message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadSmokeError {
    code: &'static str,
    message: String,
}

impl LoadSmokeError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn io(code: &'static str, context: &str, error: io::Error) -> Self {
        Self::new(code, format!("{context}: {error}"))
    }

    fn frame(code: &'static str, context: &str, error: impl fmt::Display) -> Self {
        Self::new(code, format!("{context}: {error}"))
    }

    /// Stable machine-readable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }

    /// Human-readable detail.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for LoadSmokeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for LoadSmokeError {}

/// Result of one concurrent chat load smoke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadSmokeReport {
    /// Client sessions in the run.
    pub clients: usize,
    /// Accepted chat sends across all clients (`clients × messages`).
    pub messages_sent: u64,
    /// Distinct logical messages every client observed. All clients must see
    /// the full `clients × messages` set, so this is not a delivery sum.
    pub messages_received: u64,
    /// Pushes the topology should produce (`clients × (clients - 1) × messages`).
    pub pushes_expected: u64,
    /// Pushed `chat_sent` frames every client received, summed across clients.
    pub pushes_received: u64,
    /// Listener per-connection outbound queue bound.
    pub queue_capacity: usize,
    /// Wall-clock duration; advisory only, never pinned.
    pub elapsed_ms: u64,
    /// Final authoritative tick (chat does not advance it).
    pub tick: u64,
    /// Order-independent FNV-1a over the observed chat `(player, text)` pairs.
    pub checksum: String,
}

/// Expected logical chat messages for a workload, or `None` on overflow.
#[must_use]
pub fn expected_messages(clients: usize, messages: usize) -> Option<u64> {
    u64::try_from(clients)
        .ok()?
        .checked_mul(u64::try_from(messages).ok()?)
}

/// Expected pushed frames across all recipients, or `None` on overflow.
#[must_use]
pub fn expected_pushes(clients: usize, messages: usize) -> Option<u64> {
    let recipients = u64::try_from(clients.checked_sub(1)?).ok()?;
    expected_messages(clients, messages)?.checked_mul(recipients)
}

/// Validate the workload before any socket is bound.
///
/// # Errors
///
/// Returns [`codes::ARGS`] when the client count is outside `2..=MAX_CLIENTS`,
/// the per-client message count is outside `1..=MAX_MESSAGES_PER_CLIENT`, or
/// the expected counters do not fit in `u64`.
pub fn validate_workload(clients: usize, messages: usize) -> Result<(), LoadSmokeError> {
    if !(2..=MAX_CLIENTS).contains(&clients) {
        return Err(LoadSmokeError::new(
            codes::ARGS,
            format!("--clients must be in 2..={MAX_CLIENTS}, got {clients}"),
        ));
    }
    if !(1..=MAX_MESSAGES_PER_CLIENT).contains(&messages) {
        return Err(LoadSmokeError::new(
            codes::ARGS,
            format!("--messages must be in 1..={MAX_MESSAGES_PER_CLIENT}, got {messages}"),
        ));
    }
    expected_messages(clients, messages)
        .and_then(|_| expected_pushes(clients, messages))
        .ok_or_else(|| {
            LoadSmokeError::new(
                codes::ARGS,
                format!("workload {clients} x {messages} overflows the counter width"),
            )
        })?;
    Ok(())
}

/// Run the deterministic concurrent listener + chat push smoke.
///
/// # Errors
///
/// Returns a coded [`LoadSmokeError`] when the listener, a socket, a protocol
/// exchange, a count/checksum assertion, or the bounded deadline fails.
pub fn run_load_smoke(clients: usize, messages: usize) -> Result<LoadSmokeReport, LoadSmokeError> {
    validate_workload(clients, messages)?;

    let started = Instant::now();
    let deadline = started + RUN_DEADLINE;
    let expected_messages =
        expected_messages(clients, messages).expect("validated message arithmetic");
    let expected_pushes = expected_pushes(clients, messages).expect("validated push arithmetic");
    let pushes_per_client = u64::try_from(clients - 1)
        .ok()
        .and_then(|recipients| recipients.checked_mul(u64::try_from(messages).ok()?))
        .expect("validated per-client push arithmetic");

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|error| LoadSmokeError::io(codes::BIND, "bind ephemeral listener", error))?;
    let address = listener
        .local_addr()
        .map_err(|error| LoadSmokeError::io(codes::BIND, "read listener address", error))?;

    // Pre-connect every client before the accept loop starts. The kernel
    // completes the loopback handshakes in the listen backlog, so `run_listen`
    // can accept exactly `clients` sessions without a missing-connect hang.
    let mut streams = Vec::with_capacity(clients);
    for _ in 0..clients {
        let stream = TcpStream::connect_timeout(&address, SESSION_IO_TIMEOUT)
            .map_err(|error| LoadSmokeError::io(codes::CONNECT, "connect client", error))?;
        configure_stream(&stream)
            .map_err(|error| LoadSmokeError::io(codes::CONNECT, "configure client", error))?;
        streams.push(stream);
    }

    let world = ServerWorld::with_role(LOOPBACK_SEED, ClientRole::Viewer);
    let reference_checksum = world.checksum();
    let session_cap = u64::try_from(clients).expect("client count fits u64");
    let server = thread::spawn(move || {
        run_listen(listener, world, ListenConfig::default(), Some(session_cap))
    });

    let barrier = Arc::new(AbortableBarrier::new(clients));
    let mut handles = Vec::with_capacity(clients);
    for (index, stream) in streams.into_iter().enumerate() {
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            let result = run_client(stream, index, clients, messages, &barrier, deadline);
            if result.is_err() {
                barrier.abort();
            }
            result
        }));
    }

    let mut outcomes = Vec::with_capacity(clients);
    let mut first_error: Option<LoadSmokeError> = None;
    for handle in handles {
        match handle.join() {
            Ok(Ok(outcome)) => outcomes.push(outcome),
            Ok(Err(error)) => {
                if first_error
                    .as_ref()
                    .is_none_or(|first| first.code == codes::GATE && error.code != codes::GATE)
                {
                    first_error = Some(error);
                }
            }
            Err(_) => {
                first_error = Some(LoadSmokeError::new(
                    codes::PANIC,
                    "load-smoke client thread panicked",
                ));
            }
        }
    }

    let server_report = server
        .join()
        .map_err(|_| LoadSmokeError::new(codes::PANIC, "load-smoke listener thread panicked"))?
        .map_err(|error| LoadSmokeError::io(codes::SERVER, "listener", error))?;

    if let Some(error) = first_error {
        return Err(error);
    }
    if outcomes.len() != clients {
        return Err(LoadSmokeError::new(
            codes::MESSAGES,
            format!("expected {clients} client results, got {}", outcomes.len()),
        ));
    }
    if server_report.sessions != session_cap {
        return Err(LoadSmokeError::new(
            codes::SERVER,
            format!(
                "listener accepted {} sessions, expected {session_cap}",
                server_report.sessions
            ),
        ));
    }
    if server_report.saves != 0 {
        return Err(LoadSmokeError::new(
            codes::SERVER,
            format!("load smoke wrote {} saves", server_report.saves),
        ));
    }
    if server_report.tick != 0 {
        return Err(LoadSmokeError::new(
            codes::SERVER,
            format!(
                "chat-only workload advanced the tick to {}",
                server_report.tick
            ),
        ));
    }
    if server_report.checksum != reference_checksum {
        return Err(LoadSmokeError::new(
            codes::SERVER,
            format!(
                "chat-only workload changed the world checksum: {} != {reference_checksum}",
                server_report.checksum
            ),
        ));
    }

    let canonical = canonical_messages(clients, messages);
    if u64::try_from(canonical.len()).ok() != Some(expected_messages) {
        return Err(LoadSmokeError::new(
            codes::MESSAGES,
            "canonical message set does not match the workload arithmetic",
        ));
    }

    let mut messages_sent = 0u64;
    let mut pushes_received = 0u64;
    let mut reference_chat_checksum: Option<String> = None;

    for (index, outcome) in outcomes.iter().enumerate() {
        if outcome.sent != u64::try_from(messages).expect("message count fits u64") {
            return Err(LoadSmokeError::new(
                codes::MESSAGES,
                format!(
                    "client {index} sent {} messages, expected {messages}",
                    outcome.sent
                ),
            ));
        }
        if outcome.pushes != pushes_per_client {
            return Err(LoadSmokeError::new(
                codes::PUSHES,
                format!(
                    "client {index} received {} pushes, expected {pushes_per_client}",
                    outcome.pushes
                ),
            ));
        }
        if outcome.observed.len() != canonical.len() {
            let missing = canonical
                .iter()
                .filter(|key| !outcome.observed.contains_key(*key))
                .count();
            let extra = outcome
                .observed
                .keys()
                .filter(|key| !canonical.contains(*key))
                .count();
            return Err(LoadSmokeError::new(
                codes::MESSAGES,
                format!(
                    "client {index} observed {} distinct messages, expected {} (missing {missing}, extra {extra})",
                    outcome.observed.len(),
                    canonical.len()
                ),
            ));
        }

        let checksum = chat_checksum(outcome.observed.keys());
        match &reference_chat_checksum {
            None => reference_chat_checksum = Some(checksum),
            Some(reference) if reference != &checksum => {
                return Err(LoadSmokeError::new(
                    codes::PUSHES,
                    format!("client {index} chat checksum {checksum} differs from {reference}"),
                ));
            }
            Some(_) => {}
        }

        messages_sent = messages_sent.saturating_add(outcome.sent);
        pushes_received = pushes_received.saturating_add(outcome.pushes);
    }

    if messages_sent != expected_messages {
        return Err(LoadSmokeError::new(
            codes::MESSAGES,
            format!("sent {messages_sent} messages, expected {expected_messages}"),
        ));
    }
    if pushes_received != expected_pushes {
        return Err(LoadSmokeError::new(
            codes::PUSHES,
            format!("received {pushes_received} pushes, expected {expected_pushes}"),
        ));
    }

    Ok(LoadSmokeReport {
        clients,
        messages_sent,
        messages_received: expected_messages,
        pushes_expected: expected_pushes,
        pushes_received,
        queue_capacity: LISTEN_QUEUE_CAPACITY,
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        tick: server_report.tick,
        checksum: reference_chat_checksum.unwrap_or_else(|| "0000000000000000".to_string()),
    })
}

/// One client's accepted sends, observed logical messages, and pushes.
struct ClientOutcome {
    sent: u64,
    pushes: u64,
    observed: BTreeMap<(String, String), ChatEntry>,
}

struct ChatClient {
    stream: TcpStream,
    player: String,
    sent: u64,
    pushes: u64,
    last_log_seq: u64,
    observed: BTreeMap<(String, String), ChatEntry>,
}

impl ChatClient {
    fn connect(stream: TcpStream, player: String) -> Self {
        Self {
            stream,
            player,
            sent: 0,
            pushes: 0,
            last_log_seq: 0,
            observed: BTreeMap::new(),
        }
    }

    fn hello(&mut self) -> Result<(), LoadSmokeError> {
        write_json_frame(
            &mut self.stream,
            &ClientCommand::Hello {
                protocol_version: PROTOCOL_VERSION,
                content_hash: CONTENT_HASH.to_string(),
                player_name: self.player.clone(),
            },
        )
        .map_err(|error| LoadSmokeError::frame(codes::HELLO, "write hello", error))?;
        match read_json_frame::<_, ServerMessage>(&mut self.stream)
            .map_err(|error| LoadSmokeError::frame(codes::HELLO, "read welcome", error))?
        {
            ServerMessage::Welcome {
                tick,
                protocol_version,
                resumed,
                ..
            } if protocol_version == PROTOCOL_VERSION && !resumed => {
                if tick != 0 {
                    return Err(LoadSmokeError::new(
                        codes::HELLO,
                        format!("expected welcome at tick 0, got tick {tick}"),
                    ));
                }
                Ok(())
            }
            other => Err(LoadSmokeError::new(
                codes::HELLO,
                format!("expected welcome, got {other:?}"),
            )),
        }
    }

    fn send_message(&mut self, message_index: usize) -> Result<(), LoadSmokeError> {
        let text = message_text(&self.player, message_index);
        write_json_frame(
            &mut self.stream,
            &ClientCommand::Chat { text: text.clone() },
        )
        .map_err(|error| LoadSmokeError::frame(codes::FRAME, "write chat", error))?;

        loop {
            let message = read_json_frame::<_, ServerMessage>(&mut self.stream)
                .map_err(|error| LoadSmokeError::frame(codes::FRAME, "read chat ack", error))?;
            match message {
                ServerMessage::ChatSent { entry, .. } if entry.player == self.player => {
                    if entry.text != text {
                        return Err(LoadSmokeError::new(
                            codes::MESSAGES,
                            format!(
                                "client {} ack text `{}` does not match sent text `{text}`",
                                self.player, entry.text
                            ),
                        ));
                    }
                    self.observe(entry)?;
                    self.sent = self.sent.saturating_add(1);
                    return Ok(());
                }
                ServerMessage::ChatSent { entry, .. } => {
                    self.observe(entry)?;
                    self.pushes = self.pushes.saturating_add(1);
                }
                ServerMessage::Reject { code, message } => {
                    return Err(LoadSmokeError::new(
                        codes::REJECT,
                        format!("chat command rejected with `{code}`: {message}"),
                    ));
                }
                other => {
                    return Err(LoadSmokeError::new(
                        codes::FRAME,
                        format!("expected chat_sent ack, got {other:?}"),
                    ));
                }
            }
        }
    }

    fn poll_once(&mut self, deadline: Instant, polls: &mut u64) -> Result<(), LoadSmokeError> {
        if *polls >= MAX_POLLS {
            return Err(LoadSmokeError::new(
                codes::TIMEOUT,
                format!(
                    "client {} exceeded {MAX_POLLS} chat_since polls",
                    self.player
                ),
            ));
        }
        *polls += 1;
        check_deadline(deadline, &self.player)?;

        write_json_frame(
            &mut self.stream,
            &ClientCommand::ChatSince {
                after: self.last_log_seq,
            },
        )
        .map_err(|error| LoadSmokeError::frame(codes::FRAME, "write chat_since", error))?;

        loop {
            check_deadline(deadline, &self.player)?;
            let message = read_json_frame::<_, ServerMessage>(&mut self.stream)
                .map_err(|error| LoadSmokeError::frame(codes::FRAME, "read chat_since", error))?;
            match message {
                ServerMessage::ChatSent { entry, .. } => {
                    if entry.player == self.player {
                        return Err(LoadSmokeError::new(
                            codes::MESSAGES,
                            format!("client {} received a duplicate own ack", self.player),
                        ));
                    }
                    self.observe(entry)?;
                    self.pushes = self.pushes.saturating_add(1);
                }
                ServerMessage::ChatLog { entries, .. } => {
                    for entry in entries {
                        self.last_log_seq = self.last_log_seq.max(entry.seq);
                        self.observe(entry)?;
                    }
                    return Ok(());
                }
                ServerMessage::Reject { code, message } => {
                    return Err(LoadSmokeError::new(
                        codes::REJECT,
                        format!("chat_since command rejected with `{code}`: {message}"),
                    ));
                }
                other => {
                    return Err(LoadSmokeError::new(
                        codes::FRAME,
                        format!("expected chat_log, got {other:?}"),
                    ));
                }
            }
        }
    }

    fn observe(&mut self, entry: ChatEntry) -> Result<(), LoadSmokeError> {
        let key = (entry.player.clone(), entry.text.clone());
        match self.observed.get(&key) {
            None => {
                self.observed.insert(key, entry);
                Ok(())
            }
            Some(existing)
                if existing.seq == entry.seq
                    && existing.tick == entry.tick
                    && existing.player_id == entry.player_id =>
            {
                Ok(())
            }
            Some(existing) => Err(LoadSmokeError::new(
                codes::MESSAGES,
                format!(
                    "client {} observed conflicting copies of `{}` (seq {} vs {})",
                    self.player, entry.text, existing.seq, entry.seq
                ),
            )),
        }
    }

    fn disconnect(mut self) -> Result<(), LoadSmokeError> {
        write_json_frame(&mut self.stream, &ClientCommand::Disconnect)
            .map_err(|error| LoadSmokeError::frame(codes::FRAME, "write disconnect", error))?;
        match read_json_frame::<_, ServerMessage>(&mut self.stream)
            .map_err(|error| LoadSmokeError::frame(codes::FRAME, "read goodbye", error))?
        {
            ServerMessage::Goodbye => Ok(()),
            other => Err(LoadSmokeError::new(
                codes::FRAME,
                format!("expected goodbye, got {other:?}"),
            )),
        }
    }
}

fn run_client(
    stream: TcpStream,
    index: usize,
    clients: usize,
    messages: usize,
    barrier: &AbortableBarrier,
    deadline: Instant,
) -> Result<ClientOutcome, LoadSmokeError> {
    let player = player_name(index);
    let mut client = ChatClient::connect(stream, player);
    client.hello()?;

    // No client sends until every client's welcome is out and the registry has
    // marked every session hello-complete. Otherwise an early sender could
    // miss pushes to a session that has not joined yet.
    barrier.wait(deadline)?;

    let expected_messages = expected_messages(clients, messages).expect("validated messages");
    let pushes_per_client = expected_pushes(clients, messages).expect("validated pushes")
        / u64::try_from(clients).expect("client count fits u64");
    let mut polls = 0u64;

    for message_index in 1..=messages {
        client.send_message(message_index)?;
        // Drain pushes every round so the bounded outbound queue stays well
        // below capacity even at the largest accepted workload.
        client.poll_once(deadline, &mut polls)?;
        barrier.wait(deadline)?;
    }

    while u64::try_from(client.observed.len()).unwrap_or(u64::MAX) < expected_messages
        || client.pushes < pushes_per_client
    {
        client.poll_once(deadline, &mut polls)?;
    }

    if client.sent != u64::try_from(messages).expect("message count fits u64") {
        return Err(LoadSmokeError::new(
            codes::MESSAGES,
            format!(
                "client {} sent {} messages, expected {messages}",
                client.player, client.sent
            ),
        ));
    }
    if client.pushes != pushes_per_client {
        return Err(LoadSmokeError::new(
            codes::PUSHES,
            format!(
                "client {} received {} pushes, expected {pushes_per_client}",
                client.player, client.pushes
            ),
        ));
    }
    if u64::try_from(client.observed.len()).ok() != Some(expected_messages) {
        return Err(LoadSmokeError::new(
            codes::MESSAGES,
            format!(
                "client {} observed {} distinct messages, expected {expected_messages}",
                client.player,
                client.observed.len()
            ),
        ));
    }

    let sent = client.sent;
    let pushes = client.pushes;
    let observed = std::mem::take(&mut client.observed);
    client.disconnect()?;

    Ok(ClientOutcome {
        sent,
        pushes,
        observed,
    })
}

fn check_deadline(deadline: Instant, player: &str) -> Result<(), LoadSmokeError> {
    if Instant::now() >= deadline {
        return Err(LoadSmokeError::new(
            codes::TIMEOUT,
            format!("client {player} exceeded the load-smoke deadline"),
        ));
    }
    Ok(())
}

/// Abortable generation barrier used after `hello` and after every send round.
///
/// `std::sync::Barrier` cannot be released when one participant fails before
/// arriving, so the smoke uses a small condvar gate with an explicit abort.
struct AbortableBarrier {
    state: Mutex<BarrierState>,
    ready: Condvar,
    total: usize,
}

#[derive(Default)]
struct BarrierState {
    generation: u64,
    arrived: usize,
    aborted: bool,
}

impl AbortableBarrier {
    fn new(total: usize) -> Self {
        Self {
            state: Mutex::new(BarrierState::default()),
            ready: Condvar::new(),
            total,
        }
    }

    fn wait(&self, deadline: Instant) -> Result<(), LoadSmokeError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| LoadSmokeError::new(codes::PANIC, "load-smoke barrier poisoned"))?;
        if state.aborted {
            return Err(aborted_error());
        }

        let generation = state.generation;
        state.arrived += 1;
        if state.arrived >= self.total {
            state.generation += 1;
            state.arrived = 0;
            self.ready.notify_all();
            return Ok(());
        }

        while state.generation == generation && !state.aborted {
            let now = Instant::now();
            if now >= deadline {
                drop(state);
                self.abort();
                return Err(LoadSmokeError::new(
                    codes::TIMEOUT,
                    "load-smoke barrier exceeded the run deadline",
                ));
            }
            let (next, timeout) = self
                .ready
                .wait_timeout(state, deadline - now)
                .map_err(|_| LoadSmokeError::new(codes::PANIC, "load-smoke barrier poisoned"))?;
            state = next;
            if timeout.timed_out() && state.generation == generation && !state.aborted {
                drop(state);
                self.abort();
                return Err(LoadSmokeError::new(
                    codes::TIMEOUT,
                    "load-smoke barrier exceeded the run deadline",
                ));
            }
        }

        if state.aborted {
            Err(aborted_error())
        } else {
            Ok(())
        }
    }

    fn abort(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.aborted = true;
            self.ready.notify_all();
        }
    }
}

fn aborted_error() -> LoadSmokeError {
    LoadSmokeError::new(codes::GATE, "another load-smoke client aborted the barrier")
}

fn player_name(index: usize) -> String {
    format!("load-{index:02}")
}

fn message_text(player: &str, message_index: usize) -> String {
    format!("{player} message {message_index:02}")
}

fn canonical_messages(clients: usize, messages: usize) -> BTreeSet<(String, String)> {
    let mut canonical = BTreeSet::new();
    for index in 0..clients {
        let player = player_name(index);
        for message_index in 1..=messages {
            canonical.insert((player.clone(), message_text(&player, message_index)));
        }
    }
    canonical
}

/// FNV-1a over the keys in `BTreeSet` order (player, then text).
fn chat_checksum<'a>(keys: impl Iterator<Item = &'a (String, String)>) -> String {
    let mut hash = FNV_OFFSET;
    for (player, text) in keys {
        hash = hash_len_prefixed(hash, player);
        hash = hash_len_prefixed(hash, text);
    }
    format!("{hash:016x}")
}

fn hash_len_prefixed(mut hash: u64, value: &str) -> u64 {
    hash = fnv_update(hash, &(value.len() as u64).to_le_bytes());
    fnv_update(hash, value.as_bytes())
}

fn fnv_update(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expected_counts_follow_the_client_topology() {
        assert_eq!(expected_messages(8, 16), Some(128));
        assert_eq!(expected_messages(4, 4), Some(16));
        assert_eq!(expected_messages(2, 1), Some(2));
        assert_eq!(expected_pushes(8, 16), Some(896));
        assert_eq!(expected_pushes(4, 4), Some(48));
        assert_eq!(expected_pushes(2, 1), Some(2));
        assert_eq!(expected_pushes(2, 48), Some(96));
        assert_eq!(expected_pushes(1, 16), Some(0));
    }

    #[test]
    fn expected_counts_reject_overflow() {
        assert_eq!(expected_messages(usize::MAX, 2), None);
        assert_eq!(expected_pushes(usize::MAX, 2), None);
    }

    #[test]
    fn workload_validation_bounds_the_socket_and_chat_burst() {
        assert!(validate_workload(2, 1).is_ok());
        assert!(validate_workload(MAX_CLIENTS, MAX_MESSAGES_PER_CLIENT).is_ok());
        assert_eq!(validate_workload(1, 16).unwrap_err().code(), codes::ARGS);
        assert_eq!(validate_workload(0, 16).unwrap_err().code(), codes::ARGS);
        assert_eq!(
            validate_workload(MAX_CLIENTS + 1, 1).unwrap_err().code(),
            codes::ARGS
        );
        assert_eq!(validate_workload(8, 0).unwrap_err().code(), codes::ARGS);
        assert_eq!(
            validate_workload(8, MAX_MESSAGES_PER_CLIENT + 1)
                .unwrap_err()
                .code(),
            codes::ARGS
        );
    }

    #[test]
    fn canonical_set_is_unique_and_matches_the_count() {
        let canonical = canonical_messages(8, 16);
        assert_eq!(canonical.len(), 128);
        assert_eq!(canonical.len() as u64, expected_messages(8, 16).unwrap());
        assert!(canonical.contains(&(player_name(7), message_text(&player_name(7), 16))));
    }

    #[test]
    fn checksum_is_independent_of_observation_order() {
        let canonical = canonical_messages(4, 4);
        let forward = chat_checksum(canonical.iter());
        let mut shuffled: Vec<_> = canonical.iter().cloned().collect();
        shuffled.reverse();
        shuffled.sort();
        let resorted = chat_checksum(shuffled.iter());
        assert_eq!(forward, resorted);
        assert_eq!(forward.len(), 16);
    }
}
