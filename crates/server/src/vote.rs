//! Deterministic multiplayer time-control vote (D-021).
//!
//! [`TimeVote`] is a pure state machine: it owns no clock, no sockets, and no
//! [`crate::world::ServerWorld`] reference. Every transition takes the
//! authoritative server time (seconds, one tick per second) and, where it
//! matters, the current online roster. The same inputs always produce the same
//! state and the same [`TimeVote::checksum`], so tests and replays are exact.
//!
//! # Rules implemented
//!
//! - A vote lasts [`VOTE_WINDOW_TICKS`] seconds and targets one [`TimeAction`].
//!   v0 time votes support **pause** and **resume**; speed and step votes are
//!   rejected with [`codes::ACTION_UNSUPPORTED`] until the executor supports
//!   speed states.
//! - The denominator is the online roster **snapshotted at vote start**
//!   ([`TimeVote::online_at_start`]). A simple majority passes:
//!   `yes > online_at_start / 2`, i.e.
//!   `yes >= online_at_start / 2 + 1`. Abstentions and non-votes count as no.
//! - At least [`MIN_ONLINE_PLAYERS`] players must be online to start, and the
//!   vote **cancels** as soon as fewer than two players remain online.
//! - The initiator counts as a **yes** at call time (documented behaviour, not
//!   configurable in v0) and cannot vote twice.
//! - Votes resolve early when the outcome is already decided:
//!   reaching the majority passes, and an unreachable majority fails. Otherwise
//!   the window expires and the vote fails unless the majority was reached.
//! - One vote at a time. A call is rejected while a vote is open
//!   ([`codes::OPEN`]) or while the cooldown is active ([`codes::COOLDOWN`]).
//! - Cooldowns are measured in server ticks from the resolution:
//!   [`FAILED_COOLDOWN_TICKS`] after a failed or cancelled vote and
//!   [`SUCCESS_COOLDOWN_TICKS`] after a passed vote.
//!
//! # Effects
//!
//! A resolution is recorded in [`TimeVote::take_outcome`]. The caller (in v0,
//! [`crate::world::ServerWorld`]) drains it exactly once and applies a passed
//! pause/resume. `effect_applied` plus the world's last-applied generation make
//! the effect idempotent, so re-delivering an outcome cannot toggle time twice.
//!
//! # Limits (v0)
//!
//! - One vote object per world; no vote history beyond counters.
//! - The server clock is explicit (`ServerWorld::advance_clock`); there is no
//!   wall-clock loop yet.
//! - Vote state is not persisted; a restart cancels any open vote and cooldown.
//! - No broadcast fan-out: the responding session gets the status message.
//!
//! The loopback harness at the bottom of this module exercises the state
//! machine through the real protocol over TCP with two sessions sharing one
//! world.

use std::collections::BTreeSet;
use std::fmt;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::protocol::{
    configure_stream, decode_client_command, frame_to_io, read_frame, write_json_frame,
    ClientCommand, Rejection, ServerMessage, TimeAction, LOOPBACK_SEED, SESSION_IO_TIMEOUT,
};

/// Vote window in authoritative seconds (ticks). D-021 default: 60 s.
pub const VOTE_WINDOW_TICKS: u64 = 60;

/// Cooldown after a failed or cancelled vote, in ticks. D-021: 5 minutes.
pub const FAILED_COOLDOWN_TICKS: u64 = 300;

/// Cooldown after a passed vote, in ticks. D-021: 2 minutes.
pub const SUCCESS_COOLDOWN_TICKS: u64 = 120;

/// Minimum players online to start a vote; below this, votes are disabled.
pub const MIN_ONLINE_PLAYERS: usize = 2;

/// Stable vote error codes. These are wire/API contracts; do not rename.
pub mod codes {
    /// A vote is already open.
    pub const OPEN: &str = "vote.open";
    /// The cooldown from the previous vote has not expired.
    pub const COOLDOWN: &str = "vote.cooldown";
    /// Fewer than [`super::MIN_ONLINE_PLAYERS`] players are online.
    pub const MIN_ONLINE: &str = "vote.min_online";
    /// The player is not in the roster snapshot for this vote.
    pub const UNKNOWN_PLAYER: &str = "vote.unknown_player";
    /// The player already voted yes or no.
    pub const DUPLICATE: &str = "vote.duplicate";
    /// No vote is open (never started, or already resolved/expired).
    pub const CLOSED: &str = "vote.closed";
    /// The transport-only session cannot run time votes.
    pub const UNAVAILABLE: &str = "vote.unavailable";
    /// v0 does not support this vote target (step, speed).
    pub const ACTION_UNSUPPORTED: &str = "vote.action_unsupported";
    /// `call` requires a `time` target action.
    pub const ACTION_REQUIRED: &str = "vote.action_required";
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

/// Stable lowercase name of a time action, matching the wire names.
#[must_use]
pub fn action_name(action: TimeAction) -> &'static str {
    match action {
        TimeAction::Pause => "pause",
        TimeAction::Resume => "resume",
        TimeAction::Step => "step",
        TimeAction::Speed1x => "speed_1x",
        TimeAction::Speed2x => "speed_2x",
        TimeAction::Speed4x => "speed_4x",
    }
}

