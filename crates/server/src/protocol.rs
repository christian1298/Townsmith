//! Authoritative server protocol v0.
//!
//! This is the Phase 6 foundation: transport framing, the first client/server
//! message set, stable validation codes, and a deterministic loopback smoke.
//! It does not change or drive the city simulation.
//!
//! Wire format: a 4-byte big-endian payload length followed by UTF-8 JSON.
//! Frames are capped at [`MAX_FRAME_LEN`] (1 MiB). Zero-length, oversized,
//! truncated, non-UTF-8, and malformed JSON frames are rejected with stable
//! codes from [`codes`].

use std::fmt;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Protocol revision carried by `Hello` and `Welcome`.
pub const PROTOCOL_VERSION: u16 = 0;

/// Maximum accepted frame payload, including the JSON body.
pub const MAX_FRAME_LEN: usize = 1024 * 1024;

/// Maximum player name length in Unicode scalar values.
pub const MAX_PLAYER_NAME_CHARS: usize = 32;

/// Tax rates are percentages.
pub const MAX_TAX_RATE: u16 = 100;

/// Maximum road length in grid cells (Chebyshev distance).
pub const MAX_ROAD_LENGTH: u32 = 1024;

/// Inclusive world coordinate bound for v0 road commands.
pub const MAX_WORLD_COORD: i32 = 1_000_000;

/// Fixed seed for the deterministic loopback smoke.
pub const LOOPBACK_SEED: u64 = 0x544F_574E_534D_4954; // "TOWNSMIT"

/// Read/write timeout used by the loopback harness. No test sleeps on a clock.
pub const SESSION_IO_TIMEOUT: Duration = Duration::from_secs(5);

/// Application frames exchanged by the loopback smoke (client + server).
pub const LOOPBACK_TOTAL_FRAMES: u64 = 8;

/// Stable error codes. These are wire/API contracts; do not rename them.
pub mod codes {
    // Framing.
    pub const FRAME_ZERO_LENGTH: &str = "frame.zero_length";
    pub const FRAME_OVERSIZED: &str = "frame.oversized";
    pub const FRAME_TRUNCATED: &str = "frame.truncated";
    pub const FRAME_INVALID_UTF8: &str = "frame.invalid_utf8";
    pub const FRAME_INVALID_JSON: &str = "frame.invalid_json";
    pub const FRAME_IO: &str = "frame.io";

    // Command decoding.
    pub const COMMAND_INVALID_JSON: &str = "command.invalid_json";
    pub const COMMAND_UNKNOWN_TAG: &str = "command.unknown_tag";
    pub const COMMAND_INVALID_FIELDS: &str = "command.invalid_fields";

    // Session sequencing and hello.
    pub const PROTOCOL_VERSION_MISMATCH: &str = "protocol.version_mismatch";
    pub const HELLO_REQUIRED: &str = "hello.required";
    pub const HELLO_DUPLICATE: &str = "hello.duplicate";
    pub const PLAYER_NAME_EMPTY: &str = "player_name.empty";
    pub const PLAYER_NAME_TOO_LONG: &str = "player_name.too_long";
    pub const PLAYER_NAME_INVALID: &str = "player_name.invalid";
    pub const CONTENT_HASH_INVALID: &str = "content_hash.invalid";
    pub const SESSION_CLOSED: &str = "session.closed";

    // Command payloads.
    pub const TAX_RATE_OUT_OF_RANGE: &str = "tax.rate_out_of_range";
    pub const ROAD_EMPTY: &str = "road.empty";
    pub const ROAD_OUT_OF_BOUNDS: &str = "road.out_of_bounds";
    pub const ROAD_TOO_LONG: &str = "road.too_long";
}

/// A framing or JSON failure. `code()` is stable for logs and tests.
#[derive(Debug)]
pub enum FrameError {
    Io(io::Error),
    ZeroLength,
    Oversized { len: usize },
    Truncated { expected: usize, actual: usize },
    InvalidUtf8 { offset: usize },
    InvalidJson { message: String },
}

impl FrameError {
    pub fn code(&self) -> &'static str {
        match self {
            FrameError::Io(_) => codes::FRAME_IO,
            FrameError::ZeroLength => codes::FRAME_ZERO_LENGTH,
            FrameError::Oversized { .. } => codes::FRAME_OVERSIZED,
            FrameError::Truncated { .. } => codes::FRAME_TRUNCATED,
            FrameError::InvalidUtf8 { .. } => codes::FRAME_INVALID_UTF8,
            FrameError::InvalidJson { .. } => codes::FRAME_INVALID_JSON,
        }
    }

    pub fn message(&self) -> String {
        match self {
            FrameError::Io(error) => error.to_string(),
            FrameError::ZeroLength => "frame payload must not be empty".to_string(),
            FrameError::Oversized { len } => {
                format!("frame payload is {len} bytes; maximum is {MAX_FRAME_LEN}")
            }
            FrameError::Truncated { expected, actual } => {
                format!("frame is truncated: expected {expected} bytes, received {actual}")
            }
            FrameError::InvalidUtf8 { offset } => {
                format!("frame payload is not valid UTF-8 at byte {offset}")
            }
            FrameError::InvalidJson { message } => message.clone(),
        }
    }
}

