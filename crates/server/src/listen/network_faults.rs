//! Deterministic network-fault coverage for the protocol v1 framing and
//! session boundary.
//!
//! Every test uses in-memory [`Read`]/[`Write`] implementations: no socket, no
//! wall-clock sleep, and no timing assumption. The tests drive both the frame
//! codec directly and [`serve_io`], so a fault at a read/write boundary is
//! checked against the authoritative world as well as the wire bytes.

use std::fs;
use std::io::{self, Cursor, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::outbound::{drain, OutboundError, OutboundQueue};
use super::{serve_io, serve_io_outbound, session_ended, ListenConfig};
use crate::protocol::{
    codes, read_frame, read_json_frame, write_frame, write_json_frame, ClientCommand, FrameError,
    ServerMessage, LOOPBACK_SEED, MAX_FRAME_LEN,
};
use crate::save::temp_path;
use crate::world::{codes as world_codes, ClientRole, ServerWorld, RESUME_GRACE_TICKS};

const SEED: u64 = LOOPBACK_SEED;

fn hello() -> ClientCommand {
    hello_named("mayor")
}

fn hello_named(player_name: &str) -> ClientCommand {
    ClientCommand::Hello {
        protocol_version: crate::protocol::PROTOCOL_VERSION,
        content_hash: "0123456789abcdef".repeat(4),
        player_name: player_name.to_string(),
    }
}

fn ping(nonce: u64) -> ClientCommand {
    ClientCommand::Ping { nonce }
}

fn set_tax(resident_rate: u16, business_rate: u16) -> ClientCommand {
    ClientCommand::SetTax {
        resident_rate,
        business_rate,
    }
}

fn place_road() -> ClientCommand {
    ClientCommand::PlaceRoad {
        start: [0, 0],
        end: [8, 0],
    }
}

fn encode_frame(command: &ClientCommand) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_json_frame(&mut bytes, command).expect("command encodes");
    bytes
}

fn encode_commands(commands: &[ClientCommand]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for command in commands {
        write_json_frame(&mut bytes, command).expect("command encodes");
    }
    bytes
}

fn decode_frames(bytes: &[u8]) -> Vec<ServerMessage> {
    let mut reader = Cursor::new(bytes);
    let mut messages = Vec::new();
    loop {
        match read_json_frame::<_, ServerMessage>(&mut reader) {
            Ok(message) => messages.push(message),
            Err(FrameError::Truncated { actual: 0, .. }) => return messages,
            Err(error) => panic!("unexpected response framing error: {error}"),
        }
    }
}

fn read_available(bytes: &[u8]) -> Result<Vec<u8>, FrameError> {
    let mut reader = Cursor::new(bytes);
    read_frame(&mut reader)
}

fn frame_io_kind(error: FrameError) -> io::ErrorKind {
    match error {
        FrameError::Io(error) => error.kind(),
        other => panic!("expected a frame I/O error, got {other:?}"),
    }
}

fn welcome_token(message: &ServerMessage) -> String {
    match message {
        ServerMessage::Welcome { session_token, .. } => session_token.clone(),
        other => panic!("expected welcome, got {other:?}"),
    }
}

fn expect_reject_code(message: &ServerMessage, expected: &str) {
    match message {
        ServerMessage::Reject { code, .. } => assert_eq!(code, expected),
        other => panic!("expected rejection `{expected}`, got {other:?}"),
    }
}

fn temp_dir() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "townsmith-network-faults-{}-{unique}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("temp dir");
    path
}

/// A reader that returns at most `chunk` bytes per call.
struct ChunkedReader {
    bytes: Vec<u8>,
    offset: usize,
    chunk: usize,
}

impl ChunkedReader {
    fn new(bytes: Vec<u8>, chunk: usize) -> Self {
        assert!(chunk > 0, "chunk size must be positive");
        Self {
            bytes,
            offset: 0,
            chunk,
        }
    }
}

impl Read for ChunkedReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.offset >= self.bytes.len() {
            return Ok(0);
        }
        let count = self
            .chunk
            .min(buffer.len())
            .min(self.bytes.len() - self.offset);
        buffer[..count].copy_from_slice(&self.bytes[self.offset..self.offset + count]);
        self.offset += count;
        Ok(count)
    }
}