/// A rejected vote transition. Convert it to a protocol [`Rejection`] at the
/// boundary; the code is stable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoteError {
    /// Stable [`codes`] value.
    pub code: &'static str,
    /// Human-readable reason for logs and UI feedback.
    pub message: String,
}

impl VoteError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// Convert to a protocol rejection without changing the code.
    #[must_use]
    pub fn into_rejection(self) -> Rejection {
        Rejection::new(self.code, self.message)
    }
}

impl fmt::Display for VoteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for VoteError {}

/// Lifecycle state of a time vote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VoteState {
    /// No vote has run yet.
    #[default]
    Idle,
    /// Accepting votes until `ends_tick`.
    Open,
    /// The majority passed; `action` takes effect.
    Passed,
    /// The window expired, the majority became unreachable, or the tally failed.
    Failed,
    /// An admin cancelled it or too few players remained online.
    Cancelled,
}

impl VoteState {
    /// Stable lowercase name used on the wire, in logs, and in checksums.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Open => "open",
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Whether the vote has reached a terminal state.
    #[must_use]
    pub fn resolved(self) -> bool {
        matches!(self, Self::Passed | Self::Failed | Self::Cancelled)
    }
}

/// One resolved vote, queued for the caller that applies its effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoteOutcome {
    /// Vote generation; monotonically increases on each accepted call.
    pub generation: u64,
    /// Terminal state (passed, failed, or cancelled).
    pub result: VoteState,
    /// Target time action requested by the call.
    pub action: TimeAction,
    /// Yes votes counted at resolution.
    pub yes: u32,
    /// No votes counted at resolution.
    pub no: u32,
    /// Players online at resolution (or at the snapshot for expiry paths).
    pub online: u32,
    /// Server tick (seconds) at resolution.
    pub tick: u64,
}

/// Deterministic multiplayer time-control vote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeVote {
    /// Current lifecycle state.
    pub state: VoteState,
    /// Vote generation; `0` before the first call.
    pub generation: u64,
    /// Target action of the current/last vote.
    pub action: Option<TimeAction>,
    /// Player who started the current/last vote.
    pub initiator: String,
    /// Yes voters, including the initiator.
    pub yes: BTreeSet<String>,
    /// No voters.
    pub no: BTreeSet<String>,
    /// Online roster snapshot at vote start; the vote denominator.
    pub online_at_start: BTreeSet<String>,
    /// Tick the vote opened.
    pub started_tick: u64,
    /// Tick the window closes (`started_tick + VOTE_WINDOW_TICKS`).
    pub ends_tick: u64,
    /// First tick at which the next call is allowed.
    pub cooldown_until: u64,
    /// Tick the vote reached a terminal state.
    pub resolved_tick: Option<u64>,
    /// Whether the world applied the passed effect. Guarded by the world too.
    pub effect_applied: bool,
    /// Accepted vote casts across the session (yes and no).
    pub votes_cast: u64,
    /// Resolutions that passed.
    pub passed_count: u64,
    /// Resolutions that failed.
    pub failed_count: u64,
    /// Resolutions that were cancelled.
    pub cancelled_count: u64,
    pending: Option<VoteOutcome>,
}

impl Default for TimeVote {
    fn default() -> Self {
        Self {
            state: VoteState::Idle,
            generation: 0,
            action: None,
            initiator: String::new(),
            yes: BTreeSet::new(),
            no: BTreeSet::new(),
            online_at_start: BTreeSet::new(),
            started_tick: 0,
            ends_tick: 0,
            cooldown_until: 0,
            resolved_tick: None,
            effect_applied: false,
            votes_cast: 0,
            passed_count: 0,
            failed_count: 0,
            cancelled_count: 0,
            pending: None,
        }
    }
}

impl TimeVote {
    /// Empty vote state (identical to [`Default`]).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Current lifecycle state.
    #[must_use]
    pub fn state(&self) -> VoteState {
        self.state
    }

    /// Vote generation (`0` before the first call).
    #[must_use]
    pub fn vote_id(&self) -> u64 {
        self.generation
    }

    /// Target action of the current/last vote.
    #[must_use]
    pub fn action(&self) -> Option<TimeAction> {
        self.action
    }

    /// Player who started the current/last vote.
    #[must_use]
    pub fn initiator(&self) -> &str {
        &self.initiator
    }

    /// Yes votes currently counted.
    #[must_use]
    pub fn yes_count(&self) -> u32 {
        u32::try_from(self.yes.len()).unwrap_or(u32::MAX)
    }

    /// No votes currently counted.
    #[must_use]
    pub fn no_count(&self) -> u32 {
        u32::try_from(self.no.len()).unwrap_or(u32::MAX)
    }

    /// Online roster size at vote start (the denominator).
    #[must_use]
    pub fn online_count(&self) -> u32 {
        u32::try_from(self.online_at_start.len()).unwrap_or(u32::MAX)
    }

    /// Yes votes needed to pass with the current snapshot.
    #[must_use]
    pub fn threshold(&self) -> usize {
        self.online_at_start.len() / 2 + 1
    }

    /// Whether the current tally has reached a simple majority.
    #[must_use]
    pub fn has_majority(&self) -> bool {
        self.yes.len() >= self.threshold()
    }

    /// Vote casts accepted across the session.
    #[must_use]
    pub fn votes_cast(&self) -> u64 {
        self.votes_cast
    }

