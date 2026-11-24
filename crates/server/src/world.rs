//! First authoritative command executor on protocol v1.
//!
//! [`ServerWorld`] owns the mutable server state shared by every connection:
//! the clock, static population counters, the treasury, the road store, the
//! time vote, and the online roster. Per-connection state (handshake, role,
//! reconnect window, interest viewport, rate bucket, audit ring) lives in a
//! [`SessionState`] table keyed by a process-local connection id. The legacy
//! single-session methods ([`ServerWorld::apply`] and
//! [`ServerWorld::apply_all`]) keep operating on a default session slot, so a
//! one-connection world behaves exactly as before.
//!
//! Persistence lives in [`crate::save`] and is opt-in: one world to one JSON
//! file, with atomic writes and a single `.bak` rotation. The executor itself
//! has no authentication, rollback, terrain sampling, or wall-clock loop. Time
//! advances only through `time_control` (`step`) or an explicit
//! [`ServerWorld::advance_day`] call, so transcripts are deterministic.
//!
//! Interest filtering v0 is a **per-session viewport**: [`ClientCommand::SetInterest`]
//! stores a centre and radius that narrow [`ServerMessage::Snapshot`] and add a
//! bounded [`ServerMessage::InterestView`] road list. It never mutates the
//! simulation, never advances the tick, and is excluded from
//! [`ServerWorld::checksum`].

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::thread;

use core_blueprint::{load_tsbp, BlueprintLibrary, BlueprintLibraryError};
use core_city::construction::{
    first_construction_payment, settle_first_installment, ConstructionError, ConstructionStore,
};
use core_city::economy::{Economy, EconomyTick, DAY_SECONDS, STARTING_BALANCE};
use core_city::placement::{
    normalize_rotation, validate_placement, Placement, PlacementError, PlacementRequest,
    PlacementStore,
};
use core_city::player::{PlayerId, PlayerRegistry};
use core_city::policy::{policy_placement_denial, DistrictPolicyError, DistrictPolicyStore};
use core_city::progression::AgeState;
use core_city::road::{RoadError, RoadRequest, RoadSegment, RoadStore};
use core_city::settlement::{
    Settlement, SettlementPermission, SettlementRequest, SettlementRole, SettlementStore,
};
use core_city::structure_age_error;

use crate::protocol::{
    configure_stream, decode_client_command, frame_to_io, normalize_blueprint_hash, read_frame,
    read_json_frame, session_id_for_seed, validate_hello, validate_interest,
    validate_place_blueprint, validate_road, validate_tax_rates, write_json_frame, BuildingView,
    ClientCommand, FrameError, PlacementView, Rejection, RoadView, ServerMessage, SessionConfig,
    SettlementMemberView, SettlementView, TimeAction, DEFAULT_INTEREST_RADIUS_M, LOOPBACK_SEED,
    MAX_INTEREST_BUILDINGS, MAX_INTEREST_ROADS, PROTOCOL_VERSION, SESSION_IO_TIMEOUT,
};

/// Fixed road width for v0 placement commands.
pub const WORLD_ROAD_WIDTH_M: u8 = 6;

/// Fallback building footprint radius used by synthetic building records.
///
/// A render hint only: placement-backed records report half their larger
/// footprint side, and only the no-placement fixture uses this constant.
pub const INTEREST_BUILDING_RADIUS_M: f32 = 8.0;

/// Spacing between synthetic building centres, in metres.
pub const INTEREST_BUILDING_SPACING_M: f32 = 64.0;

/// Buildings per synthetic grid row.
pub const INTEREST_BUILDING_COLUMNS: u64 = 16;

/// Longest road a single `place_road` command may add, in cells.
pub const MAX_WORLD_ROAD_LENGTH: i32 = 512;

/// Placeholder flat ground height used for road validation.
///
/// The server has no terrain yet. This is above `WATER_LEVEL_M` (24 m) and
/// constant, so only geometry, bounds, and length rules can fail.
pub const PLACEHOLDER_GROUND_M: u16 = 40;

/// Crew slots in a server-created construction store.
///
/// Matches [`ConstructionStore::default`], so a server placement and a bridge
/// placement start the same site state.
pub const SERVER_CONSTRUCTION_CREW_CAPACITY: u16 = 4;

/// Material litres in a server-created construction store.
///
/// Matches [`ConstructionStore::default`].
pub const SERVER_CONSTRUCTION_MATERIALS_STOCK: u64 = 1_000_000;

/// Deterministic client commands exchanged by the authority loopback.
pub const AUTHORITY_COMMANDS: u64 = 6;

/// Application frames exchanged in both directions by the authority loopback.
pub const AUTHORITY_FRAMES: u64 = 12;

/// Client commands exchanged by the interest loopback.
pub const INTEREST_COMMANDS: u64 = 8;

/// Application frames exchanged in both directions by the interest loopback.
///
/// Eight commands yield twelve responses: `set_interest` answers with a
/// `snapshot` plus an `interest_view`.
pub const INTEREST_FRAMES: u64 = 20;

/// Sessions exercised by the role/rate-limit loopback (viewer, mayor, admin).
pub const ROLES_SESSIONS: u64 = 3;

/// Maximum audit entries retained per session.
pub const AUDIT_CAPACITY: usize = 256;

/// Audit code recorded for an accepted command.
pub const AUDIT_ACCEPTED: &str = "ok";

/// Sustained command rate, in credits per authoritative second.
///
/// The rate is nominal: v0 has no wall-clock loop, so virtual time comes from
/// the authoritative tick and the monotonic command counter.
pub const RATE_CREDITS_PER_SECOND: u64 = 20;

/// Token-bucket burst size, in credits.
pub const RATE_BURST_CREDITS: u64 = 40;

/// Virtual milliseconds granted per authoritative tick (one nominal second).
pub const RATE_TICK_MS: u64 = 1_000;

/// Virtual milliseconds granted per applied command (a nominal 100 commands/s).
pub const RATE_COMMAND_MS: u64 = 10;

/// Thousandths of a credit; one command costs [`RATE_CREDIT_MILLI`].
const RATE_CREDIT_MILLI: u64 = 1_000;

/// One credit per second is one thousandth of a credit per millisecond.
const RATE_MILLI_CREDITS_PER_MS: u64 = RATE_CREDITS_PER_SECOND;

/// Authoritative ticks a disconnected session stays resumable.
///
/// The window is measured in ticks, never wall-clock time: a server with no
/// running simulation loop keeps the window open until the tick advances.
pub const RESUME_GRACE_TICKS: u64 = 300;

/// Fresh `hello` attempts allowed while a resume window is open before the
/// stale window is abandoned.
pub const MAX_HELLO_RESUME_ATTEMPTS: u32 = 3;

/// Stable executor error codes. These are wire/API contracts; do not rename.
pub mod codes {
    /// A non-hello command arrived before an accepted `hello`.
    pub const NOT_HELLO: &str = "not_hello";
    /// The command is not permitted for the session's role.
    pub const FORBIDDEN: &str = "role.forbidden";
    /// The deterministic command token bucket is empty.
    pub const RATE_LIMITED: &str = "rate.limited";
    /// A time action the executor does not support (speed controls).
    pub const TIME_INVALID_ACTION: &str = "time.invalid_action";
    /// Another online session already uses this player name.
    pub const PLAYER_NAME_DUPLICATE: &str = "player_name.duplicate";
    /// A player name cannot be resolved to exactly one registry identity.
    pub const PLAYER_IDENTITY: &str = "player.identity";
    /// The resume token is empty, unknown, or does not match this world.
    pub const RESUME_UNKNOWN: &str = "resume.unknown";
    /// `last_tick` is ahead of the authoritative tick.
    pub const RESUME_STALE: &str = "resume.stale";
    /// The reconnect grace window has passed.
    pub const RESUME_EXPIRED: &str = "resume.expired";
    /// A session is already active; `resume` cannot run twice.
    pub const RESUME_DUPLICATE: &str = "resume.duplicate";
    /// A fresh `hello` was sent while a disconnected session can still resume.
    pub const HELLO_RESUMABLE: &str = "hello.resumable";
    /// Road endpoints do not share an axis.
    pub const ROAD_NOT_AXIS_ALIGNED: &str = "road.not_axis_aligned";
    /// An identical road already exists.
    pub const ROAD_DUPLICATE: &str = "road.duplicate";
    /// A collinear road shares more than one cell with this one.
    pub const ROAD_OVERLAP: &str = "road.overlap";
    /// The road store is full.
    pub const ROAD_STORE_FULL: &str = "road.store_full";
    /// Road ids are exhausted.
    pub const ROAD_ID_EXHAUSTED: &str = "road.id_exhausted";
    /// A store-level rejection not covered by a narrower code.
    pub const ROAD_REJECTED: &str = "road.rejected";
    /// Time-vote commands are unavailable in the transport-only session.
    pub const VOTE_UNAVAILABLE: &str = "vote.unavailable";
    /// A settlement placement needs an authenticated player identity.
    pub const SETTLEMENT_IDENTITY: &str = "settlement.identity";
    /// The settlement containing the placement has no owner.
    pub const SETTLEMENT_UNOWNED: &str = "settlement.unowned";
    /// The player is a member but lacks [`SettlementPermission::BUILD`].
    pub const SETTLEMENT_FORBIDDEN: &str = "settlement.forbidden";
    /// The player is not a member of the settlement containing the placement.
    pub const SETTLEMENT_NOT_MEMBER: &str = "settlement.not_member";
    /// The settlement store belongs to a different world seed.
    pub const SETTLEMENT_SEED_MISMATCH: &str = "settlement.seed_mismatch";
    /// A settlement command carried an unknown or disallowed role.
    pub const SETTLEMENT_ROLE_INVALID: &str = "settlement.role_invalid";
    /// No settlement store exists or the requested id is unknown.
    pub const SETTLEMENT_UNKNOWN: &str = "settlement.unknown";
    /// A transfer recipient is not in the server player registry.
    pub const SETTLEMENT_UNKNOWN_PLAYER: &str = "settlement.unknown_player";
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

/// Player role for one protocol v0 session.
///
/// Roles are ordered `Viewer < Mayor < Admin`: a role may issue every command
/// its rank permits. v0 has no accounts or per-player permission storage; the
/// role is assigned by the server process (`--role` on the dev harness) and is
/// **not** authentication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum ClientRole {
    /// Read-only: `hello`, `ping`, `disconnect`, and its own `set_interest`.
    #[default]
    Viewer,
    /// Viewer plus economy and placement commands (`set_tax`, `place_road`,
    /// `place_blueprint`).
    Mayor,
    /// Mayor plus `time_control`.
    Admin,
}

impl ClientRole {
    /// Every role, weakest first.
    pub const ALL: [Self; 3] = [Self::Viewer, Self::Mayor, Self::Admin];

    /// Stable lowercase name used on the wire, in logs, and in `--role`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Mayor => "mayor",
            Self::Admin => "admin",
        }
    }

    /// Parse a role name (case-insensitive). Unknown names are rejected.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|role| role.as_str().eq_ignore_ascii_case(value))
    }

    /// Minimum role required to issue a command `tag`.
    ///
    /// Unknown tags require [`Self::Admin`]: the matrix fails closed so a new
    /// command cannot become open to everyone by omission.
    #[must_use]
    pub fn required_role(tag: &str) -> Self {
        match tag {
            "hello" | "resume" | "ping" | "disconnect" | "time_vote" | "set_interest"
            | "settlement_list" | "settlement_at" => Self::Viewer,
            "set_tax"
            | "place_road"
            | "place_blueprint"
            | "found_settlement"
            | "settlement_invite"
            | "settlement_set_role"
            | "settlement_remove_member"
            | "settlement_claim"
            | "settlement_rename"
            | "settlement_transfer" => Self::Mayor,
            // Time control is admin-only in D-021; unknown tags fail closed.
            // Settlement release is the Q-113 admin override.
            "time_control" | "settlement_release" => Self::Admin,
            _ => Self::Admin,
        }
    }

    /// Minimum role for a concrete command. `time_control` (pause, resume,
    /// step) is Admin per D-021; a vote cancel is Admin, while call/yes/no
    /// stay open to every role.
    #[must_use]
    pub fn required_role_for(command: &ClientCommand) -> Self {
        match command {
            ClientCommand::TimeVote {
                action: crate::protocol::VoteAction::Cancel,
                ..
            } => Self::Admin,
            other => Self::required_role(other.tag()),
        }
    }

    /// Whether this role may issue a command that requires `required`.
    #[must_use]
    pub fn allows(self, required: Self) -> bool {
        self >= required
    }
}

/// Deterministic token bucket for one session.
///
/// Virtual time is measured in milliseconds and comes from the authoritative
/// tick plus a monotonic command counter, never from `Instant`. One credit is
/// [`RATE_CREDIT_MILLI`] milli-credits and every command costs one credit.
#[derive(Debug, Clone)]
struct RateBucket {
    milli_credits: u64,
    clock_ms: u64,
}

impl RateBucket {
    fn full() -> Self {
        Self {
            milli_credits: RATE_BURST_CREDITS * RATE_CREDIT_MILLI,
            clock_ms: 0,
        }
    }

    /// Credits available at `now_ms`, including unapplied replenishment.
    fn credits_milli_at(&self, now_ms: u64) -> u64 {
        let gained = now_ms
            .saturating_sub(self.clock_ms)
            .saturating_mul(RATE_MILLI_CREDITS_PER_MS);
        self.milli_credits
            .saturating_add(gained)
            .min(RATE_BURST_CREDITS * RATE_CREDIT_MILLI)
    }

    /// Apply replenishment up to `now_ms` and remember the new clock.
    fn refill(&mut self, now_ms: u64) {
        if now_ms > self.clock_ms {
            self.milli_credits = self.credits_milli_at(now_ms);
            self.clock_ms = now_ms;
        }
    }

    /// Spend one credit at `now_ms`. Returns false when the bucket is empty;
    /// a failed spend never consumes a partial credit.
    fn try_consume(&mut self, now_ms: u64) -> bool {
        self.refill(now_ms);
        if self.milli_credits >= RATE_CREDIT_MILLI {
            self.milli_credits -= RATE_CREDIT_MILLI;
            true
        } else {
            false
        }
    }
}

/// One entry in the bounded per-session audit ring.
///
/// Accepted commands use [`AUDIT_ACCEPTED`] as their code. The ring is
/// deliberately excluded from [`ServerWorld::checksum`] so the snapshot golden
/// stays a pure function of simulation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditEntry {
    /// Authoritative tick after the command was evaluated.
    pub tick: u64,
    /// Role the session held when the command arrived.
    pub role: ClientRole,
    /// Command tag, for example `place_road`.
    pub tag: &'static str,
    /// Rejection code, or [`AUDIT_ACCEPTED`].
    pub code: &'static str,
}

impl AuditEntry {
    /// Whether the command was accepted.
    #[must_use]
    pub fn accepted(self) -> bool {
        self.code == AUDIT_ACCEPTED
    }
}

fn audit_entry_hash(mut hash: u64, entry: &AuditEntry) -> u64 {
    hash = fnv_update(hash, &entry.tick.to_le_bytes());
    hash = fnv_update(hash, entry.role.as_str().as_bytes());
    hash = fnv_update(hash, &[0x1f]);
    hash = fnv_update(hash, entry.tag.as_bytes());
    hash = fnv_update(hash, &[0x1f]);
    hash = fnv_update(hash, entry.code.as_bytes());
    fnv_update(hash, &[0x1e])
}

/// In-memory reconnect record for a disconnected session.
///
/// The record is deliberately excluded from [`ServerWorld::checksum`] and is
/// not written to [`crate::save::ServerSave`]. Reconnect is process-local; a
/// loaded world always starts with no resumable session.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ResumeWindow {
    /// Last authoritative tick at which `resume` is still accepted.
    deadline_tick: u64,
    /// Player name from the disconnected session, restored on resume.
    player: Option<String>,
    /// Fresh `hello` attempts rejected while this window was open. After a
    /// small bounded number the window is abandoned so a client that lost its
    /// token can never be blocked forever.
    hello_attempts: u32,
}

impl ResumeWindow {
    fn starting_at(tick: u64, player: Option<String>) -> Self {
        Self {
            deadline_tick: tick.saturating_add(RESUME_GRACE_TICKS),
            player,
            hello_attempts: 0,
        }
    }

    fn is_open_at(&self, tick: u64) -> bool {
        tick <= self.deadline_tick
    }
}

/// Per-connection session state for a shared [`ServerWorld`].
///
/// Simulation state (tick, pause, economy, roads, vote, and the online roster)
/// stays on the world and is shared by every connection. Everything a client
/// owns lives here: handshake and close flags, player name, role, reconnect
/// window, interest viewport, deterministic rate bucket, audit ring, last-seen
/// tick, and the last checksum the session observed.
///
/// A connection id is a process-local `u64`. Connection `1` keeps the
/// seed-derived session id so a single-connection world (and the legacy
/// [`ServerWorld::apply`] path) is byte-identical to previous releases; later
/// connections append `-<id>` to the token. The token still identifies a
/// session, not a player, and is **not** authentication.
#[derive(Debug, Clone)]
struct SessionState {
    hello: bool,
    player: Option<String>,
    player_id: Option<PlayerId>,
    role: ClientRole,
    closed: bool,
    resume_window: Option<ResumeWindow>,
    session_id: String,
    interest_center: [f32; 2],
    interest_radius_m: f32,
    rate: RateBucket,
    audit: VecDeque<AuditEntry>,
    last_seen_tick: u64,
    session_checksum: String,
    commands: u64,
}

impl SessionState {
    fn new(session_id: String, role: ClientRole, tick: u64) -> Self {
        Self {
            hello: false,
            player: None,
            player_id: None,
            role,
            closed: false,
            resume_window: None,
            session_id,
            interest_center: [0.0, 0.0],
            interest_radius_m: DEFAULT_INTEREST_RADIUS_M as f32,
            rate: RateBucket::full(),
            audit: VecDeque::with_capacity(AUDIT_CAPACITY),
            last_seen_tick: tick,
            session_checksum: String::new(),
            commands: 0,
        }
    }
}

/// Session identifier for a connection in a shared world.
///
/// Connection `1` uses [`session_id_for_seed`] unchanged so existing loopback
/// transcripts and reconnect tokens stay pinned. Every later connection gets a
/// deterministic suffix, so two online players never share a resume token.
fn session_id_for_connection(seed: u64, connection_id: u64) -> String {
    let base = session_id_for_seed(seed);
    if connection_id <= 1 {
        base
    } else {
        format!("{base}-{connection_id}")
    }
}

/// Authoritative state for one protocol v0 server process.
///
/// Simulation fields are shared by every connection. Per-connection fields
/// belong to the active [`SessionState`] slot; use [`Self::apply_session`] for
/// a multi-connection harness and [`Self::apply`] for the legacy
/// single-session path.
#[derive(Debug, Clone)]
pub struct ServerWorld {
    /// World seed; also seeds the treasury.
    pub seed: u64,
    /// Authoritative command/tick counter.
    pub tick: u64,
    /// Static population used by the economy tick.
    pub population: u32,
    /// Static employed-citizen count used by the economy tick.
    pub employed: u32,
    /// Static completed-building count used by the economy tick.
    pub buildings: u32,
    /// City treasury.
    pub economy: Economy,
    /// Road network.
    pub roads: RoadStore,
    /// Authoritative blueprint placements (D-032).
    ///
    /// `None` until the first accepted `place_blueprint` or a save/load that
    /// carries placements, so existing worlds and their checksums are
    /// untouched.
    pub placements: Option<PlacementStore>,
    /// Authoritative construction sites (D-023/D-028).
    ///
    /// `None` until the first accepted placement or a save/load that carries a
    /// construction store.
    pub construction: Option<ConstructionStore>,
    /// Save-local player address book (D-033).
    ///
    /// The registry maps stable [`PlayerId`] values to mutable display names.
    /// It starts empty and is excluded from the checksum until a name is
    /// registered, so worlds without players keep their previous goldens.
    pub players: PlayerRegistry,
    /// Optional settlement layer (D-019/D-033).
    ///
    /// `None` until a save/load carries a settlement store; a `Some` store is
    /// hashed even when it holds no settlements.
    pub settlements: Option<SettlementStore>,
    /// Pre-seeded content-addressed package library (server config, not save
    /// data). `None` makes `place_blueprint` reject with
    /// `PLACEMENT_LIBRARY_UNAVAILABLE`.
    pub blueprint_library: Option<BlueprintLibrary>,
    /// District policy registry used by the placement gate (server config, not
    /// save data). A foreign-seed registry is never installed.
    pub policies: Option<DistrictPolicyStore>,
    /// Optional pinned content-registry hash (server config, not save data).
    ///
    /// When set, every `hello` must carry this exact 64-character lowercase
    /// hex hash or it is rejected with `content_hash.mismatch`, before any
    /// session state is mutated.
    pub content_hash: Option<String>,
    /// Per-session interest viewport centre `[x, z]` in metres.
    ///
    /// View state only: excluded from [`Self::checksum`] and not saved.
    interest_center: [f32; 2],
    /// Per-session interest radius in metres (view state only).
    interest_radius_m: f32,
    /// Whether `advance_day` is frozen.
    pub paused: bool,
    /// Deterministic time-control vote state (D-021).
    pub vote: crate::vote::TimeVote,
    /// Players currently online in this process.
    online: BTreeSet<String>,
    /// Hello player name for the session.
    player: Option<String>,
    /// Stable player identity for the session, if one has been assigned.
    player_id: Option<PlayerId>,
    /// Checksum of the last accepted state, refreshed after each mutation.
    pub session_checksum: String,
    role: ClientRole,
    /// Role new connections receive from the server process.
    ///
    /// [`Self::set_role`] updates both this default and the active session.
    default_role: ClientRole,
    /// Monotonic count of applied commands, including rejections.
    commands: u64,
    rate: RateBucket,
    audit: VecDeque<AuditEntry>,
    hello: bool,
    closed: bool,
    resume_window: Option<ResumeWindow>,
    last_seen_tick: u64,
    session_id: String,
    /// Parked per-connection state, keyed by connection id.
    ///
    /// The active connection's state lives in the fields above; it is parked
    /// here when another connection is activated. Excluded from the world
    /// checksum and from [`crate::save::ServerSave`].
    sessions: BTreeMap<u64, SessionState>,
    /// Connection id currently installed in the per-session fields.
    ///
    /// `None` is the legacy default slot used by [`Self::apply`].
    active_connection: Option<u64>,
    /// Last connection id issued by [`Self::open_connection`].
    next_connection: u64,
}

