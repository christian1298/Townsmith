//! First authoritative command executor on top of protocol v0.
//!
//! [`ServerWorld`] owns the mutable server state for one session: the clock,
//! static population counters, the treasury, and the road store. It applies
//! decoded [`ClientCommand`]s and answers with one [`ServerMessage`].
//!
//! Persistence lives in [`crate::save`] and is opt-in: one world to one JSON
//! file, with atomic writes and a single `.bak` rotation. The executor itself
//! has no authentication, interest management, rollback, terrain sampling, or
//! wall-clock loop. Time advances only through `time_control` (`step`) or an
//! explicit [`ServerWorld::advance_day`] call, so transcripts are
//! deterministic.

use std::collections::{BTreeSet, VecDeque};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::thread;

use core_city::economy::{Economy, EconomyTick, DAY_SECONDS, STARTING_BALANCE};
use core_city::road::{RoadError, RoadRequest, RoadStore};

use crate::protocol::{
    configure_stream, decode_client_command, frame_to_io, read_frame, read_json_frame,
    session_id_for_seed, validate_hello, validate_road, validate_tax_rates, write_json_frame,
    ClientCommand, FrameError, Rejection, ServerMessage, SessionConfig, TimeAction, LOOPBACK_SEED,
    PROTOCOL_VERSION, SESSION_IO_TIMEOUT,
};

/// Fixed road width for v0 placement commands.
pub const WORLD_ROAD_WIDTH_M: u8 = 6;

/// Longest road a single `place_road` command may add, in cells.
pub const MAX_WORLD_ROAD_LENGTH: i32 = 512;

/// Placeholder flat ground height used for road validation.
///
/// The server has no terrain yet. This is above `WATER_LEVEL_M` (24 m) and
/// constant, so only geometry, bounds, and length rules can fail.
pub const PLACEHOLDER_GROUND_M: u16 = 40;

/// Deterministic client commands exchanged by the authority loopback.
pub const AUTHORITY_COMMANDS: u64 = 6;