    /// Drain the pending outcome. Returns it exactly once per resolution.
    pub fn take_outcome(&mut self) -> Option<VoteOutcome> {
        self.pending.take()
    }

    /// Peek at the pending outcome without consuming it.
    #[must_use]
    pub fn outcome(&self) -> Option<&VoteOutcome> {
        self.pending.as_ref()
    }

    /// Start a vote.
    ///
    /// `online_players` is the authoritative roster; it is snapshotted and used
    /// as the denominator for the whole vote. The initiator is counted as a yes.
    ///
    /// Rejections, in order: unsupported action, open vote, cooldown, fewer
    /// than [`MIN_ONLINE_PLAYERS`] online, initiator not online.
    pub fn call<S: AsRef<str>>(
        &mut self,
        initiator: &str,
        action: TimeAction,
        online_players: impl IntoIterator<Item = S>,
        now_tick: u64,
    ) -> Result<(), VoteError> {
        if !matches!(action, TimeAction::Pause | TimeAction::Resume) {
            return Err(VoteError::new(
                codes::ACTION_UNSUPPORTED,
                format!(
                    "v0 time votes support pause and resume only; `{}` is not available",
                    action_name(action)
                ),
            ));
        }
        if self.state == VoteState::Open {
            return Err(VoteError::new(
                codes::OPEN,
                format!(
                    "vote #{} is already open until tick {}",
                    self.generation, self.ends_tick
                ),
            ));
        }
        if now_tick < self.cooldown_until {
            return Err(VoteError::new(
                codes::COOLDOWN,
                format!(
                    "time votes are on cooldown for {} more ticks",
                    self.cooldown_until - now_tick
                ),
            ));
        }

        let online: BTreeSet<String> = online_players
            .into_iter()
            .map(|player| player.as_ref().to_string())
            .collect();
        if online.len() < MIN_ONLINE_PLAYERS {
            return Err(VoteError::new(
                codes::MIN_ONLINE,
                format!(
                    "time votes need at least {MIN_ONLINE_PLAYERS} players online; {} online",
                    online.len()
                ),
            ));
        }
        if initiator.trim().is_empty() || !online.contains(initiator) {
            return Err(VoteError::new(
                codes::UNKNOWN_PLAYER,
                format!("`{initiator}` is not in the online roster"),
            ));
        }

        self.generation = self.generation.saturating_add(1);
        self.state = VoteState::Open;
        self.action = Some(action);
        self.initiator = initiator.to_string();
        self.online_at_start = online;
        self.yes = BTreeSet::from([initiator.to_string()]);
        self.no.clear();
        self.started_tick = now_tick;
        self.ends_tick = now_tick.saturating_add(VOTE_WINDOW_TICKS);
        self.cooldown_until = 0;
        self.resolved_tick = None;
        self.effect_applied = false;
        self.pending = None;
        Ok(())
    }

    /// Cast one vote (yes when `approve`, otherwise no).
    ///
    /// Rejections, in order: no open vote or the window is over, unknown player,
    /// duplicate vote. A late vote resolves the expired window first and is then
    /// rejected with [`codes::CLOSED`].
    pub fn vote(&mut self, player: &str, approve: bool, now_tick: u64) -> Result<(), VoteError> {
        if self.state != VoteState::Open {
            return Err(VoteError::new(
                codes::CLOSED,
                format!("no time vote is open (state is {})", self.state.as_str()),
            ));
        }
        if now_tick >= self.ends_tick {
            // Resolve the expired window with the snapshot so a late vote never
            // silently extends it.
            let snapshot = self.online_at_start.clone();
            let result = if self.has_majority() {
                VoteState::Passed
            } else {
                VoteState::Failed
            };
            self.resolve(result, now_tick, &snapshot);
            return Err(VoteError::new(
                codes::CLOSED,
                format!(
                    "vote #{} closed at tick {}",
                    self.generation, self.ends_tick
                ),
            ));
        }
        if !self.online_at_start.contains(player) {
            return Err(VoteError::new(
                codes::UNKNOWN_PLAYER,
                format!("`{player}` is not in this vote's online roster"),
            ));
        }
        if self.yes.contains(player) || self.no.contains(player) {
            return Err(VoteError::new(
                codes::DUPLICATE,
                format!("`{player}` already voted on vote #{}", self.generation),
            ));
        }

        if approve {
            self.yes.insert(player.to_string());
        } else {
            self.no.insert(player.to_string());
        }
        self.votes_cast = self.votes_cast.saturating_add(1);

        // Early resolution: a majority is final, and an unreachable majority is
        // final. Both are monotone, so resolving now matches resolving at expiry.
        if self.has_majority() {
            let snapshot = self.online_at_start.clone();
            self.resolve(VoteState::Passed, now_tick, &snapshot);
        } else if self.yes.len() + self.undecided() < self.threshold() {
            let snapshot = self.online_at_start.clone();
            self.resolve(VoteState::Failed, now_tick, &snapshot);
        }
        Ok(())
    }