/// A reader that returns its bytes and then a deterministic error.
struct FaultyReader {
    bytes: Vec<u8>,
    offset: usize,
    fail_at: usize,
    kind: io::ErrorKind,
}

impl FaultyReader {
    /// Fail after every byte has been read (an idle timeout or reset).
    fn after(bytes: Vec<u8>, kind: io::ErrorKind) -> Self {
        let fail_at = bytes.len();
        Self {
            bytes,
            offset: 0,
            fail_at,
            kind,
        }
    }

    /// Fail after `fail_at` bytes (a reset in the middle of a frame).
    fn at(bytes: Vec<u8>, fail_at: usize, kind: io::ErrorKind) -> Self {
        Self {
            bytes,
            offset: 0,
            fail_at,
            kind,
        }
    }
}

impl Read for FaultyReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.offset >= self.fail_at {
            return Err(io::Error::new(self.kind, "injected read fault"));
        }
        let available = self
            .bytes
            .len()
            .saturating_sub(self.offset)
            .min(self.fail_at - self.offset);
        if available == 0 || buffer.is_empty() {
            return Ok(0);
        }
        let count = available.min(buffer.len());
        buffer[..count].copy_from_slice(&self.bytes[self.offset..self.offset + count]);
        self.offset += count;
        Ok(count)
    }
}

/// A reader that returns `Interrupted` once, then behaves normally.
struct InterruptOnceReader {
    bytes: Vec<u8>,
    offset: usize,
    interrupted: bool,
}

impl InterruptOnceReader {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            offset: 0,
            interrupted: false,
        }
    }
}

impl Read for InterruptOnceReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if !self.interrupted {
            self.interrupted = true;
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        if self.offset >= self.bytes.len() {
            return Ok(0);
        }
        let count = buffer.len().min(self.bytes.len() - self.offset);
        buffer[..count].copy_from_slice(&self.bytes[self.offset..self.offset + count]);
        self.offset += count;
        Ok(count)
    }
}

/// A writer that accepts one byte per `write` call.
#[derive(Default)]
struct OneByteWriter {
    bytes: Vec<u8>,
}

impl Write for OneByteWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let count = buffer.len().min(1);
        self.bytes.extend_from_slice(&buffer[..count]);
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A writer that accepts `budget` bytes and then fails with `kind`.
struct FailingWriter {
    bytes: Vec<u8>,
    budget: usize,
    kind: io::ErrorKind,
}

impl FailingWriter {
    fn new(budget: usize, kind: io::ErrorKind) -> Self {
        Self {
            bytes: Vec::new(),
            budget,
            kind,
        }
    }
}

impl Write for FailingWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.budget == 0 {
            return Err(io::Error::new(self.kind, "injected write fault"));
        }
        let count = buffer.len().min(self.budget);
        self.bytes.extend_from_slice(&buffer[..count]);
        self.budget -= count;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Served {
    commands: io::Result<u64>,
    responses: Vec<ServerMessage>,
    saves: u64,
}

fn serve_bytes(
    world: &mut ServerWorld,
    bytes: Vec<u8>,
    chunk: usize,
    config: &ListenConfig,
) -> Served {
    let mut reader = ChunkedReader::new(bytes, chunk);
    let mut writer = Vec::new();
    let mut last_saved_tick = world.tick;
    let mut saves = 0;
    let commands = serve_io(
        &mut reader,
        &mut writer,
        world,
        config,
        &mut last_saved_tick,
        &mut saves,
    );
    Served {
        commands,
        responses: decode_frames(&writer),
        saves,
    }
}

fn mayor_world() -> ServerWorld {
    ServerWorld::with_role(SEED, ClientRole::Mayor)
}

#[test]
fn one_byte_at_a_time_frame_decodes_and_then_reports_clean_eof() {
    let command = hello_named("Jörg-€");
    let mut reader = ChunkedReader::new(encode_frame(&command), 1);

    let decoded: ClientCommand = read_json_frame(&mut reader).expect("fragmented frame decodes");
    assert_eq!(decoded, command);

    match read_frame(&mut reader) {
        Err(FrameError::Truncated { expected, actual }) => {
            assert_eq!((expected, actual), (4, 0));
        }
        other => panic!("expected a clean EOF at the header boundary, got {other:?}"),
    }
}

