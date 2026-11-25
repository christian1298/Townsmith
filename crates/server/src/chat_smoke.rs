//! Deterministic two-connection chat loopback smoke.
//!
//! The harness binds one local listener on port 0 and drives two independent
//! phases through the real protocol, each with a fresh [`ServerWorld`]:
//!
//! 1. **log**: Alice and Bob `hello`; Alice sends `"  hello  "` and the
//!    server trims it; Bob polls `chat_since {after: 0}` and sees the line;
//!    Bob sends `"from b"` and Alice polls `chat_since {after: 1}` and sees
//!    only Bob's line; empty, over-long, and control-character messages reject
//!    with `chat.empty`, `chat.too_long`, and `chat.invalid` and append
//!    nothing.
//! 2. **retention**: a fresh world, Alice and Bob alternate 40 chat sends
//!    each (80 accepted lines) and the retained log caps at
//!    [`CHAT_LOG_CAPACITY`] entries. The per-session token bucket accepts 48
//!    chat sends after `hello`, so 40 per session stays below the burst and
//!    every send is accepted without relying on wall-clock time.
//!
//! Chat is in-memory side state: no phase advances the authoritative tick and
//! the world checksum is unchanged. The chat checksum is FNV-1a over every
//! retained entry (phase order, then sequence order), so two runs produce the
//! same report without sleeps or `Instant`.
//!
//! The module is test scaffolding only: it does not change the protocol, the
//! world, or persistence.

use std::collections::VecDeque;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::protocol::{
    configure_stream, decode_client_command, frame_to_io, read_frame, write_json_frame, ChatEntry,
    ClientCommand, FrameError, ServerMessage, CHAT_LOG_CAPACITY, LOOPBACK_SEED, MAX_CHAT_CHARS,
    PROTOCOL_VERSION, SESSION_IO_TIMEOUT,
};
use crate::world::{codes, ClientRole, ServerWorld};

/// Round-trip content hash carried by every hello in the smoke.
const CONTENT_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// Chat sends per session in the retention phase.
///
/// A fresh session accepts 48 chat commands after `hello` before the
/// deterministic token bucket rejects one (40 burst credits, one credit per
/// command, 200 milli-credits refilled per applied command). 40 stays below
/// that burst, so the phase never depends on timing.
pub const CHAT_BURST_PER_SESSION: u64 = 40;

/// Sessions in each phase (Alice and Bob).
const PHASE_SESSIONS: u64 = 2;

/// Expected accepted chat lines in the log phase.
const LOG_PHASE_MESSAGES: u64 = 2;

/// Expected accepted chat lines in the retention phase.
const RETENTION_PHASE_MESSAGES: u64 = CHAT_BURST_PER_SESSION * 2;

/// Result of one deterministic chat loopback run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatSmokeReport {
    /// Accepted chat lines across both phases.
    pub messages: u64,
    /// Retained chat entries in the final (retention) world.
    pub entries: u64,
    /// FNV-1a over every phase's retained chat entries and a phase separator.
    pub chat_checksum: String,
    /// World checksum after the final phase; identical to a fresh world.
    pub world_checksum: String,
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv_update(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn hash_str(hash: u64, value: &str) -> u64 {
    let hash = fnv_update(hash, value.as_bytes());
    fnv_update(hash, &[0x1f])
}

fn hash_entry(mut hash: u64, entry: &ChatEntry) -> u64 {
    hash = fnv_update(hash, &entry.seq.to_le_bytes());
    hash = fnv_update(hash, &entry.tick.to_le_bytes());
    hash = fnv_update(hash, &entry.player_id.to_le_bytes());
    hash = hash_str(hash, &entry.player);
    hash = hash_str(hash, &entry.text);
    hash
}

/// Run the deterministic two-connection chat loopback over TCP.
///
/// # Errors
///
/// Returns an error when the listener, a socket, or an assertion in the
/// harness fails. No wall-clock sleeps are used; every ordering point is a
/// request/response pair, so a passing run is reproducible.
pub fn run_chat_smoke() -> io::Result<ChatSmokeReport> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let address = listener.local_addr()?;

    let server = thread::spawn(move || serve_chat(listener));
    let client_result = run_chat_client(address);
    let server_result = server
        .join()
        .map_err(|_| io::Error::other("chat loopback server thread panicked"))?;

    client_result?;
    server_result
}