    /// Advance the vote clock and prune players who left.
    ///
    /// Cancels below [`MIN_ONLINE_PLAYERS`], resolves the window at `ends_tick`,
    /// and resolves early when the outcome is already decided. Returns the new
    /// outcome (also queued for [`Self::take_outcome`]) when it resolves.
    pub fn tick<S: AsRef<str>>(
        &mut self,
        now_tick: u64,
        online_players: impl IntoIterator<Item = S>,
    ) -> Option<VoteOutcome> {
        if self.state != VoteState::Open {
            return None;
        }

        let online: BTreeSet<String> = online_players
            .into_iter()
            .map(|player| player.as_ref().to_string())
            .collect();
        self.yes.retain(|player| online.contains(player));
        self.no.retain(|player| online.contains(player));

        let result = if online.len() < MIN_ONLINE_PLAYERS {
            Some(VoteState::Cancelled)
        } else if now_tick >= self.ends_tick {
            Some(if self.has_majority() {
                VoteState::Passed
            } else {
                VoteState::Failed
            })
        } else if self.has_majority() {
            Some(VoteState::Passed)
        } else if self.yes.len() + self.undecided() < self.threshold() {
            Some(VoteState::Failed)
        } else {
            None
        };

        result.map(|result| self.resolve(result, now_tick, &online))
    }

    /// Cancel an open vote (admin path, or a presence loss handled by the
    /// caller). A cancelled vote starts the failed-vote cooldown; the tuning is
    /// tracked in Q-078.
    pub fn cancel(&mut self, now_tick: u64) -> Result<(), VoteError> {
        if self.state != VoteState::Open {
            return Err(VoteError::new(
                codes::CLOSED,
                format!("no time vote is open (state is {})", self.state.as_str()),
            ));
        }
        let snapshot = self.online_at_start.clone();
        self.resolve(VoteState::Cancelled, now_tick, &snapshot);
        Ok(())
    }

    /// Mark the passed effect as applied. The world also tracks the generation;
    /// this flag keeps the state machine self-describing.
    pub fn mark_effect_applied(&mut self) {
        self.effect_applied = true;
    }

    /// Deterministic FNV-1a checksum over the vote state.
    ///
    /// The pending outcome is transient and excluded; every durable field is
    /// hashed in a fixed order, so identical transcripts compare equal.
    #[must_use]
    pub fn checksum(&self) -> String {
        let mut hash = fnv_update(FNV_OFFSET, &self.generation.to_le_bytes());
        hash = hash_str(hash, self.state.as_str());
        hash = hash_str(
            hash,
            self.action.map_or("none", |action| action_name(action)),
        );
        hash = hash_str(hash, &self.initiator);
        hash = fnv_update(hash, &self.started_tick.to_le_bytes());
        hash = fnv_update(hash, &self.ends_tick.to_le_bytes());
        hash = fnv_update(hash, &self.cooldown_until.to_le_bytes());
        match self.resolved_tick {
            Some(tick) => {
                hash = fnv_update(hash, &[1]);
                hash = fnv_update(hash, &tick.to_le_bytes());
            }
            None => hash = fnv_update(hash, &[0]),
        }
        hash = fnv_update(hash, &[u8::from(self.effect_applied)]);
        hash = fnv_update(hash, &self.votes_cast.to_le_bytes());
        hash = fnv_update(hash, &self.passed_count.to_le_bytes());
        hash = fnv_update(hash, &self.failed_count.to_le_bytes());
        hash = fnv_update(hash, &self.cancelled_count.to_le_bytes());
        for player in &self.yes {
            hash = hash_str(hash, player);
        }
        hash = fnv_update(hash, &[0x1e]);
        for player in &self.no {
            hash = hash_str(hash, player);
        }
        hash = fnv_update(hash, &[0x1e]);
        for player in &self.online_at_start {
            hash = hash_str(hash, player);
        }
        format!("{hash:016x}")
    }

    fn undecided(&self) -> usize {
        self.online_at_start
            .iter()
            .filter(|player| !self.yes.contains(*player) && !self.no.contains(*player))
            .count()
    }

    fn resolve(
        &mut self,
        result: VoteState,
        now_tick: u64,
        online: &BTreeSet<String>,
    ) -> VoteOutcome {
        self.state = result;
        self.resolved_tick = Some(now_tick);
        self.cooldown_until = now_tick.saturating_add(match result {
            VoteState::Passed => SUCCESS_COOLDOWN_TICKS,
            _ => FAILED_COOLDOWN_TICKS,
        });
        match result {
            VoteState::Passed => self.passed_count = self.passed_count.saturating_add(1),
            VoteState::Failed => self.failed_count = self.failed_count.saturating_add(1),
            VoteState::Cancelled => {
                self.cancelled_count = self.cancelled_count.saturating_add(1);
            }
            VoteState::Idle | VoteState::Open => {}
        }

        let outcome = VoteOutcome {
            generation: self.generation,
            result,
            action: self.action.unwrap_or(TimeAction::Pause),
            yes: self.yes_count(),
            no: self.no_count(),
            online: u32::try_from(online.len()).unwrap_or(u32::MAX),
            tick: now_tick,
        };
        self.pending = Some(outcome.clone());
        outcome
    }
}

/// Result of one deterministic time-vote loopback run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeVoteReport {
    /// Sessions served across every phase.
    pub sessions: u64,
    /// Accepted yes/no casts across every phase.
    pub votes: u64,
    /// Votes that passed.
    pub passed: u64,
    /// Votes that failed.
    pub failed: u64,
    /// Votes that were cancelled.
    pub cancelled: u64,
    /// Whether the world was left paused by the passing vote.
    pub paused: bool,
    /// FNV-1a over every phase's vote checksum and paused flag.
    pub checksum: String,
}