#[test]
fn coalesced_frames_decode_in_order() {
    let commands = vec![hello(), ping(7), set_tax(12, 8)];
    let mut reader = Cursor::new(encode_commands(&commands));

    for expected in &commands {
        let actual: ClientCommand = read_json_frame(&mut reader).expect("coalesced frame decodes");
        assert_eq!(&actual, expected);
    }
    assert!(matches!(
        read_frame(&mut reader),
        Err(FrameError::Truncated { actual: 0, .. })
    ));
}

#[test]
fn header_split_across_reads_decodes() {
    // Three-byte reads split the four-byte header into 3 + 1.
    let mut reader = ChunkedReader::new(encode_frame(&hello()), 3);
    let decoded: ClientCommand = read_json_frame(&mut reader).expect("split header decodes");
    assert_eq!(decoded, hello());
}

#[test]
fn truncated_header_is_reported_at_every_prefix() {
    let frame = encode_frame(&hello());
    for prefix in 0..4 {
        match read_available(&frame[..prefix]) {
            Err(FrameError::Truncated { expected, actual }) => {
                assert_eq!(expected, 4, "header prefix {prefix}");
                assert_eq!(actual, prefix, "header prefix {prefix}");
            }
            other => panic!("header prefix {prefix}: expected truncation, got {other:?}"),
        }
    }
}

#[test]
fn truncated_payload_is_reported_at_every_prefix() {
    let frame = encode_frame(&hello());
    let header: [u8; 4] = frame[..4].try_into().expect("header");
    let payload_len = u32::from_be_bytes(header) as usize;
    assert_eq!(payload_len, frame.len() - 4);

    for prefix in 0..payload_len {
        match read_available(&frame[..4 + prefix]) {
            Err(FrameError::Truncated { expected, actual }) => {
                assert_eq!(expected, payload_len, "payload prefix {prefix}");
                assert_eq!(actual, prefix, "payload prefix {prefix}");
            }
            other => panic!("payload prefix {prefix}: expected truncation, got {other:?}"),
        }
    }

    assert_eq!(
        read_available(&frame).expect("complete frame"),
        frame[4..].to_vec()
    );
}

#[test]
fn zero_length_oversized_and_maximum_headers_are_classified() {
    assert_eq!(
        write_frame(&mut Vec::new(), &[])
            .expect_err("empty payload")
            .code(),
        codes::FRAME_ZERO_LENGTH
    );

    let zero = 0u32.to_be_bytes();
    assert_eq!(
        read_available(&zero).expect_err("zero length").code(),
        codes::FRAME_ZERO_LENGTH
    );

    let oversized = u32::try_from(MAX_FRAME_LEN + 1)
        .expect("fits u32")
        .to_be_bytes();
    assert_eq!(
        read_available(&oversized).expect_err("oversized").code(),
        codes::FRAME_OVERSIZED
    );

    // The cap itself is a valid length; the payload is simply truncated here.
    let at_cap = u32::try_from(MAX_FRAME_LEN)
        .expect("fits u32")
        .to_be_bytes();
    match read_available(&at_cap) {
        Err(FrameError::Truncated { expected, actual }) => {
            assert_eq!(expected, MAX_FRAME_LEN);
            assert_eq!(actual, 0);
        }
        other => panic!("expected a truncated maximum-size frame, got {other:?}"),
    }

    let oversized_payload = vec![b'x'; MAX_FRAME_LEN + 1];
    assert_eq!(
        write_frame(&mut Vec::new(), &oversized_payload)
            .expect_err("oversized payload")
            .code(),
        codes::FRAME_OVERSIZED
    );
}

#[test]
fn interrupted_reads_are_retried() {
    let mut reader = InterruptOnceReader::new(encode_frame(&hello()));
    let decoded: ClientCommand = read_json_frame(&mut reader).expect("interrupted read is retried");
    assert_eq!(decoded, hello());
}