fn serve_chat(listener: TcpListener) -> io::Result<ChatSmokeReport> {
    let reference = ServerWorld::with_role(LOOPBACK_SEED, ClientRole::Viewer);
    let world_checksum = reference.checksum();

    let mut hash = FNV_OFFSET;
    let mut messages = 0u64;
    let mut entries = 0u64;

    for (phase, expected_messages, expected_entries) in [
        ("log", LOG_PHASE_MESSAGES, LOG_PHASE_MESSAGES as usize),
        ("retention", RETENTION_PHASE_MESSAGES, CHAT_LOG_CAPACITY),
    ] {
        let world = Arc::new(Mutex::new(ServerWorld::with_role(
            LOOPBACK_SEED,
            ClientRole::Viewer,
        )));
        let mut handles = Vec::with_capacity(PHASE_SESSIONS as usize);

        for connection_id in 1..=PHASE_SESSIONS {
            let (stream, _peer) = listener.accept()?;
            let shared = Arc::clone(&world);
            handles.push(thread::spawn(move || {
                serve_chat_stream(stream, shared, connection_id)
            }));
        }
        for handle in handles {
            handle
                .join()
                .map_err(|_| io::Error::other("chat session thread panicked"))??;
        }

        let world = world
            .lock()
            .map_err(|_| io::Error::other("chat world mutex poisoned"))?;
        if world.tick != 0 {
            return Err(io::Error::other(format!(
                "chat phase `{phase}` moved the tick to {}",
                world.tick
            )));
        }
        if world.checksum() != world_checksum {
            return Err(io::Error::other(format!(
                "chat phase `{phase}` changed the world checksum: {} != {world_checksum}",
                world.checksum()
            )));
        }
        let accepted = world.chat.back().map_or(0, |entry| entry.seq);
        if accepted != expected_messages {
            return Err(io::Error::other(format!(
                "chat phase `{phase}` expected {expected_messages} accepted lines, got {accepted}"
            )));
        }
        if world.chat.len() != expected_entries {
            return Err(io::Error::other(format!(
                "chat phase `{phase}` expected {expected_entries} retained entries, got {}",
                world.chat.len()
            )));
        }
        for entry in &world.chat {
            hash = hash_entry(hash, entry);
        }
        hash = fnv_update(hash, &[0x1e]);

        messages += accepted;
        entries = u64::try_from(world.chat.len()).unwrap_or(u64::MAX);
    }

    Ok(ChatSmokeReport {
        messages,
        entries,
        chat_checksum: format!("{hash:016x}"),
        world_checksum,
    })
}

fn serve_chat_stream(
    stream: TcpStream,
    world: Arc<Mutex<ServerWorld>>,
    connection_id: u64,
) -> io::Result<()> {
    configure_stream(&stream)?;
    let mut stream = stream;

    loop {
        let payload = match read_frame(&mut stream) {
            Ok(payload) => payload,
            Err(FrameError::Truncated { .. }) | Err(FrameError::Io(_)) => break,
            Err(error) => return Err(io::Error::other(error.to_string())),
        };

        let (responses, closed) = match decode_client_command(&payload) {
            Ok(command) => {
                let mut world = world
                    .lock()
                    .map_err(|_| io::Error::other("chat world mutex poisoned"))?;
                let responses = world.apply_session(connection_id, &command);
                let closed = world.session_closed(connection_id);
                (responses, closed)
            }
            Err(rejection) => (vec![rejection.into_message()], false),
        };
        for response in &responses {
            write_json_frame(&mut stream, response).map_err(frame_to_io)?;
        }

        if closed {
            break;
        }
    }

    world
        .lock()
        .map_err(|_| io::Error::other("chat world mutex poisoned"))?
        .close_connection(connection_id);

    Ok(())
}

