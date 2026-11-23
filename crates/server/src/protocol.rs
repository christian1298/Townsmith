//! Authoritative server protocol v1.
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
pub const PROTOCOL_VERSION: u16 = 1;

/// Canonical prefix of a `.tsbp` BLAKE3 content hash.
pub const BLUEPRINT_HASH_PREFIX: &str = "blake3:";

/// Maximum accepted frame payload, including the JSON body.
pub const MAX_FRAME_LEN: usize = 1024 * 1024;

/// Maximum player name length in Unicode scalar values.
pub const MAX_PLAYER_NAME_CHARS: usize = 32;

/// Maximum number of client-provided characters echoed in a rejection message.
///
/// Rejections must always fit in one [`MAX_FRAME_LEN`] frame: a client can send
/// a ~1 MiB tag, so echoing it verbatim would make the server's own response
/// oversized and fail the write ([`crate::listen`] then ends the session). The
/// limit is applied with [`str::chars`], so truncation never splits a UTF-8
/// scalar or panics.
pub const MAX_ECHO_CHARS: usize = 64;

/// Tax rates are percentages.
pub const MAX_TAX_RATE: u16 = 100;

/// Maximum road length in grid cells (Chebyshev distance).
pub const MAX_ROAD_LENGTH: u32 = 1024;

/// Inclusive world coordinate bound for v0 road commands.
pub const MAX_WORLD_COORD: i32 = 1_000_000;

/// Hard cap on a session's interest radius, in metres.
///
/// Interest filtering is view-only in v0: a larger radius is rejected rather
/// than clamped so the client always knows the radius the server is using.
pub const MAX_INTEREST_RADIUS_M: f64 = 2_048.0;

/// Default interest radius for a fresh session, in metres (the v0 cap).
pub const DEFAULT_INTEREST_RADIUS_M: f64 = MAX_INTEREST_RADIUS_M;

/// Maximum road details carried by one [`ServerMessage::InterestView`].
///
/// The list is bounded so a viewport covering a large city can never grow the
/// response past [`MAX_FRAME_LEN`]. Roads are ordered by ascending id and
/// `truncated` marks that more roads matched than fit in the list.
pub const MAX_INTEREST_ROADS: usize = 64;