impl fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code(), self.message())
    }
}

impl std::error::Error for FrameError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FrameError::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for FrameError {
    fn from(error: io::Error) -> Self {
        FrameError::Io(error)
    }
}

/// A validation or command-decoding rejection, ready to become `Reject`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub code: &'static str,
    pub message: String,
}

impl Rejection {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn into_message(self) -> ServerMessage {
        ServerMessage::Reject {
            code: self.code.to_string(),
            message: self.message,
        }
    }
}

/// Commands a client may send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientCommand {
    Hello {
        protocol_version: u16,
        content_hash: String,
        player_name: String,
    },
    Ping {
        nonce: u64,
    },
    TimeControl {
        action: TimeAction,
    },
    SetTax {
        resident_rate: u16,
        business_rate: u16,
    },
    PlaceRoad {
        start: [i32; 2],
        end: [i32; 2],
    },
    Disconnect,
}

impl ClientCommand {
    pub fn tag(&self) -> &'static str {
        match self {
            ClientCommand::Hello { .. } => "hello",
            ClientCommand::Ping { .. } => "ping",
            ClientCommand::TimeControl { .. } => "time_control",
            ClientCommand::SetTax { .. } => "set_tax",
            ClientCommand::PlaceRoad { .. } => "place_road",
            ClientCommand::Disconnect => "disconnect",
        }
    }
}

/// Time control actions accepted in v0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeAction {
    Pause,
    Resume,
    /// Advance exactly one game day; the authority executor supports this,
    /// the transport-only session treats it as one command tick.
    Step,
    #[serde(rename = "speed_1x")]
    Speed1x,
    #[serde(rename = "speed_2x")]
    Speed2x,
    #[serde(rename = "speed_4x")]
    Speed4x,
}

/// Messages the authoritative server sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Welcome {
        session_id: String,
        tick: u64,
        protocol_version: u16,
    },
    Pong {
        nonce: u64,
    },
    Snapshot {
        tick: u64,
        population: u32,
        balance: i64,
        checksum: String,
    },
    Reject {
        code: String,
        message: String,
    },
    Goodbye,
}

/// Write one length-prefixed frame.
pub fn write_frame<W: Write>(writer: &mut W, payload: &[u8]) -> Result<(), FrameError> {
    if payload.is_empty() {
        return Err(FrameError::ZeroLength);
    }
    if payload.len() > MAX_FRAME_LEN {
        return Err(FrameError::Oversized { len: payload.len() });
    }
    let length =
        u32::try_from(payload.len()).map_err(|_| FrameError::Oversized { len: payload.len() })?;
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(payload)?;
    writer.flush()?;
    Ok(())
}

/// Read one length-prefixed frame. A clean EOF at the header boundary is
/// reported as `frame.truncated` in v0; callers with a known frame count
/// (the loopback harness) never rely on an EOF sentinel.
pub fn read_frame<R: Read>(reader: &mut R) -> Result<Vec<u8>, FrameError> {
    let mut header = [0u8; 4];
    let header_read = read_fully(reader, &mut header)?;
    if header_read < header.len() {
        return Err(FrameError::Truncated {
            expected: header.len(),
            actual: header_read,
        });
    }

    let length = u32::from_be_bytes(header) as usize;
    if length == 0 {
        return Err(FrameError::ZeroLength);
    }
    if length > MAX_FRAME_LEN {
        return Err(FrameError::Oversized { len: length });
    }

    let mut payload = vec![0u8; length];
    let payload_read = read_fully(reader, &mut payload)?;
    if payload_read < length {
        return Err(FrameError::Truncated {
            expected: length,
            actual: payload_read,
        });
    }
    Ok(payload)
}

/// Serialize a message to a JSON payload.
pub fn encode_json<T: Serialize>(message: &T) -> Result<Vec<u8>, FrameError> {
    serde_json::to_vec(message).map_err(|error| FrameError::InvalidJson {
        message: error.to_string(),
    })
}

/// Deserialize a JSON payload as UTF-8.
pub fn decode_json<T: DeserializeOwned>(payload: &[u8]) -> Result<T, FrameError> {
    let text = std::str::from_utf8(payload).map_err(|error| FrameError::InvalidUtf8 {
        offset: error.valid_up_to(),
    })?;
    serde_json::from_str(text).map_err(|error| FrameError::InvalidJson {
        message: error.to_string(),
    })
}