/// Run the deterministic time-vote loopback over TCP.
///
/// Three independent worlds (each with its own listener accept batch) exercise
/// the D-021 rules through the real protocol:
///
/// 1. **pass**: two players, call + yes; a viewer's `cancel` is forbidden, the
///    vote passes, the world pauses, and the follow-up call is on cooldown;
/// 2. **fail**: three players, call + two no votes; the majority is
///    unreachable, the vote fails, and time stays running;
/// 3. **cancel**: two players, call; one disconnects, the next command observes
///    fewer than two online and cancels the vote.
///
/// No wall-clock sleeps: every ordering point is a request/response pair.
#[allow(dead_code)]
pub fn run_time_vote_loopback() -> io::Result<TimeVoteReport> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let address = listener.local_addr()?;

    let server = thread::spawn(move || serve_time_vote(listener));
    let client_result = run_time_vote_client(address);
    let server_result = server
        .join()
        .map_err(|_| io::Error::other("time-vote loopback server thread panicked"))?;

    client_result?;
    server_result
}

fn serve_time_vote(listener: TcpListener) -> io::Result<TimeVoteReport> {
    // (connections, expected pause state after the phase).
    let phases = [(2usize, true), (3, false), (2, false)];
    let mut report = TimeVoteReport {
        sessions: 0,
        votes: 0,
        passed: 0,
        failed: 0,
        cancelled: 0,
        paused: false,
        checksum: String::new(),
    };
    let mut hash = FNV_OFFSET;

    for (connections, expected_pause) in phases {
        let world = Arc::new(Mutex::new(crate::world::ServerWorld::with_role(
            LOOPBACK_SEED,
            crate::world::ClientRole::Viewer,
        )));
        let mut handles = Vec::with_capacity(connections);

        for _ in 0..connections {
            let (stream, _peer) = listener.accept()?;
            let shared = Arc::clone(&world);
            handles.push(thread::spawn(move || serve_vote_stream(stream, shared)));
        }
        for handle in handles {
            handle
                .join()
                .map_err(|_| io::Error::other("time-vote session thread panicked"))??;
        }

        let world = world
            .lock()
            .map_err(|_| io::Error::other("time-vote world mutex poisoned"))?;
        if world.paused != expected_pause {
            return Err(io::Error::other(format!(
                "time-vote phase expected paused={expected_pause}, got {}",
                world.paused
            )));
        }
        report.sessions += connections as u64;
        report.votes += world.vote.votes_cast();
        report.passed += world.vote.passed_count;
        report.failed += world.vote.failed_count;
        report.cancelled += world.vote.cancelled_count;
        report.paused = world.paused;
        hash = fnv_update(hash, world.vote.checksum().as_bytes());
        hash = fnv_update(hash, &[u8::from(world.paused)]);
        hash = fnv_update(hash, world.checksum().as_bytes());
    }

    report.checksum = format!("{hash:016x}");
    Ok(report)
}

fn serve_vote_stream(
    stream: TcpStream,
    world: Arc<Mutex<crate::world::ServerWorld>>,
) -> io::Result<()> {
    configure_stream(&stream)?;
    let mut stream = stream;

    loop {
        let payload = match read_frame(&mut stream) {
            Ok(payload) => payload,
            Err(crate::protocol::FrameError::Truncated { .. })
            | Err(crate::protocol::FrameError::Io(_)) => break,
            Err(error) => return Err(io::Error::other(error.to_string())),
        };

        let response = match decode_client_command(&payload) {
            Ok(command) => world
                .lock()
                .map_err(|_| io::Error::other("time-vote world mutex poisoned"))?
                .apply(&command),
            Err(rejection) => rejection.into_message(),
        };
        write_json_frame(&mut stream, &response).map_err(frame_to_io)?;

        let closed = world
            .lock()
            .map_err(|_| io::Error::other("time-vote world mutex poisoned"))?
            .is_closed();
        if closed {
            break;
        }
    }

    Ok(())
}

struct VoteClient {
    stream: TcpStream,
}

impl VoteClient {
    fn connect(address: SocketAddr, player: &str) -> io::Result<Self> {
        let stream = TcpStream::connect_timeout(&address, SESSION_IO_TIMEOUT)?;
        configure_stream(&stream)?;
        let mut client = Self { stream };
        match client.send(&ClientCommand::Hello {
            protocol_version: crate::protocol::PROTOCOL_VERSION,
            content_hash: "0123456789abcdef".repeat(4),
            player_name: player.to_string(),
        })? {
            ServerMessage::Welcome { .. } => Ok(client),
            other => Err(io::Error::other(format!(
                "time-vote hello expected welcome, got {other:?}"
            ))),
        }
    }

    fn send(&mut self, command: &ClientCommand) -> io::Result<ServerMessage> {
        write_json_frame(&mut self.stream, command).map_err(frame_to_io)?;
        crate::protocol::read_json_frame(&mut self.stream).map_err(frame_to_io)
    }

    fn vote(&mut self, action: crate::protocol::VoteAction) -> io::Result<ServerMessage> {
        self.send(&ClientCommand::TimeVote { action, time: None })
    }

    fn call(&mut self, time: TimeAction) -> io::Result<ServerMessage> {
        self.send(&ClientCommand::TimeVote {
            action: crate::protocol::VoteAction::Call,
            time: Some(time),
        })
    }