/// Maximum building details carried by one [`ServerMessage::InterestView`].
///
/// Like [`MAX_INTEREST_ROADS`], the list is bounded so a viewport covering a
/// large city can never grow the response past [`MAX_FRAME_LEN`]. Buildings are
/// ordered by ascending id and `buildings_truncated` marks that more buildings
/// matched than fit in the list.
pub const MAX_INTEREST_BUILDINGS: usize = 64;

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
    /// `resume` is only implemented by the authoritative executor; the
    /// transport-only reference session rejects it.
    pub const RESUME_UNAVAILABLE: &str = "resume.unavailable";

    // Command payloads.
    pub const TAX_RATE_OUT_OF_RANGE: &str = "tax.rate_out_of_range";
    pub const ROAD_EMPTY: &str = "road.empty";
    pub const ROAD_OUT_OF_BOUNDS: &str = "road.out_of_bounds";
    pub const ROAD_TOO_LONG: &str = "road.too_long";

    // Blueprint placement (D-032). These are the stable wire codes for the
    // `place_blueprint` command and its rejections.
    /// The transport-only reference session has no package library.
    pub const PLACEMENT_UNAVAILABLE: &str = "PLACEMENT_UNAVAILABLE";
    /// The authoritative world has no blueprint library configured.
    pub const PLACEMENT_LIBRARY_UNAVAILABLE: &str = "PLACEMENT_LIBRARY_UNAVAILABLE";
    /// The requested hash is not `blake3:<64 lowercase hex>` (or bare 64 hex).
    pub const PLACEMENT_HASH_INVALID: &str = "PLACEMENT_HASH_INVALID";
    /// A valid hash that the server library does not store.
    pub const PLACEMENT_PACKAGE_MISSING: &str = "PLACEMENT_PACKAGE_MISSING";
    /// The stored package failed `.tsbp` validation or metadata checks.
    pub const PLACEMENT_PACKAGE_INVALID: &str = "PLACEMENT_PACKAGE_INVALID";
    /// A district policy blocks the placement; the bridge policy code is in
    /// the message.
    pub const PLACEMENT_POLICY_DENIED: &str = "PLACEMENT_POLICY_DENIED";
    /// A coordinate is non-finite or outside the world range.
    pub const PLACEMENT_OUT_OF_RANGE: &str = "PLACEMENT_OUT_OF_RANGE";
    /// The rotation is not a multiple of 90 degrees.
    pub const PLACEMENT_ROTATION_INVALID: &str = "PLACEMENT_ROTATION_INVALID";
    /// The rotated footprint overlaps an existing placement.
    pub const PLACEMENT_OVERLAP: &str = "PLACEMENT_OVERLAP";
    /// The settlement age has not unlocked the blueprint's structure.
    pub const PLACEMENT_AGE_LOCKED: &str = "PLACEMENT_AGE_LOCKED";
    /// The treasury cannot afford the first construction installment.
    pub const PLACEMENT_INSUFFICIENT_FUNDS: &str = "PLACEMENT_INSUFFICIENT_FUNDS";
    /// The placement or construction store is at capacity.
    pub const PLACEMENT_STORE_FULL: &str = "PLACEMENT_STORE_FULL";
    /// A placement or construction failure not covered by a narrower code.
    pub const PLACEMENT_REJECTED: &str = "PLACEMENT_REJECTED";

    // Time votes. The state-machine codes live in `vote::codes`; this one is
    // specific to the transport-only session, which has no vote state.
    pub const VOTE_UNAVAILABLE: &str = "vote.unavailable";

    // Interest filtering. `unavailable` is specific to the transport-only
    // session, which has no interest state.
    pub const INTEREST_UNAVAILABLE: &str = "interest.unavailable";
    pub const INTEREST_INVALID_CENTER: &str = "interest.invalid_center";
    pub const INTEREST_RADIUS_OUT_OF_RANGE: &str = "interest.radius_out_of_range";
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientCommand {
    Hello {
        protocol_version: u16,
        content_hash: String,
        player_name: String,
    },
    /// Reconnect to a session that disconnected within the grace window.
    ///
    /// `session_id` carries the token the server returned in `welcome`. v0
    /// has no authentication: the token identifies a session, it does not
    /// prove ownership.
    Resume {
        session_id: String,
        last_tick: u64,
    },
    Ping {
        nonce: u64,
    },
    TimeControl {
        action: TimeAction,
    },
    /// D-021 multiplayer time vote: call, cast yes/no, or cancel.
    ///
    /// `time` is required for [`VoteAction::Call`] and rejected for the other
    /// actions. `cancel` requires the admin role; `call`/`yes`/`no` are open to
    /// every role because `time.vote` defaults to all players. The vote rules
    /// live in [`crate::vote`].
    TimeVote {
        action: VoteAction,
        time: Option<TimeAction>,
    },
    SetTax {
        resident_rate: u16,
        business_rate: u16,
    },
    PlaceRoad {
        start: [i32; 2],
        end: [i32; 2],
    },
    /// Per-session viewport used by interest filtering.
    ///
    /// This never changes the authoritative simulation: it only narrows what
    /// the next snapshot reports. `radius_m` must be finite and within
    /// `0..=MAX_INTEREST_RADIUS_M`; the centre must be finite. A successful
    /// command is answered with [`ServerMessage::Snapshot`] followed by
    /// [`ServerMessage::InterestView`] so the client gets the filtered road
    /// details immediately, without a second round trip.
    SetInterest {
        center_x: f64,
        center_z: f64,
        radius_m: f64,
    },
    /// D-032 authoritative blueprint placement.
    ///
    /// The wire carries only the content-addressed package hash, the rotated
    /// footprint centre in metres, and a rotation in degrees. The package path
    /// never crosses the wire: the server resolves the hash in its own
    /// pre-seeded content-addressed library. Requires the Mayor role.
    PlaceBlueprint {
        /// `blake3:<64 lowercase hex>` or bare 64 lowercase hex. Normalized to
        /// the canonical prefixed form before lookup.
        blueprint_hash: String,
        /// Footprint centre X in world metres.
        x: f64,
        /// Footprint centre Z in world metres.
        z: f64,
        /// Rotation around Y in degrees; must be a multiple of 90.
        rotation: i32,
    },
    Disconnect,
}