impl ServerWorld {
    /// Create a session world from a seed.
    ///
    /// The default role is [`ClientRole::Viewer`]; use [`Self::with_role`] for
    /// a dev harness that needs more permissions.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self::with_population(seed, SessionConfig::for_seed(seed).population)
    }

    /// Create a session world with an explicit population and the default
    /// dev-session role (mayor). Use [`Self::with_population_and_role`] for
    /// viewer/admin sessions.
    ///
    /// `employed` is 60% of the population and `buildings` is one per 25
    /// citizens (at least one), both derived deterministically. The treasury
    /// starts at [`STARTING_BALANCE`].
    #[must_use]
    pub fn with_population(seed: u64, population: u32) -> Self {
        Self::with_population_and_role(seed, population, ClientRole::Mayor)
    }

    /// Create a session world with an explicit role and the seed population.
    #[must_use]
    pub fn with_role(seed: u64, role: ClientRole) -> Self {
        Self::with_population_and_role(seed, SessionConfig::for_seed(seed).population, role)
    }

    /// Create a session world with an explicit population and role.
    #[must_use]
    pub fn with_population_and_role(seed: u64, population: u32, role: ClientRole) -> Self {
        let population = population.max(1);
        let employed = population.saturating_mul(3) / 5;
        let buildings = (population / 25).max(1);
        let mut world = Self {
            seed,
            tick: 0,
            population,
            employed,
            buildings,
            economy: Economy::new(seed),
            roads: RoadStore::new(seed),
            placements: None,
            construction: None,
            players: PlayerRegistry::new(seed),
            settlements: None,
            blueprint_library: None,
            policies: None,
            content_hash: None,
            interest_center: [0.0, 0.0],
            interest_radius_m: DEFAULT_INTEREST_RADIUS_M as f32,
            paused: false,
            vote: crate::vote::TimeVote::default(),
            online: BTreeSet::new(),
            player: None,
            player_id: None,
            session_checksum: String::new(),
            role,
            default_role: role,
            commands: 0,
            rate: RateBucket::full(),
            audit: VecDeque::with_capacity(AUDIT_CAPACITY),
            hello: false,
            closed: false,
            resume_window: None,
            last_seen_tick: 0,
            session_id: session_id_for_seed(seed),
            sessions: BTreeMap::new(),
            active_connection: None,
            next_connection: 0,
        };
        world.refresh_checksum();
        world
    }

    /// Dev-only role for this session. Not authentication.
    #[must_use]
    pub fn role(&self) -> ClientRole {
        self.role
    }

    /// Applied commands, including rejected ones.
    #[must_use]
    pub fn commands(&self) -> u64 {
        self.commands
    }
    /// Override the dev-session role. Not authentication.
    ///
    /// Sets the role of the active session and the default for connections
    /// opened later. Parked sessions keep the role they were assigned.
    pub fn set_role(&mut self, role: ClientRole) {
        self.role = role;
        self.default_role = role;
    }

    /// Pin the content-registry hash required from every `hello`.
    ///
    /// `None` clears the pin. The hash must be 64 lowercase hexadecimal
    /// characters; validation failure returns the same `content_hash.invalid`
    /// rejection the protocol uses. This is process configuration, not save
    /// data.
    ///
    /// # Errors
    ///
    /// Returns the stable `content_hash.invalid` rejection for a malformed
    /// hash.
    pub fn set_content_hash(&mut self, hash: Option<&str>) -> Result<(), Rejection> {
        self.content_hash = match hash {
            None => None,
            Some(value) => {
                crate::protocol::validate_content_hash(value)?;
                Some(value.to_string())
            }
        };
        Ok(())
    }

    /// Configure the pre-seeded content-addressed blueprint library.
    ///
    /// The root must be an absolute, existing directory without `.` or `..`
    /// components; [`BlueprintLibrary::new`] performs the validation. This is
    /// process configuration: it is not part of [`crate::save::ServerSave`] and
    /// a loaded world starts without a library.
    ///
    /// # Errors
    ///
    /// Returns the library root/open error; a failed call leaves the previous
    /// library untouched.
    pub fn set_blueprint_library(&mut self, root: PathBuf) -> Result<(), BlueprintLibraryError> {
        let library = BlueprintLibrary::new(root)?;
        self.blueprint_library = Some(library);
        Ok(())
    }

    /// Drop the configured blueprint library.
    ///
    /// `place_blueprint` then rejects with `PLACEMENT_LIBRARY_UNAVAILABLE`.
    /// Existing placements are untouched.
    pub fn clear_blueprint_library(&mut self) {
        self.blueprint_library = None;
    }

    /// Configure the district policy registry used by the placement gate.
    ///
    /// The registry is validated and must belong to this world's seed; a
    /// foreign-seed or invalid registry is rejected without replacing the
    /// current one. Like the library, it is process configuration and is not
    /// saved or restored.
    ///
    /// # Errors
    ///
    /// Returns [`DistrictPolicyError::SeedMismatch`] for a foreign-seed
    /// registry or the registry validation error.
    pub fn set_policy_registry(
        &mut self,
        store: DistrictPolicyStore,
    ) -> Result<(), DistrictPolicyError> {
        store.validate()?;
        if store.seed != self.seed {
            return Err(DistrictPolicyError::SeedMismatch {
                expected: self.seed,
                found: store.seed,
            });
        }
        self.policies = Some(store);
        Ok(())
    }

    /// Drop the configured policy registry; placement policy is then neutral.
    pub fn clear_policy_registry(&mut self) {
        self.policies = None;
    }

    /// Role of a connection's session. Not authentication.
    ///
    /// Activates the session to read it, which is also how a harness can seed a
    /// role before the connection's first command.
    pub fn session_role(&mut self, connection_id: u64) -> ClientRole {
        self.activate_session(connection_id);
        self.role
    }

    /// Override one connection's dev-session role. Not authentication.
    pub fn set_session_role(&mut self, connection_id: u64, role: ClientRole) {
        self.activate_session(connection_id);
        self.role = role;
    }

    /// Whether a connection's session has been closed by `disconnect`.
    pub fn session_closed(&mut self, connection_id: u64) -> bool {
        self.activate_session(connection_id);
        self.closed
    }

    /// Rebuild a world from an already validated [`crate::save::ServerSave`].
    ///
    /// The restored session is open, has not accepted a `hello`, and has **no
    /// resumable session**: reconnect state (the grace window and
    /// `last_seen_tick`) is process-local and is not part of the save. The
    /// session token is derived from the seed, so a client that knew it before
    /// the restart still needs a fresh `hello`; `resume` answers
    /// `resume.unknown` until a new session disconnects. Saved tick, paused
    /// flag, treasury, and roads resume unchanged; the checksum is recomputed
    /// from the restored state.
    pub(crate) fn restore(save: crate::save::ServerSave) -> Self {
        let players = match save.players {
            Some(players) => players,
            None => PlayerRegistry::new(save.seed),
        };
        let mut world = Self {
            seed: save.seed,
            tick: save.tick,
            population: save.population,
            employed: save.employed,
            buildings: save.buildings,
            economy: save.economy,
            roads: save.roads,
            placements: save.placements,
            construction: save.construction,
            players,
            settlements: save.settlements,
            // The package library and policy registry are process
            // configuration, not save data: a loaded world starts without
            // them and the caller reconfigures placement before use.
            blueprint_library: None,
            policies: None,
            content_hash: None,
            interest_center: [0.0, 0.0],
            interest_radius_m: DEFAULT_INTEREST_RADIUS_M as f32,
            paused: save.paused,
            vote: crate::vote::TimeVote::default(),
            online: BTreeSet::new(),
            player: None,
            player_id: None,
            session_checksum: String::new(),
            role: ClientRole::Mayor,
            default_role: ClientRole::Mayor,
            commands: 0,
            rate: RateBucket::full(),
            audit: VecDeque::with_capacity(AUDIT_CAPACITY),
            hello: false,
            closed: false,
            resume_window: None,
            last_seen_tick: save.tick,
            session_id: session_id_for_seed(save.seed),
            sessions: BTreeMap::new(),
            active_connection: None,
            next_connection: 0,
        };
        world.refresh_checksum();
        world
    }

    /// Deterministic session identifier.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Stable reconnect token returned in `welcome`.
    ///
    /// Connection `1` derives it from the seed and keeps it equal to
    /// [`Self::session_id`]; later connections append their connection id so
    /// two players never share a token. It identifies a session; it is **not**
    /// an authentication credential.
    #[must_use]
    pub fn session_token(&self) -> &str {
        &self.session_id
    }

    /// Tick at which the current reconnect window closes, if one is open.
    #[must_use]
    pub fn resume_deadline(&self) -> Option<u64> {
        self.resume_window
            .as_ref()
            .map(|window| window.deadline_tick)
    }

    /// Whether a disconnected session can still be resumed at the current tick.
    #[must_use]
    pub fn is_resumable(&self) -> bool {
        self.resume_window
            .as_ref()
            .is_some_and(|window| window.is_open_at(self.tick))
    }

    /// Last authoritative tick at which the session was seen.
    #[must_use]
    pub fn last_seen_tick(&self) -> u64 {
        self.last_seen_tick
    }

    /// Prepare the world for a newly accepted connection.
    ///
    /// A clean `disconnect` already closed the previous session and opened the
    /// grace window. A connection that dropped without `disconnect` (EOF, read
    /// timeout, reset) is closed here and also becomes resumable. Either way
    /// the grace window survives: only [`Self::tick`] advancing past its
    /// deadline expires it.
    pub fn begin_connection(&mut self) {
        if self.hello {
            self.remember_disconnect();
        }
        self.hello = false;
        self.closed = false;
    }

    /// Issue a connection id and install a fresh per-connection session.
    ///
    /// This is the multi-connection counterpart of [`Self::begin_connection`].
    /// The first id is `1` and keeps the seed-derived session token; later ids
    /// get a deterministic suffix. Any session left in the active slot is
    /// parked in the session table first.
    pub fn open_connection(&mut self) -> u64 {
        self.next_connection = self.next_connection.saturating_add(1);
        let connection_id = self.next_connection;
        self.activate_session(connection_id);
        connection_id
    }

    /// Apply a command for one connection and return every response.
    ///
    /// The shared simulation (tick, pause, economy, roads, vote, online roster)
    /// is common to all connections. The handshake, role, interest viewport,
    /// rate bucket, audit ring, last-seen tick, and checksum are swapped in for
    /// this connection and saved back when another connection runs.
    pub fn apply_session(
        &mut self,
        connection_id: u64,
        command: &ClientCommand,
    ) -> Vec<ServerMessage> {
        self.activate_session(connection_id);
        self.apply_all(command)
    }

    /// Finish a connection, remembering a disconnect for a later resume.
    ///
    /// A clean `disconnect` already closed the session and opened its window;
    /// this handles EOF, reset, timeout, and write failures too. Other
    /// connections keep their sessions, online roster entries, and votes.
    pub fn close_connection(&mut self, connection_id: u64) {
        self.activate_session(connection_id);
        if self.hello {
            self.remember_disconnect();
        }
        self.hello = false;
        self.closed = false;
    }

    /// Install the state for `connection_id`, parking the previous session.
    ///
    /// The legacy [`Self::apply`] path never calls this function, so it keeps
    /// operating on the untouched default slot (`active_connection == None`).
    fn activate_session(&mut self, connection_id: u64) {
        if self.active_connection == Some(connection_id) {
            return;
        }
        if let Some(previous) = self.active_connection.take() {
            let state = self.take_session();
            self.sessions.insert(previous, state);
        }
        let state = self.sessions.remove(&connection_id).unwrap_or_else(|| {
            SessionState::new(
                session_id_for_connection(self.seed, connection_id),
                self.default_role,
                self.tick,
            )
        });
        self.install_session(state);
        self.active_connection = Some(connection_id);
    }

    /// Move the active per-session fields into a [`SessionState`].
    ///
    /// Shared fields are left untouched; the per-session fields are reset to
    /// their defaults and immediately replaced by [`Self::install_session`].
    fn take_session(&mut self) -> SessionState {
        SessionState {
            hello: self.hello,
            player: self.player.take(),
            player_id: self.player_id,
            role: self.role,
            closed: self.closed,
            resume_window: self.resume_window.take(),
            session_id: std::mem::take(&mut self.session_id),
            interest_center: self.interest_center,
            interest_radius_m: self.interest_radius_m,
            rate: std::mem::replace(&mut self.rate, RateBucket::full()),
            audit: std::mem::take(&mut self.audit),
            last_seen_tick: self.last_seen_tick,
            session_checksum: std::mem::take(&mut self.session_checksum),
            commands: self.commands,
        }
    }

    /// Restore per-session fields from a [`SessionState`].
    fn install_session(&mut self, state: SessionState) {
        self.hello = state.hello;
        self.player = state.player;
        self.player_id = state.player_id;
        self.role = state.role;
        self.closed = state.closed;
        self.resume_window = state.resume_window;
        self.session_id = state.session_id;
        self.interest_center = state.interest_center;
        self.interest_radius_m = state.interest_radius_m;
        self.rate = state.rate;
        self.audit = state.audit;
        self.last_seen_tick = state.last_seen_tick;
        self.session_checksum = state.session_checksum;
        self.commands = state.commands;
    }

    /// Connection id whose session carries `token`, looking only at parked
    /// sessions. The active session is matched through [`Self::session_token`].
    fn parked_session_by_token(&self, token: &str) -> Option<u64> {
        self.sessions
            .iter()
            .find(|(_, state)| state.session_id == token)
            .map(|(connection_id, _)| *connection_id)
    }

    fn remember_disconnect(&mut self) {
        let player = self.player.take();
        if let Some(name) = &player {
            self.online.remove(name);
        }
        self.last_seen_tick = self.tick;
        self.resume_window = Some(ResumeWindow::starting_at(self.tick, player));
    }

    /// Whether an accepted `hello` has been seen.
    #[must_use]
    pub fn hello_accepted(&self) -> bool {
        self.hello
    }

    /// Stable identity assigned to the active session, if any.
    ///
    /// The id is issued on the first successful `hello`/`resume` whose display
    /// name is registered (or newly registered) in [`Self::players`]. It is a
    /// per-save address-book id, not an authentication credential.
    #[must_use]
    pub fn player_id(&self) -> Option<PlayerId> {
        self.player_id
    }

    /// Resolve a display name to a registry id, registering it on first use.
    ///
    /// Names match case-insensitively. A malformed registry (more than one
    /// match) or a registration failure rejects with
    /// [`codes::PLAYER_IDENTITY`] instead of panicking or picking an
    /// ambiguous identity.
    fn resolve_player_id(&mut self, display_name: &str) -> Result<PlayerId, &'static str> {
        let matches = self.players.find_by_display_name(display_name);
        if matches.len() == 1 {
            return Ok(matches[0].id);
        }
        if !matches.is_empty() {
            return Err(codes::PLAYER_IDENTITY);
        }
        self.players
            .register(display_name)
            .map(|player| player.id)
            .map_err(|_| codes::PLAYER_IDENTITY)
    }

    /// Whether the client disconnected.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Whole credits currently available in the command bucket.
    #[must_use]
    pub fn rate_credits(&self) -> u64 {
        self.rate.credits_milli_at(self.rate_clock_ms()) / RATE_CREDIT_MILLI
    }

    /// Audit entries, oldest first.
    #[must_use]
    pub fn audit_state(&self) -> Vec<AuditEntry> {
        self.audit.iter().copied().collect()
    }

    /// Number of retained audit entries (capped at [`AUDIT_CAPACITY`]).
    #[must_use]
    pub fn audit_len(&self) -> usize {
        self.audit.len()
    }

    /// FNV-1a checksum over the retained audit ring.
    ///
    /// This is intentionally separate from [`Self::checksum`]: auditing never
    /// changes the world snapshot golden.
    #[must_use]
    pub fn audit_checksum(&self) -> String {
        format!(
            "{:016x}",
            self.audit.iter().fold(FNV_OFFSET, audit_entry_hash)
        )
    }

    /// Virtual milliseconds elapsed for the token bucket.
    fn rate_clock_ms(&self) -> u64 {
        self.tick
            .saturating_mul(RATE_TICK_MS)
            .saturating_add(self.commands.saturating_mul(RATE_COMMAND_MS))
    }

    fn record_audit(&mut self, tag: &'static str, code: &'static str) {
        if self.audit.len() >= AUDIT_CAPACITY {
            self.audit.pop_front();
        }
        self.audit.push_back(AuditEntry {
            tick: self.tick,
            role: self.role,
            tag,
            code,
        });
    }

    /// Treasury balance rounded to whole cost units for the snapshot.
    #[must_use]
    pub fn balance_units(&self) -> i64 {
        self.economy.balance.round() as i64
    }

    /// Interest viewport centre `[x, z]` in metres (view state only).
    #[must_use]
    pub fn interest_center(&self) -> [f32; 2] {
        self.interest_center
    }

    /// Interest viewport radius in metres (view state only).
    #[must_use]
    pub fn interest_radius_m(&self) -> f32 {
        self.interest_radius_m
    }

    /// Roads whose midpoint is inside the interest circle.
    ///
    /// The circle is inclusive (`distance <= radius`). Roads are counted, not
    /// streamed: v0 has no per-entity payload. The count depends only on the
    /// authoritative road store and the session viewport, so it is
    /// deterministic and independent of iteration order.
    #[must_use]
    pub fn visible_roads(&self) -> u32 {
        let count = self
            .roads
            .roads
            .iter()
            .filter(|road| road_in_interest(road, self.interest_center, self.interest_radius_m))
            .count();
        u32::try_from(count).unwrap_or(u32::MAX)
    }

    /// Bounded road details for the current viewport, ascending by id.
    ///
    /// The filter is identical to [`Self::visible_roads`]: a road is included
    /// when its midpoint satisfies `distance <= radius`. The list is capped at
    /// [`MAX_INTEREST_ROADS`]; use [`Self::interest_view`] to learn whether
    /// more roads matched.
    #[must_use]
    pub fn interest_roads(&self) -> Vec<RoadView> {
        let mut roads = self.interest_matches();
        roads.truncate(MAX_INTEREST_ROADS);
        roads
    }

    /// Bounded building details for the current viewport, ascending by id.
    ///
    /// When the world has an authoritative placement store with at least one
    /// record, the view is placement-backed: every placement is a candidate
    /// (v1 does not model completion, so a construction site is included), its
    /// centre is `[center.x, center.z]`, and its radius is half the larger
    /// rotated footprint side as a render hint. Records are filtered by the
    /// same inclusive interest circle and capped at [`MAX_INTEREST_BUILDINGS`].
    ///
    /// A world with no store, or an empty one, keeps the deterministic
    /// synthetic fixture used before D-032: [`synthetic_building`] on a fixed
    /// 64 m grid anchored at the origin, 16 per row on `z = 0`, then the next
    /// row, and so on. The approximation is stable per id but it is **not** a
    /// footprint of where the city's buildings really are.
    #[must_use]
    pub fn interest_buildings(&self) -> Vec<BuildingView> {
        let (mut buildings, _truncated) = self.interest_building_matches();
        buildings.truncate(MAX_INTEREST_BUILDINGS);
        buildings
    }

    /// Interest viewport plus the bounded road and building detail lists.
    ///
    /// `truncated` and `buildings_truncated` are true when more roads or
    /// buildings matched than the corresponding list carries. The view is a
    /// read-only copy of authoritative/view-state data: building it mutates
    /// nothing and never advances the tick.
    #[must_use]
    pub fn interest_view(&self) -> ServerMessage {
        let mut roads = self.interest_matches();
        let truncated = roads.len() > MAX_INTEREST_ROADS;
        roads.truncate(MAX_INTEREST_ROADS);
        let (mut buildings, buildings_truncated) = self.interest_building_matches();
        buildings.truncate(MAX_INTEREST_BUILDINGS);
        ServerMessage::InterestView {
            tick: self.tick,
            center: self.interest_center,
            radius_m: self.interest_radius_m,
            roads,
            truncated,
            buildings,
            buildings_truncated,
        }
    }

    /// Building matches for the inclusive interest circle, at most
    /// [`MAX_INTEREST_BUILDINGS`] entries plus a truncation flag.
    ///
    /// A non-empty placement store is authoritative and takes precedence over
    /// the synthetic fixture. An empty store is treated exactly like no store,
    /// so a world that never placed a blueprint keeps its previous view.
    fn interest_building_matches(&self) -> (Vec<BuildingView>, bool) {
        if let Some(placements) = self
            .placements
            .as_ref()
            .filter(|store| !store.placements.is_empty())
        {
            return self.placement_building_matches(&placements.placements);
        }
        self.synthetic_building_matches()
    }

    /// Placement-backed matches, ascending by id and capped with a truncation
    /// flag.
    ///
    /// A `BTreeMap` keeps only the lowest [`MAX_INTEREST_BUILDINGS`] ids, so an
    /// oversized loaded store cannot inflate one viewport query. Iteration is
    /// already in ascending id order, so the result never sorts.
    fn placement_building_matches(&self, placements: &[Placement]) -> (Vec<BuildingView>, bool) {
        let mut matches: BTreeMap<u64, usize> = BTreeMap::new();
        let mut truncated = false;
        for (index, placement) in placements.iter().enumerate() {
            if !point_in_interest(
                [placement.center[0], placement.center[2]],
                self.interest_center,
                self.interest_radius_m,
            ) {
                continue;
            }
            matches.insert(placement.id, index);
            if matches.len() > MAX_INTEREST_BUILDINGS {
                matches.pop_last();
                truncated = true;
            }
        }
        let buildings = matches
            .values()
            .map(|&index| placement_building(&placements[index]))
            .collect();
        (buildings, truncated)
    }

    /// Synthetic matches for the inclusive interest circle, at most
    /// [`MAX_INTEREST_BUILDINGS`] + 1 entries so truncation is observable.
    ///
    /// Candidate grid cells are derived from the circle bounds instead of
    /// iterating every possible id: an oversized loaded building count must not
    /// turn one viewport query into millions of synthesized records.
    fn synthetic_building_matches(&self) -> (Vec<BuildingView>, bool) {
        let mut buildings = Vec::new();
        let Some((row_min, row_max, column_min, column_max)) = self.building_grid_bounds() else {
            return (buildings, false);
        };

        for row in row_min..=row_max {
            for column in column_min..=column_max {
                let id = row
                    .saturating_mul(INTEREST_BUILDING_COLUMNS)
                    .saturating_add(column)
                    .saturating_add(1);
                if id > u64::from(self.buildings) {
                    continue;
                }
                let building = synthetic_building(id);
                if !point_in_interest(
                    building.center,
                    self.interest_center,
                    self.interest_radius_m,
                ) {
                    continue;
                }
                buildings.push(building);
                if buildings.len() > MAX_INTEREST_BUILDINGS {
                    return (buildings, true);
                }
            }
        }
        (buildings, false)
    }

    /// Grid rows and columns whose synthetic centres can fall inside the circle.
    ///
    /// Bounds are inclusive, clamped to the 16-column grid and to ids the
    /// static building count actually contains. Ascending row/column order
    /// matches ascending id order, so iteration never sorts.
    fn building_grid_bounds(&self) -> Option<(u64, u64, u64, u64)> {
        let buildings = u64::from(self.buildings);
        if buildings == 0 {
            return None;
        }

        let spacing = f64::from(INTEREST_BUILDING_SPACING_M);
        let center_x = f64::from(self.interest_center[0]);
        let center_z = f64::from(self.interest_center[1]);
        let radius = f64::from(self.interest_radius_m);

        let max_row = (buildings - 1) / INTEREST_BUILDING_COLUMNS;
        let (column_min, column_max) = grid_axis_bounds(
            center_x - radius,
            center_x + radius,
            spacing,
            INTEREST_BUILDING_COLUMNS - 1,
        )?;
        let (row_min, row_max) =
            grid_axis_bounds(center_z - radius, center_z + radius, spacing, max_row)?;
        Some((row_min, row_max, column_min, column_max))
    }

    /// Every road whose midpoint is inside the inclusive interest circle,
    /// ordered by ascending id.
    ///
    /// Sorting explicitly keeps the wire order stable even if a loaded save
    /// stores segments out of id order.
    fn interest_matches(&self) -> Vec<RoadView> {
        let mut roads: Vec<RoadView> = self
            .roads
            .roads
            .iter()
            .filter(|road| road_in_interest(road, self.interest_center, self.interest_radius_m))
            .map(|road| RoadView {
                id: road.id,
                start: road.start,
                end: road.end,
                width_m: road.width_m,
            })
            .collect();
        roads.sort_unstable_by_key(|road| road.id);
        roads
    }

    /// Stable FNV-1a checksum over tick, population, balance, road checksum,
    /// economy checksum, and — only when present — the placement, construction,
    /// player, and settlement checksums. No wall clock or seed is mixed in.
    ///
    /// A world without placements, construction, players, and settlements
    /// hashes exactly as it did before D-032; the optional stores are appended
    /// only when non-empty (or `Some` for settlements), so old pins stay put
    /// until the first placement or player.
    ///
    /// The interest viewport is deliberately excluded: it is per-session view
    /// state, so a client changing its camera must not move the world golden.
    #[must_use]
    pub fn checksum(&self) -> String {
        let mut hash = fnv_update(FNV_OFFSET, &self.tick.to_le_bytes());
        hash = fnv_update(hash, &self.population.to_le_bytes());
        hash = fnv_update(hash, &self.balance_units().to_le_bytes());
        hash = fnv_update(hash, &self.roads.checksum().to_le_bytes());
        hash = fnv_update(hash, &self.economy.checksum().to_le_bytes());
        if let Some(placements) = &self.placements {
            hash = fnv_update(hash, &placements.checksum().to_le_bytes());
        }
        if let Some(construction) = &self.construction {
            hash = fnv_update(hash, &construction.checksum().to_le_bytes());
        }
        // The player registry is a session/identity sidecar, not world
        // gameplay state. Keep it out of the world checksum so existing
        // protocol and loopback goldens do not move when a name is registered;
        // membership changes reach the checksum through `settlements` only.
        if let Some(settlements) = &self.settlements {
            hash = fnv_update(hash, &settlements.checksum().to_le_bytes());
        }
        format!("{hash:016x}")
    }

    /// Current authoritative snapshot.
    ///
    /// Interest fields describe this session's viewport; they are not part of
    /// the checksum and do not imply that entity or terrain streaming exists.
    #[must_use]
    pub fn snapshot(&self) -> ServerMessage {
        ServerMessage::Snapshot {
            tick: self.tick,
            population: self.population,
            balance: self.balance_units(),
            checksum: self.checksum(),
            interest_center: self.interest_center,
            interest_radius_m: self.interest_radius_m,
            visible_roads: self.visible_roads(),
        }
    }

    /// Validate and apply one command on the default session, returning exactly
    /// one response.
    ///
    /// This is the single-session convenience wrapper used by tests and the
    /// single-connection loopbacks. It operates on the default session slot; a
    /// multi-connection harness must use [`Self::apply_session`] instead. A
    /// successful `resume` produces two messages (`welcome`, then `snapshot`);
    /// a successful `set_interest` produces `snapshot`, then `interest_view`.
    /// This method returns the first message and drops the rest. Transport code
    /// must use [`Self::apply_all`] or [`Self::apply_session`].
    ///
    /// The batch order is fixed:
    ///
    /// 1. closed sessions reject everything;
    /// 2. `hello`/`resume` sequencing (`hello.duplicate`, `hello.resumable`,
    ///    `resume.duplicate`, then `not_hello`);
    /// 3. role authorization ([`codes::FORBIDDEN`]);
    /// 4. the deterministic token bucket ([`codes::RATE_LIMITED`]);
    /// 5. payload validation and mutation.
    ///
    /// A rejection never mutates simulation state. Every command, accepted or
    /// rejected, is recorded in the bounded audit ring.
    pub fn apply(&mut self, command: &ClientCommand) -> ServerMessage {
        self.apply_all(command)
            .into_iter()
            .next()
            .expect("every command yields at least one response")
    }

    /// Validate and apply one command, returning every response it produces.
    ///
    /// Most commands yield one message. A successful `resume` yields
    /// `welcome { resumed: true }` followed by the current `snapshot`, and a
    /// successful `set_interest` yields `snapshot` followed by the filtered
    /// `interest_view`. Rejections never mutate simulation state.
    pub fn apply_all(&mut self, command: &ClientCommand) -> Vec<ServerMessage> {
        self.commands = self.commands.saturating_add(1);
        let (messages, code) = self.evaluate(command);
        self.record_audit(command.tag(), code);
        if self.hello && code == AUDIT_ACCEPTED {
            self.last_seen_tick = self.tick;
        }
        messages
    }

    fn evaluate(&mut self, command: &ClientCommand) -> (Vec<ServerMessage>, &'static str) {
        if self.closed {
            return reject_with(
                crate::protocol::codes::SESSION_CLOSED,
                "session is already closed",
            );
        }

        // Handshake sequencing comes before role and rate checks so a
        // duplicate, resumable, or pre-hello command keeps its stable code.
        self.tick_vote();
        match command {
            ClientCommand::Hello { .. } if self.hello => {
                return reject_with(
                    crate::protocol::codes::HELLO_DUPLICATE,
                    "hello was already accepted",
                );
            }
            ClientCommand::Hello { .. } => {
                if let Some(window) = self.resume_window.as_mut() {
                    if window.is_open_at(self.tick) {
                        window.hello_attempts = window.hello_attempts.saturating_add(1);
                        if window.hello_attempts < MAX_HELLO_RESUME_ATTEMPTS {
                            return reject_with(
                                codes::HELLO_RESUMABLE,
                                format!(
                                    "a disconnected session can resume until tick {}; \
                                     send `resume` or retry hello",
                                    window.deadline_tick
                                ),
                            );
                        }
                    }
                    // Expired, or a bounded number of fresh hellos: abandon
                    // the stale window so a client that lost its token can
                    // start a new session.
                    self.resume_window = None;
                }
            }
            ClientCommand::Resume { .. } | ClientCommand::Disconnect => {}
            _ if !self.hello => {
                return reject_with(
                    codes::NOT_HELLO,
                    format!(
                        "`{}` requires a successful hello or resume first",
                        command.tag()
                    ),
                );
            }
            _ => {}
        }

        let required = ClientRole::required_role_for(command);
        if !self.role.allows(required) {
            return reject_with(
                codes::FORBIDDEN,
                format!(
                    "`{}` requires the {} role; this session is {}",
                    command.tag(),
                    required.as_str(),
                    self.role.as_str()
                ),
            );
        }

        // Handshake commands are never throttled: a client must always be able
        // to close or reclaim its session.
        let now = self.rate_clock_ms();
        if !matches!(command, ClientCommand::Disconnect) && !self.rate.try_consume(now) {
            return reject_with(
                codes::RATE_LIMITED,
                format!(
                    "command rate exceeded (nominal {RATE_CREDITS_PER_SECOND} credits/s, \
                     burst {RATE_BURST_CREDITS}); wait for the next tick"
                ),
            );
        }

        self.tick_vote();
        match command {
            ClientCommand::Hello {
                protocol_version,
                content_hash,
                player_name,
            } => {
                if let Err(rejection) = validate_hello(*protocol_version, content_hash, player_name)
                {
                    return from_rejection(rejection);
                }
                if let Some(expected) = &self.content_hash {
                    if content_hash != expected {
                        return reject_with(
                            crate::protocol::codes::CONTENT_HASH_MISMATCH,
                            "client content registry does not match the server pin",
                        );
                    }
                }
                if self.online.contains(player_name) {
                    return reject_with(
                        codes::PLAYER_NAME_DUPLICATE,
                        "another session is already using that player name",
                    );
                }
                let player_id = match self.resolve_player_id(player_name) {
                    Ok(player_id) => player_id,
                    Err(code) => {
                        return reject_with(
                            code,
                            "the player name cannot be assigned a stable identity",
                        );
                    }
                };
                self.hello = true;
                self.player = Some(player_name.clone());
                self.player_id = Some(player_id);
                self.online.insert(player_name.clone());
                self.closed = false;
                self.resume_window = None;
                self.last_seen_tick = self.tick;
                self.refresh_checksum();
                accepted(welcome(self, false))
            }
            ClientCommand::Resume {
                session_id,
                last_tick,
            } => self.apply_resume(session_id, *last_tick),
            ClientCommand::Disconnect => {
                let was_active = self.hello;
                self.closed = true;
                self.hello = false;
                if was_active {
                    self.remember_disconnect();
                }
                accepted(ServerMessage::Goodbye)
            }
            ClientCommand::Ping { nonce } => accepted(ServerMessage::Pong { nonce: *nonce }),
            ClientCommand::TimeControl { action } => self.apply_time_control(*action),
            ClientCommand::SetTax {
                resident_rate,
                business_rate,
            } => {
                if let Err(rejection) = validate_tax_rates(*resident_rate, *business_rate) {
                    return from_rejection(rejection);
                }
                self.economy
                    .set_taxes(f64::from(*resident_rate), f64::from(*business_rate));
                self.tick = self.tick.saturating_add(1);
                self.refresh_checksum();
                accepted(self.snapshot())
            }
            ClientCommand::PlaceRoad { start, end } => self.apply_place_road(*start, *end),
            ClientCommand::PlaceBlueprint {
                blueprint_hash,
                x,
                z,
                rotation,
            } => self.apply_place_blueprint(blueprint_hash, *x, *z, *rotation),
            ClientCommand::FoundSettlement {
                name,
                x,
                z,
                radius_m,
            } => self.apply_found_settlement(name, *x, *z, *radius_m),
            ClientCommand::SettlementList => self.apply_settlement_list(),
            ClientCommand::SettlementInvite {
                settlement_id,
                player_id,
                role,
                expected_revision,
            } => self.apply_settlement_invite(*settlement_id, *player_id, role, *expected_revision),
            ClientCommand::SettlementSetRole {
                settlement_id,
                player_id,
                role,
                expected_revision,
            } => {
                self.apply_settlement_set_role(*settlement_id, *player_id, role, *expected_revision)
            }
            ClientCommand::SettlementRemoveMember {
                settlement_id,
                player_id,
                expected_revision,
            } => {
                self.apply_settlement_remove_member(*settlement_id, *player_id, *expected_revision)
            }
            ClientCommand::SettlementClaim {
                settlement_id,
                expected_revision,
            } => self.apply_settlement_claim(*settlement_id, *expected_revision),
            ClientCommand::SettlementAt { x, z } => self.apply_settlement_at(*x, *z),
            ClientCommand::SettlementRename {
                settlement_id,
                name,
                expected_revision,
            } => self.apply_settlement_rename(*settlement_id, name, *expected_revision),
            ClientCommand::SettlementTransfer {
                settlement_id,
                player_id,
                expected_revision,
            } => self.apply_settlement_transfer(*settlement_id, *player_id, *expected_revision),
            ClientCommand::SettlementRelease {
                settlement_id,
                expected_revision,
            } => self.apply_settlement_release(*settlement_id, *expected_revision),
            ClientCommand::SetInterest {
                center_x,
                center_z,
                radius_m,
            } => self.apply_set_interest(*center_x, *center_z, *radius_m),
            ClientCommand::TimeVote { action, time } => self.apply_time_vote(*action, *time),
        }
    }

    fn apply_time_vote(
        &mut self,
        action: crate::protocol::VoteAction,
        time: Option<TimeAction>,
    ) -> (Vec<ServerMessage>, &'static str) {
        let Some(player) = self.player.clone() else {
            return reject_with(codes::NOT_HELLO, "send `hello` before voting");
        };
        let result = match action {
            crate::protocol::VoteAction::Call => match time {
                Some(time) => self
                    .vote
                    .call(&player, time, self.online.iter().cloned(), self.tick),
                None => return reject_with(codes::VOTE_UNAVAILABLE, "`call` requires `time`"),
            },
            crate::protocol::VoteAction::Yes => self.vote.vote(&player, true, self.tick),
            crate::protocol::VoteAction::No => self.vote.vote(&player, false, self.tick),
            crate::protocol::VoteAction::Cancel => self.vote.cancel(self.tick),
        };
        if let Err(error) = result {
            let rejection = error.into_rejection();
            return reject_with(rejection.code, rejection.message);
        }
        self.tick_vote();
        accepted(self.vote_status())
    }

    fn tick_vote(&mut self) {
        // `TimeVote::tick` resolves an open vote; a ballot cast through
        // `TimeVote::vote` can resolve immediately instead, in which case
        // `tick` is already a no-op. Drain the queued outcome so a passed
        // pause/resume is applied exactly once in both paths.
        let _ = self.vote.tick(self.tick, self.online.iter().cloned());
        if let Some(outcome) = self.vote.take_outcome() {
            self.apply_vote_outcome(&outcome);
        }
    }

    fn apply_vote_outcome(&mut self, outcome: &crate::vote::VoteOutcome) {
        if outcome.result != crate::vote::VoteState::Passed {
            return;
        }
        match outcome.action {
            TimeAction::Pause => self.paused = true,
            TimeAction::Resume => self.paused = false,
            _ => return,
        }
        self.vote.mark_effect_applied();
        self.refresh_checksum();
    }

    fn vote_status(&self) -> ServerMessage {
        ServerMessage::TimeVoteStatus {
            vote_id: self.vote.vote_id(),
            state: self.vote.state().as_str().to_string(),
            action: self
                .vote
                .action
                .map(crate::vote::action_name)
                .map(str::to_string),
            initiator: self.vote.initiator.clone(),
            yes: u32::try_from(self.vote.yes.len()).unwrap_or(u32::MAX),
            no: u32::try_from(self.vote.no.len()).unwrap_or(u32::MAX),
            online: u32::try_from(self.vote.online_at_start.len()).unwrap_or(u32::MAX),
            started_tick: self.vote.started_tick,
            ends_tick: self.vote.ends_tick,
            cooldown_until: self.vote.cooldown_until,
            paused: self.paused,
            clock: self.tick,
            checksum: self.vote.checksum(),
        }
    }

    /// Validate a reconnect claim without changing simulation state.
    ///
    /// The token is checked before the grace window so a wrong token never
    /// reveals whether a session is resumable. A token can name the active
    /// session (the legacy single-slot path) or a session parked under another
    /// connection id; adopting a parked session moves only that session's
    /// player, role, viewport, audit ring, and rate bucket onto this
    /// connection. Other online players keep their sessions.
    fn apply_resume(&mut self, token: &str, last_tick: u64) -> (Vec<ServerMessage>, &'static str) {
        if self.hello {
            return reject_with(
                codes::RESUME_DUPLICATE,
                "a session is already active; only a disconnected session can resume",
            );
        }
        if token.is_empty() {
            return reject_with(codes::RESUME_UNKNOWN, "resume token must not be empty");
        }
        if token != self.session_token() {
            return self.adopt_parked_session(token, last_tick);
        }
        let Some((deadline_tick, player)) = self
            .resume_window
            .as_ref()
            .map(|window| (window.deadline_tick, window.player.clone()))
        else {
            return reject_with(
                codes::RESUME_UNKNOWN,
                "no disconnected session is waiting to resume",
            );
        };
        if self.tick > deadline_tick {
            self.resume_window = None;
            return reject_with(
                codes::RESUME_EXPIRED,
                format!(
                    "the reconnect window closed at tick {deadline_tick}; the current tick is {}",
                    self.tick
                ),
            );
        }
        if last_tick > self.tick {
            return reject_with(
                codes::RESUME_STALE,
                format!(
                    "last_tick {last_tick} is ahead of the authoritative tick {}",
                    self.tick
                ),
            );
        }

        let player_id = match player.as_deref() {
            Some(name) => match self.resolve_player_id(name) {
                Ok(player_id) => Some(player_id),
                Err(code) => {
                    return reject_with(
                        code,
                        "the resumed player name cannot be assigned a stable identity",
                    );
                }
            },
            None => None,
        };
        self.resume_window = None;
        self.player_id = player_id;
        if let Some(player) = player {
            self.player = Some(player.clone());
            self.online.insert(player);
        }
        self.hello = true;
        self.closed = false;
        self.last_seen_tick = self.tick;
        accepted_all(vec![welcome(self, true), self.snapshot()])
    }

    /// Adopt a disconnected session parked under a different connection id.
    ///
    /// Rejections leave every session untouched except for clearing an expired
    /// window, matching the single-slot path. A successful claim removes the
    /// parked record and installs it on the calling connection, so the old
    /// connection id can no longer resume or vote as that player.
    fn adopt_parked_session(
        &mut self,
        token: &str,
        last_tick: u64,
    ) -> (Vec<ServerMessage>, &'static str) {
        let Some(connection_id) = self.parked_session_by_token(token) else {
            return reject_with(
                codes::RESUME_UNKNOWN,
                "resume token does not match any session in this world",
            );
        };
        let state = self.sessions.get(&connection_id).expect("token was found");
        if state.hello {
            return reject_with(
                codes::RESUME_DUPLICATE,
                "that session is already active on another connection",
            );
        }
        let Some(deadline_tick) = state
            .resume_window
            .as_ref()
            .map(|window| window.deadline_tick)
        else {
            return reject_with(
                codes::RESUME_UNKNOWN,
                "no disconnected session is waiting to resume",
            );
        };
        if self.tick > deadline_tick {
            if let Some(state) = self.sessions.get_mut(&connection_id) {
                state.resume_window = None;
            }
            return reject_with(
                codes::RESUME_EXPIRED,
                format!(
                    "the reconnect window closed at tick {deadline_tick}; the current tick is {}",
                    self.tick
                ),
            );
        }
        if last_tick > self.tick {
            return reject_with(
                codes::RESUME_STALE,
                format!(
                    "last_tick {last_tick} is ahead of the authoritative tick {}",
                    self.tick
                ),
            );
        }

        let player = self
            .sessions
            .get(&connection_id)
            .and_then(|state| state.resume_window.as_ref())
            .and_then(|window| window.player.clone());
        let player_id = match player.as_deref() {
            Some(name) => match self.resolve_player_id(name) {
                Ok(player_id) => Some(player_id),
                Err(code) => {
                    return reject_with(
                        code,
                        "the resumed player name cannot be assigned a stable identity",
                    );
                }
            },
            None => None,
        };

        let mut state = self
            .sessions
            .remove(&connection_id)
            .expect("token was found");
        state.player_id = player_id;
        state.resume_window = None;
        state.hello = true;
        state.closed = false;
        state.last_seen_tick = self.tick;
        self.install_session(state);
        if let Some(player) = player {
            self.player = Some(player.clone());
            self.online.insert(player);
        }
        self.hello = true;
        self.closed = false;
        self.last_seen_tick = self.tick;
        accepted_all(vec![welcome(self, true), self.snapshot()])
    }

    fn apply_time_control(&mut self, action: TimeAction) -> (Vec<ServerMessage>, &'static str) {
        match action {
            TimeAction::Pause => self.paused = true,
            TimeAction::Resume => self.paused = false,
            TimeAction::Step => {
                self.advance_unpaused(DAY_SECONDS);
            }
            TimeAction::Speed1x | TimeAction::Speed2x | TimeAction::Speed4x => {
                return reject_with(
                    codes::TIME_INVALID_ACTION,
                    "speed controls are not supported by the v0 authority executor",
                );
            }
        }
        self.refresh_checksum();
        accepted(self.snapshot())
    }

    fn apply_place_road(
        &mut self,
        start: [i32; 2],
        end: [i32; 2],
    ) -> (Vec<ServerMessage>, &'static str) {
        if let Err(rejection) = validate_road(start, end) {
            return from_rejection(rejection);
        }

        let dx = i64::from(end[0]) - i64::from(start[0]);
        let dz = i64::from(end[1]) - i64::from(start[1]);
        if dx != 0 && dz != 0 {
            return reject_with(
                codes::ROAD_NOT_AXIS_ALIGNED,
                "road must be horizontal or vertical; diagonals are not supported",
            );
        }
        let length = dx.abs() + dz.abs();
        if length > i64::from(MAX_WORLD_ROAD_LENGTH) {
            return reject_with(
                crate::protocol::codes::ROAD_TOO_LONG,
                format!(
                    "road length {length} exceeds the v0 maximum of {MAX_WORLD_ROAD_LENGTH} cells"
                ),
            );
        }
        if let Some(code) = self.road_conflict(start, end) {
            return reject_with(code, "road duplicates or overlaps an existing segment");
        }

        let request = RoadRequest {
            start,
            end,
            width_m: WORLD_ROAD_WIDTH_M,
        };
        match self.roads.add(&request, |_, _| PLACEHOLDER_GROUND_M) {
            Ok(_) => {
                self.tick = self.tick.saturating_add(1);
                self.refresh_checksum();
                accepted(self.snapshot())
            }
            Err(error) => reject_road_error(error),
        }
    }

    /// Authoritative `place_blueprint` (D-032).
    ///
    /// The validation order is fixed and nothing mutates until every check has
    /// passed:
    ///
    /// 1. payload: hash shape, finite coordinates, rotation multiple of 90;
    /// 2. D-033 settlement `build` permission at the placement centre, when a
    ///    settlement contains it;
    /// 3. server library configured, hash resolves, the stored package loads
    ///    and its metadata is consistent;
    /// 4. idempotency: an exact existing placement (same canonical hash,
    ///    centre rounded to the millimetre, and rotation) returns its result
    ///    without a second charge;
    /// 5. district policy at the placement centre, when a same-seed registry
    ///    is configured;
    /// 6. settlement age against the blueprint's declared structure;
    /// 7. terrain/geometry/overlap against the placeholder flat sampler;
    /// 8. treasury affordability of the first D-028 installment.
    ///
    /// The permission check precedes library load and idempotency, so a repeat
    /// is only acknowledged for an actor who may still build there. Only after
    /// every check are the placement, construction site, charge, tick, and
    /// checksum committed. Every rejection leaves all state unchanged,
    /// including a placement or construction store created by this call.
    fn apply_place_blueprint(
        &mut self,
        blueprint_hash: &str,
        x: f64,
        z: f64,
        rotation: i32,
    ) -> (Vec<ServerMessage>, &'static str) {
        if let Err(rejection) = validate_place_blueprint(blueprint_hash, x, z, rotation) {
            return from_rejection(rejection);
        }
        let hash = normalize_blueprint_hash(blueprint_hash)
            .expect("validate_place_blueprint checked the hash");
        let Ok(rotation_deg) = normalize_rotation(i64::from(rotation)) else {
            return reject_with(
                crate::protocol::codes::PLACEMENT_ROTATION_INVALID,
                format!("placement rotation {rotation} is not a multiple of 90 degrees"),
            );
        };
        if let Some((code, message)) = self.settlement_build_denial(x, z) {
            return reject_with(code, message);
        }
        let Some(library) = self.blueprint_library.as_ref() else {
            return reject_with(
                crate::protocol::codes::PLACEMENT_LIBRARY_UNAVAILABLE,
                "no blueprint library is configured on this server",
            );
        };
        let path = match library.resolve_checked(&hash) {
            Ok(Some(path)) => path,
            Ok(None) => {
                return reject_with(
                    crate::protocol::codes::PLACEMENT_PACKAGE_MISSING,
                    format!("blueprint package {hash} is not stored on this server"),
                )
            }
            Err(_) => {
                return reject_with(
                    crate::protocol::codes::PLACEMENT_PACKAGE_INVALID,
                    format!("blueprint package {hash} could not be resolved safely"),
                )
            }
        };
        let blueprint = match load_tsbp(&path) {
            Ok(blueprint) => blueprint,
            Err(error) => {
                return reject_with(
                    crate::protocol::codes::PLACEMENT_PACKAGE_INVALID,
                    format!("blueprint package {hash} failed validation: {error}"),
                )
            }
        };
        let metadata = &blueprint.metadata;
        let metadata_error = if metadata.content_hash != hash {
            Some("the stored package content hash does not match its library address")
        } else if metadata.id.trim().is_empty() {
            Some("the blueprint id is empty")
        } else if metadata.name.trim().is_empty() {
            Some("the blueprint name is empty")
        } else if metadata
            .bounds_m
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.0)
        {
            Some("the blueprint bounds must be finite and positive")
        } else if metadata.solid_voxels == 0 {
            Some("the blueprint has no solid voxels")
        } else if !metadata.stats.material_cost.is_finite() || metadata.stats.material_cost < 0.0 {
            Some("the blueprint material cost must be finite and non-negative")
        } else {
            None
        };
        if let Some(message) = metadata_error {
            return reject_with(
                crate::protocol::codes::PLACEMENT_PACKAGE_INVALID,
                format!("blueprint package {hash} is inconsistent: {message}"),
            );
        }
        let request = PlacementRequest {
            blueprint_id: metadata.id.clone(),
            blueprint_hash: hash.clone(),
            blueprint_path: placement_package_reference(&hash),
            name: metadata.name.clone(),
            bounds_m: metadata.bounds_m,
            center_x: x as f32,
            center_z: z as f32,
            rotation_deg,
            material_cost: metadata.stats.material_cost,
            resident_capacity: metadata.stats.resident_capacity,
            job_slots: metadata.stats.job_slots,
            solid_voxels: metadata.solid_voxels,
            structure: metadata.structure,
        };

        // Idempotency comes before policy, age, terrain, and overlap: a repeat
        // of the same accepted request is a success, not a second charge.
        if let Some(store) = self.placements.as_ref() {
            if let Some(existing) = store
                .placements
                .iter()
                .find(|placement| same_placement(placement, &request))
            {
                return accepted(ServerMessage::PlaceBlueprintResult {
                    tick: self.tick,
                    placement_id: existing.id,
                    blueprint_hash: existing.blueprint_hash.clone(),
                    placement_checksum: format!("{:016x}", store.checksum()),
                    world_checksum: self.checksum(),
                    placement: placement_view(existing),
                });
            }
        }

        if let Some(policies) = self.policies.as_ref() {
            if policies.seed == self.seed {
                if let Some(policy) = policies.policy_at([request.center_x, request.center_z]) {
                    if let Some(denial) = policy_placement_denial(policy, &request.blueprint_id) {
                        return reject_with(
                            crate::protocol::codes::PLACEMENT_POLICY_DENIED,
                            format!(
                                "district policy {} blocks placing blueprint {} here ({})",
                                policy.id,
                                request.blueprint_id,
                                denial.code()
                            ),
                        );
                    }
                }
            }
        }

        if let Some(structure) = request.structure.as_ref() {
            let age = AgeState::from_population(u64::from(self.population)).age;
            if let Some((_code, message)) = structure_age_error(structure, age) {
                return reject_with(crate::protocol::codes::PLACEMENT_AGE_LOCKED, message);
            }
        }

        let height_at = |_, _| PLACEHOLDER_GROUND_M;
        let plan = match self.placements.as_ref() {
            Some(store) => store.validate(&request, height_at),
            None => validate_placement(&request, &[], height_at),
        };
        if let Err(error) = plan {
            return reject_placement_error(error);
        }

        let first_installment = first_construction_payment(request.material_cost);
        if !self.economy.can_afford(first_installment) {
            return reject_with(
                crate::protocol::codes::PLACEMENT_INSUFFICIENT_FUNDS,
                format!(
                    "the first construction installment needs {first_installment:.2} but the treasury has {:.2}",
                    self.economy.balance
                ),
            );
        }

        let created_placements = self.placements.is_none();
        let created_construction = self.construction.is_none();
        let placement = {
            let store = self
                .placements
                .get_or_insert_with(|| PlacementStore::new(self.seed));
            match store.place(&request, height_at) {
                Ok(placement) => placement,
                Err(error) => {
                    if created_placements {
                        self.placements = None;
                    }
                    return reject_placement_error(error);
                }
            }
        };
        let site = self
            .construction
            .get_or_insert_with(|| {
                ConstructionStore::new(
                    SERVER_CONSTRUCTION_CREW_CAPACITY,
                    SERVER_CONSTRUCTION_MATERIALS_STOCK,
                )
            })
            .add_priced_site(placement.id, request.solid_voxels, request.material_cost)
            .map(|_| ());
        if let Err(error) = site {
            if let Some(store) = self.placements.as_mut() {
                store.remove(placement.id);
            }
            if created_placements {
                self.placements = None;
            }
            if created_construction {
                self.construction = None;
            }
            return reject_construction_error(error);
        }
        let paid = {
            let construction = self
                .construction
                .as_mut()
                .expect("the construction site was just created");
            settle_first_installment(
                &mut self.economy,
                construction,
                placement.id,
                first_installment,
            )
        };
        if !paid {
            if let Some(construction) = self.construction.as_mut() {
                construction.remove_site(placement.id);
            }
            if let Some(store) = self.placements.as_mut() {
                store.remove(placement.id);
            }
            if created_placements {
                self.placements = None;
            }
            if created_construction {
                self.construction = None;
            }
            return reject_with(
                crate::protocol::codes::PLACEMENT_INSUFFICIENT_FUNDS,
                "the treasury changed while placing; construction was cancelled",
            );
        }

        self.tick = self.tick.saturating_add(1);
        self.refresh_checksum();
        let placement_checksum = self.placements.as_ref().map_or(0, PlacementStore::checksum);
        accepted(ServerMessage::PlaceBlueprintResult {
            tick: self.tick,
            placement_id: placement.id,
            blueprint_hash: placement.blueprint_hash.clone(),
            placement_checksum: format!("{placement_checksum:016x}"),
            world_checksum: self.checksum(),
            placement: placement_view(&placement),
        })
    }

    fn apply_found_settlement(
        &mut self,
        name: &str,
        x: i32,
        z: i32,
        radius_m: u32,
    ) -> (Vec<ServerMessage>, &'static str) {
        let Some(player_id) = self.player_id else {
            return reject_with(
                codes::SETTLEMENT_IDENTITY,
                "founding a settlement requires a player identity",
            );
        };
        let Some(display_name) = self
            .players
            .get(player_id)
            .map(|player| player.display_name.clone())
        else {
            return reject_with(
                codes::SETTLEMENT_IDENTITY,
                format!("player {player_id} is not in the server registry"),
            );
        };
        if let Some(store) = self.settlements.as_ref() {
            if store.seed != self.seed {
                return reject_with(
                    codes::SETTLEMENT_SEED_MISMATCH,
                    format!(
                        "settlement store seed {} does not match world seed {}",
                        store.seed, self.seed
                    ),
                );
            }
        }
        let request = SettlementRequest {
            name: name.to_string(),
            owner: display_name,
            center: [x, z],
            radius_m,
        };
        let outcome = {
            let store = self
                .settlements
                .get_or_insert_with(|| SettlementStore::new(self.seed));
            store.found_for_player(&request, player_id)
        };
        match outcome {
            Ok(settlement) => {
                self.refresh_checksum();
                let settlement_checksum = self
                    .settlements
                    .as_ref()
                    .map_or(0, SettlementStore::checksum);
                accepted(ServerMessage::SettlementFound {
                    tick: self.tick,
                    settlement: settlement_view(&settlement),
                    settlement_checksum: format!("{settlement_checksum:016x}"),
                    world_checksum: self.checksum(),
                })
            }
            Err(error) => reject_with(error.code(), error.to_string()),
        }
    }

    fn apply_settlement_list(&mut self) -> (Vec<ServerMessage>, &'static str) {
        let Some(store) = self.settlements.as_ref() else {
            return accepted(ServerMessage::SettlementList {
                tick: self.tick,
                count: 0,
                checksum: "0000000000000000".to_string(),
                settlements: Vec::new(),
            });
        };
        if store.seed != self.seed {
            return reject_with(
                codes::SETTLEMENT_SEED_MISMATCH,
                format!(
                    "settlement store seed {} does not match world seed {}",
                    store.seed, self.seed
                ),
            );
        }
        accepted(ServerMessage::SettlementList {
            tick: self.tick,
            count: store.count(),
            checksum: format!("{:016x}", store.checksum()),
            settlements: store.settlements.iter().map(settlement_view).collect(),
        })
    }

    fn settlement_store_mut(
        &mut self,
    ) -> Result<&mut SettlementStore, (Vec<ServerMessage>, &'static str)> {
        let seed = self.seed;
        let Some(store) = self.settlements.as_mut() else {
            return Err(reject_with(
                codes::SETTLEMENT_UNKNOWN,
                "no settlement store exists in this world",
            ));
        };
        if store.seed != seed {
            return Err(reject_with(
                codes::SETTLEMENT_SEED_MISMATCH,
                format!(
                    "settlement store seed {} does not match world seed {seed}",
                    store.seed
                ),
            ));
        }
        Ok(store)
    }

    fn finish_settlement_update(
        &mut self,
        settlement: Settlement,
    ) -> (Vec<ServerMessage>, &'static str) {
        self.refresh_checksum();
        let settlement_checksum = self
            .settlements
            .as_ref()
            .map_or(0, SettlementStore::checksum);
        accepted(ServerMessage::SettlementUpdated {
            tick: self.tick,
            settlement: settlement_view(&settlement),
            settlement_checksum: format!("{settlement_checksum:016x}"),
            world_checksum: self.checksum(),
        })
    }

    fn apply_settlement_invite(
        &mut self,
        settlement_id: u64,
        player_id: u64,
        role: &str,
        expected_revision: u32,
    ) -> (Vec<ServerMessage>, &'static str) {
        let Some(actor) = self.player_id else {
            return reject_with(
                codes::SETTLEMENT_IDENTITY,
                "settlement membership requires a player identity",
            );
        };
        let Some(role) = parse_settlement_role(role) else {
            return reject_with(
                codes::SETTLEMENT_ROLE_INVALID,
                format!("unknown settlement role `{role}`"),
            );
        };
        let Ok(target) = PlayerId::new(player_id) else {
            return reject_with(
                codes::SETTLEMENT_IDENTITY,
                format!("player id {player_id} is outside the supported range"),
            );
        };
        let outcome = {
            let store = match self.settlement_store_mut() {
                Ok(store) => store,
                Err(rejection) => return rejection,
            };
            store.invite(actor, settlement_id, target, role, expected_revision)
        };
        match outcome {
            Ok(settlement) => self.finish_settlement_update(settlement),
            Err(error) => reject_with(error.code(), error.to_string()),
        }
    }

    fn apply_settlement_set_role(
        &mut self,
        settlement_id: u64,
        player_id: u64,
        role: &str,
        expected_revision: u32,
    ) -> (Vec<ServerMessage>, &'static str) {
        let Some(actor) = self.player_id else {
            return reject_with(
                codes::SETTLEMENT_IDENTITY,
                "settlement membership requires a player identity",
            );
        };
        let Some(role) = parse_settlement_role(role) else {
            return reject_with(
                codes::SETTLEMENT_ROLE_INVALID,
                format!("unknown settlement role `{role}`"),
            );
        };
        let Ok(target) = PlayerId::new(player_id) else {
            return reject_with(
                codes::SETTLEMENT_IDENTITY,
                format!("player id {player_id} is outside the supported range"),
            );
        };
        let outcome = {
            let store = match self.settlement_store_mut() {
                Ok(store) => store,
                Err(rejection) => return rejection,
            };
            store.set_member_role(actor, settlement_id, target, role, expected_revision)
        };
        match outcome {
            Ok(settlement) => self.finish_settlement_update(settlement),
            Err(error) => reject_with(error.code(), error.to_string()),
        }
    }

    fn apply_settlement_remove_member(
        &mut self,
        settlement_id: u64,
        player_id: u64,
        expected_revision: u32,
    ) -> (Vec<ServerMessage>, &'static str) {
        let Some(actor) = self.player_id else {
            return reject_with(
                codes::SETTLEMENT_IDENTITY,
                "settlement membership requires a player identity",
            );
        };
        let Ok(target) = PlayerId::new(player_id) else {
            return reject_with(
                codes::SETTLEMENT_IDENTITY,
                format!("player id {player_id} is outside the supported range"),
            );
        };
        let outcome = {
            let store = match self.settlement_store_mut() {
                Ok(store) => store,
                Err(rejection) => return rejection,
            };
            store.remove_member(actor, settlement_id, target, expected_revision)
        };
        match outcome {
            Ok(settlement) => self.finish_settlement_update(settlement),
            Err(error) => reject_with(error.code(), error.to_string()),
        }
    }

    fn apply_settlement_claim(
        &mut self,
        settlement_id: u64,
        expected_revision: u32,
    ) -> (Vec<ServerMessage>, &'static str) {
        let Some(actor) = self.player_id else {
            return reject_with(
                codes::SETTLEMENT_IDENTITY,
                "claiming a settlement requires a player identity",
            );
        };
        let outcome = {
            let store = match self.settlement_store_mut() {
                Ok(store) => store,
                Err(rejection) => return rejection,
            };
            store.claim(actor, settlement_id, expected_revision)
        };
        match outcome {
            Ok(settlement) => self.finish_settlement_update(settlement),
            Err(error) => reject_with(error.code(), error.to_string()),
        }
    }

    fn apply_settlement_at(&mut self, x: i32, z: i32) -> (Vec<ServerMessage>, &'static str) {
        let Some(store) = self.settlements.as_ref() else {
            return accepted(ServerMessage::SettlementAt {
                tick: self.tick,
                found: false,
                settlement: None,
            });
        };
        if store.seed != self.seed {
            return reject_with(
                codes::SETTLEMENT_SEED_MISMATCH,
                format!(
                    "settlement store seed {} does not match world seed {}",
                    store.seed, self.seed
                ),
            );
        }
        let settlement = store.at(x, z).map(settlement_view);
        accepted(ServerMessage::SettlementAt {
            tick: self.tick,
            found: settlement.is_some(),
            settlement,
        })
    }

    fn apply_settlement_rename(
        &mut self,
        settlement_id: u64,
        name: &str,
        expected_revision: u32,
    ) -> (Vec<ServerMessage>, &'static str) {
        let Some(actor) = self.player_id else {
            return reject_with(
                codes::SETTLEMENT_IDENTITY,
                "renaming a settlement requires a player identity",
            );
        };
        let outcome = {
            let store = match self.settlement_store_mut() {
                Ok(store) => store,
                Err(rejection) => return rejection,
            };
            store.rename(actor, settlement_id, name, expected_revision)
        };
        match outcome {
            Ok(settlement) => self.finish_settlement_update(settlement),
            Err(error) => reject_with(error.code(), error.to_string()),
        }
    }

    fn apply_settlement_transfer(
        &mut self,
        settlement_id: u64,
        player_id: u64,
        expected_revision: u32,
    ) -> (Vec<ServerMessage>, &'static str) {
        let Some(actor) = self.player_id else {
            return reject_with(
                codes::SETTLEMENT_IDENTITY,
                "transferring a settlement requires a player identity",
            );
        };
        let Ok(recipient) = PlayerId::new(player_id) else {
            return reject_with(
                codes::SETTLEMENT_IDENTITY,
                format!("player id {player_id} is outside the supported range"),
            );
        };
        if self.players.get(recipient).is_none() {
            return reject_with(
                codes::SETTLEMENT_UNKNOWN_PLAYER,
                format!("player {player_id} is not in the server registry"),
            );
        }
        let outcome = {
            let store = match self.settlement_store_mut() {
                Ok(store) => store,
                Err(rejection) => return rejection,
            };
            store.transfer(actor, settlement_id, recipient, expected_revision)
        };
        match outcome {
            Ok(settlement) => self.finish_settlement_update(settlement),
            Err(error) => reject_with(error.code(), error.to_string()),
        }
    }

    fn apply_settlement_release(
        &mut self,
        settlement_id: u64,
        expected_revision: u32,
    ) -> (Vec<ServerMessage>, &'static str) {
        let outcome = {
            let store = match self.settlement_store_mut() {
                Ok(store) => store,
                Err(rejection) => return rejection,
            };
            store.force_release(settlement_id, expected_revision)
        };
        match outcome {
            Ok(settlement) => self.finish_settlement_update(settlement),
            Err(error) => reject_with(error.code(), error.to_string()),
        }
    }

    /// D-033 `build` permission inside a settlement.
    ///
    /// Wilderness (no settlement store, or no settlement containing the
    /// placement centre) keeps the global-role behavior and returns `None`.
    /// Inside a settlement an admin bypasses every settlement check. Otherwise
    /// the actor must be identified, the settlement must be owned, the actor
    /// must be a member, and that member must hold
    /// [`SettlementPermission::BUILD`].
    ///
    /// The placement centre is rounded to the nearest whole metre because
    /// settlement containment is metre-based. This is a pure read: a rejection
    /// leaves the settlement store untouched, and the caller audits the code.
    fn settlement_build_denial(&self, x: f64, z: f64) -> Option<(&'static str, String)> {
        if self.role == ClientRole::Admin {
            return None;
        }
        let cell_x = x.round() as i32;
        let cell_z = z.round() as i32;
        let settlement = self.settlements.as_ref()?.at(cell_x, cell_z)?;
        let Some(player) = self.player_id else {
            return Some((
                codes::SETTLEMENT_IDENTITY,
                format!(
                    "placing inside settlement {} requires a player identity",
                    settlement.name
                ),
            ));
        };
        if !settlement.is_owned() {
            return Some((
                codes::SETTLEMENT_UNOWNED,
                format!(
                    "settlement {} has no owner; only an admin may place blueprints here",
                    settlement.name
                ),
            ));
        }
        if settlement.role_of(player).is_none() {
            return Some((
                codes::SETTLEMENT_NOT_MEMBER,
                format!(
                    "player {player} is not a member of settlement {}",
                    settlement.name
                ),
            ));
        }
        if !settlement.has_permission(player, SettlementPermission::BUILD) {
            return Some((
                codes::SETTLEMENT_FORBIDDEN,
                format!(
                    "player {player} lacks the build permission in settlement {}",
                    settlement.name
                ),
            ));
        }
        None
    }

    /// Update this session's interest viewport and answer with the bounded view.
    /// The batch is `snapshot` followed by `interest_view`, so a client gets the
    /// filtered road details immediately instead of waiting for another
    /// command. This is view-only: it does not change the tick, treasury,
    /// roads, or checksum, and it is never persisted. `set_interest` still
    /// consumes a rate credit and is audited like every other command.
    fn apply_set_interest(
        &mut self,
        center_x: f64,
        center_z: f64,
        radius_m: f64,
    ) -> (Vec<ServerMessage>, &'static str) {
        if let Err(rejection) = validate_interest(center_x, center_z, radius_m) {
            return from_rejection(rejection);
        }
        self.interest_center = [center_x as f32, center_z as f32];
        self.interest_radius_m = radius_m as f32;
        let view = self.interest_view();
        accepted_all(vec![self.snapshot(), view])
    }

    /// Reject an exact duplicate or a collinear overlap of more than one cell.
    /// Roads that only share one endpoint cell (a T-junction) are allowed, as
    /// are crossings on different axes or perpendicular lines.
    fn road_conflict(&self, start: [i32; 2], end: [i32; 2]) -> Option<&'static str> {
        let along_x = start[1] == end[1];
        let (perpendicular, min, max) = if along_x {
            (
                i64::from(start[1]),
                i64::from(start[0].min(end[0])),
                i64::from(start[0].max(end[0])),
            )
        } else {
            (
                i64::from(start[0]),
                i64::from(start[1].min(end[1])),
                i64::from(start[1].max(end[1])),
            )
        };

        for road in &self.roads.roads {
            if road.runs_along_x() != along_x {
                continue;
            }
            let (road_perpendicular, road_min, road_max) = if along_x {
                (
                    i64::from(road.start[1]),
                    i64::from(road.start[0].min(road.end[0])),
                    i64::from(road.start[0].max(road.end[0])),
                )
            } else {
                (
                    i64::from(road.start[0]),
                    i64::from(road.start[1].min(road.end[1])),
                    i64::from(road.start[1].max(road.end[1])),
                )
            };
            if road_perpendicular != perpendicular {
                continue;
            }
            if road_min == min && road_max == max {
                return Some(codes::ROAD_DUPLICATE);
            }
            let shared_low = road_min.max(min);
            let shared_high = road_max.min(max);
            if shared_high > shared_low {
                return Some(codes::ROAD_OVERLAP);
            }
        }
        None
    }

    /// Advance the economy by `dt_s` seconds, capped at one day.
    ///
    /// The pop/employed/buildings counts are the world's static values and v0
    /// has no utility or civic service sites, so infrastructure upkeep is zero.
    /// Every call increments the tick, including a zero-length step. While
    /// paused this is a no-op that returns the current ledger.
    pub fn advance_day(&mut self, dt_s: f64) -> EconomyTick {
        if self.paused {
            return self.idle_tick();
        }
        self.advance_unpaused(dt_s)
    }

    /// Alias for [`Self::advance_day`].
    pub fn tick(&mut self, dt_s: f64) -> EconomyTick {
        self.advance_day(dt_s)
    }

    fn advance_unpaused(&mut self, dt_s: f64) -> EconomyTick {
        let dt = sanitize_dt(dt_s);
        let tick = self.economy.tick_with_infrastructure(
            dt,
            u64::from(self.population),
            u64::from(self.employed),
            u64::from(self.buildings),
            0,
            0,
        );
        self.tick = self.tick.saturating_add(1);
        let now = self.rate_clock_ms();
        self.rate.refill(now);
        self.refresh_checksum();
        tick
    }

    fn idle_tick(&self) -> EconomyTick {
        EconomyTick {
            income: 0.0,
            expenses: 0.0,
            infrastructure: 0.0,
            balance: self.economy.balance,
            days_elapsed: self.economy.days_elapsed,
            loan_interest: 0.0,
            loan_principal: 0.0,
            debt: self.economy.debt(),
            bankrupt: self.economy.bankrupt,
        }
    }

    fn refresh_checksum(&mut self) {
        self.session_checksum = self.checksum();
    }
}