/// Write a message as one JSON frame.
pub fn write_json_frame<W: Write, T: Serialize>(
    writer: &mut W,
    message: &T,
) -> Result<(), FrameError> {
    let payload = encode_json(message)?;
    write_frame(writer, &payload)
}

/// Read one frame and decode it as JSON.
pub fn read_json_frame<R: Read, T: DeserializeOwned>(reader: &mut R) -> Result<T, FrameError> {
    let payload = read_frame(reader)?;
    decode_json(&payload)
}

/// Decode a client command with stable errors for unknown tags and bad fields.
pub fn decode_client_command(payload: &[u8]) -> Result<ClientCommand, Rejection> {
    let text = std::str::from_utf8(payload).map_err(|error| {
        Rejection::new(
            codes::COMMAND_INVALID_JSON,
            format!("payload is not UTF-8 at byte {}", error.valid_up_to()),
        )
    })?;

    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|error| Rejection::new(codes::COMMAND_INVALID_JSON, error.to_string()))?;

    let tag = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            Rejection::new(
                codes::COMMAND_INVALID_FIELDS,
                "command object requires a string `type` field",
            )
        })?;

    const KNOWN_TAGS: &[&str] = &[
        "hello",
        "ping",
        "time_control",
        "set_tax",
        "place_road",
        "disconnect",
    ];
    if !KNOWN_TAGS.contains(&tag) {
        return Err(Rejection::new(
            codes::COMMAND_UNKNOWN_TAG,
            format!("unknown command tag `{tag}`"),
        ));
    }

    serde_json::from_value(value)
        .map_err(|error| Rejection::new(codes::COMMAND_INVALID_FIELDS, error.to_string()))
}

/// Validate a hello payload. Version is checked before content.
pub fn validate_hello(
    protocol_version: u16,
    content_hash: &str,
    player_name: &str,
) -> Result<(), Rejection> {
    if protocol_version != PROTOCOL_VERSION {
        return Err(Rejection::new(
            codes::PROTOCOL_VERSION_MISMATCH,
            format!(
                "protocol version {protocol_version} is not supported; server speaks {PROTOCOL_VERSION}"
            ),
        ));
    }
    validate_player_name(player_name)?;
    validate_content_hash(content_hash)?;
    Ok(())
}

pub fn validate_player_name(player_name: &str) -> Result<(), Rejection> {
    if player_name.trim().is_empty() {
        return Err(Rejection::new(
            codes::PLAYER_NAME_EMPTY,
            "player name must not be empty",
        ));
    }
    if player_name.chars().count() > MAX_PLAYER_NAME_CHARS {
        return Err(Rejection::new(
            codes::PLAYER_NAME_TOO_LONG,
            format!("player name must be at most {MAX_PLAYER_NAME_CHARS} characters"),
        ));
    }
    if player_name.chars().any(|character| character.is_control()) {
        return Err(Rejection::new(
            codes::PLAYER_NAME_INVALID,
            "player name must not contain control characters",
        ));
    }
    Ok(())
}

pub fn validate_content_hash(content_hash: &str) -> Result<(), Rejection> {
    let valid = content_hash.len() == 64
        && content_hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !valid {
        return Err(Rejection::new(
            codes::CONTENT_HASH_INVALID,
            "content hash must be 64 lowercase hexadecimal characters",
        ));
    }
    Ok(())
}

pub fn validate_tax_rates(resident_rate: u16, business_rate: u16) -> Result<(), Rejection> {
    if resident_rate > MAX_TAX_RATE || business_rate > MAX_TAX_RATE {
        return Err(Rejection::new(
            codes::TAX_RATE_OUT_OF_RANGE,
            format!(
                "tax rates must be within 0..={MAX_TAX_RATE} (resident={resident_rate}, business={business_rate})"
            ),
        ));
    }
    Ok(())
}

pub fn validate_road(start: [i32; 2], end: [i32; 2]) -> Result<(), Rejection> {
    for coordinate in [start[0], start[1], end[0], end[1]] {
        if !(-MAX_WORLD_COORD..=MAX_WORLD_COORD).contains(&coordinate) {
            return Err(Rejection::new(
                codes::ROAD_OUT_OF_BOUNDS,
                format!("road coordinate {coordinate} is outside ±{MAX_WORLD_COORD}"),
            ));
        }
    }

    let dx = (i64::from(end[0]) - i64::from(start[0])).abs();
    let dy = (i64::from(end[1]) - i64::from(start[1])).abs();
    if dx == 0 && dy == 0 {
        return Err(Rejection::new(
            codes::ROAD_EMPTY,
            "road start and end must differ",
        ));
    }
    if dx.max(dy) > i64::from(MAX_ROAD_LENGTH) {
        return Err(Rejection::new(
            codes::ROAD_TOO_LONG,
            format!(
                "road length {} exceeds the maximum of {MAX_ROAD_LENGTH} cells",
                dx.max(dy)
            ),
        ));
    }
    Ok(())
}