impl ClientCommand {
    pub fn tag(&self) -> &'static str {
        match self {
            ClientCommand::Hello { .. } => "hello",
            ClientCommand::Resume { .. } => "resume",
            ClientCommand::Ping { .. } => "ping",
            ClientCommand::TimeControl { .. } => "time_control",
            ClientCommand::TimeVote { .. } => "time_vote",
            ClientCommand::SetTax { .. } => "set_tax",
            ClientCommand::PlaceRoad { .. } => "place_road",
            ClientCommand::SetInterest { .. } => "set_interest",
            ClientCommand::PlaceBlueprint { .. } => "place_blueprint",
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

/// Vote actions for [`ClientCommand::TimeVote`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoteAction {
    /// Start a vote for the `time` target (pause/resume in v0).
    Call,
    /// Vote yes.
    Yes,
    /// Vote no.
    No,
    /// Admin-only cancel.
    Cancel,
}

/// One road detail in [`ServerMessage::InterestView`].
///
/// This is a read-only copy of the authoritative segment, limited to the
/// geometry a client needs to draw the road. It carries no cost, ownership, or
/// simulation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoadView {
    /// Stable road id within the world.
    pub id: u64,
    /// Start centreline cell `[x, z]`.
    pub start: [i32; 2],
    /// End centreline cell `[x, z]`.
    pub end: [i32; 2],
    /// Road width in metres.
    pub width_m: u8,
}

/// One building detail in [`ServerMessage::InterestView`].
///
/// v0 has no per-building placement store in [`crate::world::ServerWorld`], so
/// this record is synthesized deterministically from the world's static
/// completed-building count (see [`crate::world::ServerWorld::interest_buildings`]).
/// It is a stable viewport approximation for previews, not a live footprint or
/// an authoritative placement record. It carries no ownership, blueprint, cost,
/// or simulation state.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BuildingView {
    /// Stable building id within the world (1-based).
    pub id: u64,
    /// Building centre `[x, z]` in metres.
    pub center: [f32; 2],
    /// Building footprint radius in metres (a render hint, not authoritative).
    pub radius_m: f32,
}

/// One authoritative placement returned by `place_blueprint`.
///
/// The view is a copy of the server-owned [`core_city::placement::Placement`]
/// record without the server-local `blueprint_path`. A remote client never
/// receives a filesystem path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementView {
    /// Stable placement id within the world save.
    pub id: u64,
    /// Stable blueprint id.
    pub blueprint_id: String,
    /// Canonical `blake3:<hex>` content hash.
    pub blueprint_hash: String,
    /// Human-readable blueprint name.
    pub name: String,
    /// Building base centre; `y` is the foundation top.
    pub center: [f32; 3],
    /// Normalized rotation around Y, 0/90/180/270.
    pub rotation_deg: u16,
    /// Rotated footprint: x, z.
    pub footprint_m: [f32; 2],
    /// Full volume bounds: width, height, depth.
    pub bounds_m: [f32; 3],
    /// Vertical distance from the lowest sampled terrain to `center.y`.
    pub foundation_m: f32,
    /// Blueprint material cost.
    pub material_cost: f64,
    /// Blueprint resident capacity.
    pub resident_capacity: u32,
    /// Blueprint job slots.
    pub job_slots: u32,
    /// Blueprint solid voxels, used as construction material litres.
    pub solid_voxels: u64,
}