fn sanitize_dt(dt_s: f64) -> f64 {
    if dt_s.is_finite() {
        dt_s.clamp(0.0, DAY_SECONDS)
    } else {
        0.0
    }
}

/// Whether `point` is inside the inclusive interest circle.
///
/// The comparison is `distance <= radius`, so an exact boundary match is
/// visible. Roads, placement-backed buildings, and the synthetic fallback use
/// this predicate, which keeps aggregate counts and detail lists consistent.
fn point_in_interest(point: [f32; 2], center: [f32; 2], radius_m: f32) -> bool {
    let radius = f64::from(radius_m);
    let radius_squared = radius * radius;
    let dx = f64::from(point[0]) - f64::from(center[0]);
    let dz = f64::from(point[1]) - f64::from(center[1]);
    dx * dx + dz * dz <= radius_squared
}

/// Whether a road's midpoint is inside the inclusive interest circle.
///
/// [`ServerWorld::visible_roads`] and [`ServerWorld::interest_view`] share this
/// predicate so the aggregate count and the detail list can never disagree.
fn road_in_interest(road: &RoadSegment, center: [f32; 2], radius_m: f32) -> bool {
    point_in_interest(road.center(), center, radius_m)
}

/// Stable synthetic record for one completed building.
///
/// This is the fallback for worlds with no placement store (or an empty one):
/// [`ServerWorld`] then keeps only the static `buildings` count, so interest
/// views approximate the built city on a fixed grid anchored at the origin.
/// Ids are 1-based, 16 per row and 64 m apart, so id 1 is at `[0, 0]`, id 2 at
/// `[64, 0]`, id 17 at `[0, 64]`, and so on. The mapping is deterministic and
/// independent of population, so the same id always reports the same centre.
#[must_use]
pub fn synthetic_building(id: u64) -> BuildingView {
    let index = id.saturating_sub(1);
    let column = index % INTEREST_BUILDING_COLUMNS;
    let row = index / INTEREST_BUILDING_COLUMNS;
    BuildingView {
        id,
        center: [
            column as f32 * INTEREST_BUILDING_SPACING_M,
            row as f32 * INTEREST_BUILDING_SPACING_M,
        ],
        radius_m: INTEREST_BUILDING_RADIUS_M,
    }
}