    fn goodbye(self) -> io::Result<ServerMessage> {
        let mut client = self;
        client.send(&ClientCommand::Disconnect)
    }
}

fn run_time_vote_client(address: SocketAddr) -> io::Result<()> {
    phase_pass(address)?;
    phase_fail(address)?;
    phase_cancel(address)?;
    Ok(())
}

fn phase_pass(address: SocketAddr) -> io::Result<()> {
    let mut alice = VoteClient::connect(address, "alice")?;
    let mut bob = VoteClient::connect(address, "bob")?;

    expect_vote_state(&alice.call(TimeAction::Pause)?, "open", 1, 0, false)?;
    // A viewer cannot cancel; cancel is admin-only.
    expect_reject(
        &alice.vote(crate::protocol::VoteAction::Cancel)?,
        crate::world::codes::FORBIDDEN,
    )?;
    // Bob's yes is the second of two, so the vote passes immediately.
    expect_vote_state(
        &bob.vote(crate::protocol::VoteAction::Yes)?,
        "passed",
        2,
        0,
        true,
    )?;
    // Success cooldown blocks an immediate follow-up call.
    expect_reject(&alice.call(TimeAction::Pause)?, codes::COOLDOWN)?;
    // Late votes are closed.
    expect_reject(&bob.vote(crate::protocol::VoteAction::Yes)?, codes::CLOSED)?;

    alice.goodbye()?;
    bob.goodbye()?;
    Ok(())
}

fn phase_fail(address: SocketAddr) -> io::Result<()> {
    let mut alice = VoteClient::connect(address, "alice")?;
    let mut bob = VoteClient::connect(address, "bob")?;
    let mut carol = VoteClient::connect(address, "carol")?;

    expect_vote_state(&alice.call(TimeAction::Pause)?, "open", 1, 0, false)?;
    expect_vote_state(
        &bob.vote(crate::protocol::VoteAction::No)?,
        "open",
        1,
        1,
        false,
    )?;
    // Carol's no makes the majority unreachable: 1 yes of 3 online.
    expect_vote_state(
        &carol.vote(crate::protocol::VoteAction::No)?,
        "failed",
        1,
        2,
        false,
    )?;
    expect_reject(&alice.call(TimeAction::Resume)?, codes::COOLDOWN)?;

    alice.goodbye()?;
    bob.goodbye()?;
    carol.goodbye()?;
    Ok(())
}

fn phase_cancel(address: SocketAddr) -> io::Result<()> {
    let mut alice = VoteClient::connect(address, "alice")?;
    let bob = VoteClient::connect(address, "bob")?;

    expect_vote_state(&alice.call(TimeAction::Resume)?, "open", 1, 0, false)?;
    bob.goodbye()?;
    // The ping advances the vote clock: one player online cancels it.
    match alice.send(&ClientCommand::Ping { nonce: 1 })? {
        ServerMessage::Pong { nonce: 1 } => {}
        other => {
            return Err(io::Error::other(format!(
                "time-vote ping expected pong, got {other:?}"
            )))
        }
    }
    expect_reject(
        &alice.vote(crate::protocol::VoteAction::Yes)?,
        codes::CLOSED,
    )?;

    alice.goodbye()?;
    Ok(())
}

fn expect_vote_state(
    message: &ServerMessage,
    state: &str,
    yes: u32,
    no: u32,
    paused: bool,
) -> io::Result<()> {
    match message {
        ServerMessage::TimeVoteStatus {
            state: actual,
            yes: actual_yes,
            no: actual_no,
            paused: actual_paused,
            ..
        } if actual == state
            && *actual_yes == yes
            && *actual_no == no
            && *actual_paused == paused =>
        {
            Ok(())
        }
        other => Err(io::Error::other(format!(
            "time-vote expected state={state} yes={yes} no={no} paused={paused}, got {other:?}"
        ))),
    }
}