/// Seed-derived configuration for a server session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConfig {
    pub seed: u64,
    pub population: u32,
    pub starting_balance: i64,
}

impl SessionConfig {
    pub fn for_seed(seed: u64) -> Self {
        let mixed = mix64(seed);
        Self {
            seed,
            population: 1_000 + (mixed % 9_001) as u32,
            starting_balance: 250_000,
        }
    }
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self::for_seed(LOOPBACK_SEED)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HelloState {
    content_hash: String,
    player_name: String,
}

/// Authoritative state for one client connection in protocol v0.
#[derive(Debug, Clone)]
pub struct Session {
    config: SessionConfig,
    session_id: String,
    hello: Option<HelloState>,
    tick: u64,
    population: u32,
    balance: i64,
    resident_rate: u16,
    business_rate: u16,
    roads: u32,
    paused: bool,
    speed: u8,
    closed: bool,
}

impl Session {
    pub fn new(seed: u64) -> Self {
        Self::with_config(SessionConfig::for_seed(seed))
    }

    pub fn with_config(config: SessionConfig) -> Self {
        let session_id = session_id_for_seed(config.seed);
        Self {
            population: config.population,
            balance: config.starting_balance,
            config,
            session_id,
            hello: None,
            tick: 0,
            resident_rate: 10,
            business_rate: 10,
            roads: 0,
            paused: false,
            speed: 1,
            closed: false,
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn tick(&self) -> u64 {
        self.tick
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Deterministic checksum of the authoritative aggregate state.
    pub fn checksum(&self) -> String {
        let mut hash = fnv_update(FNV_OFFSET, &self.config.seed.to_le_bytes());
        hash = fnv_update(hash, &self.tick.to_le_bytes());
        hash = fnv_update(hash, &self.population.to_le_bytes());
        hash = fnv_update(hash, &self.balance.to_le_bytes());
        hash = fnv_update(hash, &self.resident_rate.to_le_bytes());
        hash = fnv_update(hash, &self.business_rate.to_le_bytes());
        hash = fnv_update(hash, &self.roads.to_le_bytes());
        hash = fnv_update(hash, &[u8::from(self.paused), self.speed]);
        if let Some(hello) = &self.hello {
            hash = fnv_update(hash, hello.content_hash.as_bytes());
            hash = fnv_update(hash, hello.player_name.as_bytes());
        }
        format!("{hash:016x}")
    }

    /// Validate and apply one command. Rejections never mutate state and never
    /// panic.
    pub fn handle(&mut self, command: ClientCommand) -> Vec<ServerMessage> {
        if self.closed {
            return reject(codes::SESSION_CLOSED, "session is already closed");
        }

        match command {
            ClientCommand::Hello {
                protocol_version,
                content_hash,
                player_name,
            } => {
                if self.hello.is_some() {
                    return reject(codes::HELLO_DUPLICATE, "hello was already accepted");
                }
                if let Err(rejection) =
                    validate_hello(protocol_version, &content_hash, &player_name)
                {
                    return vec![rejection.into_message()];
                }
                self.hello = Some(HelloState {
                    content_hash,
                    player_name,
                });
                vec![ServerMessage::Welcome {
                    session_id: self.session_id.clone(),
                    tick: self.tick,
                    protocol_version: PROTOCOL_VERSION,
                }]
            }
            ClientCommand::Disconnect => {
                self.closed = true;
                vec![ServerMessage::Goodbye]
            }
            _ if self.hello.is_none() => reject(
                codes::HELLO_REQUIRED,
                format!("`{}` requires a successful hello first", command.tag()),
            ),
            ClientCommand::Ping { nonce } => vec![ServerMessage::Pong { nonce }],
            ClientCommand::TimeControl { action } => {
                match action {
                    TimeAction::Pause => self.paused = true,
                    TimeAction::Resume => {
                        self.paused = false;
                        self.speed = 1;
                    }
                    TimeAction::Step => {}
                    TimeAction::Speed1x => {
                        self.paused = false;
                        self.speed = 1;
                    }
                    TimeAction::Speed2x => {
                        self.paused = false;
                        self.speed = 2;
                    }
                    TimeAction::Speed4x => {
                        self.paused = false;
                        self.speed = 4;
                    }
                }
                self.tick = self.tick.saturating_add(1);
                vec![self.snapshot()]
            }
            ClientCommand::SetTax {
                resident_rate,
                business_rate,
            } => {
                if let Err(rejection) = validate_tax_rates(resident_rate, business_rate) {
                    return vec![rejection.into_message()];
                }
                self.resident_rate = resident_rate;
                self.business_rate = business_rate;
                self.tick = self.tick.saturating_add(1);
                vec![self.snapshot()]
            }
            ClientCommand::PlaceRoad { start, end } => {
                if let Err(rejection) = validate_road(start, end) {
                    return vec![rejection.into_message()];
                }
                self.roads = self.roads.saturating_add(1);
                self.tick = self.tick.saturating_add(1);
                vec![self.snapshot()]
            }
        }
    }

    fn snapshot(&self) -> ServerMessage {
        ServerMessage::Snapshot {
            tick: self.tick,
            population: self.population,
            balance: self.balance,
            checksum: self.checksum(),
        }
    }
}

/// Result of a successful loopback smoke run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopbackReport {
    /// Application frames exchanged in both directions.
    pub frames: u64,
    pub checksum: String,
    pub session_id: String,
}

/// Run one deterministic client/server exchange over TCP loopback:
/// hello → welcome, ping → pong, set_tax → snapshot, disconnect → goodbye.
///
/// Both sides use bounded socket timeouts; the harness never sleeps.
pub fn run_loopback_smoke() -> io::Result<LoopbackReport> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let address = listener.local_addr()?;

    let server = thread::spawn(move || serve_loopback(listener));
    let client_result = run_loopback_client(address);
    let server_result = server
        .join()
        .map_err(|_| io::Error::other("loopback server thread panicked"))?;

    client_result?;
    server_result
}

fn serve_loopback(listener: TcpListener) -> io::Result<LoopbackReport> {
    let (stream, _peer) = listener.accept()?;
    let (session, frames) = serve_stream(stream, LOOPBACK_SEED)?;

    Ok(LoopbackReport {
        frames,
        checksum: session.checksum(),
        session_id: session.session_id().to_string(),
    })
}

/// Serve one accepted TCP connection until the peer closes or sends
/// `disconnect`, and return the number of application frames exchanged.
///
/// A peer that connects and closes without sending a frame is a clean session
/// close, not an error: port probes and cancelled clients rely on this.
pub fn serve_connection(stream: TcpStream, seed: u64) -> io::Result<u64> {
    serve_stream(stream, seed).map(|(_session, frames)| frames)
}

fn serve_stream(mut stream: TcpStream, seed: u64) -> io::Result<(Session, u64)> {
    configure_stream(&stream)?;

    let mut session = Session::new(seed);
    let mut frames = 0u64;

    loop {
        let payload = match read_frame(&mut stream) {
            Ok(payload) => payload,
            // EOF at the header or body boundary means the peer went away.
            Err(FrameError::Truncated { .. }) | Err(FrameError::Io(_)) => break,
            Err(error) => return Err(io::Error::other(error.to_string())),
        };
        frames += 1;

        let mut closing = false;
        let messages = match decode_client_command(&payload) {
            Ok(command) => {
                closing = matches!(command, ClientCommand::Disconnect);
                session.handle(command)
            }
            Err(rejection) => vec![rejection.into_message()],
        };

        for message in &messages {
            write_json_frame(&mut stream, message).map_err(frame_to_io)?;
            frames += 1;
        }

        if closing || session.is_closed() {
            break;
        }
    }

    Ok((session, frames))
}

fn run_loopback_client(address: SocketAddr) -> io::Result<()> {
    let mut stream = TcpStream::connect_timeout(&address, SESSION_IO_TIMEOUT)?;
    configure_stream(&stream)?;

    let content_hash = "0123456789abcdef".repeat(4);
    write_json_frame(
        &mut stream,
        &ClientCommand::Hello {
            protocol_version: PROTOCOL_VERSION,
            content_hash,
            player_name: "loopback".to_string(),
        },
    )
    .map_err(frame_to_io)?;

    match read_json_frame::<_, ServerMessage>(&mut stream).map_err(frame_to_io)? {
        ServerMessage::Welcome {
            session_id,
            tick,
            protocol_version,
        } if protocol_version == PROTOCOL_VERSION
            && tick == 0
            && session_id == session_id_for_seed(LOOPBACK_SEED) => {}
        other => return Err(unexpected("welcome", &other)),
    }

    write_json_frame(&mut stream, &ClientCommand::Ping { nonce: 7 }).map_err(frame_to_io)?;
    match read_json_frame::<_, ServerMessage>(&mut stream).map_err(frame_to_io)? {
        ServerMessage::Pong { nonce: 7 } => {}
        other => return Err(unexpected("pong", &other)),
    }

    write_json_frame(
        &mut stream,
        &ClientCommand::SetTax {
            resident_rate: 10,
            business_rate: 15,
        },
    )
    .map_err(frame_to_io)?;
    match read_json_frame::<_, ServerMessage>(&mut stream).map_err(frame_to_io)? {
        ServerMessage::Snapshot {
            tick: 1,
            population,
            balance,
            checksum,
        } if population > 0 && balance > 0 && checksum.len() == 16 => {}
        other => return Err(unexpected("snapshot", &other)),
    }

    write_json_frame(&mut stream, &ClientCommand::Disconnect).map_err(frame_to_io)?;
    match read_json_frame::<_, ServerMessage>(&mut stream).map_err(frame_to_io)? {
        ServerMessage::Goodbye => {}
        other => return Err(unexpected("goodbye", &other)),
    }

    Ok(())
}

pub(crate) fn configure_stream(stream: &TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(SESSION_IO_TIMEOUT))?;
    stream.set_write_timeout(Some(SESSION_IO_TIMEOUT))?;
    stream.set_nodelay(true)?;
    Ok(())
}

fn unexpected(expected: &str, actual: &ServerMessage) -> io::Error {
    io::Error::other(format!("loopback expected {expected}, received {actual:?}"))
}

pub(crate) fn frame_to_io(error: FrameError) -> io::Error {
    io::Error::other(error.to_string())
}

fn read_fully<R: Read>(reader: &mut R, buffer: &mut [u8]) -> Result<usize, io::Error> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(count) => filled += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

fn reject(code: &'static str, message: impl Into<String>) -> Vec<ServerMessage> {
    vec![Rejection::new(code, message).into_message()]
}

/// Deterministic session identifier for a seed.
pub fn session_id_for_seed(seed: u64) -> String {
    format!("session-{:016x}", mix64(seed ^ 0xA5A5_5A5A_0123_4567))
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

fn mix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut mixed = value;
    mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    mixed ^ (mixed >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_hash() -> String {
        "0123456789abcdef".repeat(4)
    }

    fn hello() -> ClientCommand {
        ClientCommand::Hello {
            protocol_version: PROTOCOL_VERSION,
            content_hash: valid_hash(),
            player_name: "mayor".to_string(),
        }
    }

    fn expect_reject(messages: &[ServerMessage], expected_code: &str) {
        match messages {
            [ServerMessage::Reject { code, message: _ }] => assert_eq!(code, expected_code),
            other => panic!("expected one rejection `{expected_code}`, got {other:?}"),
        }
    }

    fn expect_snapshot(messages: &[ServerMessage]) -> &ServerMessage {
        match messages {
            [snapshot @ ServerMessage::Snapshot { .. }] => snapshot,
            other => panic!("expected one snapshot, got {other:?}"),
        }
    }

    #[test]
    fn frame_round_trip_preserves_message() {
        let message = hello();
        let mut buffer = Vec::new();
        write_json_frame(&mut buffer, &message).unwrap();

        let payload_len = u32::from_be_bytes(buffer[..4].try_into().unwrap()) as usize;
        assert_eq!(payload_len, buffer.len() - 4);

        let decoded: ClientCommand = read_json_frame(&mut buffer.as_slice()).unwrap();
        assert_eq!(decoded, message);
    }

    #[test]
    fn rejects_zero_length_frames() {
        assert_eq!(
            write_frame(&mut Vec::new(), &[]).unwrap_err().code(),
            codes::FRAME_ZERO_LENGTH
        );

        let frame = 0u32.to_be_bytes();
        assert_eq!(
            read_frame(&mut frame.as_slice()).unwrap_err().code(),
            codes::FRAME_ZERO_LENGTH
        );
    }

    #[test]
    fn rejects_oversized_frames() {
        let oversized = vec![b' '; MAX_FRAME_LEN + 1];
        assert_eq!(
            write_frame(&mut Vec::new(), &oversized).unwrap_err().code(),
            codes::FRAME_OVERSIZED
        );

        let length = u32::try_from(MAX_FRAME_LEN + 1).unwrap().to_be_bytes();
        assert_eq!(
            read_frame(&mut length.as_slice()).unwrap_err().code(),
            codes::FRAME_OVERSIZED
        );
    }

    #[test]
    fn rejects_truncated_header_and_body() {
        let partial_header = [0u8, 0, 2];
        assert_eq!(
            read_frame(&mut partial_header.as_slice())
                .unwrap_err()
                .code(),
            codes::FRAME_TRUNCATED
        );

        let mut frame = Vec::new();
        frame.extend_from_slice(&10u32.to_be_bytes());
        frame.extend_from_slice(b"abc");
        assert_eq!(
            read_frame(&mut frame.as_slice()).unwrap_err().code(),
            codes::FRAME_TRUNCATED
        );
    }

    #[test]
    fn rejects_non_utf8_and_malformed_json() {
        let mut invalid_utf8 = Vec::new();
        write_frame(&mut invalid_utf8, &[0xff]).unwrap();
        assert_eq!(
            read_json_frame::<_, ClientCommand>(&mut invalid_utf8.as_slice())
                .unwrap_err()
                .code(),
            codes::FRAME_INVALID_UTF8
        );

        let mut malformed = Vec::new();
        write_frame(&mut malformed, b"{").unwrap();
        assert_eq!(
            read_json_frame::<_, ClientCommand>(&mut malformed.as_slice())
                .unwrap_err()
                .code(),
            codes::FRAME_INVALID_JSON
        );
    }

    #[test]
    fn rejects_unknown_command_tag() {
        let rejection = decode_client_command(br#"{"type":"launch_missiles"}"#).unwrap_err();
        assert_eq!(rejection.code, codes::COMMAND_UNKNOWN_TAG);
    }

    #[test]
    fn rejects_command_without_required_fields() {
        let rejection = decode_client_command(br#"{"type":"ping"}"#).unwrap_err();
        assert_eq!(rejection.code, codes::COMMAND_INVALID_FIELDS);
    }

    #[test]
    fn hello_validation_covers_version_name_and_hash() {
        let hash = valid_hash();
        assert_eq!(
            validate_hello(PROTOCOL_VERSION + 1, &hash, "mayor")
                .unwrap_err()
                .code,
            codes::PROTOCOL_VERSION_MISMATCH
        );
        assert_eq!(
            validate_hello(PROTOCOL_VERSION, &hash, "   ")
                .unwrap_err()
                .code,
            codes::PLAYER_NAME_EMPTY
        );
        assert_eq!(
            validate_hello(
                PROTOCOL_VERSION,
                &hash,
                &"a".repeat(MAX_PLAYER_NAME_CHARS + 1)
            )
            .unwrap_err()
            .code,
            codes::PLAYER_NAME_TOO_LONG
        );
        assert_eq!(
            validate_hello(PROTOCOL_VERSION, &hash, "bad\nname")
                .unwrap_err()
                .code,
            codes::PLAYER_NAME_INVALID
        );
        assert_eq!(
            validate_hello(PROTOCOL_VERSION, "deadbeef", "mayor")
                .unwrap_err()
                .code,
            codes::CONTENT_HASH_INVALID
        );
        assert_eq!(
            validate_hello(PROTOCOL_VERSION, &valid_hash().to_uppercase(), "mayor")
                .unwrap_err()
                .code,
            codes::CONTENT_HASH_INVALID
        );
        assert!(
            validate_hello(PROTOCOL_VERSION, &hash, &"a".repeat(MAX_PLAYER_NAME_CHARS)).is_ok()
        );
    }

    #[test]
    fn tax_validation_enforces_range() {
        assert!(validate_tax_rates(0, 0).is_ok());
        assert!(validate_tax_rates(MAX_TAX_RATE, MAX_TAX_RATE).is_ok());
        assert_eq!(
            validate_tax_rates(MAX_TAX_RATE + 1, 10).unwrap_err().code,
            codes::TAX_RATE_OUT_OF_RANGE
        );
        assert_eq!(
            validate_tax_rates(10, MAX_TAX_RATE + 1).unwrap_err().code,
            codes::TAX_RATE_OUT_OF_RANGE
        );
    }

    #[test]
    fn road_validation_covers_empty_bounds_and_length() {
        assert_eq!(
            validate_road([2, 3], [2, 3]).unwrap_err().code,
            codes::ROAD_EMPTY
        );
        assert_eq!(
            validate_road([MAX_WORLD_COORD + 1, 0], [0, 0])
                .unwrap_err()
                .code,
            codes::ROAD_OUT_OF_BOUNDS
        );
        assert_eq!(
            validate_road([0, 0], [0, -MAX_WORLD_COORD - 1])
                .unwrap_err()
                .code,
            codes::ROAD_OUT_OF_BOUNDS
        );
        assert_eq!(
            validate_road([0, 0], [MAX_ROAD_LENGTH as i32 + 1, 0])
                .unwrap_err()
                .code,
            codes::ROAD_TOO_LONG
        );
        assert!(validate_road([0, 0], [MAX_ROAD_LENGTH as i32, 0]).is_ok());
        assert!(validate_road([-1, -1], [0, 0]).is_ok());
    }

    #[test]
    fn commands_before_hello_are_rejected() {
        let mut session = Session::new(LOOPBACK_SEED);
        for command in [
            ClientCommand::Ping { nonce: 1 },
            ClientCommand::SetTax {
                resident_rate: 10,
                business_rate: 10,
            },
            ClientCommand::PlaceRoad {
                start: [0, 0],
                end: [1, 0],
            },
            ClientCommand::TimeControl {
                action: TimeAction::Pause,
            },
        ] {
            expect_reject(&session.handle(command), codes::HELLO_REQUIRED);
        }

        match session.handle(hello()).as_slice() {
            [ServerMessage::Welcome {
                protocol_version, ..
            }] => assert_eq!(*protocol_version, PROTOCOL_VERSION),
            other => panic!("expected welcome, got {other:?}"),
        }
        assert_eq!(
            session.handle(ClientCommand::Ping { nonce: 9 }),
            vec![ServerMessage::Pong { nonce: 9 }]
        );
    }

    #[test]
    fn duplicate_hello_is_rejected_without_state_change() {
        let mut session = Session::new(LOOPBACK_SEED);
        assert!(matches!(
            session.handle(hello()).as_slice(),
            [ServerMessage::Welcome { .. }]
        ));
        expect_reject(&session.handle(hello()), codes::HELLO_DUPLICATE);
        assert_eq!(session.tick(), 0);
    }

    #[test]
    fn rejected_hello_does_not_block_a_later_valid_hello() {
        let mut session = Session::new(LOOPBACK_SEED);
        expect_reject(
            &session.handle(ClientCommand::Hello {
                protocol_version: PROTOCOL_VERSION + 1,
                content_hash: valid_hash(),
                player_name: "mayor".to_string(),
            }),
            codes::PROTOCOL_VERSION_MISMATCH,
        );
        assert!(matches!(
            session.handle(hello()).as_slice(),
            [ServerMessage::Welcome { .. }]
        ));
    }

    #[test]
    fn disconnect_before_hello_closes_the_session() {
        let mut session = Session::new(LOOPBACK_SEED);
        assert_eq!(
            session.handle(ClientCommand::Disconnect),
            vec![ServerMessage::Goodbye]
        );
        assert!(session.is_closed());
        expect_reject(
            &session.handle(ClientCommand::Ping { nonce: 1 }),
            codes::SESSION_CLOSED,
        );
    }

    #[test]
    fn invalid_payloads_do_not_advance_the_tick() {
        let mut session = Session::new(LOOPBACK_SEED);
        session.handle(hello());

        expect_reject(
            &session.handle(ClientCommand::SetTax {
                resident_rate: 101,
                business_rate: 10,
            }),
            codes::TAX_RATE_OUT_OF_RANGE,
        );
        expect_reject(
            &session.handle(ClientCommand::PlaceRoad {
                start: [0, 0],
                end: [0, 0],
            }),
            codes::ROAD_EMPTY,
        );
        assert_eq!(session.tick(), 0);

        let messages = session.handle(ClientCommand::SetTax {
            resident_rate: 12,
            business_rate: 8,
        });
        match expect_snapshot(&messages) {
            ServerMessage::Snapshot { tick, .. } => assert_eq!(*tick, 1),
            _ => unreachable!(),
        }
        assert_eq!(session.tick(), 1);
    }

    #[test]
    fn time_control_updates_the_snapshot() {
        let mut session = Session::new(LOOPBACK_SEED);
        session.handle(hello());
        let messages = session.handle(ClientCommand::TimeControl {
            action: TimeAction::Pause,
        });
        match expect_snapshot(&messages) {
            ServerMessage::Snapshot { tick, .. } => assert_eq!(*tick, 1),
            _ => unreachable!(),
        }
        assert_eq!(session.tick(), 1);
    }

    #[test]
    fn snapshots_are_deterministic_for_a_seed() {
        fn transcript() -> Vec<ServerMessage> {
            let mut session = Session::new(LOOPBACK_SEED);
            let mut messages = session.handle(hello());
            messages.extend(session.handle(ClientCommand::Ping { nonce: 3 }));
            messages.extend(session.handle(ClientCommand::PlaceRoad {
                start: [0, 0],
                end: [8, 4],
            }));
            messages.extend(session.handle(ClientCommand::SetTax {
                resident_rate: 11,
                business_rate: 7,
            }));
            messages.extend(session.handle(ClientCommand::Disconnect));
            messages
        }

        let first = transcript();
        let second = transcript();
        assert_eq!(first, second);
        match first.first() {
            Some(ServerMessage::Welcome { session_id, .. }) => {
                assert_eq!(session_id, &session_id_for_seed(LOOPBACK_SEED));
            }
            other => panic!("expected welcome, got {other:?}"),
        }
        assert_ne!(
            session_id_for_seed(LOOPBACK_SEED),
            session_id_for_seed(LOOPBACK_SEED + 1)
        );
    }

    #[test]
    fn loopback_handshake_is_deterministic() {
        let first = run_loopback_smoke().expect("loopback smoke");
        assert_eq!(first.frames, LOOPBACK_TOTAL_FRAMES);
        assert_eq!(first.session_id, session_id_for_seed(LOOPBACK_SEED));
        assert_eq!(first.checksum.len(), 16);
        assert!(first.checksum.bytes().all(|byte| byte.is_ascii_hexdigit()));

        let second = run_loopback_smoke().expect("second loopback smoke");
        assert_eq!(first, second);
    }

    #[test]
    fn serve_connection_treats_an_empty_peer_close_as_clean() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (stream, _peer) = listener.accept().unwrap();
            serve_connection(stream, LOOPBACK_SEED)
        });

        // A port probe that connects and immediately drops writes no frame.
        drop(TcpStream::connect_timeout(&address, SESSION_IO_TIMEOUT).unwrap());
        assert_eq!(server.join().unwrap().unwrap(), 0);
    }
}