/// One placement-backed building record for the interest view.
///
/// The wire shape carries a single radius, so it reports half the larger
/// rotated footprint side. A hand-built record with a non-finite or
/// non-positive footprint falls back to [`INTEREST_BUILDING_RADIUS_M`]; the
/// value is a render hint and is not revalidated against the blueprint.
fn placement_building(placement: &Placement) -> BuildingView {
    let half = placement.footprint_m[0].max(placement.footprint_m[1]) * 0.5;
    BuildingView {
        id: placement.id,
        center: [placement.center[0], placement.center[2]],
        radius_m: if half.is_finite() && half > 0.0 {
            half
        } else {
            INTEREST_BUILDING_RADIUS_M
        },
    }
}

/// Inclusive grid indices whose cell centres can lie within `low..=high`.
///
/// Returns `None` when the interval misses the grid or would start past
/// `max_index`. The result is clamped, so an extreme viewport centre can never
/// produce an out-of-grid loop bound or overflow the caller's id arithmetic.
fn grid_axis_bounds(low: f64, high: f64, spacing: f64, max_index: u64) -> Option<(u64, u64)> {
    let first = (low / spacing).ceil();
    let last = (high / spacing).floor();
    if !first.is_finite() || !last.is_finite() || last < 0.0 || first > max_index as f64 {
        return None;
    }
    let first = first.max(0.0) as u64;
    let last = last.min(max_index as f64) as u64;
    (first <= last).then_some((first, last))
}

fn welcome(world: &ServerWorld, resumed: bool) -> ServerMessage {
    ServerMessage::Welcome {
        session_id: world.session_id().to_string(),
        session_token: world.session_token().to_string(),
        tick: world.tick,
        protocol_version: PROTOCOL_VERSION,
        resumed,
    }
}

fn reject_with(
    code: &'static str,
    message: impl Into<String>,
) -> (Vec<ServerMessage>, &'static str) {
    (vec![Rejection::new(code, message).into_message()], code)
}

fn from_rejection(rejection: Rejection) -> (Vec<ServerMessage>, &'static str) {
    let code = rejection.code;
    (vec![rejection.into_message()], code)
}

fn accepted(message: ServerMessage) -> (Vec<ServerMessage>, &'static str) {
    (vec![message], AUDIT_ACCEPTED)
}

fn accepted_all(messages: Vec<ServerMessage>) -> (Vec<ServerMessage>, &'static str) {
    (messages, AUDIT_ACCEPTED)
}

fn reject_road_error(error: RoadError) -> (Vec<ServerMessage>, &'static str) {
    let code = match error {
        RoadError::TooMany => codes::ROAD_STORE_FULL,
        RoadError::IdExhausted => codes::ROAD_ID_EXHAUSTED,
        RoadError::ZeroLength => crate::protocol::codes::ROAD_EMPTY,
        RoadError::TooLong { .. } => crate::protocol::codes::ROAD_TOO_LONG,
        RoadError::CoordinateOutOfRange => crate::protocol::codes::ROAD_OUT_OF_BOUNDS,
        _ => codes::ROAD_REJECTED,
    };
    reject_with(code, error.to_string())
}

/// Stable content-addressed package reference stored in a server placement.
///
/// The canonical hash identifies the package; the server library root is
/// process configuration and must never enter a save record or checksum. The
/// `tsbp:` prefix keeps the reference distinct from the canonical
/// `blueprint_hash` field while staying stable across roots and machines.
#[must_use]
pub fn placement_package_reference(canonical_hash: &str) -> String {
    format!("tsbp:{canonical_hash}")
}

/// Read-only wire copy of one authoritative placement.
///
/// The server-local `blueprint_path` is deliberately omitted; a remote client
/// only needs the content hash.
#[must_use]
fn parse_settlement_role(value: &str) -> Option<SettlementRole> {
    SettlementRole::ALL
        .into_iter()
        .find(|role| role.as_str().eq_ignore_ascii_case(value))
}

/// Read-only wire copy of one settlement record.
fn settlement_view(settlement: &Settlement) -> SettlementView {
    SettlementView {
        id: settlement.id,
        name: settlement.name.clone(),
        owner: settlement.owner.clone(),
        owner_id: settlement.owner_id.map(core_city::player::PlayerId::get),
        owned: settlement.is_owned(),
        center: settlement.center,
        radius_m: settlement.radius_m,
        members: settlement
            .members
            .iter()
            .map(|member| SettlementMemberView {
                player: member.player.get(),
                role: member.role.as_str().to_string(),
                permissions: member.role.permissions().bits(),
            })
            .collect(),
        membership_revision: settlement.membership_revision,
    }
}

pub fn placement_view(placement: &Placement) -> PlacementView {
    PlacementView {
        id: placement.id,
        blueprint_id: placement.blueprint_id.clone(),
        blueprint_hash: placement.blueprint_hash.clone(),
        name: placement.name.clone(),
        center: placement.center,
        rotation_deg: placement.rotation_deg,
        footprint_m: placement.footprint_m,
        bounds_m: placement.bounds_m,
        foundation_m: placement.foundation_m,
        material_cost: placement.material_cost,
        resident_capacity: placement.resident_capacity,
        job_slots: placement.job_slots,
        solid_voxels: placement.solid_voxels,
    }
}

/// Whether an existing placement is the exact request, for idempotency.
///
/// The hash and rotation must match exactly and the centre must match to the
/// nearest millimetre on X and Z. The result is independent of the stored Y,
/// foundation, and cost.
fn same_placement(placement: &Placement, request: &PlacementRequest) -> bool {
    placement.blueprint_hash == request.blueprint_hash
        && placement.rotation_deg == request.rotation_deg
        && centre_mm(placement.center[0]) == centre_mm(request.center_x)
        && centre_mm(placement.center[2]) == centre_mm(request.center_z)
}

fn centre_mm(value: f32) -> i64 {
    (f64::from(value) * 1_000.0).round() as i64
}

fn reject_placement_error(error: PlacementError) -> (Vec<ServerMessage>, &'static str) {
    let code = match &error {
        PlacementError::Overlap { .. } => crate::protocol::codes::PLACEMENT_OVERLAP,
        PlacementError::CoordinateOutOfRange { .. } | PlacementError::InvalidCoordinate { .. } => {
            crate::protocol::codes::PLACEMENT_OUT_OF_RANGE
        }
        PlacementError::InvalidRotation { .. } => {
            crate::protocol::codes::PLACEMENT_ROTATION_INVALID
        }
        PlacementError::IdExhausted => crate::protocol::codes::PLACEMENT_STORE_FULL,
        _ => crate::protocol::codes::PLACEMENT_REJECTED,
    };
    reject_with(code, error.to_string())
}

fn reject_construction_error(error: ConstructionError) -> (Vec<ServerMessage>, &'static str) {
    let code = match &error {
        ConstructionError::TooManySites(_) => crate::protocol::codes::PLACEMENT_STORE_FULL,
        _ => crate::protocol::codes::PLACEMENT_REJECTED,
    };
    reject_with(code, error.to_string())
}

/// Result of one deterministic authority loopback run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorityReport {
    /// Client commands accepted or rejected by the executor.
    pub commands: u64,
    /// Application frames exchanged in both directions.
    pub frames: u64,
    /// Final world checksum.
    pub checksum: String,
    /// Final authoritative tick.
    pub tick: u64,
    /// Final treasury balance.
    pub balance: i64,
    /// Population at the end of the run.
    pub population: u32,
    /// Deterministic session identifier.
    pub session_id: String,
}

/// Run one deterministic authority exchange over TCP loopback:
/// hello → welcome, set_tax → snapshot, place_road → snapshot, ping → pong,
/// resume → snapshot, disconnect → goodbye.
///
/// Both sides use bounded socket timeouts; the harness never sleeps.
pub fn run_authority_loopback() -> io::Result<AuthorityReport> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let address = listener.local_addr()?;

    let server = thread::spawn(move || serve_authority(listener));
    let client_result = run_authority_client(address);
    let server_result = server
        .join()
        .map_err(|_| io::Error::other("authority loopback server thread panicked"))?;

    client_result?;
    server_result
}

/// Counters for one served session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct SessionStats {
    commands: u64,
    forbidden: u64,
    rate_limited: u64,
    resumed: bool,
}

fn response_code(response: &ServerMessage) -> Option<&str> {
    match response {
        ServerMessage::Reject { code, .. } => Some(code.as_str()),
        _ => None,
    }
}

/// Serve one accepted TCP connection with the authoritative executor.
///
/// Reads length-prefixed JSON commands until `disconnect`, EOF, or an I/O
/// error, and returns the number of application frames exchanged. A peer that
/// connects and closes without a frame is a clean session close.
pub fn serve_world_connection(stream: TcpStream, seed: u64, role: ClientRole) -> io::Result<u64> {
    serve_world_session(stream, ServerWorld::with_role(seed, role))
        .map(|(_world, frames, _stats)| frames)
}

fn serve_world_session(
    mut stream: TcpStream,
    mut world: ServerWorld,
) -> io::Result<(ServerWorld, u64, SessionStats)> {
    configure_stream(&stream)?;

    let mut frames = 0u64;
    let mut stats = SessionStats::default();

    loop {
        let payload = match read_frame(&mut stream) {
            Ok(payload) => payload,
            // EOF at the header or body boundary means the peer went away.
            Err(FrameError::Truncated { .. }) | Err(FrameError::Io(_)) => break,
            Err(error) => return Err(io::Error::other(error.to_string())),
        };
        frames += 1;
        stats.commands += 1;

        let responses = match decode_client_command(&payload) {
            Ok(command) => {
                let responses = world.apply_all(&command);
                for response in &responses {
                    match response_code(response) {
                        Some(codes::FORBIDDEN) => stats.forbidden += 1,
                        Some(codes::RATE_LIMITED) => stats.rate_limited += 1,
                        _ => {}
                    }
                    if matches!(response, ServerMessage::Welcome { resumed: true, .. }) {
                        stats.resumed = true;
                    }
                }
                responses
            }
            Err(rejection) => vec![rejection.into_message()],
        };
        for response in &responses {
            write_json_frame(&mut stream, response).map_err(frame_to_io)?;
            frames += 1;
        }

        if world.is_closed() {
            break;
        }
    }

    Ok((world, frames, stats))
}

fn serve_authority(listener: TcpListener) -> io::Result<AuthorityReport> {
    let (stream, _peer) = listener.accept()?;
    // `time_control` is admin-only, so the authority transcript runs as admin.
    let (world, frames, stats) = serve_world_session(
        stream,
        ServerWorld::with_role(LOOPBACK_SEED, ClientRole::Admin),
    )?;

    Ok(AuthorityReport {
        commands: stats.commands,
        frames,
        checksum: world.checksum(),
        tick: world.tick,
        balance: world.balance_units(),
        population: world.population,
        session_id: world.session_id().to_string(),
    })
}

fn run_authority_client(address: SocketAddr) -> io::Result<()> {
    let mut stream = TcpStream::connect_timeout(&address, SESSION_IO_TIMEOUT)?;
    configure_stream(&stream)?;

    // A local reference world pins the exact expected responses. Any transport
    // or state divergence fails the exchange.
    let mut reference = ServerWorld::with_role(LOOPBACK_SEED, ClientRole::Admin);
    let expected_population = SessionConfig::for_seed(LOOPBACK_SEED).population;

    let hello = ClientCommand::Hello {
        protocol_version: PROTOCOL_VERSION,
        content_hash: "0123456789abcdef".repeat(4),
        player_name: "authority".to_string(),
    };
    match exchange(&mut stream, &mut reference, &hello)? {
        ServerMessage::Welcome {
            session_id,
            tick,
            protocol_version,
            resumed: false,
            ..
        } if tick == 0
            && protocol_version == PROTOCOL_VERSION
            && session_id == session_id_for_seed(LOOPBACK_SEED) => {}
        other => return Err(unexpected("welcome", &other)),
    }

    let tax = ClientCommand::SetTax {
        resident_rate: 12,
        business_rate: 8,
    };
    match exchange(&mut stream, &mut reference, &tax)? {
        ServerMessage::Snapshot {
            tick,
            population,
            balance,
            checksum,
            ..
        } if tick == 1
            && population == expected_population
            && balance == STARTING_BALANCE as i64
            && checksum.len() == 16 => {}
        other => return Err(unexpected("set_tax snapshot", &other)),
    }

    let road = ClientCommand::PlaceRoad {
        start: [0, 0],
        end: [8, 0],
    };
    let road_snapshot = exchange(&mut stream, &mut reference, &road)?;
    match road_snapshot {
        ServerMessage::Snapshot {
            tick,
            population,
            balance,
            checksum,
            ..
        } if tick == 2
            && population == expected_population
            && balance == STARTING_BALANCE as i64
            && checksum.len() == 16 => {}
        other => return Err(unexpected("place_road snapshot", &other)),
    }

    match exchange(
        &mut stream,
        &mut reference,
        &ClientCommand::Ping { nonce: 7 },
    )? {
        ServerMessage::Pong { nonce: 7 } => {}
        other => return Err(unexpected("pong", &other)),
    }

    let resume = ClientCommand::TimeControl {
        action: TimeAction::Resume,
    };
    match exchange(&mut stream, &mut reference, &resume)? {
        ServerMessage::Snapshot { tick: 2, .. } => {}
        other => return Err(unexpected("resume snapshot", &other)),
    }

    match exchange(&mut stream, &mut reference, &ClientCommand::Disconnect)? {
        ServerMessage::Goodbye => {}
        other => return Err(unexpected("goodbye", &other)),
    }

    Ok(())
}

/// Result of one deterministic interest loopback run.
#[derive(Debug, Clone, PartialEq)]
pub struct InterestReport {
    /// Client commands accepted or rejected by the executor.
    pub commands: u64,
    /// Application frames exchanged in both directions.
    pub frames: u64,
    /// Final interest centre `[x, z]` in metres.
    pub center: [f32; 2],
    /// Final interest radius in metres.
    pub radius_m: f32,
    /// Roads whose midpoint was inside the final interest circle.
    pub visible_roads: u32,
    /// Road details carried by the final `interest_view`.
    pub view_roads: u32,
    /// Whether the final `interest_view` was truncated by the 64-road cap.
    pub view_truncated: bool,
    /// Building details carried by the final `interest_view`.
    pub view_buildings: u32,
    /// Whether the final `interest_view` was truncated by the 64-building cap.
    pub view_buildings_truncated: bool,
    /// Final authoritative tick (interest commands do not advance it).
    pub tick: u64,
    /// Final world checksum.
    pub checksum: String,
}

/// Run one deterministic interest exchange over TCP loopback.
///
/// Two roads are placed, one inside the final viewport and one far outside.
/// `set_interest` is then narrowed, moved, and restored; every response is
/// compared against a local reference world, including `visible_roads` and the
/// unchanged tick and checksum. Both sides use bounded socket timeouts; the
/// harness never sleeps.
pub fn run_interest_loopback() -> io::Result<InterestReport> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let address = listener.local_addr()?;

    let server = thread::spawn(move || serve_interest(listener));
    let client_result = run_interest_client(address);
    let server_result = server
        .join()
        .map_err(|_| io::Error::other("interest loopback server thread panicked"))?;

    client_result?;
    server_result
}

fn serve_interest(listener: TcpListener) -> io::Result<InterestReport> {
    let (stream, _peer) = listener.accept()?;
    // Roads need the mayor role; set_interest itself is open to viewers too.
    let (world, frames, stats) = serve_world_session(
        stream,
        ServerWorld::with_role(LOOPBACK_SEED, ClientRole::Mayor),
    )?;

    let ServerMessage::InterestView {
        roads,
        truncated,
        buildings,
        buildings_truncated,
        ..
    } = world.interest_view()
    else {
        return Err(io::Error::other(
            "interest view built an unexpected server message",
        ));
    };

    Ok(InterestReport {
        commands: stats.commands,
        frames,
        center: world.interest_center(),
        radius_m: world.interest_radius_m(),
        visible_roads: world.visible_roads(),
        view_roads: u32::try_from(roads.len()).unwrap_or(u32::MAX),
        view_truncated: truncated,
        view_buildings: u32::try_from(buildings.len()).unwrap_or(u32::MAX),
        view_buildings_truncated: buildings_truncated,
        tick: world.tick,
        checksum: world.checksum(),
    })
}

fn run_interest_client(address: SocketAddr) -> io::Result<()> {
    let mut stream = TcpStream::connect_timeout(&address, SESSION_IO_TIMEOUT)?;
    configure_stream(&stream)?;

    let mut reference = ServerWorld::with_role(LOOPBACK_SEED, ClientRole::Mayor);
    expect_welcome(&exchange(
        &mut stream,
        &mut reference,
        &loopback_hello("interest"),
    )?)?;

    for (expected_tick, expected_visible, road) in
        [(1, 1, ([0, 0], [8, 0])), (2, 2, ([100, 100], [108, 100]))]
    {
        match exchange(
            &mut stream,
            &mut reference,
            &ClientCommand::PlaceRoad {
                start: road.0,
                end: road.1,
            },
        )? {
            ServerMessage::Snapshot {
                tick,
                visible_roads,
                ..
            } if tick == expected_tick && visible_roads == expected_visible => {}
            other => return Err(unexpected("interest place_road snapshot", &other)),
        }
    }
    let checksum = reference.checksum();
    // The seeded city has 131 completed buildings, so the default viewport
    // exercises the 64-building cap rather than an empty list.
    assert!(
        reference.buildings > MAX_INTEREST_BUILDINGS as u32,
        "loopback needs more than {MAX_INTEREST_BUILDINGS} buildings to exercise truncation"
    );
    let full_buildings: Vec<u64> = (1..=MAX_INTEREST_BUILDINGS as u64).collect();

    let full_view = ClientCommand::SetInterest {
        center_x: 0.0,
        center_z: 0.0,
        radius_m: 2_048.0,
    };
    expect_interest_batch(
        &exchange_all(&mut stream, &mut reference, &full_view)?,
        &checksum,
        [0.0, 0.0],
        2_048.0,
        2,
        &[
            (1, [0, 0], [8, 0], WORLD_ROAD_WIDTH_M),
            (2, [100, 100], [108, 100], WORLD_ROAD_WIDTH_M),
        ],
        false,
        &full_buildings,
        true,
    )?;

    let near_origin = ClientCommand::SetInterest {
        center_x: 0.0,
        center_z: 0.0,
        radius_m: 16.0,
    };
    expect_interest_batch(
        &exchange_all(&mut stream, &mut reference, &near_origin)?,
        &checksum,
        [0.0, 0.0],
        16.0,
        1,
        &[(1, [0, 0], [8, 0], WORLD_ROAD_WIDTH_M)],
        false,
        &[1],
        false,
    )?;

    let near_far_road = ClientCommand::SetInterest {
        center_x: 100.0,
        center_z: 100.0,
        radius_m: 16.0,
    };
    expect_interest_batch(
        &exchange_all(&mut stream, &mut reference, &near_far_road)?,
        &checksum,
        [100.0, 100.0],
        16.0,
        1,
        &[(2, [100, 100], [108, 100], WORLD_ROAD_WIDTH_M)],
        false,
        &[],
        false,
    )?;

    // Restore the small origin viewport so the reported line shows filtering.
    expect_interest_batch(
        &exchange_all(&mut stream, &mut reference, &near_origin)?,
        &checksum,
        [0.0, 0.0],
        16.0,
        1,
        &[(1, [0, 0], [8, 0], WORLD_ROAD_WIDTH_M)],
        false,
        &[1],
        false,
    )?;

    expect_goodbye(&exchange(
        &mut stream,
        &mut reference,
        &ClientCommand::Disconnect,
    )?)
}

/// Assert one `set_interest` batch: snapshot first, filtered view second.
#[allow(clippy::too_many_arguments)]
fn expect_interest_batch(
    messages: &[ServerMessage],
    checksum: &str,
    expected_center: [f32; 2],
    expected_radius: f32,
    expected_visible: u32,
    expected_roads: &[(u64, [i32; 2], [i32; 2], u8)],
    expected_truncated: bool,
    expected_buildings: &[u64],
    expected_buildings_truncated: bool,
) -> io::Result<()> {
    match messages {
        [ServerMessage::Snapshot {
            tick: 2,
            checksum: observed,
            interest_center,
            interest_radius_m,
            visible_roads,
            ..
        }, ServerMessage::InterestView {
            tick: 2,
            center,
            radius_m,
            roads,
            truncated,
            buildings,
            buildings_truncated,
        }] if observed == checksum
            && *interest_center == expected_center
            && (*interest_radius_m - expected_radius).abs() < f32::EPSILON
            && *visible_roads == expected_visible
            && *center == expected_center
            && (*radius_m - expected_radius).abs() < f32::EPSILON
            && *truncated == expected_truncated
            && roads.len() == expected_roads.len()
            && roads.iter().zip(expected_roads).all(|(road, expected)| {
                road.id == expected.0
                    && road.start == expected.1
                    && road.end == expected.2
                    && road.width_m == expected.3
            })
            && buildings.len() == expected_buildings.len()
            && buildings
                .iter()
                .zip(expected_buildings)
                .all(|(building, id)| building == &synthetic_building(*id))
            && *buildings_truncated == expected_buildings_truncated =>
        {
            Ok(())
        }
        other => Err(io::Error::other(format!(
            "interest loopback expected snapshot + interest_view at {expected_center:?} \
             radius {expected_radius}, received {other:?}"
        ))),
    }
}

/// Result of one deterministic role/rate-limit loopback run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolesReport {
    /// Sessions served (viewer, mayor, admin).
    pub sessions: u64,
    /// Commands denied by the role matrix.
    pub role_checks: u64,
    /// Commands rejected by the deterministic token bucket.
    pub rate_limited: u64,
    /// Retained audit entries across every session.
    pub audit: u64,
    /// FNV-1a over every audited command, in session order.
    pub checksum: String,
}

/// Run three deterministic sessions over TCP loopback:
///
/// * viewer: `set_tax`, `place_road`, and `time_control` are forbidden;
/// * mayor: `set_tax` and `place_road` succeed, `time_control` is forbidden;
/// * admin: `step` succeeds, then pings until the token bucket rate-limits one.
///
/// Both sides use bounded socket timeouts; the harness never sleeps.
pub fn run_roles_loopback() -> io::Result<RolesReport> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let address = listener.local_addr()?;

    let server = thread::spawn(move || serve_roles(listener));
    let client_result = run_roles_client(address);
    let server_result = server
        .join()
        .map_err(|_| io::Error::other("roles loopback server thread panicked"))?;

    client_result?;
    server_result
}

fn serve_roles(listener: TcpListener) -> io::Result<RolesReport> {
    let mut report = RolesReport {
        sessions: 0,
        role_checks: 0,
        rate_limited: 0,
        audit: 0,
        checksum: String::new(),
    };
    let mut hash = FNV_OFFSET;

    for role in ClientRole::ALL {
        let (stream, _peer) = listener.accept()?;
        let (world, _frames, stats) =
            serve_world_session(stream, ServerWorld::with_role(LOOPBACK_SEED, role))?;
        report.sessions += 1;
        report.role_checks += stats.forbidden;
        report.rate_limited += stats.rate_limited;
        for entry in world.audit_state() {
            report.audit += 1;
            hash = audit_entry_hash(hash, &entry);
        }
    }

    report.checksum = format!("{hash:016x}");
    Ok(report)
}

fn run_roles_client(address: SocketAddr) -> io::Result<()> {
    run_viewer_role_session(address)?;
    run_mayor_role_session(address)?;
    run_admin_role_session(address)?;
    Ok(())
}

fn role_reference(address: SocketAddr, role: ClientRole) -> io::Result<(TcpStream, ServerWorld)> {
    let stream = TcpStream::connect_timeout(&address, SESSION_IO_TIMEOUT)?;
    configure_stream(&stream)?;
    Ok((stream, ServerWorld::with_role(LOOPBACK_SEED, role)))
}

fn loopback_hello(player_name: &str) -> ClientCommand {
    ClientCommand::Hello {
        protocol_version: PROTOCOL_VERSION,
        content_hash: "0123456789abcdef".repeat(4),
        player_name: player_name.to_string(),
    }
}

fn expect_welcome(message: &ServerMessage) -> io::Result<()> {
    match message {
        ServerMessage::Welcome {
            session_id,
            tick: 0,
            protocol_version,
            resumed: false,
            ..
        } if session_id == &session_id_for_seed(LOOPBACK_SEED)
            && *protocol_version == PROTOCOL_VERSION =>
        {
            Ok(())
        }
        other => Err(unexpected("welcome", other)),
    }
}

fn expect_goodbye(message: &ServerMessage) -> io::Result<()> {
    match message {
        ServerMessage::Goodbye => Ok(()),
        other => Err(unexpected("goodbye", other)),
    }
}

fn expect_forbidden(message: &ServerMessage) -> io::Result<()> {
    match message {
        ServerMessage::Reject { code, .. } if code == codes::FORBIDDEN => Ok(()),
        other => Err(unexpected("role.forbidden", other)),
    }
}

fn run_viewer_role_session(address: SocketAddr) -> io::Result<()> {
    let (mut stream, mut reference) = role_reference(address, ClientRole::Viewer)?;
    expect_welcome(&exchange(
        &mut stream,
        &mut reference,
        &loopback_hello("role"),
    )?)?;

    for command in [
        ClientCommand::SetTax {
            resident_rate: 10,
            business_rate: 10,
        },
        ClientCommand::PlaceRoad {
            start: [0, 0],
            end: [8, 0],
        },
        ClientCommand::TimeControl {
            action: TimeAction::Step,
        },
    ] {
        expect_forbidden(&exchange(&mut stream, &mut reference, &command)?)?;
    }

    match exchange(
        &mut stream,
        &mut reference,
        &ClientCommand::Ping { nonce: 1 },
    )? {
        ServerMessage::Pong { nonce: 1 } => {}
        other => return Err(unexpected("viewer pong", &other)),
    }
    expect_goodbye(&exchange(
        &mut stream,
        &mut reference,
        &ClientCommand::Disconnect,
    )?)
}

fn run_mayor_role_session(address: SocketAddr) -> io::Result<()> {
    let (mut stream, mut reference) = role_reference(address, ClientRole::Mayor)?;
    expect_welcome(&exchange(
        &mut stream,
        &mut reference,
        &loopback_hello("role"),
    )?)?;

    match exchange(
        &mut stream,
        &mut reference,
        &ClientCommand::SetTax {
            resident_rate: 12,
            business_rate: 8,
        },
    )? {
        ServerMessage::Snapshot { tick: 1, .. } => {}
        other => return Err(unexpected("mayor set_tax snapshot", &other)),
    }
    match exchange(
        &mut stream,
        &mut reference,
        &ClientCommand::PlaceRoad {
            start: [0, 0],
            end: [8, 0],
        },
    )? {
        ServerMessage::Snapshot { tick: 2, .. } => {}
        other => return Err(unexpected("mayor place_road snapshot", &other)),
    }
    expect_forbidden(&exchange(
        &mut stream,
        &mut reference,
        &ClientCommand::TimeControl {
            action: TimeAction::Step,
        },
    )?)?;

    match exchange(
        &mut stream,
        &mut reference,
        &ClientCommand::Ping { nonce: 2 },
    )? {
        ServerMessage::Pong { nonce: 2 } => {}
        other => return Err(unexpected("mayor pong", &other)),
    }
    expect_goodbye(&exchange(
        &mut stream,
        &mut reference,
        &ClientCommand::Disconnect,
    )?)
}

fn run_admin_role_session(address: SocketAddr) -> io::Result<()> {
    let (mut stream, mut reference) = role_reference(address, ClientRole::Admin)?;
    expect_welcome(&exchange(
        &mut stream,
        &mut reference,
        &loopback_hello("role"),
    )?)?;

    match exchange(
        &mut stream,
        &mut reference,
        &ClientCommand::TimeControl {
            action: TimeAction::Step,
        },
    )? {
        ServerMessage::Snapshot { tick: 1, .. } => {}
        other => return Err(unexpected("admin step snapshot", &other)),
    }

    // A full bucket plus the command-clock refill is a few dozen commands.
    let limit = RATE_BURST_CREDITS * 4;
    let mut limited = false;
    for nonce in 0..limit {
        match exchange(&mut stream, &mut reference, &ClientCommand::Ping { nonce })? {
            ServerMessage::Pong { nonce: echoed } if echoed == nonce => {}
            ServerMessage::Reject { code, .. } if code == codes::RATE_LIMITED => {
                limited = true;
                break;
            }
            other => return Err(unexpected("pong or rate.limited", &other)),
        }
    }
    if !limited {
        return Err(io::Error::other(format!(
            "rate limit was not reached within {limit} pings"
        )));
    }

    expect_goodbye(&exchange(
        &mut stream,
        &mut reference,
        &ClientCommand::Disconnect,
    )?)
}

/// Result of one deterministic reconnect loopback run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeReport {
    /// TCP sessions served (the initial session and the resumed one).
    pub sessions: u64,
    /// Client commands accepted or rejected by the executor.
    pub commands: u64,
    /// Application frames exchanged in both directions.
    pub frames: u64,
    /// Whether the second session resumed instead of sending `hello`.
    pub resumed: bool,
    /// Final authoritative tick.
    pub tick: u64,
    /// Final world checksum.
    pub checksum: String,
    /// Deterministic session identifier / token.
    pub session_id: String,
}

/// Run one deterministic reconnect exchange over TCP loopback.
///
/// Connection 1 runs hello, set-tax, and disconnect. Connection 2 resumes the
/// same in-memory world and receives a resumed welcome plus a snapshot before
/// disconnecting. The client compares every response against a local reference
/// world, so transport and state divergence fail the smoke. Both sides use
/// bounded socket timeouts; the harness never sleeps.
pub fn run_resume_loopback() -> io::Result<ResumeReport> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let address = listener.local_addr()?;

    let server = thread::spawn(move || serve_resume(listener));
    let client_result = run_resume_client(address);
    let server_result = server
        .join()
        .map_err(|_| io::Error::other("resume loopback server thread panicked"))?;

    client_result?;
    server_result
}