/// Messages the authoritative server sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Welcome {
        session_id: String,
        /// Stable reconnect token for this world/session.
        ///
        /// v0 derives it from the seed and sets it to the same value as
        /// `session_id`. A future authenticated revision can rotate it without
        /// changing the `resume` command shape.
        #[serde(default)]
        session_token: String,
        tick: u64,
        protocol_version: u16,
        /// True when this `welcome` answers a successful `resume`.
        #[serde(default)]
        resumed: bool,
    },
    Pong {
        nonce: u64,
    },
    Snapshot {
        tick: u64,
        population: u32,
        balance: i64,
        checksum: String,
        /// Interest viewport centre for this session, `[x, z]` in metres.
        ///
        /// Part of the snapshot envelope, **not** of `checksum`.
        #[serde(default)]
        interest_center: [f32; 2],
        /// Interest viewport radius in metres. View-only; not in `checksum`.
        #[serde(default = "default_interest_radius")]
        interest_radius_m: f32,
        /// Roads whose midpoint is inside the interest circle.
        ///
        /// Aggregate count only; the authoritative road store is unchanged.
        /// [`ServerMessage::InterestView`] carries the bounded per-road detail
        /// for the same viewport, so this count can exceed `roads.len()`.
        #[serde(default)]
        visible_roads: u32,
    },
    /// Bounded road and building details for the current interest viewport.
    ///
    /// Sent after the matching [`ServerMessage::Snapshot`] whenever
    /// `set_interest` succeeds. This is the first filtered snapshot payload: it
    /// is a view-only copy, never a mutation. Roads are filtered by their
    /// midpoint against the inclusive circle, ordered by ascending id, and
    /// capped at [`MAX_INTEREST_ROADS`]; `truncated` is true when more roads
    /// matched than the list carries. Buildings use the same inclusive
    /// circle over their centres, ascending id, and are capped separately at
    /// [`MAX_INTEREST_BUILDINGS`].
    InterestView {
        /// Authoritative tick the view was built from.
        tick: u64,
        /// Viewport centre `[x, z]` in metres.
        center: [f32; 2],
        /// Viewport radius in metres.
        radius_m: f32,
        /// Matching roads, ascending by id, at most [`MAX_INTEREST_ROADS`].
        roads: Vec<RoadView>,
        /// True when more roads matched than fit in `roads`.
        truncated: bool,
        /// Matching buildings, ascending by id, at most
        /// [`MAX_INTEREST_BUILDINGS`].
        ///
        /// Defaults to an empty list for older payloads that predate the
        /// building list.
        #[serde(default)]
        buildings: Vec<BuildingView>,
        /// True when more buildings matched than fit in `buildings`.
        ///
        /// Defaults to false for older payloads that predate the building list.
        #[serde(default)]
        buildings_truncated: bool,
    },
    Reject {
        code: String,
        message: String,
    },
    /// Accepted authoritative blueprint placement (D-032).
    ///
    /// Sent only when the whole command succeeded: the package resolved and
    /// validated, policy/age/terrain passed, and the treasury accepted the
    /// first D-028 installment. `placement_checksum` covers the placement store
    /// and `world_checksum` covers the whole world, both as stable hex.
    PlaceBlueprintResult {
        /// Authoritative tick after the accepted placement.
        tick: u64,
        /// Id of the stored placement.
        placement_id: u64,
        /// Canonical `blake3:<hex>` hash that was placed.
        blueprint_hash: String,
        /// Stable checksum of the server placement store.
        placement_checksum: String,
        /// Stable checksum of the whole world.
        world_checksum: String,
        /// Copy of the stored placement without any server filesystem path.
        placement: PlacementView,
    },
    /// Vote snapshot/status, sent after every accepted vote command.
    ///
    /// `online` is the roster snapshot at vote start (the denominator), `clock`
    /// is the server clock in seconds, and `checksum` is the vote-state golden.
    TimeVoteStatus {
        vote_id: u64,
        state: String,
        action: Option<String>,
        initiator: String,
        yes: u32,
        no: u32,
        online: u32,
        started_tick: u64,
        ends_tick: u64,
        cooldown_until: u64,
        paused: bool,
        clock: u64,
        checksum: String,
    },
    Goodbye,
}

/// Default radius used when a peer omits `interest_radius_m` (older payloads).
fn default_interest_radius() -> f32 {
    DEFAULT_INTEREST_RADIUS_M as f32
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
        "resume",
        "ping",
        "time_control",
        "time_vote",
        "set_tax",
        "place_road",
        "set_interest",
        "place_blueprint",
        "disconnect",
    ];
    if !KNOWN_TAGS.contains(&tag) {
        return Err(Rejection::new(
            codes::COMMAND_UNKNOWN_TAG,
            format!("unknown command tag `{}`", truncate_for_message(tag)),
        ));
    }

    serde_json::from_value(value)
        .map_err(|error| Rejection::new(codes::COMMAND_INVALID_FIELDS, error.to_string()))
}