fn expect_reject(message: &ServerMessage, code: &str) -> io::Result<()> {
    match message {
        ServerMessage::Reject {
            code: actual_code, ..
        } if actual_code == code => Ok(()),
        other => Err(io::Error::other(format!(
            "time-vote expected rejection `{code}`, got {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn online(players: &[&str]) -> Vec<String> {
        players.iter().map(|player| (*player).to_string()).collect()
    }

    fn opened(players: &[&str], initiator: &str, now: u64) -> TimeVote {
        let mut vote = TimeVote::new();
        vote.call(initiator, TimeAction::Pause, online(players), now)
            .expect("call");
        vote
    }

    fn expect_code(result: Result<(), VoteError>, code: &str) {
        match result {
            Err(error) => assert_eq!(error.code, code),
            Ok(()) => panic!("expected `{code}`, got success"),
        }
    }

    #[test]
    fn call_requires_at_least_two_online_players() {
        let mut vote = TimeVote::new();
        expect_code(
            vote.call("alice", TimeAction::Pause, online(&["alice"]), 0),
            codes::MIN_ONLINE,
        );
        expect_code(
            vote.call("alice", TimeAction::Pause, online(&[]), 0),
            codes::MIN_ONLINE,
        );
        assert_eq!(vote.state(), VoteState::Idle);
        assert_eq!(vote.vote_id(), 0);
        assert!(vote
            .call("alice", TimeAction::Pause, online(&["alice", "bob"]), 0)
            .is_ok());
        assert_eq!(vote.state(), VoteState::Open);
        assert_eq!(vote.vote_id(), 1);
    }

    #[test]
    fn initiator_is_auto_yes_and_cannot_vote_twice() {
        let mut vote = opened(&["alice", "bob"], "alice", 10);
        assert_eq!(vote.yes_count(), 1);
        assert!(vote.yes.contains("alice"));
        assert_eq!(vote.started_tick, 10);
        assert_eq!(vote.ends_tick, 70);
        expect_code(vote.vote("alice", true, 11), codes::DUPLICATE);
        expect_code(vote.vote("alice", false, 11), codes::DUPLICATE);
        assert_eq!(vote.state(), VoteState::Open);
        assert_eq!(vote.votes_cast(), 0);
    }

    #[test]
    fn unknown_players_cannot_vote_or_start() {
        let mut vote = opened(&["alice", "bob"], "alice", 0);
        expect_code(vote.vote("mallory", true, 1), codes::UNKNOWN_PLAYER);
        expect_code(vote.vote("", true, 1), codes::UNKNOWN_PLAYER);
        assert!(vote.vote("bob", false, 1).is_ok());

        let mut other = TimeVote::new();
        expect_code(
            other.call("mallory", TimeAction::Pause, online(&["alice", "bob"]), 0),
            codes::UNKNOWN_PLAYER,
        );
        assert_eq!(other.state(), VoteState::Idle);
    }

    #[test]
    fn a_simple_majority_passes_early() {
        let mut vote = opened(&["alice", "bob", "carol"], "alice", 0);
        assert_eq!(vote.threshold(), 2);
        vote.vote("bob", true, 5).expect("yes");
        assert_eq!(vote.state(), VoteState::Passed);
        assert_eq!(vote.resolved_tick, Some(5));
        assert_eq!(vote.passed_count, 1);
        let outcome = vote.take_outcome().expect("outcome");
        assert_eq!(outcome.result, VoteState::Passed);
        assert_eq!((outcome.yes, outcome.no, outcome.online), (2, 0, 3));
        assert_eq!(vote.cooldown_until, 5 + SUCCESS_COOLDOWN_TICKS);
    }

    #[test]
    fn a_tie_is_not_a_majority_and_fails_at_the_window() {
        let mut vote = opened(&["alice", "bob", "carol", "dave"], "alice", 0);
        vote.vote("bob", true, 1).expect("yes");
        // 2 of 4 is a tie, not `> half`; carol and dave can still reach 3.
        assert_eq!(vote.state(), VoteState::Open);
        assert_eq!(vote.yes_count(), 2);
        assert!(vote
            .tick(59, online(&["alice", "bob", "carol", "dave"]))
            .is_none());
        let outcome = vote
            .tick(60, online(&["alice", "bob", "carol", "dave"]))
            .expect("window close");
        assert_eq!(outcome.result, VoteState::Failed);
        assert_eq!((outcome.yes, outcome.no, outcome.online), (2, 0, 4));
        assert_eq!(vote.state(), VoteState::Failed);
        assert_eq!(vote.cooldown_until, 60 + FAILED_COOLDOWN_TICKS);
    }

    #[test]
    fn abstentions_count_as_no_when_the_window_expires() {
        let mut vote = opened(&["alice", "bob"], "alice", 0);
        assert!(vote.tick(59, online(&["alice", "bob"])).is_none());
        let outcome = vote.tick(60, online(&["alice", "bob"])).expect("expiry");
        assert_eq!(outcome.result, VoteState::Failed);
        assert_eq!(outcome.yes, 1);
        assert_eq!(vote.state(), VoteState::Failed);
    }

    #[test]
    fn voting_after_the_window_is_rejected_and_resolves_it() {
        let mut vote = opened(&["alice", "bob"], "alice", 0);
        expect_code(vote.vote("bob", true, 60), codes::CLOSED);
        assert_eq!(vote.state(), VoteState::Failed);
        assert_eq!(vote.resolved_tick, Some(60));
        assert_eq!(vote.failed_count, 1);
        expect_code(vote.vote("bob", true, 61), codes::CLOSED);
    }

    #[test]
    fn failed_votes_use_the_five_minute_cooldown() {
        let mut vote = opened(&["alice", "bob"], "alice", 0);
        vote.tick(60, online(&["alice", "bob"]));
        expect_code(
            vote.call("alice", TimeAction::Pause, online(&["alice", "bob"]), 359),
            codes::COOLDOWN,
        );
        assert!(vote
            .call("alice", TimeAction::Pause, online(&["alice", "bob"]), 360)
            .is_ok());
        assert_eq!(vote.vote_id(), 2);
        assert_eq!(
            vote.cooldown_until, 0,
            "a fresh call clears the old cooldown"
        );
    }

    #[test]
    fn passed_votes_use_the_two_minute_cooldown() {
        let mut vote = opened(&["alice", "bob"], "alice", 0);
        vote.vote("bob", true, 10).expect("pass");
        expect_code(
            vote.call("alice", TimeAction::Resume, online(&["alice", "bob"]), 129),
            codes::COOLDOWN,
        );
        assert!(vote
            .call("alice", TimeAction::Resume, online(&["alice", "bob"]), 130)
            .is_ok());
    }

    #[test]
    fn only_one_vote_can_be_open() {
        let mut vote = opened(&["alice", "bob", "carol"], "alice", 0);
        expect_code(
            vote.call(
                "bob",
                TimeAction::Resume,
                online(&["alice", "bob", "carol"]),
                1,
            ),
            codes::OPEN,
        );
        assert_eq!(vote.vote_id(), 1);
        assert_eq!(vote.action(), Some(TimeAction::Pause));
    }

    #[test]
    fn dropping_below_two_online_cancels_the_vote() {
        let mut vote = opened(&["alice", "bob", "carol", "dave"], "alice", 0);
        vote.vote("bob", true, 1).expect("yes");
        let outcome = vote
            .tick(2, online(&["alice"]))
            .expect("cancelled below quorum");
        assert_eq!(outcome.result, VoteState::Cancelled);
        assert_eq!(vote.state(), VoteState::Cancelled);
        assert_eq!(vote.cancelled_count, 1);
        assert_eq!(vote.cooldown_until, 2 + FAILED_COOLDOWN_TICKS);
        assert_eq!(vote.yes_count(), 1, "bob left and was pruned");
    }

    #[test]
    fn an_unreachable_majority_fails_early() {
        let mut vote = opened(&["alice", "bob", "carol"], "alice", 0);
        vote.vote("bob", false, 1).expect("no");
        assert_eq!(vote.state(), VoteState::Open);
        vote.vote("carol", false, 2).expect("no");
        // 1 yes, 2 no, 0 undecided of 3: majority (2) is unreachable.
        assert_eq!(vote.state(), VoteState::Failed);
        assert_eq!(vote.resolved_tick, Some(2));
    }

    #[test]
    fn cancel_requires_an_open_vote_and_cools_down() {
        let mut vote = TimeVote::new();
        expect_code(vote.cancel(0), codes::CLOSED);

        let mut vote = opened(&["alice", "bob"], "alice", 5);
        vote.cancel(6).expect("cancel");
        assert_eq!(vote.state(), VoteState::Cancelled);
        assert_eq!(vote.resolved_tick, Some(6));
        assert_eq!(vote.cooldown_until, 6 + FAILED_COOLDOWN_TICKS);
        expect_code(vote.cancel(7), codes::CLOSED);
        expect_code(
            vote.call("alice", TimeAction::Pause, online(&["alice", "bob"]), 7),
            codes::COOLDOWN,
        );
    }

    #[test]
    fn unsupported_vote_targets_are_rejected() {
        let mut vote = TimeVote::new();
        for action in [
            TimeAction::Step,
            TimeAction::Speed1x,
            TimeAction::Speed2x,
            TimeAction::Speed4x,
        ] {
            expect_code(
                vote.call("alice", action, online(&["alice", "bob"]), 0),
                codes::ACTION_UNSUPPORTED,
            );
        }
        assert_eq!(vote.state(), VoteState::Idle);
    }

    #[test]
    fn outcomes_are_emitted_once() {
        let mut vote = opened(&["alice", "bob"], "alice", 0);
        vote.vote("bob", true, 1).expect("pass");
        assert!(vote.outcome().is_some());
        assert_eq!(vote.take_outcome().expect("once").result, VoteState::Passed);
        assert!(vote.outcome().is_none());
        assert!(vote.take_outcome().is_none());
        vote.mark_effect_applied();
        assert!(vote.effect_applied);
    }

    #[test]
    fn identical_transcripts_produce_identical_checksums() {
        fn run() -> TimeVote {
            let mut vote = opened(&["alice", "bob", "carol"], "bob", 7);
            vote.vote("carol", true, 9).expect("pass");
            vote
        }

        let first = run();
        let second = run();
        assert_eq!(first.checksum(), second.checksum());
        assert_eq!(first.checksum().len(), 16);

        let other = opened(&["alice", "bob", "carol"], "alice", 7);
        let mut other = other;
        other.vote("bob", false, 9).expect("no");
        assert_ne!(first.checksum(), other.checksum());
    }

    #[test]
    fn a_rejected_call_never_changes_the_checksum() {
        let mut vote = opened(&["alice", "bob"], "alice", 0);
        let before = vote.checksum();
        expect_code(vote.vote("alice", true, 1), codes::DUPLICATE);
        expect_code(vote.vote("mallory", true, 1), codes::UNKNOWN_PLAYER);
        expect_code(
            vote.call("bob", TimeAction::Pause, online(&["alice", "bob"]), 1),
            codes::OPEN,
        );
        assert_eq!(vote.checksum(), before);
    }

    #[test]
    fn transport_protocol_rejects_votes_with_a_stable_code() {
        let mut session = crate::protocol::Session::new(LOOPBACK_SEED);
        assert!(matches!(
            session
                .handle(ClientCommand::Hello {
                    protocol_version: crate::protocol::PROTOCOL_VERSION,
                    content_hash: "0123456789abcdef".repeat(4),
                    player_name: "alice".to_string(),
                })
                .as_slice(),
            [ServerMessage::Welcome { .. }]
        ));
        match session
            .handle(ClientCommand::TimeVote {
                action: crate::protocol::VoteAction::Call,
                time: Some(TimeAction::Pause),
            })
            .as_slice()
        {
            [ServerMessage::Reject { code, .. }] => assert_eq!(code, codes::UNAVAILABLE),
            other => panic!("expected vote.unavailable, got {other:?}"),
        }
    }
}