#[test]
fn reader_faults_map_to_frame_io_errors() {
    let frame = encode_frame(&hello());

    let mut mid_header = FaultyReader::at(frame.clone(), 2, io::ErrorKind::ConnectionReset);
    assert_eq!(
        frame_io_kind(read_frame(&mut mid_header).expect_err("reset mid-header")),
        io::ErrorKind::ConnectionReset
    );

    let mut after_frame = FaultyReader::after(frame.clone(), io::ErrorKind::TimedOut);
    assert_eq!(
        read_frame(&mut after_frame).expect("frame before the fault"),
        frame[4..].to_vec()
    );
    assert_eq!(
        frame_io_kind(read_frame(&mut after_frame).expect_err("timeout after the frame")),
        io::ErrorKind::TimedOut
    );
}

#[test]
fn session_ended_maps_transport_disconnects_and_timeouts() {
    for kind in [
        io::ErrorKind::UnexpectedEof,
        io::ErrorKind::ConnectionReset,
        io::ErrorKind::ConnectionAborted,
        io::ErrorKind::BrokenPipe,
        io::ErrorKind::WouldBlock,
        io::ErrorKind::TimedOut,
    ] {
        assert!(
            session_ended(&io::Error::from(kind)),
            "{kind:?} must end the session"
        );
    }
    for kind in [
        io::ErrorKind::InvalidData,
        io::ErrorKind::InvalidInput,
        io::ErrorKind::NotFound,
        io::ErrorKind::PermissionDenied,
        io::ErrorKind::OutOfMemory,
    ] {
        assert!(
            !session_ended(&io::Error::from(kind)),
            "{kind:?} must not be treated as a client disconnect"
        );
    }
}

#[test]
fn a_slow_writer_reassembles_the_frame() {
    let message = ServerMessage::Pong { nonce: 42 };
    let mut writer = OneByteWriter::default();
    write_json_frame(&mut writer, &message).expect("one-byte writes succeed");

    let mut reader = Cursor::new(writer.bytes);
    let decoded: ServerMessage = read_json_frame(&mut reader).expect("round trip");
    assert_eq!(decoded, message);
}

#[test]
fn broken_pipe_is_reported_at_each_write_phase() {
    let message = ServerMessage::Pong { nonce: 7 };
    let mut encoded = Vec::new();
    write_json_frame(&mut encoded, &message).expect("encode");

    let mut header_failure = FailingWriter::new(0, io::ErrorKind::BrokenPipe);
    let error = write_json_frame(&mut header_failure, &message).expect_err("header write fails");
    assert_eq!(error.code(), codes::FRAME_IO);
    assert_eq!(frame_io_kind(error), io::ErrorKind::BrokenPipe);
    assert!(header_failure.bytes.is_empty());

    let mut payload_failure = FailingWriter::new(7, io::ErrorKind::BrokenPipe);
    let error = write_json_frame(&mut payload_failure, &message).expect_err("payload write fails");
    assert_eq!(error.code(), codes::FRAME_IO);
    assert_eq!(frame_io_kind(error), io::ErrorKind::BrokenPipe);
    assert_eq!(payload_failure.bytes.len(), 7);
    assert_eq!(payload_failure.bytes, encoded[..7]);
}

#[test]
fn incomplete_framing_ends_the_session_without_reaching_the_executor() {
    let mut zero_length = 0u32.to_be_bytes().to_vec();
    zero_length.extend_from_slice(&encode_commands(&[hello()]));

    let mut oversized = u32::try_from(MAX_FRAME_LEN + 1)
        .expect("fits u32")
        .to_be_bytes()
        .to_vec();
    oversized.extend_from_slice(&encode_commands(&[hello()]));

    let partial_header = vec![0u8, 0, 1];
    // A header with no payload bytes is `Truncated { actual: 0 }`, the same
    // shape as an EOF at a frame boundary; it must still never execute.
    let header_without_payload = 10u32.to_be_bytes().to_vec();
    let mut partial_payload = 10u32.to_be_bytes().to_vec();
    partial_payload.extend_from_slice(b"abc");

    for (label, input) in [
        ("zero-length", zero_length),
        ("oversized", oversized),
        ("partial header", partial_header),
        ("header without payload", header_without_payload),
        ("partial payload", partial_payload),
    ] {
        let mut world = mayor_world();
        let checksum = world.checksum();
        let served = serve_bytes(&mut world, input, usize::MAX, &ListenConfig::default());

        assert_eq!(
            served.commands.expect("fatal framing ends cleanly"),
            0,
            "{label}"
        );
        assert!(served.responses.is_empty(), "{label}");
        assert_eq!(world.commands(), 0, "{label} must not reach the executor");
        assert_eq!(world.checksum(), checksum, "{label}");
    }
}