fn serve_resume(listener: TcpListener) -> io::Result<ResumeReport> {
    let (first, _peer) = listener.accept()?;
    let (mut world, first_frames, first_stats) = serve_world_session(
        first,
        ServerWorld::with_role(LOOPBACK_SEED, ClientRole::Admin),
    )?;

    // The reconnect happens on a new socket against the same world.
    world.begin_connection();

    let (second, _peer) = listener.accept()?;
    let (world, second_frames, second_stats) = serve_world_session(second, world)?;

    Ok(ResumeReport {
        sessions: 2,
        commands: first_stats.commands.saturating_add(second_stats.commands),
        frames: first_frames.saturating_add(second_frames),
        resumed: first_stats.resumed || second_stats.resumed,
        tick: world.tick,
        checksum: world.checksum(),
        session_id: world.session_id().to_string(),
    })
}

fn run_resume_client(address: SocketAddr) -> io::Result<()> {
    let mut reference = ServerWorld::with_role(LOOPBACK_SEED, ClientRole::Admin);

    let mut first = TcpStream::connect_timeout(&address, SESSION_IO_TIMEOUT)?;
    configure_stream(&first)?;

    let token = match exchange(&mut first, &mut reference, &loopback_hello("resume"))? {
        ServerMessage::Welcome {
            session_token,
            resumed: false,
            ..
        } => session_token,
        other => return Err(unexpected("fresh welcome", &other)),
    };

    match exchange(
        &mut first,
        &mut reference,
        &ClientCommand::SetTax {
            resident_rate: 12,
            business_rate: 8,
        },
    )? {
        ServerMessage::Snapshot { tick: 1, .. } => {}
        other => return Err(unexpected("set_tax snapshot", &other)),
    }

    expect_goodbye(&exchange(
        &mut first,
        &mut reference,
        &ClientCommand::Disconnect,
    )?)?;
    drop(first);

    // A new connection starts from the resumable record left by `disconnect`.
    reference.begin_connection();

    let mut second = TcpStream::connect_timeout(&address, SESSION_IO_TIMEOUT)?;
    configure_stream(&second)?;

    let resumed = exchange_all(
        &mut second,
        &mut reference,
        &ClientCommand::Resume {
            session_id: token.clone(),
            last_tick: 1,
        },
    )?;
    match resumed.as_slice() {
        [ServerMessage::Welcome {
            resumed: true,
            tick: 1,
            session_token,
            ..
        }, ServerMessage::Snapshot { tick: 1, .. }]
            if session_token == &token => {}
        other => {
            return Err(io::Error::other(format!(
                "resume loopback expected resumed welcome + snapshot, received {other:?}"
            )));
        }
    }

    expect_goodbye(&exchange(
        &mut second,
        &mut reference,
        &ClientCommand::Disconnect,
    )?)
}

fn exchange(
    stream: &mut TcpStream,
    reference: &mut ServerWorld,
    command: &ClientCommand,
) -> io::Result<ServerMessage> {
    exchange_all(stream, reference, command).map(|mut messages| messages.remove(0))
}

fn exchange_all(
    stream: &mut TcpStream,
    reference: &mut ServerWorld,
    command: &ClientCommand,
) -> io::Result<Vec<ServerMessage>> {
    let expected = reference.apply_all(command);
    write_json_frame(stream, command).map_err(frame_to_io)?;
    let mut actual = Vec::with_capacity(expected.len());
    for _ in 0..expected.len() {
        actual.push(read_json_frame::<_, ServerMessage>(stream).map_err(frame_to_io)?);
    }
    if actual != expected {
        return Err(io::Error::other(format!(
            "authority loopback mismatch for `{}`: expected {expected:?}, received {actual:?}",
            command.tag()
        )));
    }
    Ok(actual)
}