/// Bound client-provided text before it is echoed in a rejection message.
///
/// The input is sliced on character boundaries, so multi-byte text is never
/// split; the appended ellipsis marks that the tag was shortened.
fn truncate_for_message(value: &str) -> String {
    let mut source = value.chars();
    let truncated: String = source.by_ref().take(MAX_ECHO_CHARS).collect();
    if source.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
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

/// Validate a set-interest payload.
///
/// The centre must be finite and representable as `f32` (the executor stores
/// it that way); the radius must be finite and within
/// `0..=MAX_INTEREST_RADIUS_M`. Out-of-range radii are rejected, never
/// clamped, so a client cannot believe it has a larger view than it does.
pub fn validate_interest(center_x: f64, center_z: f64, radius_m: f64) -> Result<(), Rejection> {
    if !center_x.is_finite()
        || !center_z.is_finite()
        || !(center_x as f32).is_finite()
        || !(center_z as f32).is_finite()
    {
        return Err(Rejection::new(
            codes::INTEREST_INVALID_CENTER,
            "interest centre must be finite",
        ));
    }
    if !radius_m.is_finite() || !(0.0..=MAX_INTEREST_RADIUS_M).contains(&radius_m) {
        return Err(Rejection::new(
            codes::INTEREST_RADIUS_OUT_OF_RANGE,
            format!("interest radius must be within 0..={MAX_INTEREST_RADIUS_M} m"),
        ));
    }
    Ok(())
}

/// Normalize a `place_blueprint` package hash.
///
/// Accepts the canonical `blake3:<64 lowercase hex>` form used by the library
/// and the bare 64-lowercase-hex form for convenience. Uppercase hex, paths,
/// traversal, and any other shape are rejected. The returned string is the
/// canonical prefixed form used as the library address.
pub fn normalize_blueprint_hash(hash: &str) -> Result<String, Rejection> {
    let hex = hash.strip_prefix(BLUEPRINT_HASH_PREFIX).unwrap_or(hash);
    let valid = hex.len() == 64
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !valid {
        return Err(Rejection::new(
            codes::PLACEMENT_HASH_INVALID,
            "blueprint hash must be `blake3:` followed by 64 lowercase hexadecimal characters",
        ));
    }
    Ok(format!("{BLUEPRINT_HASH_PREFIX}{hex}"))
}

/// Validate a `place_blueprint` payload before the executor touches a store.
///
/// The package hash is normalized; coordinates must be finite, fit in `f32`,
/// and stay within [`MAX_WORLD_COORD`]; the rotation must be a multiple of 90
/// degrees. Payload validation is store-independent, so it runs before the
/// library lookup.
pub fn validate_place_blueprint(
    blueprint_hash: &str,
    x: f64,
    z: f64,
    rotation: i32,
) -> Result<(), Rejection> {
    normalize_blueprint_hash(blueprint_hash)?;
    for (axis, value) in [("x", x), ("z", z)] {
        if !value.is_finite() || !(value as f32).is_finite() {
            return Err(Rejection::new(
                codes::PLACEMENT_OUT_OF_RANGE,
                format!("placement {axis} must be finite"),
            ));
        }
        if value.abs() > f64::from(MAX_WORLD_COORD) {
            return Err(Rejection::new(
                codes::PLACEMENT_OUT_OF_RANGE,
                format!("placement {axis} {value} is outside ±{MAX_WORLD_COORD} m"),
            ));
        }
    }
    if rotation.rem_euclid(90) != 0 {
        return Err(Rejection::new(
            codes::PLACEMENT_ROTATION_INVALID,
            format!("placement rotation {rotation} is not a multiple of 90 degrees"),
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
                    session_token: self.session_id.clone(),
                    tick: self.tick,
                    protocol_version: PROTOCOL_VERSION,
                    resumed: false,
                }]
            }
            ClientCommand::Resume { .. } => reject(
                codes::RESUME_UNAVAILABLE,
                "resume is not supported by the transport-only session",
            ),
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
            ClientCommand::TimeVote { .. } => reject(
                codes::VOTE_UNAVAILABLE,
                "time votes require the authoritative executor, not the transport-only session",
            ),
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
            ClientCommand::SetInterest { .. } => reject(
                codes::INTEREST_UNAVAILABLE,
                "interest filtering requires the authoritative executor, not the transport-only session",
            ),
            ClientCommand::PlaceBlueprint { .. } => reject(
                codes::PLACEMENT_UNAVAILABLE,
                "blueprint placement requires the authoritative executor with a package library, not the transport-only session",
            ),
        }
    }

    fn snapshot(&self) -> ServerMessage {
        ServerMessage::Snapshot {
            tick: self.tick,
            population: self.population,
            balance: self.balance,
            checksum: self.checksum(),
            // The transport-only session has no interest state; it answers
            // with the default viewport and its aggregate road count.
            interest_center: [0.0, 0.0],
            interest_radius_m: default_interest_radius(),
            visible_roads: self.roads,
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
            resumed,
            ..
        } if protocol_version == PROTOCOL_VERSION
            && tick == 0
            && !resumed
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
            ..
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
        assert_eq!(rejection.message, "unknown command tag `launch_missiles`");
    }

    #[test]
    fn oversized_unknown_tags_are_truncated_to_a_bounded_rejection() {
        // The largest frame the transport accepts must still produce a
        // rejection that fits in a single response frame.
        let tag = "x".repeat(MAX_FRAME_LEN - 16);
        let payload = format!(r#"{{"type":"{tag}"}}"#);
        assert!(payload.len() <= MAX_FRAME_LEN);

        let rejection = decode_client_command(payload.as_bytes()).unwrap_err();
        assert_eq!(rejection.code, codes::COMMAND_UNKNOWN_TAG);
        assert!(
            rejection.message.chars().count() <= MAX_ECHO_CHARS + 40,
            "rejection echoes too much client input: {} chars",
            rejection.message.chars().count()
        );
        assert!(rejection.message.contains('…'));

        let message = rejection.into_message();
        let encoded = encode_json(&message).expect("rejection encodes");
        assert!(
            encoded.len() < MAX_FRAME_LEN,
            "rejection encodes to {} bytes",
            encoded.len()
        );
        let mut frame = Vec::new();
        write_json_frame(&mut frame, &message).expect("rejection fits one frame");
    }

    #[test]
    fn tag_truncation_is_char_safe() {
        // Multi-byte scalars must not be split; each `€` is three UTF-8 bytes.
        let tag = "€".repeat(MAX_ECHO_CHARS + 10);
        let payload = format!(r#"{{"type":"{tag}"}}"#);
        let rejection = decode_client_command(payload.as_bytes()).unwrap_err();
        assert_eq!(rejection.code, codes::COMMAND_UNKNOWN_TAG);
        assert_eq!(
            truncate_for_message(&tag),
            format!("{}…", "€".repeat(MAX_ECHO_CHARS))
        );
        assert!(rejection.message.is_char_boundary(rejection.message.len()));
        // A tag at or under the limit is echoed verbatim.
        let short = "y".repeat(MAX_ECHO_CHARS);
        assert_eq!(truncate_for_message(&short), short);
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
    fn place_blueprint_decodes_and_validates_its_payload() {
        let hash = format!("{BLUEPRINT_HASH_PREFIX}{}", "ab".repeat(32));
        let payload = format!(
            r#"{{"type":"place_blueprint","blueprint_hash":"{hash}","x":16.5,"z":-4.0,"rotation":-90}}"#
        );
        let decoded = decode_client_command(payload.as_bytes()).expect("place_blueprint decodes");
        assert_eq!(
            decoded,
            ClientCommand::PlaceBlueprint {
                blueprint_hash: hash.clone(),
                x: 16.5,
                z: -4.0,
                rotation: -90,
            }
        );
        assert_eq!(decoded.tag(), "place_blueprint");
        assert!(validate_place_blueprint(&hash, 16.5, -4.0, -90).is_ok());

        // Bare lowercase hex is accepted and normalized to the canonical form.
        let bare = "ab".repeat(32);
        assert_eq!(normalize_blueprint_hash(&bare).expect("bare hash"), hash);
        assert_eq!(normalize_blueprint_hash(&hash).expect("prefixed"), hash);

        // Non-JSON fields are a decode error.
        let rejection = decode_client_command(
            br#"{"type":"place_blueprint","blueprint_hash":1,"x":0,"z":0,"rotation":0}"#,
        )
        .expect_err("numeric hash must not decode");
        assert_eq!(rejection.code, codes::COMMAND_INVALID_FIELDS);
    }

    #[test]
    fn place_blueprint_rejects_malformed_hashes_and_rotations() {
        let valid = format!("{BLUEPRINT_HASH_PREFIX}{}", "ab".repeat(32));
        let bad_hashes = [
            String::new(),
            "blake3:".to_string(),
            "ab".to_string(),
            "AB".repeat(32),
            format!("blake3:{}", "AB".repeat(32)),
            format!("{valid}/../x"),
            format!("sha256:{}", "ab".repeat(32)),
        ];
        for bad in &bad_hashes {
            assert_eq!(
                validate_place_blueprint(bad, 0.0, 0.0, 0).unwrap_err().code,
                codes::PLACEMENT_HASH_INVALID,
                "hash `{bad}` must be rejected"
            );
        }

        for rotation in [1, 45, 89, 91, -45, i32::MAX] {
            assert_eq!(
                validate_place_blueprint(&valid, 0.0, 0.0, rotation)
                    .unwrap_err()
                    .code,
                codes::PLACEMENT_ROTATION_INVALID,
                "rotation {rotation} must be rejected"
            );
        }
        for rotation in [0, 90, 180, 270, -90, 360, -270] {
            assert!(
                validate_place_blueprint(&valid, 0.0, 0.0, rotation).is_ok(),
                "rotation {rotation} must be accepted"
            );
        }
    }

    #[test]
    fn place_blueprint_rejects_non_finite_and_out_of_range_coordinates() {
        let valid = format!("{BLUEPRINT_HASH_PREFIX}{}", "ab".repeat(32));
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX] {
            assert_eq!(
                validate_place_blueprint(&valid, value, 0.0, 0)
                    .unwrap_err()
                    .code,
                codes::PLACEMENT_OUT_OF_RANGE
            );
            assert_eq!(
                validate_place_blueprint(&valid, 0.0, value, 0)
                    .unwrap_err()
                    .code,
                codes::PLACEMENT_OUT_OF_RANGE
            );
        }
        assert_eq!(
            validate_place_blueprint(&valid, 1_000_001.0, 0.0, 0)
                .unwrap_err()
                .code,
            codes::PLACEMENT_OUT_OF_RANGE
        );
        assert!(validate_place_blueprint(&valid, -1_000_000.0, 1_000_000.0, 0).is_ok());
    }

    #[test]
    fn place_blueprint_result_round_trips_without_a_server_path() {
        let message = ServerMessage::PlaceBlueprintResult {
            tick: 7,
            placement_id: 3,
            blueprint_hash: format!("{BLUEPRINT_HASH_PREFIX}{}", "cd".repeat(32)),
            placement_checksum: "0123456789abcdef".to_string(),
            world_checksum: "fedcba9876543210".to_string(),
            placement: PlacementView {
                id: 3,
                blueprint_id: "townsmith:villa".to_string(),
                blueprint_hash: format!("{BLUEPRINT_HASH_PREFIX}{}", "cd".repeat(32)),
                name: "Villa".to_string(),
                center: [16.0, 41.0, 32.0],
                rotation_deg: 90,
                footprint_m: [8.0, 12.0],
                bounds_m: [12.0, 9.6, 8.0],
                foundation_m: 1.0,
                material_cost: 38_028.8,
                resident_capacity: 2,
                job_slots: 2,
                solid_voxels: 64_224,
            },
        };
        let mut buffer = Vec::new();
        write_json_frame(&mut buffer, &message).expect("encode");
        let decoded: ServerMessage = read_json_frame(&mut buffer.as_slice()).expect("decode");
        assert_eq!(decoded, message);

        let json = serde_json::to_string(&message).expect("json");
        assert!(!json.contains("blueprint_path"));
        assert!(json.contains("\"type\":\"place_blueprint_result\""));
    }

    #[test]
    fn transport_session_rejects_place_blueprint_without_mutation() {
        let mut session = Session::new(LOOPBACK_SEED);
        session.handle(hello());
        let tick = session.tick();
        let checksum = session.checksum();
        let messages = session.handle(ClientCommand::PlaceBlueprint {
            blueprint_hash: format!("{BLUEPRINT_HASH_PREFIX}{}", "ab".repeat(32)),
            x: 0.0,
            z: 0.0,
            rotation: 0,
        });
        expect_reject(&messages, codes::PLACEMENT_UNAVAILABLE);
        assert_eq!(session.tick(), tick);
        assert_eq!(session.checksum(), checksum);
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
    fn set_interest_decodes_and_enforces_the_radius_bound() {
        let decoded = decode_client_command(
            br#"{"type":"set_interest","center_x":1.5,"center_z":-2.5,"radius_m":64}"#,
        )
        .expect("set_interest decodes");
        assert_eq!(
            decoded,
            ClientCommand::SetInterest {
                center_x: 1.5,
                center_z: -2.5,
                radius_m: 64.0,
            }
        );
        assert_eq!(decoded.tag(), "set_interest");

        for radius in [0.0, 1.0, MAX_INTEREST_RADIUS_M] {
            assert!(
                validate_interest(0.0, 0.0, radius).is_ok(),
                "radius {radius} must be accepted"
            );
        }
        for radius in [
            -1.0,
            MAX_INTEREST_RADIUS_M + 0.1,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ] {
            assert_eq!(
                validate_interest(0.0, 0.0, radius).unwrap_err().code,
                codes::INTEREST_RADIUS_OUT_OF_RANGE,
                "radius {radius} must be rejected"
            );
        }
    }

    #[test]
    fn set_interest_rejects_non_finite_centres() {
        for center in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX] {
            assert_eq!(
                validate_interest(center, 0.0, 10.0).unwrap_err().code,
                codes::INTEREST_INVALID_CENTER
            );
            assert_eq!(
                validate_interest(0.0, center, 10.0).unwrap_err().code,
                codes::INTEREST_INVALID_CENTER
            );
        }
        assert!(validate_interest(-1_000_000.0, 1_000_000.0, 2_048.0).is_ok());
    }

    #[test]
    fn transport_session_rejects_set_interest_without_state_change() {
        let mut session = Session::new(LOOPBACK_SEED);
        session.handle(hello());
        let tick = session.tick();
        let checksum = session.checksum();

        expect_reject(
            &session.handle(ClientCommand::SetInterest {
                center_x: 10.0,
                center_z: -5.0,
                radius_m: 32.0,
            }),
            codes::INTEREST_UNAVAILABLE,
        );
        assert_eq!(session.tick(), tick);
        assert_eq!(session.checksum(), checksum);
    }

    #[test]
    fn interest_view_round_trips_with_bounded_road_details() {
        let message = ServerMessage::InterestView {
            tick: 7,
            center: [4.0, -2.0],
            radius_m: 64.0,
            roads: vec![RoadView {
                id: 1,
                start: [0, 0],
                end: [8, 0],
                width_m: 6,
            }],
            truncated: true,
            buildings: vec![BuildingView {
                id: 1,
                center: [0.0, 0.0],
                radius_m: 8.0,
            }],
            buildings_truncated: true,
        };

        let payload = encode_json(&message).expect("interest view encodes");
        let value: serde_json::Value = serde_json::from_slice(&payload).expect("valid JSON");
        assert_eq!(value["type"], "interest_view");
        assert_eq!(value["roads"][0]["id"], 1);
        assert_eq!(value["roads"][0]["start"][0], 0);
        assert_eq!(value["roads"][0]["width_m"], 6);
        assert_eq!(value["truncated"], true);
        assert_eq!(value["buildings"][0]["id"], 1);
        assert_eq!(value["buildings"][0]["center"][1], 0.0);
        assert_eq!(value["buildings"][0]["radius_m"], 8.0);
        assert_eq!(value["buildings_truncated"], true);

        let decoded: ServerMessage = decode_json(&payload).expect("interest view decodes");
        assert_eq!(decoded, message);
    }

    #[test]
    fn interest_view_defaults_building_fields_for_older_payloads() {
        // A v0 payload written before the building list existed must still
        // decode: both building fields default rather than failing the frame.
        let legacy = br#"{
            "type": "interest_view",
            "tick": 2,
            "center": [0.0, 0.0],
            "radius_m": 16.0,
            "roads": [],
            "truncated": false
        }"#;

        match decode_json::<ServerMessage>(legacy).expect("legacy interest view decodes") {
            ServerMessage::InterestView {
                tick: 2,
                roads,
                truncated: false,
                buildings,
                buildings_truncated,
                ..
            } => {
                assert!(roads.is_empty());
                assert!(buildings.is_empty());
                assert!(!buildings_truncated);
            }
            other => panic!("expected interest view, got {other:?}"),
        }
    }

    #[test]
    fn interest_building_list_is_bounded_by_the_wire_cap() {
        assert_eq!(MAX_INTEREST_BUILDINGS, 64);

        let buildings: Vec<BuildingView> = (1..=MAX_INTEREST_BUILDINGS as u64)
            .map(|id| BuildingView {
                id,
                center: [(id as f32) * 64.0, 0.0],
                radius_m: 8.0,
            })
            .collect();
        let message = ServerMessage::InterestView {
            tick: 0,
            center: [0.0, 0.0],
            radius_m: 2_048.0,
            roads: Vec::new(),
            truncated: false,
            buildings,
            buildings_truncated: true,
        };
        let encoded = encode_json(&message).expect("bounded building list encodes");
        assert!(
            encoded.len() < MAX_FRAME_LEN,
            "interest view with a full building list must fit one frame"
        );
    }

    #[test]
    fn transport_session_rejects_set_interest_and_never_emits_a_view() {
        let mut session = Session::new(LOOPBACK_SEED);
        session.handle(hello());
        let tick = session.tick();
        let checksum = session.checksum();

        let messages = session.handle(ClientCommand::SetInterest {
            center_x: 4.0,
            center_z: 0.0,
            radius_m: 16.0,
        });
        expect_reject(&messages, codes::INTEREST_UNAVAILABLE);
        assert!(
            !messages
                .iter()
                .any(|message| matches!(message, ServerMessage::InterestView { .. })),
            "the transport-only session has no road view"
        );
        assert_eq!(session.tick(), tick);
        assert_eq!(session.checksum(), checksum);
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