#[test]
fn idle_timeout_ends_the_session_after_applying_complete_commands() {
    let mut world = mayor_world();
    let mut reader = FaultyReader::after(
        encode_commands(&[hello(), ping(1)]),
        io::ErrorKind::TimedOut,
    );
    let mut writer = Vec::new();
    let mut last_saved_tick = world.tick;
    let mut saves = 0;

    let commands = serve_io(
        &mut reader,
        &mut writer,
        &mut world,
        &ListenConfig::default(),
        &mut last_saved_tick,
        &mut saves,
    )
    .expect("a timeout ends the session cleanly");

    assert_eq!(commands, 2);
    let responses = decode_frames(&writer);
    assert_eq!(responses.len(), 2);
    assert!(matches!(responses[0], ServerMessage::Welcome { .. }));
    assert_eq!(responses[1], ServerMessage::Pong { nonce: 1 });
    assert!(world.hello_accepted());
    assert_eq!(world.tick, 0);
    assert_eq!(saves, 0);
}

#[test]
fn clean_eof_ends_the_session_at_the_header_boundary() {
    let mut world = mayor_world();
    let served = serve_bytes(
        &mut world,
        encode_commands(&[hello()]),
        usize::MAX,
        &ListenConfig::default(),
    );
    assert_eq!(served.commands.expect("clean EOF"), 1);
    assert_eq!(served.responses.len(), 1);
    assert!(matches!(served.responses[0], ServerMessage::Welcome { .. }));
    assert!(world.hello_accepted());

    let mut empty_world = mayor_world();
    let checksum = empty_world.checksum();
    let served = serve_bytes(
        &mut empty_world,
        Vec::new(),
        usize::MAX,
        &ListenConfig::default(),
    );
    assert_eq!(served.commands.expect("empty peer"), 0);
    assert!(served.responses.is_empty());
    assert_eq!(empty_world.commands(), 0);
    assert_eq!(empty_world.checksum(), checksum);
}

#[test]
fn fragmented_and_coalesced_sessions_produce_identical_responses() {
    let commands = [hello(), ping(3), set_tax(12, 8), place_road(), ping(4)];
    let bytes = encode_commands(&commands);

    let mut coalesced_world = mayor_world();
    let coalesced = serve_bytes(
        &mut coalesced_world,
        bytes.clone(),
        usize::MAX,
        &ListenConfig::default(),
    );

    let mut fragmented_world = mayor_world();
    let fragmented = serve_bytes(&mut fragmented_world, bytes, 1, &ListenConfig::default());

    assert_eq!(coalesced.commands.expect("coalesced session"), 5);
    assert_eq!(fragmented.commands.expect("fragmented session"), 5);
    assert_eq!(coalesced.responses, fragmented.responses);
    assert_eq!(coalesced_world.tick, fragmented_world.tick);
    assert_eq!(coalesced_world.checksum(), fragmented_world.checksum());
}

#[test]
fn coalesced_session_interleaves_accepted_and_rejected_commands() {
    let mut world = mayor_world();
    let commands = [
        hello(),
        set_tax(12, 8),
        ping(1),
        place_road(),
        ping(2),
        ClientCommand::Disconnect,
    ];
    let served = serve_bytes(
        &mut world,
        encode_commands(&commands),
        1,
        &ListenConfig::default(),
    );

    assert_eq!(served.commands.expect("session"), 6);
    assert_eq!(served.responses.len(), 6);
    assert!(matches!(
        served.responses[0],
        ServerMessage::Welcome { tick: 0, .. }
    ));
    assert!(matches!(
        served.responses[1],
        ServerMessage::Snapshot { tick: 1, .. }
    ));
    assert_eq!(served.responses[2], ServerMessage::Pong { nonce: 1 });
    assert!(matches!(
        served.responses[3],
        ServerMessage::Snapshot { tick: 2, .. }
    ));
    assert_eq!(served.responses[4], ServerMessage::Pong { nonce: 2 });
    assert_eq!(served.responses[5], ServerMessage::Goodbye);

    assert_eq!(world.tick, 2);
    assert_eq!(world.roads.roads.len(), 1);
    assert!(world.is_closed());
    assert_eq!(served.saves, 0);
}