struct ChatClient {
    stream: TcpStream,
}

impl ChatClient {
    fn connect(address: SocketAddr, player: &str) -> io::Result<Self> {
        let stream = TcpStream::connect_timeout(&address, SESSION_IO_TIMEOUT)?;
        configure_stream(&stream)?;
        let mut client = Self { stream };
        match client.send(&ClientCommand::Hello {
            protocol_version: PROTOCOL_VERSION,
            content_hash: CONTENT_HASH.to_string(),
            player_name: player.to_string(),
        })? {
            ServerMessage::Welcome {
                tick: 0,
                protocol_version,
                ..
            } if protocol_version == PROTOCOL_VERSION => Ok(client),
            other => Err(io::Error::other(format!(
                "chat hello expected welcome at tick 0, got {other:?}"
            ))),
        }
    }

    fn send(&mut self, command: &ClientCommand) -> io::Result<ServerMessage> {
        write_json_frame(&mut self.stream, command).map_err(frame_to_io)?;
        crate::protocol::read_json_frame(&mut self.stream).map_err(frame_to_io)
    }

    fn chat(&mut self, text: &str) -> io::Result<ServerMessage> {
        self.send(&ClientCommand::Chat {
            text: text.to_string(),
        })
    }

    fn chat_since(&mut self, after: u64) -> io::Result<ServerMessage> {
        self.send(&ClientCommand::ChatSince { after })
    }

    fn goodbye(self) -> io::Result<ServerMessage> {
        let mut client = self;
        client.send(&ClientCommand::Disconnect)
    }
}

fn run_chat_client(address: SocketAddr) -> io::Result<()> {
    phase_chat_log(address)?;
    phase_chat_retention(address)?;
    Ok(())
}

fn phase_chat_log(address: SocketAddr) -> io::Result<()> {
    let mut alice = ChatClient::connect(address, "alice")?;
    let mut bob = ChatClient::connect(address, "bob")?;

    // 1. Alice's line is trimmed and tagged with her stable id and tick 0.
    let alice_entry = expect_chat_sent(alice.chat("  hello  ")?, 1, 1, "alice", "hello")?;
    // 2. Bob polls from the start and sees Alice's line.
    expect_chat_log(bob.chat_since(0)?, std::slice::from_ref(&alice_entry))?;

    // 3. Bob's line is visible to Alice, and only newer than `after: 1`.
    let bob_entry = expect_chat_sent(bob.chat("from b")?, 2, 2, "bob", "from b")?;
    if bob_entry.player_id == alice_entry.player_id {
        return Err(io::Error::other(
            "chat assigned the same player id to both sessions",
        ));
    }
    expect_chat_log(alice.chat_since(1)?, std::slice::from_ref(&bob_entry))?;

    // 4. Invalid messages reject with stable codes and append nothing.
    expect_reject(alice.chat("")?, codes::CHAT_EMPTY)?;
    expect_reject(
        alice.chat(&"x".repeat(MAX_CHAT_CHARS + 1))?,
        codes::CHAT_TOO_LONG,
    )?;
    expect_reject(alice.chat("line\nbreak")?, codes::CHAT_INVALID)?;
    expect_chat_log(
        alice.chat_since(0)?,
        &[alice_entry.clone(), bob_entry.clone()],
    )?;
    expect_chat_log(alice.chat_since(2)?, &[])?;

    alice.goodbye()?;
    bob.goodbye()?;
    Ok(())
}