fn unexpected(expected: &str, actual: &ServerMessage) -> io::Error {
    io::Error::other(format!(
        "authority loopback expected {expected}, received {actual:?}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_city::economy::{
        DEFAULT_BUSINESS_TAX_PER_JOB_PER_DAY, DEFAULT_TAX_PER_RESIDENT_PER_DAY,
    };
    use core_city::population::WAGE_PER_WORKER_PER_DAY;
    use core_city::settlement::{SettlementRequest, SettlementRole};

    use crate::protocol::{codes as protocol_codes, decode_client_command, MAX_INTEREST_RADIUS_M};

    const SEED: u64 = LOOPBACK_SEED;

    fn hello() -> ClientCommand {
        ClientCommand::Hello {
            protocol_version: PROTOCOL_VERSION,
            content_hash: "0123456789abcdef".repeat(4),
            player_name: "mayor".to_string(),
        }
    }

    fn hello_named(player_name: &str) -> ClientCommand {
        ClientCommand::Hello {
            protocol_version: PROTOCOL_VERSION,
            content_hash: "0123456789abcdef".repeat(4),
            player_name: player_name.to_string(),
        }
    }

    fn first_response(messages: Vec<ServerMessage>) -> ServerMessage {
        messages
            .into_iter()
            .next()
            .expect("every command yields a response")
    }

    fn welcome_token(message: &ServerMessage) -> String {
        match message {
            ServerMessage::Welcome { session_token, .. } => session_token.clone(),
            other => panic!("expected welcome, got {other:?}"),
        }
    }

    fn vote(action: crate::protocol::VoteAction, time: Option<TimeAction>) -> ClientCommand {
        ClientCommand::TimeVote { action, time }
    }

    #[test]
    fn hello_content_hash_must_match_the_server_pin() {
        let mut world = ServerWorld::with_role(SEED, ClientRole::Admin);
        assert!(world.set_content_hash(Some("not-a-hash")).is_err());

        world
            .set_content_hash(Some(&"f".repeat(64)))
            .expect("valid pin");
        match world.apply(&hello()) {
            ServerMessage::Reject { code, .. } => {
                assert_eq!(code, protocol_codes::CONTENT_HASH_MISMATCH);
            }
            other => panic!("expected a content mismatch, got {other:?}"),
        }
        assert!(!world.hello_accepted());
        assert_eq!(
            world.players.count(),
            0,
            "rejection must not register a player"
        );

        world
            .set_content_hash(Some(&"0123456789abcdef".repeat(4)))
            .expect("matching pin");
        assert!(matches!(
            world.apply(&hello()),
            ServerMessage::Welcome { .. }
        ));

        world.set_content_hash(None).expect("clear pin");
        let mut unpinned = ServerWorld::with_role(SEED, ClientRole::Admin);
        assert!(matches!(
            unpinned.apply(&hello()),
            ServerMessage::Welcome { .. }
        ));
    }

    fn world_with_hello() -> ServerWorld {
        let mut world = ServerWorld::with_role(SEED, ClientRole::Admin);
        assert!(matches!(
            world.apply(&hello()),
            ServerMessage::Welcome { .. }
        ));
        world
    }

    fn world_with_buildings(buildings: u32) -> ServerWorld {
        let mut world =
            ServerWorld::with_population_and_role(SEED, buildings * 25, ClientRole::Mayor);
        assert_eq!(world.buildings, buildings);
        assert!(matches!(
            world.apply(&hello()),
            ServerMessage::Welcome { .. }
        ));
        world
    }

    /// One hand-built authoritative placement for interest-view tests.
    ///
    /// The interest view reads only `id`, `center`, and `footprint_m`, so the
    /// record does not need a resolvable package. Values match what a validated
    /// placement would carry.
    fn interest_placement(id: u64, x: f32, z: f32, footprint_m: [f32; 2]) -> Placement {
        Placement {
            id,
            blueprint_id: format!("local:interest_{id}"),
            blueprint_hash: format!("blake3:{id:064x}"),
            blueprint_path: format!("tsbp:blake3:{id:064x}"),
            blueprint_ref: None,
            name: format!("Interest House {id}"),
            center: [x, 40.0, z],
            rotation_deg: 0,
            footprint_m,
            bounds_m: [footprint_m[0], 8.0, footprint_m[1]],
            foundation_m: 0.0,
            material_cost: 1.0,
            resident_capacity: 1,
            job_slots: 0,
            solid_voxels: 1,
            structure: None,
        }
    }

    fn world_with_placements(placements: Vec<Placement>) -> ServerWorld {
        let mut world = world_with_hello();
        let mut store = PlacementStore::new(SEED);
        store.next_id = placements
            .iter()
            .map(|placement| placement.id)
            .max()
            .unwrap_or(0)
            + 1;
        store.placements = placements;
        world.placements = Some(store);
        world
    }

    fn expect_reject(message: &ServerMessage, expected_code: &str) {
        match message {
            ServerMessage::Reject { code, message: _ } => assert_eq!(code, expected_code),
            other => panic!("expected rejection `{expected_code}`, got {other:?}"),
        }
    }

    fn expect_snapshot(message: &ServerMessage) -> (u64, u32, i64, String) {
        match message {
            ServerMessage::Snapshot {
                tick,
                population,
                balance,
                checksum,
                ..
            } => (*tick, *population, *balance, checksum.clone()),
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    fn set_interest(center_x: f64, center_z: f64, radius_m: f64) -> ClientCommand {
        ClientCommand::SetInterest {
            center_x,
            center_z,
            radius_m,
        }
    }

    fn interest_building_view(world: &ServerWorld) -> (Vec<BuildingView>, bool) {
        match world.interest_view() {
            ServerMessage::InterestView {
                buildings,
                buildings_truncated,
                ..
            } => (buildings, buildings_truncated),
            other => panic!("expected interest view, got {other:?}"),
        }
    }

    #[test]
    fn new_worlds_start_with_the_full_default_viewport() {
        let world = ServerWorld::new(SEED);
        assert_eq!(world.interest_center(), [0.0, 0.0]);
        assert_eq!(world.interest_radius_m(), DEFAULT_INTEREST_RADIUS_M as f32);
        assert_eq!(world.visible_roads(), 0);

        match world.snapshot() {
            ServerMessage::Snapshot {
                interest_center,
                interest_radius_m,
                visible_roads: 0,
                ..
            } => {
                assert_eq!(interest_center, [0.0, 0.0]);
                assert_eq!(interest_radius_m, DEFAULT_INTEREST_RADIUS_M as f32);
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    #[test]
    fn set_interest_updates_the_viewport_without_advancing_the_tick() {
        let mut world = world_with_hello();
        world.apply(&ClientCommand::PlaceRoad {
            start: [0, 0],
            end: [8, 0],
        });
        let tick = world.tick;
        let checksum = world.checksum();

        match world.apply(&set_interest(4.0, 0.0, 1.0)) {
            ServerMessage::Snapshot {
                tick: observed_tick,
                checksum: observed_checksum,
                interest_center,
                interest_radius_m,
                visible_roads,
                ..
            } => {
                assert_eq!(observed_tick, tick);
                assert_eq!(observed_checksum, checksum);
                assert_eq!(interest_center, [4.0, 0.0]);
                assert_eq!(interest_radius_m, 1.0);
                assert_eq!(visible_roads, 1);
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
        assert_eq!(world.tick, tick, "set_interest is view-only");
        assert_eq!(world.interest_center(), [4.0, 0.0]);
        assert_eq!(world.interest_radius_m(), 1.0);
    }

    #[test]
    fn invalid_interest_is_rejected_without_mutation() {
        let mut world = world_with_hello();
        let before = world.checksum();

        for (command, code) in [
            (
                set_interest(f64::NAN, 0.0, 10.0),
                protocol_codes::INTEREST_INVALID_CENTER,
            ),
            (
                set_interest(0.0, f64::NEG_INFINITY, 10.0),
                protocol_codes::INTEREST_INVALID_CENTER,
            ),
            (
                set_interest(0.0, 0.0, -1.0),
                protocol_codes::INTEREST_RADIUS_OUT_OF_RANGE,
            ),
            (
                set_interest(0.0, 0.0, MAX_INTEREST_RADIUS_M + 0.1),
                protocol_codes::INTEREST_RADIUS_OUT_OF_RANGE,
            ),
            (
                set_interest(0.0, 0.0, f64::INFINITY),
                protocol_codes::INTEREST_RADIUS_OUT_OF_RANGE,
            ),
        ] {
            expect_reject(&world.apply(&command), code);
        }

        assert_eq!(world.tick, 0);
        assert_eq!(world.interest_center(), [0.0, 0.0]);
        assert_eq!(world.interest_radius_m(), DEFAULT_INTEREST_RADIUS_M as f32);
        assert_eq!(world.checksum(), before);
    }

    #[test]
    fn interest_is_excluded_from_the_world_checksum() {
        let mut world = world_with_hello();
        world.apply(&ClientCommand::PlaceRoad {
            start: [0, 0],
            end: [8, 0],
        });
        let before = world.checksum();

        world.apply(&set_interest(100.0, 100.0, 8.0));
        assert_eq!(world.checksum(), before);
        assert_eq!(world.session_checksum, before);

        world.apply(&set_interest(4.0, 0.0, 2_048.0));
        assert_eq!(world.checksum(), before);
    }

    #[test]
    fn viewer_role_may_set_its_own_interest() {
        let mut world = ServerWorld::with_role(SEED, ClientRole::Viewer);
        assert!(matches!(
            world.apply(&hello()),
            ServerMessage::Welcome { .. }
        ));
        assert_eq!(
            ClientRole::required_role_for(&set_interest(0.0, 0.0, 32.0)),
            ClientRole::Viewer
        );

        match world.apply(&set_interest(6.0, -2.0, 32.0)) {
            ServerMessage::Snapshot {
                interest_center,
                interest_radius_m,
                ..
            } => {
                assert_eq!(interest_center, [6.0, -2.0]);
                assert_eq!(interest_radius_m, 32.0);
            }
            other => panic!("expected snapshot, got {other:?}"),
        }

        let audit = world.audit_state();
        let last = audit.last().expect("audit entry");
        assert_eq!(last.tag, "set_interest");
        assert_eq!(last.role, ClientRole::Viewer);
        assert!(last.accepted());
    }

    #[test]
    fn visible_roads_use_an_inclusive_midpoint_distance() {
        let mut world = world_with_hello();
        for (start, end) in [
            ([-3, -2], [3, -2]), // centre (0,-2): inside
            ([8, 0], [12, 0]),   // centre (10,0): exactly on the r=10 boundary
            ([11, 1], [13, 1]),  // centre (12,1): outside
        ] {
            assert!(matches!(
                world.apply(&ClientCommand::PlaceRoad { start, end }),
                ServerMessage::Snapshot { .. }
            ));
        }
        assert_eq!(world.visible_roads(), 3, "default radius sees every road");

        world.apply(&set_interest(0.0, 0.0, 10.0));
        assert_eq!(world.visible_roads(), 2, "inside plus boundary");

        world.apply(&set_interest(0.0, 0.0, 9.9));
        assert_eq!(world.visible_roads(), 1, "boundary is exclusive at 9.9 m");

        world.apply(&set_interest(10.0, 0.0, 0.0));
        assert_eq!(
            world.visible_roads(),
            1,
            "a zero radius still includes a road whose midpoint is the centre"
        );

        world.apply(&set_interest(100.0, 100.0, 10.0));
        assert_eq!(world.visible_roads(), 0);
    }

    #[test]
    fn interest_view_is_empty_when_no_road_midpoint_matches() {
        let mut world = world_with_hello();
        world.apply(&ClientCommand::PlaceRoad {
            start: [10_000, 10_000],
            end: [10_008, 10_000],
        });
        let tick = world.tick;
        let checksum = world.checksum();

        assert!(world.interest_roads().is_empty());
        match world.interest_view() {
            ServerMessage::InterestView {
                tick: view_tick,
                center,
                radius_m,
                roads,
                truncated,
                ..
            } => {
                assert_eq!(view_tick, tick);
                assert_eq!(center, [0.0, 0.0]);
                assert_eq!(radius_m, DEFAULT_INTEREST_RADIUS_M as f32);
                assert!(roads.is_empty(), "the only road is outside the view");
                assert!(!truncated);
            }
            other => panic!("expected interest view, got {other:?}"),
        }

        let messages = world.apply_all(&set_interest(0.0, 0.0, 16.0));
        match messages.as_slice() {
            [ServerMessage::Snapshot {
                tick: snapshot_tick,
                visible_roads: 0,
                ..
            }, ServerMessage::InterestView {
                roads, truncated, ..
            }] => {
                assert_eq!(*snapshot_tick, tick);
                assert!(roads.is_empty());
                assert!(!truncated);
            }
            other => panic!("expected snapshot + empty interest view, got {other:?}"),
        }
        assert_eq!(world.checksum(), checksum);
    }

    #[test]
    fn interest_view_filters_by_inclusive_midpoint() {
        let mut world = world_with_hello();
        for (start, end) in [
            ([-3, -2], [3, -2]), // centre (0,-2): inside
            ([8, 0], [12, 0]),   // centre (10,0): exactly on the r=10 boundary
            ([11, 1], [13, 1]),  // centre (12,1): outside
        ] {
            world.apply(&ClientCommand::PlaceRoad { start, end });
        }

        let messages = world.apply_all(&set_interest(0.0, 0.0, 10.0));
        match messages.as_slice() {
            [ServerMessage::Snapshot { .. }, ServerMessage::InterestView {
                roads, truncated, ..
            }] => {
                assert_eq!(
                    roads.iter().map(|road| road.id).collect::<Vec<_>>(),
                    vec![1, 2],
                    "inside plus boundary, ascending id"
                );
                assert!(!truncated);
            }
            other => panic!("expected snapshot + interest view, got {other:?}"),
        }

        let messages = world.apply_all(&set_interest(0.0, 0.0, 9.9));
        match messages.as_slice() {
            [_, ServerMessage::InterestView { roads, .. }] => {
                assert_eq!(roads.len(), 1, "boundary is exclusive at 9.9 m");
            }
            other => panic!("expected snapshot + interest view, got {other:?}"),
        }

        // A zero radius still includes a road whose midpoint is the centre.
        let messages = world.apply_all(&set_interest(10.0, 0.0, 0.0));
        match messages.as_slice() {
            [_, ServerMessage::InterestView { roads, .. }] => {
                assert_eq!(roads.len(), 1);
                assert_eq!(roads[0].id, 2);
                assert_eq!(roads[0].start, [8, 0]);
                assert_eq!(roads[0].end, [12, 0]);
                assert_eq!(roads[0].width_m, WORLD_ROAD_WIDTH_M);
            }
            other => panic!("expected snapshot + interest view, got {other:?}"),
        }
    }

    #[test]
    fn interest_view_caps_at_64_and_reports_truncation() {
        let mut world = world_with_hello();
        for index in 0..70 {
            let z = index * 4;
            assert!(matches!(
                world.apply(&ClientCommand::PlaceRoad {
                    start: [10, z],
                    end: [18, z],
                }),
                ServerMessage::Snapshot { .. }
            ));
        }
        world.apply(&set_interest(14.0, 0.0, 2_048.0));
        assert_eq!(world.visible_roads(), 70);
        assert_eq!(world.interest_roads().len(), MAX_INTEREST_ROADS);
        assert_eq!(world.interest_roads()[0].id, 1);

        match world.interest_view() {
            ServerMessage::InterestView {
                roads, truncated, ..
            } => {
                assert_eq!(roads.len(), MAX_INTEREST_ROADS);
                assert!(truncated, "70 matches must mark the view truncated");
                assert_eq!(
                    roads.iter().map(|road| road.id).collect::<Vec<_>>(),
                    (1..=MAX_INTEREST_ROADS as u64).collect::<Vec<_>>(),
                    "the cap keeps the lowest ids"
                );
            }
            other => panic!("expected interest view, got {other:?}"),
        }
    }

    #[test]
    fn interest_view_is_not_truncated_at_exactly_the_cap() {
        let mut world = world_with_hello();
        for index in 0..MAX_INTEREST_ROADS {
            let z = i32::try_from(index).expect("small index") * 4;
            world.apply(&ClientCommand::PlaceRoad {
                start: [10, z],
                end: [18, z],
            });
        }
        world.apply(&set_interest(14.0, 0.0, 2_048.0));
        assert_eq!(world.visible_roads(), MAX_INTEREST_ROADS as u32);

        match world.interest_view() {
            ServerMessage::InterestView {
                roads, truncated, ..
            } => {
                assert_eq!(roads.len(), MAX_INTEREST_ROADS);
                assert!(!truncated, "exactly the cap is not truncated");
            }
            other => panic!("expected interest view, got {other:?}"),
        }
    }

    #[test]
    fn interest_view_orders_roads_by_id_even_when_the_store_is_shuffled() {
        let mut world = world_with_hello();
        for index in 0..5 {
            world.apply(&ClientCommand::PlaceRoad {
                start: [10, index * 4],
                end: [18, index * 4],
            });
        }
        world.roads.roads.reverse();
        world.roads.roads.swap(0, 2);
        world.apply(&set_interest(14.0, 0.0, 2_048.0));

        match world.interest_view() {
            ServerMessage::InterestView { roads, .. } => {
                assert_eq!(
                    roads.iter().map(|road| road.id).collect::<Vec<_>>(),
                    vec![1, 2, 3, 4, 5]
                );
            }
            other => panic!("expected interest view, got {other:?}"),
        }
    }

    #[test]
    fn interest_buildings_are_empty_without_completed_placements() {
        let mut world = world_with_hello();
        world.buildings = 0;

        let (buildings, truncated) = interest_building_view(&world);
        assert!(buildings.is_empty());
        assert!(!truncated, "an empty world cannot drop building records");
        assert!(world.interest_buildings().is_empty());
    }

    #[test]
    fn interest_buildings_carry_exact_centres_in_ascending_id_order() {
        let mut world = world_with_buildings(3);
        world.apply(&set_interest(0.0, 0.0, 200.0));

        let (buildings, truncated) = interest_building_view(&world);
        assert!(!truncated);
        assert_eq!(
            buildings,
            vec![
                BuildingView {
                    id: 1,
                    center: [0.0, 0.0],
                    radius_m: INTEREST_BUILDING_RADIUS_M,
                },
                BuildingView {
                    id: 2,
                    center: [INTEREST_BUILDING_SPACING_M, 0.0],
                    radius_m: INTEREST_BUILDING_RADIUS_M,
                },
                BuildingView {
                    id: 3,
                    center: [2.0 * INTEREST_BUILDING_SPACING_M, 0.0],
                    radius_m: INTEREST_BUILDING_RADIUS_M,
                },
            ]
        );
    }

    #[test]
    fn interest_buildings_cap_at_64_and_report_truncation() {
        let mut world = world_with_buildings(100);
        world.apply(&set_interest(0.0, 0.0, DEFAULT_INTEREST_RADIUS_M));
        let (buildings, truncated) = interest_building_view(&world);

        assert_eq!(buildings.len(), MAX_INTEREST_BUILDINGS);
        assert!(truncated, "100 matches must mark the view truncated");
        assert_eq!(
            buildings
                .iter()
                .map(|building| building.id)
                .collect::<Vec<_>>(),
            (1..=MAX_INTEREST_BUILDINGS as u64).collect::<Vec<_>>(),
            "the cap keeps the lowest ids in deterministic order"
        );

        let mut exact = world_with_buildings(MAX_INTEREST_BUILDINGS as u32);
        exact.apply(&set_interest(0.0, 0.0, DEFAULT_INTEREST_RADIUS_M));
        let (buildings, truncated) = interest_building_view(&exact);
        assert_eq!(buildings.len(), MAX_INTEREST_BUILDINGS);
        assert!(!truncated, "exactly the cap is not truncated");
    }

    #[test]
    fn interest_buildings_outside_the_radius_are_excluded() {
        let mut world = world_with_buildings(10);

        world.apply(&set_interest(0.0, 0.0, 16.0));
        let (buildings, truncated) = interest_building_view(&world);
        assert_eq!(
            buildings
                .iter()
                .map(|building| building.id)
                .collect::<Vec<_>>(),
            vec![1],
            "64 m spacing leaves only the origin building inside r=16"
        );
        assert!(!truncated);

        // A zero radius still includes a building whose centre is the centre.
        world.apply(&set_interest(INTEREST_BUILDING_SPACING_M as f64, 0.0, 0.0));
        let (buildings, _) = interest_building_view(&world);
        assert_eq!(
            buildings
                .iter()
                .map(|building| building.id)
                .collect::<Vec<_>>(),
            vec![2]
        );

        world.apply(&set_interest(100.0, 100.0, 16.0));
        let (buildings, truncated) = interest_building_view(&world);
        assert!(
            buildings.is_empty(),
            "every synthetic centre is out of range"
        );
        assert!(!truncated);
    }

    #[test]
    fn interest_building_view_is_deterministic_across_calls_and_worlds() {
        let mut world = world_with_buildings(80);
        world.apply(&set_interest(32.0, 100.0, 512.0));

        let first = world.interest_view();
        let second = world.interest_view();
        assert_eq!(first, second);
        let first_json = serde_json::to_string(&first).expect("interest view serializes");
        let second_json = serde_json::to_string(&second).expect("interest view serializes");
        assert_eq!(first_json, second_json, "same view, same wire payload");
        assert!(
            first_json.contains("\"buildings\":[{"),
            "the payload actually carries building records"
        );

        let mut replay = world_with_buildings(80);
        replay.apply(&set_interest(32.0, 100.0, 512.0));
        assert_eq!(
            serde_json::to_string(&replay.interest_view()).expect("replay serializes"),
            first_json,
            "an identically seeded world produces the same building records"
        );
    }

    #[test]
    fn interest_buildings_without_a_placement_store_keep_the_synthetic_pin() {
        let mut world = ServerWorld::with_role(SEED, ClientRole::Mayor);
        world.apply(&loopback_hello("interest"));
        world.apply(&ClientCommand::PlaceRoad {
            start: [0, 0],
            end: [8, 0],
        });
        world.apply(&ClientCommand::PlaceRoad {
            start: [100, 100],
            end: [108, 100],
        });
        world.apply(&set_interest(0.0, 0.0, 2_048.0));

        let (buildings, truncated) = interest_building_view(&world);
        assert_eq!(buildings.len(), MAX_INTEREST_BUILDINGS);
        assert!(truncated, "the default viewport still exercises the cap");
        assert_eq!(
            buildings,
            (1..=MAX_INTEREST_BUILDINGS as u64)
                .map(synthetic_building)
                .collect::<Vec<_>>(),
            "no placement store must keep the exact synthetic fixture"
        );
        assert_eq!(
            world.checksum(),
            "6d92be74bdbc11f5",
            "the default interest-world pin must not move"
        );
    }

    #[test]
    fn interest_buildings_use_placement_records_instead_of_the_fixture() {
        let mut world = world_with_placements(vec![interest_placement(7, 12.0, -4.0, [16.0, 9.0])]);
        assert!(
            world.buildings > 1,
            "the synthetic fixture would carry several records"
        );
        world.apply(&set_interest(0.0, 0.0, 64.0));

        let (buildings, truncated) = interest_building_view(&world);
        assert!(!truncated);
        assert_eq!(
            buildings,
            vec![BuildingView {
                id: 7,
                center: [12.0, -4.0],
                radius_m: 8.0,
            }]
        );
        assert_eq!(world.interest_buildings(), buildings);
    }

    #[test]
    fn interest_buildings_filter_placements_by_the_inclusive_circle() {
        let mut world = world_with_placements(vec![
            interest_placement(2, 10.0, 0.0, [4.0, 4.0]),
            interest_placement(1, 100.0, 0.0, [20.0, 10.0]),
        ]);

        // A zero radius includes the placement whose centre is the viewport
        // centre and excludes the other one.
        world.apply(&set_interest(10.0, 0.0, 0.0));
        let (buildings, truncated) = interest_building_view(&world);
        assert!(!truncated);
        assert_eq!(
            buildings,
            vec![BuildingView {
                id: 2,
                center: [10.0, 0.0],
                radius_m: 2.0,
            }]
        );

        // An exact boundary match is inclusive; both placements are in range
        // and the shuffled store is reported in ascending id order.
        world.apply(&set_interest(50.0, 0.0, 50.0));
        let (buildings, truncated) = interest_building_view(&world);
        assert!(!truncated);
        assert_eq!(
            buildings,
            vec![
                BuildingView {
                    id: 1,
                    center: [100.0, 0.0],
                    radius_m: 10.0,
                },
                BuildingView {
                    id: 2,
                    center: [10.0, 0.0],
                    radius_m: 2.0,
                },
            ]
        );
    }

    #[test]
    fn interest_buildings_cap_placement_records_and_report_truncation() {
        let placements: Vec<Placement> = (1..=MAX_INTEREST_BUILDINGS as u64 + 1)
            .rev()
            .map(|id| interest_placement(id, id as f32, 0.0, [2.0, 2.0]))
            .collect();
        let mut world = world_with_placements(placements);
        world.apply(&set_interest(0.0, 0.0, DEFAULT_INTEREST_RADIUS_M));

        let (buildings, truncated) = interest_building_view(&world);
        assert_eq!(buildings.len(), MAX_INTEREST_BUILDINGS);
        assert!(truncated, "65 matches must mark the view truncated");
        assert_eq!(
            buildings
                .iter()
                .map(|building| building.id)
                .collect::<Vec<_>>(),
            (1..=MAX_INTEREST_BUILDINGS as u64).collect::<Vec<_>>(),
            "the cap keeps the lowest ids in ascending order"
        );

        let mut exact = world_with_placements(
            (1..=MAX_INTEREST_BUILDINGS as u64)
                .rev()
                .map(|id| interest_placement(id, id as f32, 0.0, [2.0, 2.0]))
                .collect(),
        );
        exact.apply(&set_interest(0.0, 0.0, DEFAULT_INTEREST_RADIUS_M));
        let (buildings, truncated) = interest_building_view(&exact);
        assert_eq!(buildings.len(), MAX_INTEREST_BUILDINGS);
        assert!(!truncated, "exactly the cap is not truncated");
    }

    #[test]
    fn set_interest_after_a_placement_returns_it_without_moving_the_checksum() {
        let mut world = world_with_placements(vec![interest_placement(3, 8.0, 0.0, [6.0, 6.0])]);
        let checksum = world.checksum();
        assert_eq!(
            interest_building_view(&world).0,
            vec![BuildingView {
                id: 3,
                center: [8.0, 0.0],
                radius_m: 3.0,
            }],
            "the default viewport already reports the placement"
        );

        let messages = world.apply_all(&set_interest(4.0, 0.0, 16.0));
        match messages.as_slice() {
            [ServerMessage::Snapshot {
                checksum: observed, ..
            }, ServerMessage::InterestView { buildings, .. }] => {
                assert_eq!(observed, &checksum);
                assert_eq!(
                    buildings,
                    &vec![BuildingView {
                        id: 3,
                        center: [8.0, 0.0],
                        radius_m: 3.0,
                    }]
                );
            }
            other => panic!("expected snapshot + interest view, got {other:?}"),
        }
        assert_eq!(world.checksum(), checksum, "set_interest is view-only");
    }

    #[test]
    fn empty_placement_store_falls_back_to_the_synthetic_fixture() {
        let mut no_store = world_with_hello();
        no_store.apply(&set_interest(0.0, 0.0, 200.0));

        let mut empty = world_with_hello();
        empty.placements = Some(PlacementStore::new(SEED));
        empty.apply(&set_interest(0.0, 0.0, 200.0));

        let (buildings, truncated) = interest_building_view(&empty);
        assert!(
            !buildings.is_empty(),
            "the synthetic fallback still reports"
        );
        assert!(!truncated);
        assert_eq!(
            (buildings, truncated),
            interest_building_view(&no_store),
            "an empty store must behave exactly like no store"
        );
        assert_eq!(empty.interest_view(), no_store.interest_view());
    }

    #[test]
    fn placement_building_radius_is_finite_and_positive() {
        let mut placement = interest_placement(1, 0.0, 0.0, [16.0, 9.0]);
        assert_eq!(placement_building(&placement).radius_m, 8.0);

        for footprint in [
            [0.0, 0.0],
            [-4.0, -4.0],
            [f32::NAN, f32::NAN],
            [f32::INFINITY, 4.0],
        ] {
            placement.footprint_m = footprint;
            let radius = placement_building(&placement).radius_m;
            assert!(
                radius.is_finite() && radius > 0.0,
                "footprint {footprint:?} produced an invalid radius {radius}"
            );
        }
    }

    #[test]
    fn malformed_interest_is_rejected_without_changing_building_records() {
        let mut world = world_with_buildings(80);
        world.apply(&set_interest(32.0, 100.0, 512.0));
        let before = world.interest_view();

        for (command, code) in [
            (
                set_interest(f64::NAN, 0.0, 16.0),
                protocol_codes::INTEREST_INVALID_CENTER,
            ),
            (
                set_interest(0.0, 0.0, -1.0),
                protocol_codes::INTEREST_RADIUS_OUT_OF_RANGE,
            ),
            (
                set_interest(0.0, 0.0, MAX_INTEREST_RADIUS_M + 0.1),
                protocol_codes::INTEREST_RADIUS_OUT_OF_RANGE,
            ),
        ] {
            match world.apply_all(&command).as_slice() {
                [ServerMessage::Reject { code: actual, .. }] => {
                    assert_eq!(actual.as_str(), code);
                }
                other => panic!("expected one rejection, got {other:?}"),
            }
            assert_eq!(
                world.interest_view(),
                before,
                "a rejected request must not move the building view"
            );
        }

        let mut unopened = ServerWorld::with_population_and_role(SEED, 2_000, ClientRole::Mayor);
        assert_eq!(unopened.buildings, 80);
        expect_reject(
            &unopened.apply(&set_interest(0.0, 0.0, 16.0)),
            codes::NOT_HELLO,
        );
        assert_eq!(
            unopened.interest_radius_m(),
            DEFAULT_INTEREST_RADIUS_M as f32,
            "the rejected command must not store the viewport"
        );
    }

    #[test]
    fn set_interest_returns_snapshot_then_view_without_mutating_simulation() {
        let mut world = world_with_hello();
        world.apply(&ClientCommand::PlaceRoad {
            start: [0, 0],
            end: [8, 0],
        });
        let tick = world.tick;
        let checksum = world.checksum();
        let balance = world.economy.balance;
        let roads = world.roads.roads.len();
        let commands = world.commands;

        let messages = world.apply_all(&set_interest(4.0, 0.0, 16.0));
        match messages.as_slice() {
            [ServerMessage::Snapshot {
                tick: observed_tick,
                checksum: observed_checksum,
                interest_center,
                interest_radius_m,
                visible_roads,
                ..
            }, ServerMessage::InterestView {
                tick: view_tick,
                center,
                radius_m,
                roads: view_roads,
                truncated,
                buildings,
                buildings_truncated,
            }] => {
                assert_eq!(*observed_tick, tick);
                assert_eq!(observed_checksum, &checksum);
                assert_eq!(*interest_center, [4.0, 0.0]);
                assert_eq!(*interest_radius_m, 16.0);
                assert_eq!(*visible_roads, 1);
                assert_eq!(*view_tick, tick);
                assert_eq!(*center, [4.0, 0.0]);
                assert_eq!(*radius_m, 16.0);
                assert_eq!(view_roads.len(), 1);
                assert!(!truncated);
                assert_eq!(buildings.len(), 1, "only the origin building is in range");
                assert_eq!(buildings[0], synthetic_building(1));
                assert!(!buildings_truncated);
            }
            other => panic!("expected snapshot + interest view, got {other:?}"),
        }

        assert_eq!(world.tick, tick, "set_interest is view-only");
        assert_eq!(world.checksum(), checksum);
        assert_eq!(world.economy.balance, balance);
        assert_eq!(world.roads.roads.len(), roads);
        assert_eq!(world.commands, commands + 1);
        assert_eq!(
            world.audit_state().last().expect("audit").tag,
            "set_interest"
        );
        assert!(world.audit_state().last().expect("audit").accepted());
    }

    #[test]
    fn viewer_set_interest_batch_is_allowed() {
        let mut world = ServerWorld::with_role(SEED, ClientRole::Viewer);
        world.apply(&hello());
        let messages = world.apply_all(&set_interest(6.0, -2.0, 32.0));

        match messages.as_slice() {
            [ServerMessage::Snapshot { .. }, ServerMessage::InterestView { .. }] => {}
            other => panic!("expected snapshot + interest view, got {other:?}"),
        }
        let audit = world.audit_state();
        let last = audit.last().expect("audit entry");
        assert_eq!(last.tag, "set_interest");
        assert_eq!(last.role, ClientRole::Viewer);
        assert!(last.accepted());
    }

    #[test]
    fn rejected_interest_returns_one_rejection_and_no_view() {
        let mut world = world_with_hello();
        let before = world.checksum();
        let messages = world.apply_all(&set_interest(0.0, 0.0, -1.0));

        match messages.as_slice() {
            [ServerMessage::Reject { code, .. }] => {
                assert_eq!(code, protocol_codes::INTEREST_RADIUS_OUT_OF_RANGE);
            }
            other => panic!("expected one rejection, got {other:?}"),
        }
        assert_eq!(world.interest_radius_m(), DEFAULT_INTEREST_RADIUS_M as f32);
        assert_eq!(world.checksum(), before);
        assert_eq!(world.tick, 0);
    }

    #[test]
    fn rate_limited_interest_returns_one_rejection_and_no_view() {
        let mut world = world_with_hello();
        world.rate.milli_credits = 0;
        let messages = world.apply_all(&set_interest(0.0, 0.0, 16.0));

        match messages.as_slice() {
            [ServerMessage::Reject { code, .. }] => {
                assert_eq!(code, codes::RATE_LIMITED);
            }
            other => panic!("expected one rejection, got {other:?}"),
        }
        assert_eq!(world.interest_radius_m(), DEFAULT_INTEREST_RADIUS_M as f32);
    }

    #[test]
    fn set_interest_consumes_a_rate_credit() {
        let mut world = world_with_hello();
        world.rate.milli_credits = 0;
        expect_reject(
            &world.apply(&set_interest(0.0, 0.0, 16.0)),
            codes::RATE_LIMITED,
        );
        assert_eq!(world.interest_radius_m(), DEFAULT_INTEREST_RADIUS_M as f32);
    }

    #[test]
    fn interest_loopback_is_deterministic() {
        let first = run_interest_loopback().expect("interest loopback");
        assert_eq!(first.commands, INTEREST_COMMANDS);
        assert_eq!(first.frames, INTEREST_FRAMES);
        assert_eq!(first.center, [0.0, 0.0]);
        assert_eq!(first.radius_m, 16.0);
        assert_eq!(first.visible_roads, 1);
        assert_eq!(first.view_roads, 1);
        assert!(!first.view_truncated);
        assert_eq!(first.view_buildings, 1);
        assert!(!first.view_buildings_truncated);
        assert_eq!(first.tick, 2);
        assert_eq!(first.checksum.len(), 16);
        assert!(first.checksum.bytes().all(|byte| byte.is_ascii_hexdigit()));

        let second = run_interest_loopback().expect("second interest loopback");
        assert_eq!(first, second);
    }

    #[test]
    fn interest_loopback_snapshot_keeps_the_default_checksum_pin() {
        let report = run_interest_loopback().expect("interest loopback");
        let mut replay = ServerWorld::new(SEED);
        replay.apply(&hello());
        replay.apply(&ClientCommand::PlaceRoad {
            start: [0, 0],
            end: [8, 0],
        });
        replay.apply(&ClientCommand::PlaceRoad {
            start: [100, 100],
            end: [108, 100],
        });
        replay.apply(&set_interest(0.0, 0.0, 16.0));
        assert_eq!(report.checksum, replay.checksum());
        assert_eq!(report.tick, replay.tick);
    }

    #[test]
    fn commands_before_hello_are_rejected_with_not_hello() {
        let mut world = ServerWorld::new(SEED);
        let checksum = world.checksum();

        for command in [
            ClientCommand::Ping { nonce: 1 },
            ClientCommand::SetTax {
                resident_rate: 10,
                business_rate: 10,
            },
            ClientCommand::PlaceRoad {
                start: [0, 0],
                end: [8, 0],
            },
            ClientCommand::TimeControl {
                action: TimeAction::Pause,
            },
        ] {
            expect_reject(&world.apply(&command), codes::NOT_HELLO);
        }

        assert_eq!(world.tick, 0);
        assert!(world.roads.roads.is_empty());
        assert_eq!(world.checksum(), checksum);
    }

    #[test]
    fn hello_is_required_once_and_returns_the_seed_session() {
        let mut world = ServerWorld::new(SEED);
        match world.apply(&hello()) {
            ServerMessage::Welcome {
                session_id,
                tick,
                protocol_version,
                resumed,
                ..
            } => {
                assert_eq!(session_id, session_id_for_seed(SEED));
                assert_eq!(tick, 0);
                assert_eq!(protocol_version, PROTOCOL_VERSION);
                assert!(!resumed);
            }
            other => panic!("expected welcome, got {other:?}"),
        }
        assert!(world.hello_accepted());
        expect_reject(&world.apply(&hello()), protocol_codes::HELLO_DUPLICATE);
        assert_eq!(world.tick, 0);
    }

    #[test]
    fn invalid_hello_is_rejected_and_does_not_unlock_commands() {
        let mut world = ServerWorld::new(SEED);
        let bad = ClientCommand::Hello {
            protocol_version: PROTOCOL_VERSION + 1,
            content_hash: "0123456789abcdef".repeat(4),
            player_name: "mayor".to_string(),
        };
        expect_reject(
            &world.apply(&bad),
            protocol_codes::PROTOCOL_VERSION_MISMATCH,
        );
        assert!(!world.hello_accepted());
        expect_reject(
            &world.apply(&ClientCommand::Ping { nonce: 1 }),
            codes::NOT_HELLO,
        );
    }

    #[test]
    fn tax_bounds_are_enforced_without_mutation() {
        let mut world = world_with_hello();
        let before = world.checksum();

        expect_reject(
            &world.apply(&ClientCommand::SetTax {
                resident_rate: 101,
                business_rate: 10,
            }),
            protocol_codes::TAX_RATE_OUT_OF_RANGE,
        );
        expect_reject(
            &world.apply(&ClientCommand::SetTax {
                resident_rate: 10,
                business_rate: 101,
            }),
            protocol_codes::TAX_RATE_OUT_OF_RANGE,
        );
        assert_eq!(world.tick, 0);
        assert_eq!(world.checksum(), before);

        let mut zero = world_with_hello();
        let (tick, _, _, _) = expect_snapshot(&zero.apply(&ClientCommand::SetTax {
            resident_rate: 0,
            business_rate: 0,
        }));
        assert_eq!(tick, 1);
        assert_eq!(zero.economy.tax_per_resident_per_day, 0.0);
        assert_eq!(zero.economy.business_tax_per_job_per_day, 0.0);

        let (tick, _, _, _) = expect_snapshot(&world.apply(&ClientCommand::SetTax {
            resident_rate: 100,
            business_rate: 100,
        }));
        assert_eq!(tick, 1);
        assert_eq!(world.economy.tax_per_resident_per_day, 100.0);
        assert_eq!(world.economy.business_tax_per_job_per_day, 100.0);
    }

    #[test]
    fn set_tax_updates_the_economy_and_snapshot_checksum() {
        let mut world = world_with_hello();
        let before = world.checksum();
        let (tick, population, balance, checksum) =
            expect_snapshot(&world.apply(&ClientCommand::SetTax {
                resident_rate: 12,
                business_rate: 8,
            }));
        assert_eq!(tick, 1);
        assert_eq!(population, world.population);
        assert_eq!(balance, STARTING_BALANCE as i64);
        assert_ne!(checksum, before);
        assert_eq!(checksum, world.session_checksum);
        assert_eq!(
            world.snapshot(),
            ServerMessage::Snapshot {
                tick: 1,
                population,
                balance,
                checksum,
                interest_center: [0.0, 0.0],
                interest_radius_m: DEFAULT_INTEREST_RADIUS_M as f32,
                visible_roads: 0,
            }
        );
    }

    #[test]
    fn invalid_roads_are_rejected_with_stable_codes() {
        let mut world = world_with_hello();
        let before = world.checksum();

        expect_reject(
            &world.apply(&ClientCommand::PlaceRoad {
                start: [0, 0],
                end: [0, 0],
            }),
            protocol_codes::ROAD_EMPTY,
        );
        expect_reject(
            &world.apply(&ClientCommand::PlaceRoad {
                start: [0, 0],
                end: [4, 4],
            }),
            codes::ROAD_NOT_AXIS_ALIGNED,
        );
        expect_reject(
            &world.apply(&ClientCommand::PlaceRoad {
                start: [0, 0],
                end: [513, 0],
            }),
            protocol_codes::ROAD_TOO_LONG,
        );
        expect_reject(
            &world.apply(&ClientCommand::PlaceRoad {
                start: [1_000_001, 0],
                end: [1_000_010, 0],
            }),
            protocol_codes::ROAD_OUT_OF_BOUNDS,
        );
        assert_eq!(world.tick, 0);
        assert!(world.roads.roads.is_empty());
        assert_eq!(world.checksum(), before);
    }

    #[test]
    fn maximum_road_length_is_inclusive() {
        let mut world = world_with_hello();
        let (tick, _, _, _) = expect_snapshot(&world.apply(&ClientCommand::PlaceRoad {
            start: [0, 0],
            end: [MAX_WORLD_ROAD_LENGTH, 0],
        }));
        assert_eq!(tick, 1);
        assert_eq!(world.roads.roads.len(), 1);
        assert_eq!(world.roads.roads[0].width_m, WORLD_ROAD_WIDTH_M);
        assert_eq!(
            world.roads.roads[0].length_m(),
            i64::from(MAX_WORLD_ROAD_LENGTH)
        );
    }

    #[test]
    fn duplicate_roads_are_rejected_in_both_directions() {
        let mut world = world_with_hello();
        world.apply(&ClientCommand::PlaceRoad {
            start: [0, 0],
            end: [8, 0],
        });
        let before = world.checksum();

        expect_reject(
            &world.apply(&ClientCommand::PlaceRoad {
                start: [0, 0],
                end: [8, 0],
            }),
            codes::ROAD_DUPLICATE,
        );
        expect_reject(
            &world.apply(&ClientCommand::PlaceRoad {
                start: [8, 0],
                end: [0, 0],
            }),
            codes::ROAD_DUPLICATE,
        );
        assert_eq!(world.roads.roads.len(), 1);
        assert_eq!(world.checksum(), before);
    }

    #[test]
    fn overlapping_roads_are_rejected_but_junctions_are_allowed() {
        let mut world = world_with_hello();
        world.apply(&ClientCommand::PlaceRoad {
            start: [0, 0],
            end: [8, 0],
        });

        expect_reject(
            &world.apply(&ClientCommand::PlaceRoad {
                start: [4, 0],
                end: [12, 0],
            }),
            codes::ROAD_OVERLAP,
        );
        expect_reject(
            &world.apply(&ClientCommand::PlaceRoad {
                start: [-4, 0],
                end: [2, 0],
            }),
            codes::ROAD_OVERLAP,
        );

        // A shared endpoint cell is a T-junction, not an overlap.
        assert!(matches!(
            world.apply(&ClientCommand::PlaceRoad {
                start: [8, 0],
                end: [16, 0],
            }),
            ServerMessage::Snapshot { .. }
        ));
        // A perpendicular crossing on the same row is fine.
        assert!(matches!(
            world.apply(&ClientCommand::PlaceRoad {
                start: [4, -4],
                end: [4, 4],
            }),
            ServerMessage::Snapshot { .. }
        ));
        assert_eq!(world.roads.roads.len(), 3);
    }

    #[test]
    fn pause_freezes_the_world_and_step_advances_one_day() {
        let mut world = world_with_hello();
        let idle = world.advance_day(DAY_SECONDS);
        assert_eq!(idle.days_elapsed, 1.0);
        assert_eq!(world.tick, 1);
        let paused_checksum = world.checksum();

        let (tick, _, _, _) = expect_snapshot(&world.apply(&ClientCommand::TimeControl {
            action: TimeAction::Pause,
        }));
        assert_eq!(tick, 1);
        assert!(world.paused);
        assert_eq!(world.checksum(), paused_checksum);

        let frozen = world.advance_day(DAY_SECONDS);
        assert_eq!(frozen.income, 0.0);
        assert_eq!(frozen.days_elapsed, 1.0);
        assert_eq!(world.tick, 1);
        assert_eq!(world.checksum(), paused_checksum);

        let (tick, _, _, _) = expect_snapshot(&world.apply(&ClientCommand::TimeControl {
            action: TimeAction::Step,
        }));
        assert_eq!(tick, 2);
        assert!(world.paused);
        assert_eq!(world.economy.days_elapsed, 2.0);

        let (tick, _, _, _) = expect_snapshot(&world.apply(&ClientCommand::TimeControl {
            action: TimeAction::Resume,
        }));
        assert_eq!(tick, 2);
        assert!(!world.paused);
    }

    #[test]
    fn speed_actions_are_rejected_as_invalid_time_actions() {
        let mut world = world_with_hello();
        let before = world.checksum();
        for action in [
            TimeAction::Speed1x,
            TimeAction::Speed2x,
            TimeAction::Speed4x,
        ] {
            expect_reject(
                &world.apply(&ClientCommand::TimeControl { action }),
                codes::TIME_INVALID_ACTION,
            );
        }
        assert_eq!(world.tick, 0);
        assert!(!world.paused);
        assert_eq!(world.checksum(), before);
    }

    #[test]
    fn advance_day_uses_static_population_and_caps_dt_at_one_day() {
        let mut world = ServerWorld::with_population(SEED, 1_000);
        let population = f64::from(world.population);
        let employed = f64::from(world.employed);
        let buildings = f64::from(world.buildings);

        let tick = world.advance_day(DAY_SECONDS * 10.0);
        assert_eq!(world.tick, 1);
        assert!((tick.days_elapsed - 1.0).abs() < 1e-9);
        let income = population * DEFAULT_TAX_PER_RESIDENT_PER_DAY
            + employed * DEFAULT_BUSINESS_TAX_PER_JOB_PER_DAY;
        let expenses = employed * WAGE_PER_WORKER_PER_DAY
            + buildings * core_city::economy::DEFAULT_UPKEEP_PER_BUILDING_PER_DAY;
        assert!((tick.income - income).abs() < 1e-6);
        assert!((tick.expenses - expenses).abs() < 1e-6);
        assert!((tick.balance - (STARTING_BALANCE + income - expenses)).abs() < 1e-6);
        assert!((world.economy.balance - tick.balance).abs() < 1e-6);

        // Non-finite and negative rates clamp to zero time but still count as a
        // deterministic tick.
        let before_days = world.economy.days_elapsed;
        world.advance_day(f64::NAN);
        world.advance_day(-DAY_SECONDS);
        assert_eq!(world.economy.days_elapsed, before_days);
        assert_eq!(world.tick, 3);
    }

    #[test]
    fn snapshots_are_deterministic_across_worlds_and_ignore_rejections() {
        fn transcript() -> (Vec<ServerMessage>, ServerWorld) {
            let mut world = ServerWorld::with_role(SEED, ClientRole::Admin);
            let messages = vec![
                world.apply(&hello()),
                world.apply(&ClientCommand::SetTax {
                    resident_rate: 14,
                    business_rate: 6,
                }),
                world.apply(&ClientCommand::PlaceRoad {
                    start: [2, 3],
                    end: [10, 3],
                }),
                world.apply(&ClientCommand::PlaceRoad {
                    start: [10, 3],
                    end: [20, 3],
                }),
                world.apply(&ClientCommand::TimeControl {
                    action: TimeAction::Step,
                }),
                world.apply(&ClientCommand::Disconnect),
            ];
            (messages, world)
        }

        let (first, first_world) = transcript();
        let (second, second_world) = transcript();
        assert_eq!(first, second);
        assert_eq!(first_world.checksum(), second_world.checksum());
        assert_eq!(first_world.tick, 4);
        assert_eq!(first_world.economy.days_elapsed, 1.0);

        let mut rejected = first_world.clone();
        let before = rejected.checksum();
        rejected.apply(&ClientCommand::SetTax {
            resident_rate: 101,
            business_rate: 0,
        });
        assert_eq!(rejected.checksum(), before);
    }

    #[test]
    fn hello_registers_a_stable_player_identity() {
        let mut world = ServerWorld::with_role(SEED, ClientRole::Admin);
        assert!(world.player_id().is_none());
        assert!(matches!(
            world.apply(&hello_named("alice")),
            ServerMessage::Welcome { .. }
        ));
        let alice = world.player_id().expect("hello assigns an id");
        assert_eq!(world.players.count(), 1);

        world.apply(&ClientCommand::Disconnect);
        world.online.clear();
        world.resume_window = None;
        world.begin_connection();
        assert!(matches!(
            world.apply(&hello_named("Alice")),
            ServerMessage::Welcome { .. }
        ));
        assert_eq!(
            world.player_id(),
            Some(alice),
            "case-insensitive name reuses the id"
        );
        assert_eq!(world.players.count(), 1);

        world.apply(&ClientCommand::Disconnect);
        world.online.clear();
        world.resume_window = None;
        world.begin_connection();
        assert!(matches!(
            world.apply(&hello_named("bob")),
            ServerMessage::Welcome { .. }
        ));
        assert_ne!(world.player_id(), Some(alice));
        assert_eq!(world.players.count(), 2);
    }

    #[test]
    fn settlements_move_the_checksum_but_players_do_not() {
        let mut world = world_with_hello();
        let before = world.checksum();
        let player = world.player_id().expect("hello assigns an id");

        let mut settlements = SettlementStore::new(SEED);
        let request = SettlementRequest {
            name: "Checkum".to_string(),
            owner: "mayor".to_string(),
            center: [0, 0],
            radius_m: 64,
        };
        settlements
            .found_for_player(&request, player)
            .expect("found settlement");
        world.settlements = Some(settlements);

        assert_ne!(world.checksum(), before);
        assert_ne!(
            world.settlements.as_ref().expect("store").checksum(),
            0,
            "settlement state has a stable checksum"
        );
    }

    #[test]
    fn checksum_changes_when_roads_or_the_treasury_change() {
        let base = world_with_hello();
        let mut with_road = base.clone();
        with_road.apply(&ClientCommand::PlaceRoad {
            start: [0, 0],
            end: [8, 0],
        });
        assert_ne!(base.checksum(), with_road.checksum());

        let mut with_tax = base.clone();
        with_tax.economy.set_taxes(20.0, 20.0);
        assert_ne!(base.checksum(), with_tax.checksum());
        assert_ne!(with_road.checksum(), with_tax.checksum());
    }

    #[test]
    fn a_new_session_starts_from_a_clean_world() {
        let mut used = world_with_hello();
        used.apply(&ClientCommand::SetTax {
            resident_rate: 100,
            business_rate: 100,
        });
        used.apply(&ClientCommand::PlaceRoad {
            start: [0, 0],
            end: [8, 0],
        });
        used.advance_day(DAY_SECONDS);
        used.apply(&ClientCommand::Disconnect);
        assert!(used.is_closed());

        let mut fresh = ServerWorld::new(SEED);
        assert_eq!(fresh.tick, 0);
        assert!(fresh.roads.roads.is_empty());
        assert_eq!(fresh.economy.balance, STARTING_BALANCE);
        assert_eq!(
            fresh.economy.tax_per_resident_per_day,
            DEFAULT_TAX_PER_RESIDENT_PER_DAY
        );
        assert_eq!(
            fresh.economy.business_tax_per_job_per_day,
            DEFAULT_BUSINESS_TAX_PER_JOB_PER_DAY
        );
        assert_eq!(
            fresh.checksum(),
            ServerWorld::new(SEED).checksum(),
            "checksum is a pure function of the fresh state"
        );
        assert!(!fresh.is_closed());
        expect_reject(
            &fresh.apply(&ClientCommand::Ping { nonce: 1 }),
            codes::NOT_HELLO,
        );
    }

    #[test]
    fn closed_sessions_reject_further_commands() {
        let mut world = world_with_hello();
        assert_eq!(
            world.apply(&ClientCommand::Disconnect),
            ServerMessage::Goodbye
        );
        expect_reject(
            &world.apply(&ClientCommand::Ping { nonce: 1 }),
            protocol_codes::SESSION_CLOSED,
        );
        expect_reject(&world.apply(&hello()), protocol_codes::SESSION_CLOSED);
    }

    #[test]
    fn malformed_commands_decode_with_stable_codes() {
        assert_eq!(
            decode_client_command(br#"{"type":"place_road","start":[0,0]}"#)
                .expect_err("missing end")
                .code,
            protocol_codes::COMMAND_INVALID_FIELDS
        );
        assert_eq!(
            decode_client_command(br#"{"type":"time_control","action":"warp"}"#)
                .expect_err("bad action")
                .code,
            protocol_codes::COMMAND_INVALID_FIELDS
        );
        assert_eq!(
            decode_client_command(br#"{"type":"nuke"}"#)
                .expect_err("unknown")
                .code,
            protocol_codes::COMMAND_UNKNOWN_TAG
        );
        assert_eq!(
            decode_client_command(br#"{"#).expect_err("json").code,
            protocol_codes::COMMAND_INVALID_JSON
        );

        let world = world_with_hello();
        let before = world.checksum();
        let rejection = decode_client_command(br#"{"type":"set_tax","resident_rate":1}"#)
            .expect_err("missing business rate");
        expect_reject(
            &rejection.into_message(),
            protocol_codes::COMMAND_INVALID_FIELDS,
        );
        assert_eq!(world.checksum(), before);
    }

    #[test]
    fn authority_loopback_is_deterministic() {
        let first = run_authority_loopback().expect("authority loopback");
        assert_eq!(first.commands, AUTHORITY_COMMANDS);
        assert_eq!(first.frames, AUTHORITY_FRAMES);
        assert_eq!(first.tick, 2);
        assert_eq!(first.population, SessionConfig::for_seed(SEED).population);
        assert_eq!(first.balance, STARTING_BALANCE as i64);
        assert_eq!(first.session_id, session_id_for_seed(SEED));
        assert_eq!(first.checksum.len(), 16);
        assert!(first.checksum.bytes().all(|byte| byte.is_ascii_hexdigit()));

        let second = run_authority_loopback().expect("second authority loopback");
        assert_eq!(first, second);
    }

    #[test]
    fn resume_returns_welcome_and_snapshot() {
        let mut world = world_with_hello();
        let token = world.session_token().to_string();
        world.apply(&ClientCommand::SetTax {
            resident_rate: 12,
            business_rate: 8,
        });
        world.apply(&ClientCommand::Disconnect);
        world.begin_connection();

        let responses = world.apply_all(&ClientCommand::Resume {
            session_id: token,
            last_tick: 1,
        });
        assert!(matches!(
            responses.first(),
            Some(ServerMessage::Welcome { resumed: true, .. })
        ));
        assert!(matches!(
            responses.get(1),
            Some(ServerMessage::Snapshot { tick: 1, .. })
        ));
        assert!(world.hello_accepted());
    }

    #[test]
    fn resume_rejects_unknown_token_and_stale_tick() {
        let mut world = world_with_hello();
        let token = world.session_token().to_string();
        world.apply(&ClientCommand::Disconnect);
        world.begin_connection();

        expect_reject(
            &world.apply(&ClientCommand::Resume {
                session_id: "wrong".to_string(),
                last_tick: 0,
            }),
            codes::RESUME_UNKNOWN,
        );
        expect_reject(
            &world.apply(&ClientCommand::Resume {
                session_id: token,
                last_tick: 99,
            }),
            codes::RESUME_STALE,
        );
    }

    #[test]
    fn resume_window_expires_after_the_grace_ticks() {
        let mut world = world_with_hello();
        let token = world.session_token().to_string();
        world.apply(&ClientCommand::Disconnect);
        world.begin_connection();
        world.tick = world
            .tick
            .saturating_add(crate::world::RESUME_GRACE_TICKS + 1);
        expect_reject(
            &world.apply(&ClientCommand::Resume {
                session_id: token,
                last_tick: 0,
            }),
            codes::RESUME_EXPIRED,
        );
    }

    #[test]
    fn resume_is_rejected_while_a_session_is_active() {
        let mut world = world_with_hello();
        let token = world.session_token().to_string();
        expect_reject(
            &world.apply(&ClientCommand::Resume {
                session_id: token,
                last_tick: 0,
            }),
            codes::RESUME_DUPLICATE,
        );
    }

    #[test]
    fn role_matrix_matches_d021() {
        assert_eq!(ClientRole::required_role("time_vote"), ClientRole::Viewer);
        assert_eq!(
            ClientRole::required_role("set_interest"),
            ClientRole::Viewer
        );
        assert_eq!(
            ClientRole::required_role_for(&ClientCommand::TimeVote {
                action: crate::protocol::VoteAction::Call,
                time: Some(TimeAction::Pause),
            }),
            ClientRole::Viewer
        );
        assert_eq!(
            ClientRole::required_role_for(&ClientCommand::TimeVote {
                action: crate::protocol::VoteAction::Cancel,
                time: None,
            }),
            ClientRole::Admin
        );
        for action in [TimeAction::Pause, TimeAction::Resume, TimeAction::Step] {
            assert_eq!(
                ClientRole::required_role_for(&ClientCommand::TimeControl { action }),
                ClientRole::Admin
            );
        }
    }

    #[test]
    fn disconnect_removes_and_resume_restores_the_player() {
        let mut world = world_with_hello();
        let token = world.session_token().to_string();
        let tick = world.tick;
        assert!(matches!(
            world.apply(&ClientCommand::Disconnect),
            ServerMessage::Goodbye
        ));
        assert!(world.online.is_empty());
        world.begin_connection();
        let responses = world.apply_all(&ClientCommand::Resume {
            session_id: token,
            last_tick: tick,
        });
        assert!(matches!(
            responses.first(),
            Some(ServerMessage::Welcome { resumed: true, .. })
        ));
        assert!(world.online.contains("mayor"));
        let message = world.apply(&ClientCommand::TimeVote {
            action: crate::protocol::VoteAction::Call,
            time: Some(TimeAction::Pause),
        });
        assert!(
            !matches!(&message, ServerMessage::Reject { code, .. } if code == codes::NOT_HELLO),
            "{message:?}"
        );
    }

    #[test]
    fn bounded_hello_attempts_expire_a_stale_resume_window() {
        let mut world = world_with_hello();
        world.apply(&ClientCommand::Disconnect);
        world.begin_connection();
        for _ in 0..MAX_HELLO_RESUME_ATTEMPTS - 1 {
            expect_reject(&world.apply(&hello()), codes::HELLO_RESUMABLE);
        }
        assert!(matches!(
            world.apply(&hello()),
            ServerMessage::Welcome { .. }
        ));
        assert!(!world.is_resumable());
    }

    #[test]
    fn duplicate_online_player_name_is_rejected() {
        let mut world = world_with_hello();
        world.online.insert("mayor".to_string());
        world.hello = false;
        expect_reject(&world.apply(&hello()), codes::PLAYER_NAME_DUPLICATE);
    }

    #[test]
    fn resume_consumes_a_rate_credit() {
        let mut world = world_with_hello();
        let token = world.session_token().to_string();
        let tick = world.tick;
        world.apply(&ClientCommand::Disconnect);
        world.begin_connection();
        world.rate.milli_credits = 0;
        expect_reject(
            &world.apply(&ClientCommand::Resume {
                session_id: token,
                last_tick: tick,
            }),
            codes::RATE_LIMITED,
        );
    }

    #[test]
    fn loopback_checksum_matches_a_local_replay() {
        let report = run_authority_loopback().expect("authority loopback");
        let mut replay = ServerWorld::new(SEED);
        replay.apply(&hello());
        replay.apply(&ClientCommand::SetTax {
            resident_rate: 12,
            business_rate: 8,
        });
        replay.apply(&ClientCommand::PlaceRoad {
            start: [0, 0],
            end: [8, 0],
        });
        replay.apply(&ClientCommand::Ping { nonce: 7 });
        replay.apply(&ClientCommand::TimeControl {
            action: TimeAction::Resume,
        });
        replay.apply(&ClientCommand::Disconnect);
        assert_eq!(report.checksum, replay.checksum());
        assert_eq!(report.tick, replay.tick);
    }

    #[test]
    fn connections_hello_independently_and_reject_duplicate_names() {
        let mut world = ServerWorld::with_role(SEED, ClientRole::Viewer);
        let alice_connection = world.open_connection();
        let bob_connection = world.open_connection();
        assert_eq!((alice_connection, bob_connection), (1, 2));

        let alice = first_response(world.apply_session(alice_connection, &hello_named("alice")));
        let bob = first_response(world.apply_session(bob_connection, &hello_named("bob")));
        let alice_token = welcome_token(&alice);
        let bob_token = welcome_token(&bob);

        assert_eq!(
            alice_token,
            session_id_for_seed(SEED),
            "connection 1 keeps the pinned seed-derived token"
        );
        assert_ne!(alice_token, bob_token, "connections must not share a token");

        let third_connection = world.open_connection();
        expect_reject(
            &first_response(world.apply_session(third_connection, &hello_named("alice"))),
            codes::PLAYER_NAME_DUPLICATE,
        );
        assert_eq!(world.online.len(), 2);
        assert!(world.online.contains("alice"));
        assert!(world.online.contains("bob"));
    }

    #[test]
    fn per_connection_roles_interest_and_audit_are_isolated() {
        let mut world = ServerWorld::with_role(SEED, ClientRole::Mayor);
        // Assign the weaker role before the connection's first command so its
        // whole audit ring (including `hello`) carries that role.
        world.set_session_role(2, ClientRole::Viewer);
        first_response(world.apply_session(1, &hello_named("alice")));
        first_response(world.apply_session(2, &hello_named("bob")));
        assert_eq!(world.session_role(1), ClientRole::Mayor);
        assert_eq!(world.session_role(2), ClientRole::Viewer);

        assert!(matches!(
            first_response(world.apply_session(
                1,
                &ClientCommand::SetTax {
                    resident_rate: 12,
                    business_rate: 8,
                },
            )),
            ServerMessage::Snapshot { tick: 1, .. }
        ));
        expect_reject(
            &first_response(world.apply_session(
                2,
                &ClientCommand::SetTax {
                    resident_rate: 12,
                    business_rate: 8,
                },
            )),
            codes::FORBIDDEN,
        );

        first_response(world.apply_session(1, &set_interest(10.0, 0.0, 16.0)));
        first_response(world.apply_session(2, &set_interest(0.0, 5.0, 1.0)));

        first_response(world.apply_session(1, &ClientCommand::Ping { nonce: 1 }));
        match world.snapshot() {
            ServerMessage::Snapshot {
                interest_center,
                interest_radius_m,
                ..
            } => {
                assert_eq!(interest_center, [10.0, 0.0]);
                assert_eq!(interest_radius_m, 16.0);
            }
            other => panic!("expected snapshot, got {other:?}"),
        }
        first_response(world.apply_session(2, &ClientCommand::Ping { nonce: 2 }));
        match world.snapshot() {
            ServerMessage::Snapshot {
                interest_center,
                interest_radius_m,
                ..
            } => {
                assert_eq!(interest_center, [0.0, 5.0]);
                assert_eq!(interest_radius_m, 1.0);
            }
            other => panic!("expected snapshot, got {other:?}"),
        }

        // Activate each session and read its own audit ring.
        first_response(world.apply_session(1, &ClientCommand::Ping { nonce: 3 }));
        let alice_audit = world.audit_state();
        assert_eq!(alice_audit.len(), 5);
        assert!(alice_audit
            .iter()
            .all(|entry| entry.role == ClientRole::Mayor));
        assert!(alice_audit
            .iter()
            .any(|entry| entry.tag == "set_tax" && entry.accepted()));

        first_response(world.apply_session(2, &ClientCommand::Ping { nonce: 4 }));
        let bob_audit = world.audit_state();
        assert_eq!(bob_audit.len(), 5);
        assert!(bob_audit
            .iter()
            .all(|entry| entry.role == ClientRole::Viewer));
        assert!(bob_audit
            .iter()
            .any(|entry| entry.tag == "set_tax" && entry.code == codes::FORBIDDEN));
        assert!(!bob_audit
            .iter()
            .any(|entry| entry.tag == "set_tax" && entry.accepted()));
    }

    #[test]
    fn disconnecting_one_connection_keeps_others_online() {
        let mut world = ServerWorld::with_role(SEED, ClientRole::Viewer);
        first_response(world.apply_session(1, &hello_named("alice")));
        first_response(world.apply_session(2, &hello_named("bob")));

        assert_eq!(
            first_response(world.apply_session(1, &ClientCommand::Disconnect)),
            ServerMessage::Goodbye
        );
        assert!(!world.online.contains("alice"));
        assert!(world.online.contains("bob"));

        assert_eq!(
            first_response(world.apply_session(2, &ClientCommand::Ping { nonce: 9 })),
            ServerMessage::Pong { nonce: 9 }
        );
        // One player online is below the D-021 quorum; alice's departure did
        // not remove bob from the roster.
        expect_reject(
            &first_response(world.apply_session(
                2,
                &vote(crate::protocol::VoteAction::Call, Some(TimeAction::Pause)),
            )),
            crate::vote::codes::MIN_ONLINE,
        );
    }

    #[test]
    fn a_vote_is_shared_across_connections_and_counts_once() {
        let mut world = ServerWorld::with_role(SEED, ClientRole::Viewer);
        first_response(world.apply_session(1, &hello_named("alice")));
        first_response(world.apply_session(2, &hello_named("bob")));

        let opened = first_response(world.apply_session(
            1,
            &vote(crate::protocol::VoteAction::Call, Some(TimeAction::Pause)),
        ));
        match opened {
            ServerMessage::TimeVoteStatus {
                vote_id,
                ref state,
                yes,
                no,
                online,
                ref initiator,
                paused,
                ..
            } => {
                assert_eq!(vote_id, 1);
                assert_eq!(state, "open");
                assert_eq!((yes, no, online), (1, 0, 2));
                assert_eq!(initiator, "alice");
                assert!(!paused);
            }
            other => panic!("expected open vote status, got {other:?}"),
        }

        let passed =
            first_response(world.apply_session(2, &vote(crate::protocol::VoteAction::Yes, None)));
        match passed {
            ServerMessage::TimeVoteStatus {
                vote_id,
                ref state,
                yes,
                no,
                paused,
                ..
            } => {
                assert_eq!(vote_id, 1, "both connections see the same vote");
                assert_eq!(state, "passed");
                assert_eq!((yes, no), (2, 0));
                assert!(paused);
            }
            other => panic!("expected passed vote status, got {other:?}"),
        }
        assert!(world.paused, "a passed pause vote freezes the shared world");
        assert_eq!(
            world.vote.votes_cast(),
            1,
            "only Bob's cast was accepted; Alice's initiator ballot is shared, not a second cast"
        );

        // Alice's initiator ballot is shared, so her own second yes cannot be
        // counted; the vote is already resolved, which is the stable rejection.
        expect_reject(
            &first_response(world.apply_session(1, &vote(crate::protocol::VoteAction::Yes, None))),
            crate::vote::codes::CLOSED,
        );
    }

    #[test]
    fn resume_adopts_only_the_requested_session() {
        let mut world = ServerWorld::with_role(SEED, ClientRole::Mayor);
        let alice_token = welcome_token(&first_response(
            world.apply_session(1, &hello_named("alice")),
        ));
        let bob_token = welcome_token(&first_response(world.apply_session(2, &hello_named("bob"))));
        first_response(world.apply_session(1, &set_interest(10.0, 0.0, 16.0)));
        first_response(world.apply_session(1, &ClientCommand::Disconnect));
        first_response(world.apply_session(2, &ClientCommand::Ping { nonce: 1 }));

        let resumed = world.apply_session(
            3,
            &ClientCommand::Resume {
                session_id: alice_token.clone(),
                last_tick: world.tick,
            },
        );
        match resumed.as_slice() {
            [ServerMessage::Welcome {
                resumed: true,
                session_token,
                ..
            }, ServerMessage::Snapshot { .. }] => assert_eq!(session_token, &alice_token),
            other => panic!("expected resumed welcome + snapshot, got {other:?}"),
        }

        // Alice's viewport and identity moved to connection 3; Bob's session
        // was not touched and is still online.
        first_response(world.apply_session(3, &ClientCommand::Ping { nonce: 2 }));
        match world.snapshot() {
            ServerMessage::Snapshot {
                interest_center, ..
            } => assert_eq!(interest_center, [10.0, 0.0]),
            other => panic!("expected snapshot, got {other:?}"),
        }
        first_response(world.apply_session(2, &ClientCommand::Ping { nonce: 3 }));
        match world.snapshot() {
            ServerMessage::Snapshot {
                interest_center, ..
            } => assert_eq!(interest_center, [0.0, 0.0]),
            other => panic!("expected snapshot, got {other:?}"),
        }
        assert!(world.online.contains("alice"));
        assert!(world.online.contains("bob"));

        // Bob is active, so a fourth connection cannot claim his token.
        expect_reject(
            &first_response(world.apply_session(
                4,
                &ClientCommand::Resume {
                    session_id: bob_token,
                    last_tick: world.tick,
                },
            )),
            codes::RESUME_DUPLICATE,
        );
    }

    #[test]
    fn single_session_apply_path_matches_connection_one() {
        fn transcript(world: &mut ServerWorld, connection: Option<u64>) -> Vec<ServerMessage> {
            let commands = [
                hello(),
                ClientCommand::Ping { nonce: 5 },
                ClientCommand::SetTax {
                    resident_rate: 12,
                    business_rate: 8,
                },
                ClientCommand::PlaceRoad {
                    start: [0, 0],
                    end: [8, 0],
                },
                set_interest(4.0, 0.0, 16.0),
                ClientCommand::TimeControl {
                    action: TimeAction::Resume,
                },
                ClientCommand::Disconnect,
            ];
            let mut responses = Vec::new();
            for command in &commands {
                let batch = match connection {
                    Some(connection_id) => world.apply_session(connection_id, command),
                    None => world.apply_all(command),
                };
                responses.extend(batch);
            }
            responses
        }

        let mut legacy = ServerWorld::with_role(SEED, ClientRole::Admin);
        let mut connection = ServerWorld::with_role(SEED, ClientRole::Admin);
        assert_eq!(
            transcript(&mut legacy, None),
            transcript(&mut connection, Some(1)),
            "the legacy default slot and connection 1 must be byte-identical"
        );
        assert_eq!(legacy.checksum(), connection.checksum());
        assert_eq!(legacy.audit_state(), connection.audit_state());
    }

    // ---- D-032 authoritative blueprint placement ----

    use core_blueprint::{
        save_tsbp_with_hash, Blueprint, BlueprintMetadata, StructuralSystem, StructureSpec,
    };
    use core_city::construction::ConstructionPhase;
    use core_city::policy::DistrictPolicyRequest;
    use core_voxel::volume::{VolumeConfig, VolumePos, VoxelVolume};
    use std::path::Path;

    static PLACEMENT_TEMP_COUNTER: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);

    struct PlacementTempDir(PathBuf);

    impl PlacementTempDir {
        fn new(label: &str) -> Self {
            let unique = PLACEMENT_TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "townsmith-server-placement-world-{label}-{}-{unique}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn library(&self) -> PathBuf {
            self.0.join("library")
        }
    }

    impl Drop for PlacementTempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_test_package(
        dir: &PlacementTempDir,
        id: &str,
        cost: f64,
        structure: Option<StructureSpec>,
    ) -> String {
        let library_root = dir.library();
        std::fs::create_dir_all(&library_root).expect("library root");
        let mut volume =
            VoxelVolume::new(VolumeConfig::new(1, 1, 1).expect("config")).expect("volume");
        volume
            .fill_box(VolumePos::new(0, 0, 0), VolumePos::new(1, 1, 1), 1)
            .expect("fill");
        let mut metadata = BlueprintMetadata::new(id, "Server Test House");
        metadata.stats.material_cost = cost;
        metadata.stats.resident_capacity = 2;
        metadata.stats.job_slots = 1;
        metadata.structure = structure;
        let blueprint = Blueprint::new(metadata, volume);
        let package = dir.path().join(format!("{}.tsbp", id.replace(':', "_")));
        let hash = save_tsbp_with_hash(&package, &blueprint).expect("save package");
        BlueprintLibrary::new(&library_root)
            .expect("open library")
            .insert_file(&package)
            .expect("insert package");
        hash
    }

    fn placement_world(dir: &PlacementTempDir) -> ServerWorld {
        let mut world = ServerWorld::with_role(SEED, ClientRole::Mayor);
        world.set_blueprint_library(dir.library()).expect("library");
        assert!(matches!(
            world.apply(&hello()),
            ServerMessage::Welcome { .. }
        ));
        world
    }

    fn place(hash: &str, x: f64, z: f64, rotation: i32) -> ClientCommand {
        ClientCommand::PlaceBlueprint {
            blueprint_hash: hash.to_string(),
            x,
            z,
            rotation,
        }
    }

    fn placement_state(world: &ServerWorld) -> (String, u64, i64, u64, usize, usize) {
        (
            world.checksum(),
            world.tick,
            world.balance_units(),
            world
                .placements
                .as_ref()
                .map_or(0, PlacementStore::checksum),
            world
                .placements
                .as_ref()
                .map_or(0, |store| store.placements.len()),
            world
                .construction
                .as_ref()
                .map_or(0, ConstructionStore::site_count),
        )
    }

    fn expect_placement_result(message: &ServerMessage) -> (&PlacementView, u64, &str, &str) {
        match message {
            ServerMessage::PlaceBlueprintResult {
                tick,
                placement_id: _,
                placement_checksum,
                world_checksum,
                placement,
                ..
            } => (placement, *tick, placement_checksum, world_checksum),
            other => panic!("expected place_blueprint_result, got {other:?}"),
        }
    }

    #[test]
    fn blueprint_library_and_policy_builders_validate_their_inputs() {
        let dir = PlacementTempDir::new("builders");
        write_test_package(&dir, "townsmith:builders", 1_000.0, None);
        let mut world = ServerWorld::new(SEED);
        let checksum = world.checksum();

        let relative = PathBuf::from("relative-blueprints");
        assert!(world.set_blueprint_library(relative).is_err());
        assert!(world.blueprint_library.is_none());
        let missing = std::env::temp_dir().join(format!(
            "townsmith-server-placement-missing-{}-{}",
            std::process::id(),
            PLACEMENT_TEMP_COUNTER.load(std::sync::atomic::Ordering::Relaxed)
        ));
        assert!(world.set_blueprint_library(missing).is_err());
        assert!(world.blueprint_library.is_none());
        world
            .set_blueprint_library(dir.library())
            .expect("valid library");
        assert!(world.blueprint_library.is_some());

        let mut invalid = DistrictPolicyStore::new(SEED);
        invalid.next_id = 0;
        assert!(world.set_policy_registry(invalid).is_err());
        assert!(world.policies.is_none());
        assert_eq!(world.checksum(), checksum);
    }

    #[test]
    fn place_blueprint_requires_the_mayor_role() {
        assert_eq!(
            ClientRole::required_role("place_blueprint"),
            ClientRole::Mayor
        );
        assert_eq!(
            ClientRole::required_role_for(&place("blake3:00", 0.0, 0.0, 0)),
            ClientRole::Mayor
        );

        let mut world = ServerWorld::with_role(SEED, ClientRole::Viewer);
        assert!(matches!(
            world.apply(&hello()),
            ServerMessage::Welcome { .. }
        ));
        let checksum = world.checksum();
        let message = world.apply(&place("blake3:00", 0.0, 0.0, 0));
        expect_reject(&message, codes::FORBIDDEN);
        assert_eq!(world.checksum(), checksum);
    }

    #[test]
    fn place_blueprint_is_rate_limited_and_audited() {
        let dir = PlacementTempDir::new("rate");
        let hash = write_test_package(&dir, "townsmith:rate", 1_000.0, None);
        let mut world = placement_world(&dir);

        let mut limited = false;
        for _ in 0..(RATE_BURST_CREDITS * 4) {
            match world.apply(&place(&hash, 0.0, 0.0, 0)) {
                ServerMessage::PlaceBlueprintResult { .. } => {
                    // The first command is accepted and every repeat is the
                    // idempotent success; both consume a credit.
                }
                ServerMessage::Reject { code, .. } if code == codes::RATE_LIMITED => {
                    limited = true;
                    break;
                }
                other => panic!("unexpected response {other:?}"),
            }
        }
        assert!(limited, "placement must consume the shared rate budget");
        let audit = world.audit_state();
        assert!(audit
            .iter()
            .any(|entry| entry.tag == "place_blueprint" && entry.code == AUDIT_ACCEPTED));
        assert!(audit
            .iter()
            .any(|entry| entry.tag == "place_blueprint" && entry.code == codes::RATE_LIMITED));
    }

    #[test]
    fn place_blueprint_rejects_without_a_configured_library() {
        let dir = PlacementTempDir::new("no-library");
        let hash = write_test_package(&dir, "townsmith:no_library", 1_000.0, None);
        let mut world = ServerWorld::with_role(SEED, ClientRole::Mayor);
        assert!(matches!(
            world.apply(&hello()),
            ServerMessage::Welcome { .. }
        ));
        let before = placement_state(&world);
        expect_reject(
            &world.apply(&place(&hash, 0.0, 0.0, 0)),
            protocol_codes::PLACEMENT_LIBRARY_UNAVAILABLE,
        );
        assert_eq!(placement_state(&world), before);
        assert!(world.placements.is_none() && world.construction.is_none());

        // The same world still rejects after the library is cleared again.
        world.set_blueprint_library(dir.library()).expect("library");
        world.clear_blueprint_library();
        expect_reject(
            &world.apply(&place(&hash, 0.0, 0.0, 0)),
            protocol_codes::PLACEMENT_LIBRARY_UNAVAILABLE,
        );
        assert_eq!(placement_state(&world), before);
    }

    #[test]
    fn place_blueprint_rejects_invalid_hashes_and_missing_packages() {
        let dir = PlacementTempDir::new("hash");
        write_test_package(&dir, "townsmith:hash", 1_000.0, None);
        let mut world = placement_world(&dir);
        let before = placement_state(&world);

        let uppercase = "AB".repeat(32);
        for bad in ["", "nope", "blake3:abc", uppercase.as_str()] {
            expect_reject(
                &world.apply(&place(bad, 0.0, 0.0, 0)),
                protocol_codes::PLACEMENT_HASH_INVALID,
            );
        }
        let missing = format!("blake3:{}", "a".repeat(64));
        expect_reject(
            &world.apply(&place(&missing, 0.0, 0.0, 0)),
            protocol_codes::PLACEMENT_PACKAGE_MISSING,
        );
        assert_eq!(placement_state(&world), before);
    }

    #[test]
    fn place_blueprint_rejects_a_corrupt_stored_package() {
        let dir = PlacementTempDir::new("corrupt");
        let hash = write_test_package(&dir, "townsmith:corrupt", 1_000.0, None);
        let hex = hash.strip_prefix("blake3:").expect("canonical hash");
        let stored = dir.library().join(&hex[..2]).join(format!("{hex}.tsbp"));
        std::fs::write(&stored, b"not a package").expect("corrupt package");
        let mut world = placement_world(&dir);
        let before = placement_state(&world);
        expect_reject(
            &world.apply(&place(&hash, 0.0, 0.0, 0)),
            protocol_codes::PLACEMENT_PACKAGE_INVALID,
        );
        assert_eq!(placement_state(&world), before);
    }

    #[test]
    fn place_blueprint_applies_the_district_policy_gate() {
        let dir = PlacementTempDir::new("policy");
        let hash = write_test_package(&dir, "townsmith:policy", 1_000.0, None);
        let mut world = placement_world(&dir);
        let mut policies = DistrictPolicyStore::new(SEED);
        policies
            .create(&DistrictPolicyRequest {
                name: "No Build".to_string(),
                center: [0.0, 0.0],
                radius_m: 16.0,
                tax_multiplier: 1.0,
                service_funding: 1.0,
                construction_allowed: false,
                blueprint_allowlist: BTreeSet::new(),
            })
            .expect("policy");
        world.set_policy_registry(policies).expect("registry");
        let before = placement_state(&world);

        match world.apply(&place(&hash, 0.0, 0.0, 0)) {
            ServerMessage::Reject { code, message } => {
                assert_eq!(code, protocol_codes::PLACEMENT_POLICY_DENIED);
                assert!(message.contains("policy_construction_disabled"));
            }
            other => panic!("expected policy denial, got {other:?}"),
        }
        assert_eq!(placement_state(&world), before);

        // The same place outside the denying circle succeeds.
        assert!(matches!(
            world.apply(&place(&hash, 64.0, 0.0, 0)),
            ServerMessage::PlaceBlueprintResult { .. }
        ));
    }

    #[test]
    fn place_blueprint_rejects_a_foreign_seed_policy_registry() {
        let dir = PlacementTempDir::new("foreign-policy");
        let hash = write_test_package(&dir, "townsmith:foreign", 1_000.0, None);
        let mut world = placement_world(&dir);
        let before = placement_state(&world);
        let error = world
            .set_policy_registry(DistrictPolicyStore::new(SEED + 1))
            .expect_err("foreign registry");
        assert!(error.to_string().contains("does not match"));
        assert!(world.policies.is_none());
        assert_eq!(placement_state(&world), before);
        // A neutral world still places.
        assert!(matches!(
            world.apply(&place(&hash, 0.0, 0.0, 0)),
            ServerMessage::PlaceBlueprintResult { .. }
        ));
    }

    #[test]
    fn place_blueprint_rejects_a_structure_locked_by_age() {
        let dir = PlacementTempDir::new("age");
        let brick = StructureSpec::new(StructuralSystem::LoadBearingBrick, 1, 3.2, 0.0);
        let hash = write_test_package(&dir, "townsmith:brick", 1_000.0, Some(brick));
        let mut world = ServerWorld::with_population_and_role(SEED, 10, ClientRole::Mayor);
        world.set_blueprint_library(dir.library()).expect("library");
        assert!(matches!(
            world.apply(&hello()),
            ServerMessage::Welcome { .. }
        ));
        assert_eq!(
            core_city::progression::AgeState::from_population(u64::from(world.population)).age,
            core_city::progression::Age::Wood
        );
        let before = placement_state(&world);
        match world.apply(&place(&hash, 0.0, 0.0, 0)) {
            ServerMessage::Reject { code, message } => {
                assert_eq!(code, protocol_codes::PLACEMENT_AGE_LOCKED);
                assert!(message.contains("masonry age"));
            }
            other => panic!("expected age lock, got {other:?}"),
        }
        assert_eq!(placement_state(&world), before);

        // A late-age city unlocks the same package.
        let mut late = ServerWorld::with_population_and_role(SEED, 300, ClientRole::Mayor);
        late.set_blueprint_library(dir.library()).expect("library");
        assert!(matches!(
            late.apply(&hello()),
            ServerMessage::Welcome { .. }
        ));
        assert!(matches!(
            late.apply(&place(&hash, 0.0, 0.0, 0)),
            ServerMessage::PlaceBlueprintResult { .. }
        ));
    }

    #[test]
    fn place_blueprint_rejects_out_of_range_and_bad_rotation() {
        let dir = PlacementTempDir::new("bounds");
        let hash = write_test_package(&dir, "townsmith:bounds", 1_000.0, None);
        let mut world = placement_world(&dir);
        let before = placement_state(&world);

        expect_reject(
            &world.apply(&place(&hash, 1_000_001.0, 0.0, 0)),
            protocol_codes::PLACEMENT_OUT_OF_RANGE,
        );
        expect_reject(
            &world.apply(&place(&hash, 0.0, f64::NEG_INFINITY, 0)),
            protocol_codes::PLACEMENT_OUT_OF_RANGE,
        );
        expect_reject(
            &world.apply(&place(&hash, 0.0, 0.0, 45)),
            protocol_codes::PLACEMENT_ROTATION_INVALID,
        );
        assert_eq!(placement_state(&world), before);
    }

    #[test]
    fn place_blueprint_rejects_overlap_and_insufficient_funds_without_mutation() {
        let dir = PlacementTempDir::new("overlap");
        let hash = write_test_package(&dir, "townsmith:overlap", 1_000.0, None);
        let mut world = placement_world(&dir);
        assert!(matches!(
            world.apply(&place(&hash, 0.0, 0.0, 0)),
            ServerMessage::PlaceBlueprintResult { .. }
        ));
        let after_first = placement_state(&world);

        match world.apply(&place(&hash, 2.0, 0.0, 0)) {
            ServerMessage::Reject { code, .. } => {
                assert_eq!(code, protocol_codes::PLACEMENT_OVERLAP)
            }
            other => panic!("expected overlap, got {other:?}"),
        }
        assert_eq!(placement_state(&world), after_first);

        let balance = world.economy.balance;
        world.economy.balance = 1.0;
        match world.apply(&place(&hash, 32.0, 0.0, 0)) {
            ServerMessage::Reject { code, .. } => {
                assert_eq!(code, protocol_codes::PLACEMENT_INSUFFICIENT_FUNDS)
            }
            other => panic!("expected insufficient funds, got {other:?}"),
        }
        world.economy.balance = balance;
        assert_eq!(placement_state(&world), after_first);
    }

    #[test]
    fn place_blueprint_charges_the_first_installment_and_creates_a_site() {
        let dir = PlacementTempDir::new("accept");
        let hash = write_test_package(&dir, "townsmith:accept", 1_000.0, None);
        let mut world = placement_world(&dir);
        let before = placement_state(&world);

        let message = world.apply(&place(&hash, 0.0, 0.0, 0));
        let (view, tick, placement_checksum, world_checksum) = expect_placement_result(&message);
        assert_eq!(tick, 1);
        assert_eq!(view.id, 1);
        assert_eq!(view.blueprint_id, "townsmith:accept");
        assert_eq!(view.blueprint_hash, hash);
        assert_eq!(view.center, [0.0, 40.0, 0.0]);
        assert_eq!(view.rotation_deg, 0);
        assert_eq!(view.footprint_m, [3.2, 3.2]);
        assert_eq!(view.material_cost, 1_000.0);
        assert_eq!(view.solid_voxels, 1);
        assert_eq!(placement_checksum.len(), 16);
        assert_eq!(world_checksum, world.checksum());
        assert_ne!(world.checksum(), before.0);

        let installment = first_construction_payment(1_000.0);
        assert!((world.economy.balance - (STARTING_BALANCE - installment)).abs() < 1.0e-9);
        let placements = world.placements.as_ref().expect("placements");
        assert_eq!(placements.placements.len(), 1);
        let stored = &placements.placements[0];
        assert_eq!(stored.blueprint_path, placement_package_reference(&hash));
        assert!(!stored.blueprint_path.contains('/'));
        let construction = world.construction.as_ref().expect("construction");
        let site = construction.site(1).expect("site");
        assert!((site.cost_total - 1_000.0).abs() < 1.0e-9);
        assert!((site.cost_paid - installment).abs() < 1.0e-9);
        assert_eq!(site.phase, ConstructionPhase::Survey);
    }

    #[test]
    fn place_blueprint_is_idempotent_for_an_exact_repeat() {
        let dir = PlacementTempDir::new("idempotent");
        let hash = write_test_package(&dir, "townsmith:idempotent", 1_000.0, None);
        let mut world = placement_world(&dir);

        let first = world.apply(&place(&hash, 0.0, 0.0, 0));
        let (view, _, placement_checksum, world_checksum) = expect_placement_result(&first);
        let placement_id = view.id;
        let tick = world.tick;
        let balance = world.economy.balance;

        for _ in 0..3 {
            let repeat = world.apply(&place(&hash, 0.0, 0.0, 0));
            let (repeat_view, repeat_tick, repeat_placement, repeat_world) =
                expect_placement_result(&repeat);
            assert_eq!(repeat_view.id, placement_id);
            assert_eq!(repeat_tick, tick);
            assert_eq!(repeat_placement, placement_checksum);
            assert_eq!(repeat_world, world_checksum);
            assert_eq!(world.economy.balance, balance);
            assert_eq!(world.tick, tick);
            assert_eq!(world.placements.as_ref().unwrap().placements.len(), 1);
            assert_eq!(world.construction.as_ref().unwrap().site_count(), 1);
        }

        // A different centre is a new placement, not a duplicate.
        assert!(matches!(
            world.apply(&place(&hash, 16.0, 0.0, 0)),
            ServerMessage::PlaceBlueprintResult { .. }
        ));
        assert_eq!(world.placements.as_ref().unwrap().placements.len(), 2);
    }

    // ---- D-033 settlement build permission ----

    /// Hello one connection and return its stable player identity.
    fn register_actor(
        world: &mut ServerWorld,
        connection: u64,
        name: &str,
        role: ClientRole,
    ) -> PlayerId {
        world.set_session_role(connection, role);
        let messages = world.apply_session(connection, &hello_named(name));
        assert!(
            matches!(messages.first(), Some(ServerMessage::Welcome { .. })),
            "expected welcome for {name}, got {messages:?}"
        );
        world
            .players
            .find_by_display_name(name)
            .first()
            .map(|player| player.id)
            .expect("registered actor")
    }

    /// Seed one settlement, owned by `owner` or unowned when `None`.
    fn seed_settlement(
        world: &mut ServerWorld,
        owner: Option<PlayerId>,
        name: &str,
        center: [i32; 2],
        radius_m: u32,
    ) -> u64 {
        let mut store = world
            .settlements
            .take()
            .unwrap_or_else(|| SettlementStore::new(SEED));
        let request = SettlementRequest {
            name: name.to_string(),
            owner: "settlement-label".to_string(),
            center,
            radius_m,
        };
        let settlement = match owner {
            Some(owner) => store.found_for_player(&request, owner),
            None => store.found(&request),
        }
        .expect("seed settlement");
        world.settlements = Some(store);
        settlement.id
    }

    /// Invite one member using the settlement's current revision.
    fn invite_actor(
        world: &mut ServerWorld,
        owner: PlayerId,
        settlement_id: u64,
        player: PlayerId,
        role: SettlementRole,
    ) {
        let revision = world
            .settlements
            .as_ref()
            .and_then(|store| store.get(settlement_id))
            .expect("settlement")
            .membership_revision;
        world
            .settlements
            .as_mut()
            .expect("settlement store")
            .invite(owner, settlement_id, player, role, revision)
            .expect("invite member");
    }

    fn settlement_checksum(world: &ServerWorld) -> Option<u64> {
        world.settlements.as_ref().map(SettlementStore::checksum)
    }

    fn last_audit_code(world: &ServerWorld) -> &'static str {
        world.audit_state().last().expect("audit entry").code
    }

    #[test]
    fn settlement_owner_places_inside_the_owned_settlement() {
        let dir = PlacementTempDir::new("settlement-owner");
        let hash = write_test_package(&dir, "townsmith:settlement_owner", 1_000.0, None);
        let mut world = ServerWorld::with_role(SEED, ClientRole::Mayor);
        world.set_blueprint_library(dir.library()).expect("library");
        let owner = register_actor(&mut world, 1, "owner", ClientRole::Mayor);
        seed_settlement(&mut world, Some(owner), "Ownerstead", [0, 0], 64);

        let message = first_response(world.apply_session(1, &place(&hash, 12.0, -8.0, 0)));
        let (view, tick, _, _) = expect_placement_result(&message);
        assert_eq!(view.id, 1);
        assert_eq!(tick, 1);
        assert_eq!(view.center, [12.0, 40.0, -8.0]);
        assert_eq!(last_audit_code(&world), AUDIT_ACCEPTED);
    }

    fn found_settlement(name: &str, x: i32, z: i32, radius_m: u32) -> ClientCommand {
        ClientCommand::FoundSettlement {
            name: name.to_string(),
            x,
            z,
            radius_m,
        }
    }

    #[test]
    fn settlement_found_assigns_the_calling_player_as_owner() {
        let mut world = world_with_hello();
        let player = world.player_id().expect("hello assigns a player id");
        let checksum_before = world.checksum();

        let found = world.apply(&found_settlement("Harbour", 10, -20, 64));
        let ServerMessage::SettlementFound {
            tick,
            settlement,
            settlement_checksum,
            world_checksum,
        } = found
        else {
            panic!("expected settlement_found, got {found:?}");
        };
        assert_eq!(tick, 0);
        assert_eq!(settlement.id, 1);
        assert_eq!(settlement.name, "Harbour");
        assert_eq!(settlement.center, [10, -20]);
        assert_eq!(settlement.radius_m, 64);
        assert_eq!(settlement.owner_id, Some(player.get()));
        assert!(settlement.owned);
        assert_eq!(settlement.members.len(), 1);
        assert_eq!(settlement.members[0].player, player.get());
        assert_eq!(settlement.members[0].role, "owner");
        assert_eq!(settlement.members[0].permissions, 15);
        assert_eq!(settlement.membership_revision, 1);
        assert_eq!(settlement_checksum.len(), 16);
        assert_ne!(world_checksum, checksum_before);
        assert_eq!(world_checksum, world.checksum());

        let list = world.apply(&ClientCommand::SettlementList);
        let ServerMessage::SettlementList {
            count,
            checksum,
            settlements,
            ..
        } = list
        else {
            panic!("expected settlement_list, got {list:?}");
        };
        assert_eq!(count, 1);
        assert_eq!(settlements.len(), 1);
        assert_eq!(settlements[0], settlement);
        assert_eq!(checksum, settlement_checksum);
    }

    #[test]
    fn settlement_found_rejects_duplicates_and_invalid_payloads_without_mutation() {
        let mut world = world_with_hello();
        world.apply(&found_settlement("Harbour", 10, 20, 64));
        let checksum = world.checksum();

        let duplicate = world.apply(&found_settlement("harbour", 30, 40, 32));
        assert!(matches!(
            duplicate,
            ServerMessage::Reject { ref code, .. } if code == "duplicate_name"
        ));
        assert_eq!(world.checksum(), checksum);

        let invalid_radius = world.apply(&found_settlement("Docks", 0, 0, 4));
        assert!(matches!(
            invalid_radius,
            ServerMessage::Reject { ref code, .. } if code == "invalid_radius"
        ));
        assert_eq!(world.checksum(), checksum);
        assert_eq!(world.settlements.as_ref().expect("store").count(), 1);
    }

    #[test]
    fn settlement_commands_require_hello_and_the_mayor_role() {
        let mut viewer = ServerWorld::with_role(SEED, ClientRole::Viewer);
        first_response(viewer.apply_all(&hello()));
        let denied = viewer.apply(&found_settlement("Harbour", 0, 0, 64));
        assert!(matches!(
            denied,
            ServerMessage::Reject { ref code, .. } if code == codes::FORBIDDEN
        ));

        let list = viewer.apply(&ClientCommand::SettlementList);
        assert!(matches!(
            list,
            ServerMessage::SettlementList { count: 0, .. }
        ));

        let mut unopened = ServerWorld::with_role(SEED, ClientRole::Mayor);
        let rejected = unopened.apply(&found_settlement("Harbour", 0, 0, 64));
        assert!(matches!(
            rejected,
            ServerMessage::Reject { ref code, .. } if code == codes::NOT_HELLO
        ));
    }

    #[test]
    fn settlement_list_starts_empty_and_found_state_survives_save_load() {
        let mut world = world_with_hello();
        let empty = world.apply(&ClientCommand::SettlementList);
        assert!(matches!(
            empty,
            ServerMessage::SettlementList {
                count: 0,
                ref checksum,
                ..
            } if checksum == "0000000000000000"
        ));

        world.apply(&found_settlement("Harbour", 10, 20, 64));
        let json = world.to_save_json_pretty().expect("save");
        let mut loaded = ServerWorld::from_save_json(&json).expect("load");
        first_response(loaded.apply_all(&hello()));
        let list = loaded.apply(&ClientCommand::SettlementList);
        let ServerMessage::SettlementList {
            count, settlements, ..
        } = list
        else {
            panic!("expected settlement_list, got {list:?}");
        };
        assert_eq!(count, 1);
        assert_eq!(settlements[0].name, "Harbour");
        assert_eq!(
            settlements[0].owner_id,
            world.player_id().map(PlayerId::get)
        );
    }

    fn settlement_invite(
        settlement_id: u64,
        player_id: u64,
        role: &str,
        expected_revision: u32,
    ) -> ClientCommand {
        ClientCommand::SettlementInvite {
            settlement_id,
            player_id,
            role: role.to_string(),
            expected_revision,
        }
    }

    fn settlement_set_role(
        settlement_id: u64,
        player_id: u64,
        role: &str,
        expected_revision: u32,
    ) -> ClientCommand {
        ClientCommand::SettlementSetRole {
            settlement_id,
            player_id,
            role: role.to_string(),
            expected_revision,
        }
    }

    fn settlement_remove_member(
        settlement_id: u64,
        player_id: u64,
        expected_revision: u32,
    ) -> ClientCommand {
        ClientCommand::SettlementRemoveMember {
            settlement_id,
            player_id,
            expected_revision,
        }
    }

    #[test]
    fn settlement_owner_invites_changes_roles_and_removes_members() {
        let mut world = world_with_hello();
        world.apply(&found_settlement("Harbour", 10, 20, 64));
        let before = world.checksum();
        let bob = world.players.register("bob").expect("bob").id.get();

        let invited = world.apply(&settlement_invite(1, bob, "builder", 1));
        let ServerMessage::SettlementUpdated {
            settlement,
            settlement_checksum,
            world_checksum,
            ..
        } = invited
        else {
            panic!("expected settlement_updated, got {invited:?}");
        };
        assert_eq!(settlement.membership_revision, 2);
        assert_eq!(settlement.members.len(), 2);
        assert_eq!(settlement.members[1].player, bob);
        assert_eq!(settlement.members[1].role, "builder");
        assert_eq!(settlement.members[1].permissions, 1);
        assert_ne!(world_checksum, before);
        assert_eq!(world_checksum, world.checksum());
        assert_eq!(settlement_checksum.len(), 16);

        let promoted = world.apply(&settlement_set_role(1, bob, "manager", 2));
        let ServerMessage::SettlementUpdated { settlement, .. } = promoted else {
            panic!("expected settlement_updated, got {promoted:?}");
        };
        assert_eq!(settlement.membership_revision, 3);
        assert_eq!(settlement.members[1].role, "manager");
        assert_eq!(settlement.members[1].permissions, 15);

        let removed = world.apply(&settlement_remove_member(1, bob, 3));
        let ServerMessage::SettlementUpdated { settlement, .. } = removed else {
            panic!("expected settlement_updated, got {removed:?}");
        };
        assert_eq!(settlement.membership_revision, 4);
        assert_eq!(settlement.members.len(), 1);
        assert_eq!(settlement.members[0].role, "owner");
    }

    #[test]
    fn settlement_membership_requires_members_permission_and_revision() {
        let mut world = world_with_hello();
        world.apply(&found_settlement("Harbour", 0, 0, 64));
        let bob = world.players.register("bob").expect("bob").id;
        let carol = world.players.register("carol").expect("carol").id.get();
        world.apply(&settlement_invite(1, bob.get(), "builder", 1));
        let checksum = world.checksum();

        world.player_id = Some(bob);
        let denied = world.apply(&settlement_invite(1, carol, "member", 2));
        assert!(matches!(
            denied,
            ServerMessage::Reject { ref code, .. } if code == "settlement.forbidden"
        ));
        assert_eq!(world.checksum(), checksum);

        world.player_id = Some(PlayerId::new(1).expect("owner"));
        let stale = world.apply(&settlement_invite(1, carol, "member", 1));
        assert!(matches!(
            stale,
            ServerMessage::Reject { ref code, .. } if code == "settlement.revision_conflict"
        ));
        let invalid_role = world.apply(&settlement_invite(1, carol, "emperor", 2));
        assert!(matches!(
            invalid_role,
            ServerMessage::Reject { ref code, .. } if code == codes::SETTLEMENT_ROLE_INVALID
        ));
        let unknown = world.apply(&settlement_invite(99, carol, "member", 2));
        assert!(matches!(
            unknown,
            ServerMessage::Reject { ref code, .. } if code == "settlement.unknown"
        ));
        let owner_remove = world.apply(&settlement_remove_member(1, 1, 2));
        assert!(matches!(
            owner_remove,
            ServerMessage::Reject { ref code, .. } if code == "settlement.owner_required"
        ));
        assert_eq!(world.checksum(), checksum);
    }

    #[test]
    fn settlement_membership_requires_the_mayor_role_and_survives_save_load() {
        let mut viewer = ServerWorld::with_role(SEED, ClientRole::Viewer);
        first_response(viewer.apply_all(&hello()));
        let denied = viewer.apply(&settlement_invite(1, 2, "member", 1));
        assert!(matches!(
            denied,
            ServerMessage::Reject { ref code, .. } if code == codes::FORBIDDEN
        ));

        let mut world = world_with_hello();
        world.apply(&found_settlement("Harbour", 0, 0, 64));
        let bob = world.players.register("bob").expect("bob").id.get();
        world.apply(&settlement_invite(1, bob, "builder", 1));
        let json = world.to_save_json_pretty().expect("save");
        let mut loaded = ServerWorld::from_save_json(&json).expect("load");
        first_response(loaded.apply_all(&hello()));
        let list = loaded.apply(&ClientCommand::SettlementList);
        let ServerMessage::SettlementList { settlements, .. } = list else {
            panic!("expected settlement_list, got {list:?}");
        };
        assert_eq!(settlements[0].members.len(), 2);
        assert_eq!(settlements[0].membership_revision, 2);
        assert_eq!(settlements[0].members[1].player, bob);
    }

    #[test]
    fn settlement_claim_takes_an_unowned_settlement_once() {
        let mut world = world_with_hello();
        let player = world.player_id().expect("player id");
        let id = seed_settlement(&mut world, None, "Old Town", [10, 10], 64);
        let before = world.checksum();

        let claimed = world.apply(&ClientCommand::SettlementClaim {
            settlement_id: id,
            expected_revision: 0,
        });
        let ServerMessage::SettlementUpdated { settlement, .. } = claimed else {
            panic!("expected settlement_updated, got {claimed:?}");
        };
        assert_eq!(settlement.owner_id, Some(player.get()));
        assert_eq!(settlement.membership_revision, 1);
        assert_eq!(settlement.members.len(), 1);
        assert_eq!(settlement.members[0].player, player.get());
        assert_eq!(settlement.members[0].role, "owner");
        assert_ne!(world.checksum(), before);

        let again = world.apply(&ClientCommand::SettlementClaim {
            settlement_id: id,
            expected_revision: 1,
        });
        assert!(matches!(
            again,
            ServerMessage::Reject { ref code, .. } if code == "settlement.already_owned"
        ));
        let stale = world.apply(&ClientCommand::SettlementClaim {
            settlement_id: id,
            expected_revision: 0,
        });
        assert!(matches!(
            stale,
            ServerMessage::Reject { ref code, .. } if code == "settlement.revision_conflict"
        ));
    }

    #[test]
    fn settlement_at_finds_the_nearest_containing_record() {
        let mut world = world_with_hello();
        let outside = world.apply(&ClientCommand::SettlementAt { x: 500, z: 500 });
        assert!(matches!(
            outside,
            ServerMessage::SettlementAt {
                found: false,
                settlement: None,
                ..
            }
        ));

        seed_settlement(&mut world, None, "Old Town", [10, 10], 64);
        let inside = world.apply(&ClientCommand::SettlementAt { x: 12, z: 12 });
        let ServerMessage::SettlementAt {
            found,
            settlement: Some(settlement),
            ..
        } = inside
        else {
            panic!("expected a containing settlement, got {inside:?}");
        };
        assert!(found);
        assert_eq!(settlement.id, 1);
        assert_eq!(settlement.name, "Old Town");

        let outside = world.apply(&ClientCommand::SettlementAt { x: 300, z: 300 });
        assert!(matches!(
            outside,
            ServerMessage::SettlementAt {
                found: false,
                settlement: None,
                ..
            }
        ));
    }

    #[test]
    fn settlement_claim_requires_mayor_but_at_is_viewer_readable() {
        let mut viewer = ServerWorld::with_role(SEED, ClientRole::Viewer);
        first_response(viewer.apply_all(&hello()));
        let denied = viewer.apply(&ClientCommand::SettlementClaim {
            settlement_id: 1,
            expected_revision: 0,
        });
        assert!(matches!(
            denied,
            ServerMessage::Reject { ref code, .. } if code == codes::FORBIDDEN
        ));
        let at = viewer.apply(&ClientCommand::SettlementAt { x: 0, z: 0 });
        assert!(matches!(
            at,
            ServerMessage::SettlementAt {
                found: false,
                settlement: None,
                ..
            }
        ));
    }

    #[test]
    fn settlement_rename_requires_manage_and_a_unique_name() {
        let mut world = world_with_hello();
        world.apply(&found_settlement("Harbour", 0, 0, 64));
        world.apply(&found_settlement("Docks", 200, 0, 64));
        let bob = world.players.register("bob").expect("bob").id.get();

        let renamed = world.apply(&ClientCommand::SettlementRename {
            settlement_id: 1,
            name: "  Old Harbour  ".to_string(),
            expected_revision: 1,
        });
        let ServerMessage::SettlementUpdated { settlement, .. } = renamed else {
            panic!("expected settlement_updated, got {renamed:?}");
        };
        assert_eq!(settlement.name, "Old Harbour");
        assert_eq!(settlement.membership_revision, 2);

        let duplicate = world.apply(&ClientCommand::SettlementRename {
            settlement_id: 1,
            name: "docks".to_string(),
            expected_revision: 2,
        });
        assert!(matches!(
            duplicate,
            ServerMessage::Reject { ref code, .. } if code == "duplicate_name"
        ));
        let stale = world.apply(&ClientCommand::SettlementRename {
            settlement_id: 1,
            name: "Harbour".to_string(),
            expected_revision: 1,
        });
        assert!(matches!(
            stale,
            ServerMessage::Reject { ref code, .. } if code == "settlement.revision_conflict"
        ));

        world.apply(&settlement_invite(1, bob, "builder", 2));
        world.player_id = Some(PlayerId::new(bob).expect("bob"));
        let forbidden = world.apply(&ClientCommand::SettlementRename {
            settlement_id: 1,
            name: "Bob's Dock".to_string(),
            expected_revision: 3,
        });
        assert!(matches!(
            forbidden,
            ServerMessage::Reject { ref code, .. } if code == "settlement.forbidden"
        ));
    }

    #[test]
    fn settlement_transfer_assigns_a_known_recipient_as_owner() {
        let mut world = world_with_hello();
        world.apply(&found_settlement("Harbour", 0, 0, 64));
        let bob = world.players.register("bob").expect("bob").id.get();
        let owner = world.player_id().expect("owner id").get();

        let transferred = world.apply(&ClientCommand::SettlementTransfer {
            settlement_id: 1,
            player_id: bob,
            expected_revision: 1,
        });
        let ServerMessage::SettlementUpdated { settlement, .. } = transferred else {
            panic!("expected settlement_updated, got {transferred:?}");
        };
        assert_eq!(settlement.owner_id, Some(bob));
        assert_eq!(settlement.membership_revision, 2);
        assert_eq!(settlement.members.len(), 2);
        assert_eq!(settlement.members[0].player, owner);
        assert_eq!(settlement.members[0].role, "manager");
        assert_eq!(settlement.members[1].player, bob);
        assert_eq!(settlement.members[1].role, "owner");

        let unknown = world.apply(&ClientCommand::SettlementTransfer {
            settlement_id: 1,
            player_id: 999,
            expected_revision: 2,
        });
        assert!(matches!(
            unknown,
            ServerMessage::Reject { ref code, .. } if code == codes::SETTLEMENT_UNKNOWN_PLAYER
        ));

        world.player_id = Some(PlayerId::new(owner).expect("owner"));
        let stale = world.apply(&ClientCommand::SettlementTransfer {
            settlement_id: 1,
            player_id: owner,
            expected_revision: 1,
        });
        assert!(matches!(
            stale,
            ServerMessage::Reject { ref code, .. } if code == "settlement.revision_conflict"
        ));
    }

    #[test]
    fn admin_release_clears_a_dead_owner_settlement() {
        let mut world = world_with_hello();
        world.apply(&found_settlement("Harbour", 0, 0, 64));
        let before = world.checksum();

        let released = world.apply(&ClientCommand::SettlementRelease {
            settlement_id: 1,
            expected_revision: 1,
        });
        let ServerMessage::SettlementUpdated { settlement, .. } = released else {
            panic!("expected settlement_updated, got {released:?}");
        };
        assert_eq!(settlement.owner_id, None);
        assert!(!settlement.owned);
        assert!(settlement.members.is_empty());
        assert_eq!(settlement.membership_revision, 2);
        assert_ne!(world.checksum(), before);

        let again = world.apply(&ClientCommand::SettlementRelease {
            settlement_id: 1,
            expected_revision: 2,
        });
        assert!(matches!(
            again,
            ServerMessage::Reject { ref code, .. } if code == "settlement.unowned"
        ));
        let stale = world.apply(&ClientCommand::SettlementRelease {
            settlement_id: 1,
            expected_revision: 0,
        });
        assert!(matches!(
            stale,
            ServerMessage::Reject { ref code, .. } if code == "settlement.revision_conflict"
        ));
    }

    #[test]
    fn settlement_release_is_admin_only() {
        let mut mayor = ServerWorld::with_role(SEED, ClientRole::Mayor);
        first_response(mayor.apply_all(&hello()));
        mayor.apply(&found_settlement("Harbour", 0, 0, 64));
        let denied = mayor.apply(&ClientCommand::SettlementRelease {
            settlement_id: 1,
            expected_revision: 1,
        });
        assert!(matches!(
            denied,
            ServerMessage::Reject { ref code, .. } if code == codes::FORBIDDEN
        ));
        assert_eq!(
            mayor
                .settlements
                .as_ref()
                .expect("store")
                .get(1)
                .expect("settlement")
                .owner_id
                .map(PlayerId::get),
            mayor.player_id().map(PlayerId::get),
            "denied release leaves ownership unchanged"
        );
    }

    #[test]
    fn settlement_members_need_the_build_permission() {
        let dir = PlacementTempDir::new("settlement-roles");
        let hash = write_test_package(&dir, "townsmith:settlement_roles", 1_000.0, None);
        let mut world = ServerWorld::with_role(SEED, ClientRole::Mayor);
        world.set_blueprint_library(dir.library()).expect("library");

        let owner = register_actor(&mut world, 1, "owner", ClientRole::Mayor);
        let builder = register_actor(&mut world, 2, "builder", ClientRole::Mayor);
        let member = register_actor(&mut world, 3, "member", ClientRole::Mayor);
        let _outsider = register_actor(&mut world, 4, "outsider", ClientRole::Mayor);

        let id = seed_settlement(&mut world, Some(owner), "Guildstead", [0, 0], 64);
        invite_actor(&mut world, owner, id, builder, SettlementRole::Builder);
        invite_actor(&mut world, owner, id, member, SettlementRole::Member);
        let settlements = settlement_checksum(&world);

        // A builder member may place.
        let message = first_response(world.apply_session(2, &place(&hash, 16.0, 0.0, 0)));
        let (view, _, _, _) = expect_placement_result(&message);
        assert_eq!(view.id, 1);

        // A plain member is a member without BUILD.
        let before = placement_state(&world);
        let denied = first_response(world.apply_session(3, &place(&hash, -16.0, 0.0, 0)));
        expect_reject(&denied, codes::SETTLEMENT_FORBIDDEN);
        assert_eq!(last_audit_code(&world), codes::SETTLEMENT_FORBIDDEN);
        assert_eq!(placement_state(&world), before);
        assert_eq!(settlement_checksum(&world), settlements);

        // An outsider is not a member at all.
        let denied = first_response(world.apply_session(4, &place(&hash, 0.0, 16.0, 0)));
        expect_reject(&denied, codes::SETTLEMENT_NOT_MEMBER);
        assert_eq!(last_audit_code(&world), codes::SETTLEMENT_NOT_MEMBER);
        assert_eq!(placement_state(&world), before);
        assert_eq!(settlement_checksum(&world), settlements);

        // Idempotency does not bypass the permission check: repeating the
        // builder's accepted request as a plain member is still rejected and
        // does not acknowledge the existing placement.
        let denied = first_response(world.apply_session(3, &place(&hash, 16.0, 0.0, 0)));
        expect_reject(&denied, codes::SETTLEMENT_FORBIDDEN);
        assert_eq!(
            world
                .placements
                .as_ref()
                .expect("placements")
                .placements
                .len(),
            1
        );
        assert_eq!(placement_state(&world), before);
    }

    #[test]
    fn unowned_settlement_rejects_non_admins() {
        let dir = PlacementTempDir::new("settlement-unowned");
        let hash = write_test_package(&dir, "townsmith:settlement_unowned", 1_000.0, None);
        let mut world = ServerWorld::with_role(SEED, ClientRole::Mayor);
        world.set_blueprint_library(dir.library()).expect("library");
        register_actor(&mut world, 1, "mayor", ClientRole::Mayor);
        seed_settlement(&mut world, None, "Freehold", [0, 0], 64);
        let before = placement_state(&world);
        let settlements = settlement_checksum(&world);

        let denied = first_response(world.apply_session(1, &place(&hash, 4.0, 4.0, 0)));
        expect_reject(&denied, codes::SETTLEMENT_UNOWNED);
        assert_eq!(last_audit_code(&world), codes::SETTLEMENT_UNOWNED);
        assert_eq!(placement_state(&world), before);
        assert_eq!(settlement_checksum(&world), settlements);
    }

    #[test]
    fn settlement_identity_is_required_for_non_admins() {
        let dir = PlacementTempDir::new("settlement-identity");
        let hash = write_test_package(&dir, "townsmith:settlement_identity", 1_000.0, None);
        let mut world = placement_world(&dir);
        let owner = world.player_id().expect("hello assigns an id");
        seed_settlement(&mut world, Some(owner), "Ownerstead", [0, 0], 64);
        // Simulate a legacy session without a registered identity.
        world.player_id = None;
        let before = placement_state(&world);

        let denied = world.apply(&place(&hash, 0.0, 0.0, 0));
        expect_reject(&denied, codes::SETTLEMENT_IDENTITY);
        assert_eq!(last_audit_code(&world), codes::SETTLEMENT_IDENTITY);
        assert_eq!(placement_state(&world), before);
    }

    #[test]
    fn admin_bypasses_every_settlement_check() {
        let dir = PlacementTempDir::new("settlement-admin");
        let hash = write_test_package(&dir, "townsmith:settlement_admin", 1_000.0, None);
        let mut world = ServerWorld::with_role(SEED, ClientRole::Admin);
        world.set_blueprint_library(dir.library()).expect("library");
        assert!(matches!(
            world.apply(&hello_named("admin")),
            ServerMessage::Welcome { .. }
        ));
        let owner = world.players.register("owner").expect("register owner").id;
        seed_settlement(&mut world, Some(owner), "Walled", [0, 0], 64);
        seed_settlement(&mut world, None, "Freehold", [128, 0], 64);
        // An admin bypasses even the identity check.
        world.player_id = None;

        let message = world.apply(&place(&hash, 0.0, 0.0, 0));
        let (view, _, _, _) = expect_placement_result(&message);
        assert_eq!(view.id, 1, "admin places in a settlement it does not own");

        let message = world.apply(&place(&hash, 128.0, 0.0, 0));
        let (view, _, _, _) = expect_placement_result(&message);
        assert_eq!(view.id, 2, "admin places in an unowned settlement");
    }

    #[test]
    fn place_blueprint_stores_two_sequential_placements() {
        let dir = PlacementTempDir::new("sequential");
        let hash = write_test_package(&dir, "townsmith:sequential", 1_000.0, None);
        let mut world = placement_world(&dir);
        let installment = first_construction_payment(1_000.0);

        for (x, expected_id) in [(0.0, 1), (16.0, 2)] {
            match world.apply(&place(&hash, x, 0.0, 0)) {
                ServerMessage::PlaceBlueprintResult {
                    placement_id,
                    placement,
                    ..
                } => {
                    assert_eq!(placement_id, expected_id);
                    assert_eq!(placement.id, expected_id);
                }
                other => panic!("expected placement {expected_id}, got {other:?}"),
            }
        }
        assert_eq!(world.placements.as_ref().unwrap().placements.len(), 2);
        assert_eq!(world.construction.as_ref().unwrap().site_count(), 2);
        assert!((world.economy.balance - (STARTING_BALANCE - 2.0 * installment)).abs() < 1.0e-9);
        assert_eq!(world.tick, 2);
    }

    #[test]
    fn restore_without_library_rejects_placement_until_reconfigured() {
        let dir = PlacementTempDir::new("restore");
        let hash = write_test_package(&dir, "townsmith:restore", 1_000.0, None);
        let mut world = placement_world(&dir);
        let first = world.apply(&place(&hash, 0.0, 0.0, 0));
        let (view, _, _, _) = expect_placement_result(&first);
        let placement_id = view.id;
        let checksum = world.checksum();

        let json = world.to_save_json_pretty().expect("json");
        let mut loaded = ServerWorld::from_save_json(&json).expect("load");
        assert!(loaded.blueprint_library.is_none());
        assert!(loaded.policies.is_none());
        assert_eq!(loaded.checksum(), checksum);
        assert!(matches!(
            loaded.apply(&hello()),
            ServerMessage::Welcome { .. }
        ));
        expect_reject(
            &loaded.apply(&place(&hash, 0.0, 0.0, 0)),
            protocol_codes::PLACEMENT_LIBRARY_UNAVAILABLE,
        );
        assert_eq!(loaded.checksum(), checksum);

        loaded
            .set_blueprint_library(dir.library())
            .expect("library");
        let repeat = loaded.apply(&place(&hash, 0.0, 0.0, 0));
        let (repeat_view, _, _, repeat_world) = expect_placement_result(&repeat);
        assert_eq!(repeat_view.id, placement_id);
        assert_eq!(repeat_world, checksum);
    }

    #[test]
    fn placement_stores_move_the_checksum_only_when_present() {
        let dir = PlacementTempDir::new("checksum");
        let hash = write_test_package(&dir, "townsmith:checksum", 1_000.0, None);
        let mut world = placement_world(&dir);
        let without = world.checksum();

        assert!(matches!(
            world.apply(&place(&hash, 0.0, 0.0, 0)),
            ServerMessage::PlaceBlueprintResult { .. }
        ));
        let with = world.checksum();
        assert_ne!(with, without);

        // A fresh world with the same state but no placement store hashes the
        // old way; the optional fields are appended only when present.
        let mut fresh = placement_world(&dir);
        assert_eq!(fresh.checksum(), without);
        assert!(matches!(
            fresh.apply(&place(&hash, 0.0, 0.0, 0)),
            ServerMessage::PlaceBlueprintResult { .. }
        ));
        assert_eq!(fresh.checksum(), with);
    }

    #[test]
    fn placement_checksums_are_root_independent_and_path_free() {
        let first = PlacementTempDir::new("root-a");
        let second = PlacementTempDir::new("root-b");
        let hash = write_test_package(&first, "townsmith:root_independent", 1_000.0, None);
        assert_eq!(
            hash,
            write_test_package(&second, "townsmith:root_independent", 1_000.0, None)
        );

        let mut world_a = placement_world(&first);
        let mut world_b = placement_world(&second);
        let message_a = world_a.apply(&place(&hash, 0.0, 0.0, 0));
        let message_b = world_b.apply(&place(&hash, 0.0, 0.0, 0));
        let (view_a, _, placement_checksum_a, world_checksum_a) =
            expect_placement_result(&message_a);
        let (view_b, _, placement_checksum_b, world_checksum_b) =
            expect_placement_result(&message_b);
        assert_eq!(placement_checksum_a, placement_checksum_b);
        assert_eq!(world_checksum_a, world_checksum_b);
        assert_eq!(world_a.checksum(), world_b.checksum());
        assert_eq!(view_a, view_b);

        let json = world_a.to_save_json_pretty().expect("save json");
        assert!(!json.contains(first.path().to_string_lossy().as_ref()));
        assert!(!json.contains(second.path().to_string_lossy().as_ref()));
        assert!(!json.contains("/tmp"));
        assert!(!json.contains("/media"));
        let value: serde_json::Value = serde_json::from_str(&json).expect("save value");
        let stored = value["placements"]["placements"][0]["blueprint_path"]
            .as_str()
            .expect("stored reference");
        assert_eq!(stored, placement_package_reference(&hash));
        assert!(!stored.contains('/'));

        for message in [&message_a, &message_b] {
            let wire = serde_json::to_string(message).expect("wire json");
            assert!(!wire.contains("blueprint_path"));
            assert!(!wire.contains("/tmp"));
            assert!(!wire.contains("/media"));
        }
    }
}