#[test]
fn rejected_commands_leave_world_state_unchanged() {
    let mut world = mayor_world();
    assert!(matches!(
        world.apply(&hello()),
        ServerMessage::Welcome { .. }
    ));
    let checksum = world.checksum();
    let tick = world.tick;
    let roads = world.roads.roads.len();
    let balance = world.balance_units();

    let mut input = Vec::new();
    write_json_frame(&mut input, &set_tax(101, 8)).expect("invalid tax frame");
    write_json_frame(
        &mut input,
        &ClientCommand::PlaceRoad {
            start: [0, 0],
            end: [0, 0],
        },
    )
    .expect("invalid road frame");
    write_frame(&mut input, br#"{"type":"launch_missiles"}"#).expect("unknown tag frame");
    write_json_frame(&mut input, &ping(9)).expect("ping frame");

    let served = serve_bytes(&mut world, input, 2, &ListenConfig::default());

    assert_eq!(served.commands.expect("session"), 4);
    assert_eq!(served.responses.len(), 4);
    expect_reject_code(&served.responses[0], codes::TAX_RATE_OUT_OF_RANGE);
    expect_reject_code(&served.responses[1], codes::ROAD_EMPTY);
    expect_reject_code(&served.responses[2], codes::COMMAND_UNKNOWN_TAG);
    assert_eq!(served.responses[3], ServerMessage::Pong { nonce: 9 });

    assert_eq!(world.tick, tick);
    assert_eq!(world.checksum(), checksum);
    assert_eq!(world.roads.roads.len(), roads);
    assert_eq!(world.balance_units(), balance);

    let audits = world.audit_state();
    assert!(audits
        .iter()
        .any(|entry| entry.tag == "set_tax" && entry.code == codes::TAX_RATE_OUT_OF_RANGE));
    assert!(audits
        .iter()
        .any(|entry| entry.tag == "place_road" && entry.code == codes::ROAD_EMPTY));
    assert!(!audits.iter().any(|entry| entry.tag == "launch_missiles"));
}

#[test]
fn malformed_command_json_is_rejected_and_the_session_continues() {
    let mut world = mayor_world();
    let mut input = encode_commands(&[hello()]);
    write_frame(&mut input, b"{").expect("malformed command frame");
    write_json_frame(&mut input, &ping(5)).expect("ping frame");

    let served = serve_bytes(&mut world, input, 2, &ListenConfig::default());

    assert_eq!(served.commands.expect("session"), 3);
    assert_eq!(served.responses.len(), 3);
    assert!(matches!(served.responses[0], ServerMessage::Welcome { .. }));
    expect_reject_code(&served.responses[1], codes::COMMAND_INVALID_JSON);
    assert_eq!(served.responses[2], ServerMessage::Pong { nonce: 5 });
    assert_eq!(world.tick, 0);
}

#[test]
fn autosave_fires_exactly_at_the_tick_boundary() {
    let directory = temp_dir();
    let path = directory.join("world.json");
    let config = ListenConfig {
        shutdown_save: false,
        save_path: Some(path.clone()),
        autosave_ticks: 2,
    };
    let mut world = mayor_world();

    let commands = [
        hello(),
        // Rejected: no tick, so this must not trigger an autosave.
        set_tax(101, 8),
        // Tick 1: below the threshold.
        set_tax(12, 8),
        ping(1),
        // Tick 2: exactly at the threshold, save 1.
        set_tax(11, 7),
        ping(2),
        // Tick 3: below the next window.
        set_tax(10, 6),
        // Tick 4: exactly at the threshold, save 2.
        set_tax(9, 5),
        ClientCommand::Disconnect,
    ];

    let served = serve_bytes(&mut world, encode_commands(&commands), 7, &config);

    assert_eq!(served.commands.expect("session"), 9);
    assert_eq!(served.saves, 2);
    assert_eq!(world.tick, 4);
    assert!(path.is_file());
    assert!(!temp_path(&path).exists());

    let loaded = ServerWorld::load_from(&path).expect("load autosave");
    assert_eq!(loaded.world.tick, 4);
    assert_eq!(loaded.world.checksum(), world.checksum());

    fs::remove_dir_all(&directory).expect("cleanup");
}

#[test]
fn a_dropped_connection_resumes_inside_the_300_tick_window() {
    let mut world = mayor_world();
    let first = serve_bytes(
        &mut world,
        encode_commands(&[hello(), set_tax(12, 8)]),
        usize::MAX,
        &ListenConfig::default(),
    );
    assert_eq!(first.commands.expect("first session"), 2);
    let token = welcome_token(&first.responses[0]);
    assert!(matches!(
        first.responses[1],
        ServerMessage::Snapshot { tick: 1, .. }
    ));

    // EOF without `disconnect`: the listener's begin_connection opens the
    // reconnect window for the dropped session.
    world.begin_connection();
    let deadline = world.resume_deadline().expect("resume window");
    assert_eq!(deadline, world.tick + RESUME_GRACE_TICKS);
    assert!(world.is_resumable());

    // The deadline tick is inclusive.
    world.tick = deadline;
    let second = serve_bytes(
        &mut world,
        encode_commands(&[ClientCommand::Resume {
            session_id: token.clone(),
            last_tick: 1,
        }]),
        usize::MAX,
        &ListenConfig::default(),
    );

    assert_eq!(second.commands.expect("resume session"), 1);
    assert_eq!(second.responses.len(), 2);
    match &second.responses[0] {
        ServerMessage::Welcome {
            session_token,
            tick,
            resumed: true,
            ..
        } => {
            assert_eq!(session_token, &token);
            assert_eq!(*tick, deadline);
        }
        other => panic!("expected a resumed welcome, got {other:?}"),
    }
    assert!(matches!(
        second.responses[1],
        ServerMessage::Snapshot { tick, .. } if tick == deadline
    ));
    assert!(world.hello_accepted());
    assert!(!world.is_resumable());
}

#[test]
fn a_resume_after_the_window_is_rejected() {
    let mut world = mayor_world();
    let first = serve_bytes(
        &mut world,
        encode_commands(&[hello()]),
        usize::MAX,
        &ListenConfig::default(),
    );
    let token = welcome_token(&first.responses[0]);

    world.begin_connection();
    let deadline = world.resume_deadline().expect("resume window");
    world.tick = deadline + 1;

    let second = serve_bytes(
        &mut world,
        encode_commands(&[ClientCommand::Resume {
            session_id: token,
            last_tick: 0,
        }]),
        usize::MAX,
        &ListenConfig::default(),
    );

    assert_eq!(second.commands.expect("resume session"), 1);
    assert_eq!(second.responses.len(), 1);
    expect_reject_code(&second.responses[0], world_codes::RESUME_EXPIRED);
    assert!(!world.is_resumable());
    assert!(!world.hello_accepted());
}

#[test]
fn queued_responses_are_byte_identical_and_leave_the_same_world() {
    let commands = [hello(), ping(3), set_tax(12, 8), place_road(), ping(4)];
    let bytes = encode_commands(&commands);
    let config = ListenConfig::default();

    let mut inline_world = mayor_world();
    let mut inline_bytes = Vec::new();
    let mut last_saved_tick = inline_world.tick;
    let mut saves = 0;
    let inline_commands = serve_io(
        &mut ChunkedReader::new(bytes.clone(), 1),
        &mut inline_bytes,
        &mut inline_world,
        &config,
        &mut last_saved_tick,
        &mut saves,
    )
    .expect("inline session");

    let mut queued_world = mayor_world();
    let queue = OutboundQueue::new(256).expect("queue");
    let mut last_saved_tick = queued_world.tick;
    let mut queued_saves = 0;
    let outcome = serve_io_outbound(
        &mut ChunkedReader::new(bytes, 1),
        &queue,
        &mut queued_world,
        &config,
        &mut last_saved_tick,
        &mut queued_saves,
    )
    .expect("queued session");

    assert_eq!(outcome.commands, inline_commands);
    assert_eq!(outcome.outbound, None);
    assert_eq!(queued_saves, saves);

    queue.close();
    let mut queued_bytes = Vec::new();
    drain(&mut queued_bytes, &queue).expect("drain queued responses");
    assert_eq!(queued_bytes, inline_bytes);
    assert_eq!(decode_frames(&queued_bytes), decode_frames(&inline_bytes));

    assert_eq!(queued_world.tick, inline_world.tick);
    assert_eq!(queued_world.checksum(), inline_world.checksum());
    assert_eq!(queued_world.commands(), inline_world.commands());
}

#[test]
fn slow_reader_backpressure_stops_reading_and_keeps_accepted_frames() {
    let commands = [hello(), ping(1), ping(2), ping(3), ping(4)];
    let mut world = mayor_world();
    let queue = OutboundQueue::new(2).expect("queue");
    let mut reader = ChunkedReader::new(encode_commands(&commands), usize::MAX);
    let mut last_saved_tick = world.tick;
    let mut saves = 0;

    let outcome = serve_io_outbound(
        &mut reader,
        &queue,
        &mut world,
        &ListenConfig::default(),
        &mut last_saved_tick,
        &mut saves,
    )
    .expect("backpressure ends the session cleanly");

    // Welcome and the first pong fit. The first pong after that is rejected,
    // so the remaining `ping(3)`/`ping(4)` frames are never read.
    assert_eq!(outcome.commands, 3);
    assert_eq!(outcome.outbound, Some(OutboundError::Full));
    assert_eq!(
        outcome.outbound.expect("full queue").code(),
        "outbound.backpressure"
    );

    // No consumer ran, so the accepted backlog is still queued in order and
    // can be drained after the close.
    assert_eq!(queue.len(), 2);
    queue.close();
    let mut written = Vec::new();
    drain(&mut written, &queue).expect("drain accepted frames");
    let responses = decode_frames(&written);
    assert_eq!(responses.len(), 2);
    assert!(matches!(responses[0], ServerMessage::Welcome { .. }));
    assert_eq!(responses[1], ServerMessage::Pong { nonce: 1 });

    assert_eq!(world.commands(), 3);
    assert_eq!(world.tick, 0);
    assert_eq!(saves, 0);
}

#[test]
fn a_closed_queue_ends_the_session_before_queueing_a_response() {
    let mut world = mayor_world();
    let queue = OutboundQueue::new(4).expect("queue");
    queue.close();
    let mut last_saved_tick = world.tick;
    let mut saves = 0;

    let outcome = serve_io_outbound(
        &mut ChunkedReader::new(encode_commands(&[hello(), ping(9)]), usize::MAX),
        &queue,
        &mut world,
        &ListenConfig::default(),
        &mut last_saved_tick,
        &mut saves,
    )
    .expect("closed queue ends the session cleanly");

    assert_eq!(outcome.commands, 1);
    assert_eq!(outcome.outbound, Some(OutboundError::Closed));
    assert_eq!(
        outcome.outbound.expect("closed queue").code(),
        "outbound.closed"
    );
    assert!(queue.is_empty());
    assert!(world.hello_accepted());
    assert_eq!(
        world.commands(),
        1,
        "the ping after the close is never read"
    );
}

#[test]
fn a_broken_pipe_ends_the_session_after_the_command_is_applied() {
    let mut world = mayor_world();
    let mut reader = ChunkedReader::new(encode_commands(&[hello(), ping(2)]), usize::MAX);
    // The header fits, but the response payload fails mid-frame.
    let mut writer = FailingWriter::new(4, io::ErrorKind::BrokenPipe);
    let mut last_saved_tick = world.tick;
    let mut saves = 0;

    let commands = serve_io(
        &mut reader,
        &mut writer,
        &mut world,
        &ListenConfig::default(),
        &mut last_saved_tick,
        &mut saves,
    )
    .expect("a write failure ends the session cleanly");

    assert_eq!(
        commands, 1,
        "the ping after the failed response is never read"
    );
    assert_eq!(writer.bytes.len(), 4);
    let header: [u8; 4] = writer.bytes[..4].try_into().expect("partial header");
    assert!(u32::from_be_bytes(header) > 0);
    assert!(world.hello_accepted());
    assert_eq!(world.tick, 0);
    assert_eq!(saves, 0);
}