fn phase_chat_retention(address: SocketAddr) -> io::Result<()> {
    let mut alice = ChatClient::connect(address, "alice")?;
    let mut bob = ChatClient::connect(address, "bob")?;

    // Alternate the two sessions so the retained order is fixed. Each session
    // stays below its post-hello token-bucket burst, so no send is throttled.
    for round in 0..CHAT_BURST_PER_SESSION {
        let text = format!("a{round}");
        expect_chat_sent(alice.chat(&text)?, round * 2 + 1, 1, "alice", &text)?;
        let text = format!("b{round}");
        expect_chat_sent(bob.chat(&text)?, round * 2 + 2, 2, "bob", &text)?;
    }

    let entries = expect_chat_log_entries(bob.chat_since(0)?)?;
    if entries.len() != CHAT_LOG_CAPACITY {
        return Err(io::Error::other(format!(
            "chat retention expected {CHAT_LOG_CAPACITY} entries, got {}",
            entries.len()
        )));
    }
    let first_seq = RETENTION_PHASE_MESSAGES + 1 - CHAT_LOG_CAPACITY as u64;
    for (offset, entry) in entries.iter().enumerate() {
        let seq = first_seq + offset as u64;
        if entry.seq != seq {
            return Err(io::Error::other(format!(
                "chat retention expected sequence {seq}, got {}",
                entry.seq
            )));
        }
        let expected_text = if seq % 2 == 1 {
            format!("a{}", (seq - 1) / 2)
        } else {
            format!("b{}", (seq / 2) - 1)
        };
        if entry.text != expected_text {
            return Err(io::Error::other(format!(
                "chat retention at sequence {seq} expected `{expected_text}`, got `{}`",
                entry.text
            )));
        }
    }
    if entries.back().map(|entry| entry.seq) != Some(RETENTION_PHASE_MESSAGES) {
        return Err(io::Error::other(
            "chat retention did not retain the newest line",
        ));
    }

    alice.goodbye()?;
    bob.goodbye()?;
    Ok(())
}

fn expect_chat_sent(
    message: ServerMessage,
    seq: u64,
    player_id: u64,
    player: &str,
    text: &str,
) -> io::Result<ChatEntry> {
    match message {
        ServerMessage::ChatSent { tick: 0, entry }
            if entry.seq == seq
                && entry.tick == 0
                && entry.player_id == player_id
                && entry.player == player
                && entry.text == text =>
        {
            Ok(entry)
        }
        other => Err(io::Error::other(format!(
            "chat expected chat_sent seq={seq} player={player} text=`{text}` at tick 0, got {other:?}"
        ))),
    }
}

fn expect_chat_log(message: ServerMessage, expected: &[ChatEntry]) -> io::Result<()> {
    let entries = expect_chat_log_entries(message)?;
    if entries == expected {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "chat expected chat_log {expected:?}, got {entries:?}"
        )))
    }
}

fn expect_chat_log_entries(message: ServerMessage) -> io::Result<VecDeque<ChatEntry>> {
    match message {
        ServerMessage::ChatLog { tick: 0, entries } => Ok(entries.into()),
        other => Err(io::Error::other(format!(
            "chat expected chat_log at tick 0, got {other:?}"
        ))),
    }
}

fn expect_reject(message: ServerMessage, code: &str) -> io::Result<()> {
    match message {
        ServerMessage::Reject {
            code: actual_code, ..
        } if actual_code == code => Ok(()),
        other => Err(io::Error::other(format!(
            "chat expected rejection `{code}`, got {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_loopback_is_deterministic_and_chat_is_not_world_state() {
        const CHAT_CHECKSUM: &str = "985fadc636f57906";
        const WORLD_CHECKSUM: &str = "ffaf7d7e3bfa00dd";
        let first = run_chat_smoke().expect("first chat loopback");
        let second = run_chat_smoke().expect("second chat loopback");
        assert_eq!(first.chat_checksum, CHAT_CHECKSUM);
        assert_eq!(first.world_checksum, WORLD_CHECKSUM);

        assert_eq!(
            first.messages,
            LOG_PHASE_MESSAGES + RETENTION_PHASE_MESSAGES,
            "two log-phase lines plus 80 retention-phase lines"
        );
        assert_eq!(first.entries, CHAT_LOG_CAPACITY as u64);
        assert_eq!(first.chat_checksum.len(), 16);
        assert!(first
            .chat_checksum
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(
            first.world_checksum,
            ServerWorld::with_role(LOOPBACK_SEED, ClientRole::Viewer).checksum(),
            "chat is not world state"
        );
        assert_eq!(first, second, "two runs must produce the same report");
    }
}