/// Application frames exchanged in both directions by the authority loopback.
pub const AUTHORITY_FRAMES: u64 = 12;

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
    /// Read-only: `hello`, `ping`, and `disconnect`.
    #[default]
    Viewer,
    /// Viewer plus economy and placement commands (`set_tax`, `place_road`).
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
            "hello" | "resume" | "ping" | "disconnect" => Self::Viewer,
            "set_tax" | "place_road" | "time_control" => Self::Mayor,
            // Any future/unknown tag.
            _ => Self::Admin,
        }
    }

    /// Minimum role for a concrete command. `time_control` is Mayor for
    /// pause/resume, but Admin for a one-day step.
    #[must_use]
    pub fn required_role_for(command: &ClientCommand) -> Self {
        match command {
            ClientCommand::TimeControl {
                action: TimeAction::Step,
            } => Self::Admin,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResumeWindow {
    /// Last authoritative tick at which `resume` is still accepted.
    deadline_tick: u64,
}

impl ResumeWindow {
    fn starting_at(tick: u64) -> Self {
        Self {
            deadline_tick: tick.saturating_add(RESUME_GRACE_TICKS),
        }
    }

    fn is_open_at(self, tick: u64) -> bool {
        tick <= self.deadline_tick
    }
}

/// Authoritative state for one protocol v0 session.
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
    /// Whether `advance_day` is frozen.
    pub paused: bool,
    /// Deterministic time-control vote state (D-021).
    pub vote: crate::vote::TimeVote,
    /// Players currently online in this process.
    online: BTreeSet<String>,
    /// Hello player name for the session.
    player: Option<String>,
    /// Checksum of the last accepted state, refreshed after each mutation.
    pub session_checksum: String,
    role: ClientRole,
    /// Monotonic count of applied commands, including rejections.
    commands: u64,
    rate: RateBucket,
    audit: VecDeque<AuditEntry>,
    hello: bool,
    closed: bool,
    resume_window: Option<ResumeWindow>,
    last_seen_tick: u64,
    session_id: String,
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
            paused: false,
            vote: crate::vote::TimeVote::default(),
            online: BTreeSet::new(),
            player: None,
            session_checksum: String::new(),
            role,
            commands: 0,
            rate: RateBucket::full(),
            audit: VecDeque::with_capacity(AUDIT_CAPACITY),
            hello: false,
            closed: false,
            resume_window: None,
            last_seen_tick: 0,
            session_id: session_id_for_seed(seed),
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
    pub fn set_role(&mut self, role: ClientRole) {
        self.role = role;
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
        let mut world = Self {
            seed: save.seed,
            tick: save.tick,
            population: save.population,
            employed: save.employed,
            buildings: save.buildings,
            economy: save.economy,
            roads: save.roads,
            paused: save.paused,
            vote: crate::vote::TimeVote::default(),
            online: BTreeSet::new(),
            player: None,
            session_checksum: String::new(),
            role: ClientRole::Mayor,
            commands: 0,
            rate: RateBucket::full(),
            audit: VecDeque::with_capacity(AUDIT_CAPACITY),
            hello: false,
            closed: false,
            resume_window: None,
            last_seen_tick: save.tick,
            session_id: session_id_for_seed(save.seed),
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
    /// v0 derives it from the seed and keeps it equal to [`Self::session_id`].
    /// It identifies a session; it is **not** an authentication credential.
    #[must_use]
    pub fn session_token(&self) -> &str {
        &self.session_id
    }

    /// Tick at which the current reconnect window closes, if one is open.
    #[must_use]
    pub fn resume_deadline(&self) -> Option<u64> {
        self.resume_window.map(|window| window.deadline_tick)
    }

    /// Whether a disconnected session can still be resumed at the current tick.
    #[must_use]
    pub fn is_resumable(&self) -> bool {
        self.resume_window
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

    fn remember_disconnect(&mut self) {
        self.last_seen_tick = self.tick;
        self.resume_window = Some(ResumeWindow::starting_at(self.tick));
    }

    /// Whether an accepted `hello` has been seen.
    #[must_use]
    pub fn hello_accepted(&self) -> bool {
        self.hello
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

    /// Stable FNV-1a checksum over tick, population, balance, road checksum,
    /// and economy checksum. No wall clock or seed is mixed in.
    #[must_use]
    pub fn checksum(&self) -> String {
        let mut hash = fnv_update(FNV_OFFSET, &self.tick.to_le_bytes());
        hash = fnv_update(hash, &self.population.to_le_bytes());
        hash = fnv_update(hash, &self.balance_units().to_le_bytes());
        hash = fnv_update(hash, &self.roads.checksum().to_le_bytes());
        hash = fnv_update(hash, &self.economy.checksum().to_le_bytes());
        format!("{hash:016x}")
    }

    /// Current authoritative snapshot.
    #[must_use]
    pub fn snapshot(&self) -> ServerMessage {
        ServerMessage::Snapshot {
            tick: self.tick,
            population: self.population,
            balance: self.balance_units(),
            checksum: self.checksum(),
        }
    }

    /// Validate and apply one command, returning exactly one response.
    ///
    /// This is the single-message convenience wrapper used by tests and the
    /// authority loopback. A successful `resume` produces two messages
    /// (`welcome`, then `snapshot`); this method returns the `welcome` and
    /// drops the snapshot. Transport code must use [`Self::apply_all`].
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
    /// Every command yields one message except a successful `resume`, which
    /// yields `welcome { resumed: true }` followed by the current `snapshot`
    /// so the client can reconcile immediately. Rejections never mutate
    /// simulation state.
    pub fn apply_all(&mut self, command: &ClientCommand) -> Vec<ServerMessage> {
        self.commands = self.commands.saturating_add(1);
        let (message, code) = self.evaluate(command);
        self.record_audit(command.tag(), code);
        if self.hello && code == AUDIT_ACCEPTED {
            self.last_seen_tick = self.tick;
        }
        if matches!(&message, ServerMessage::Welcome { resumed: true, .. }) {
            vec![message, self.snapshot()]
        } else {
            vec![message]
        }
    }

    fn evaluate(&mut self, command: &ClientCommand) -> (ServerMessage, &'static str) {
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
                if let Some(window) = self.resume_window {
                    if window.is_open_at(self.tick) {
                        return reject_with(
                            codes::HELLO_RESUMABLE,
                            format!(
                                "a disconnected session can resume until tick {}; \
                                 send `resume` or wait for the grace window to expire",
                                window.deadline_tick
                            ),
                        );
                    }
                    // The grace window expired: a fresh session may start.
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
        if !matches!(
            command,
            ClientCommand::Disconnect | ClientCommand::Resume { .. }
        ) && !self.rate.try_consume(now)
        {
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
                self.hello = true;
                self.player = Some(player_name.clone());
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
                if let Some(player) = self.player.take() {
                    self.online.remove(&player);
                }
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
            ClientCommand::TimeVote { action, time } => self.apply_time_vote(*action, *time),
        }
    }

    fn apply_time_vote(
        &mut self,
        action: crate::protocol::VoteAction,
        time: Option<TimeAction>,
    ) -> (ServerMessage, &'static str) {
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
        if let Some(outcome) = self.vote.tick(self.tick, self.online.iter().cloned()) {
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
    /// reveals whether a session is resumable. The role is not part of the
    /// claim: it is the role the world already holds, so a resumed session
    /// keeps the role assigned by the server process.
    fn apply_resume(&mut self, token: &str, last_tick: u64) -> (ServerMessage, &'static str) {
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
            return reject_with(
                codes::RESUME_UNKNOWN,
                "resume token does not match this world",
            );
        }
        let Some(window) = self.resume_window else {
            return reject_with(
                codes::RESUME_UNKNOWN,
                "no disconnected session is waiting to resume",
            );
        };
        if !window.is_open_at(self.tick) {
            self.resume_window = None;
            return reject_with(
                codes::RESUME_EXPIRED,
                format!(
                    "the reconnect window closed at tick {}; the current tick is {}",
                    window.deadline_tick, self.tick
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

        self.resume_window = None;
        self.hello = true;
        self.closed = false;
        self.last_seen_tick = self.tick;
        accepted(welcome(self, true))
    }

    fn apply_time_control(&mut self, action: TimeAction) -> (ServerMessage, &'static str) {
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
    ) -> (ServerMessage, &'static str) {
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

fn welcome(world: &ServerWorld, resumed: bool) -> ServerMessage {
    ServerMessage::Welcome {
        session_id: world.session_id().to_string(),
        session_token: world.session_token().to_string(),
        tick: world.tick,
        protocol_version: PROTOCOL_VERSION,
        resumed,
    }
}

fn reject_with(code: &'static str, message: impl Into<String>) -> (ServerMessage, &'static str) {
    (Rejection::new(code, message).into_message(), code)
}

fn from_rejection(rejection: Rejection) -> (ServerMessage, &'static str) {
    let code = rejection.code;
    (rejection.into_message(), code)
}

fn accepted(message: ServerMessage) -> (ServerMessage, &'static str) {
    (message, AUDIT_ACCEPTED)
}

fn reject_road_error(error: RoadError) -> (ServerMessage, &'static str) {
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

    use crate::protocol::{codes as protocol_codes, decode_client_command};

    const SEED: u64 = LOOPBACK_SEED;

    fn hello() -> ClientCommand {
        ClientCommand::Hello {
            protocol_version: PROTOCOL_VERSION,
            content_hash: "0123456789abcdef".repeat(4),
            player_name: "mayor".to_string(),
        }
    }

    fn world_with_hello() -> ServerWorld {
        let mut world = ServerWorld::with_role(SEED, ClientRole::Admin);
        assert!(matches!(
            world.apply(&hello()),
            ServerMessage::Welcome { .. }
        ));
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
            } => (*tick, *population, *balance, checksum.clone()),
            other => panic!("expected snapshot, got {other:?}"),
        }
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
}
