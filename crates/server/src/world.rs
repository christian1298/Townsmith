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

use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::thread;

use core_city::economy::{Economy, EconomyTick, DAY_SECONDS, STARTING_BALANCE};
use core_city::road::{RoadError, RoadRequest, RoadStore};

use crate::protocol::{
    codes as protocol_codes, configure_stream, decode_client_command, frame_to_io, read_frame,
    read_json_frame, session_id_for_seed, validate_hello, validate_road, validate_tax_rates,
    write_json_frame, ClientCommand, Rejection, ServerMessage, SessionConfig, TimeAction,
    LOOPBACK_SEED, PROTOCOL_VERSION, SESSION_IO_TIMEOUT,
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

/// Stable executor error codes. These are wire/API contracts; do not rename.
pub mod codes {
    /// A non-hello command arrived before an accepted `hello`.
    pub const NOT_HELLO: &str = "not_hello";
    /// A time action the executor does not support (speed controls).
    pub const TIME_INVALID_ACTION: &str = "time.invalid_action";
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
    /// Checksum of the last accepted state, refreshed after each mutation.
    pub session_checksum: String,
    hello: bool,
    closed: bool,
    session_id: String,
}

impl ServerWorld {
    /// Create a session world from a seed.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self::with_population(seed, SessionConfig::for_seed(seed).population)
    }

    /// Create a session world with an explicit population.
    ///
    /// `employed` is 60% of the population and `buildings` is one per 25
    /// citizens (at least one), both derived deterministically. The treasury
    /// starts at [`STARTING_BALANCE`].
    #[must_use]
    pub fn with_population(seed: u64, population: u32) -> Self {
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
            session_checksum: String::new(),
            hello: false,
            closed: false,
            session_id: session_id_for_seed(seed),
        };
        world.refresh_checksum();
        world
    }

    /// Rebuild a world from an already validated [`crate::save::ServerSave`].
    ///
    /// The restored session is open and has not accepted a `hello`, so a
    /// reconnecting client must handshake again. Saved tick, paused flag,
    /// treasury, and roads resume unchanged; the checksum is recomputed from
    /// the restored state.
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
            session_checksum: String::new(),
            hello: false,
            closed: false,
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
    /// Rejections never mutate state. `hello` and `disconnect` are accepted
    /// before `hello`; every other command is rejected with
    /// [`codes::NOT_HELLO`].
    pub fn apply(&mut self, command: &ClientCommand) -> ServerMessage {
        if self.closed {
            return reject(protocol_codes::SESSION_CLOSED, "session is already closed");
        }

        match command {
            ClientCommand::Hello {
                protocol_version,
                content_hash,
                player_name,
            } => {
                if self.hello {
                    return reject(
                        protocol_codes::HELLO_DUPLICATE,
                        "hello was already accepted",
                    );
                }
                if let Err(rejection) = validate_hello(*protocol_version, content_hash, player_name)
                {
                    return rejection.into_message();
                }
                self.hello = true;
                self.refresh_checksum();
                ServerMessage::Welcome {
                    session_id: self.session_id.clone(),
                    tick: self.tick,
                    protocol_version: PROTOCOL_VERSION,
                }
            }
            ClientCommand::Disconnect => {
                self.closed = true;
                ServerMessage::Goodbye
            }
            _ if !self.hello => reject(
                codes::NOT_HELLO,
                format!("`{}` requires a successful hello first", command.tag()),
            ),
            ClientCommand::Ping { nonce } => ServerMessage::Pong { nonce: *nonce },
            ClientCommand::TimeControl { action } => self.apply_time_control(*action),
            ClientCommand::SetTax {
                resident_rate,
                business_rate,
            } => {
                if let Err(rejection) = validate_tax_rates(*resident_rate, *business_rate) {
                    return rejection.into_message();
                }
                self.economy
                    .set_taxes(f64::from(*resident_rate), f64::from(*business_rate));
                self.tick = self.tick.saturating_add(1);
                self.refresh_checksum();
                self.snapshot()
            }
            ClientCommand::PlaceRoad { start, end } => self.apply_place_road(*start, *end),
        }
    }

    fn apply_time_control(&mut self, action: TimeAction) -> ServerMessage {
        match action {
            TimeAction::Pause => self.paused = true,
            TimeAction::Resume => self.paused = false,
            TimeAction::Step => {
                self.advance_unpaused(DAY_SECONDS);
            }
            TimeAction::Speed1x | TimeAction::Speed2x | TimeAction::Speed4x => {
                return reject(
                    codes::TIME_INVALID_ACTION,
                    "speed controls are not supported by the v0 authority executor",
                );
            }
        }
        self.refresh_checksum();
        self.snapshot()
    }

    fn apply_place_road(&mut self, start: [i32; 2], end: [i32; 2]) -> ServerMessage {
        if let Err(rejection) = validate_road(start, end) {
            return rejection.into_message();
        }

        let dx = i64::from(end[0]) - i64::from(start[0]);
        let dz = i64::from(end[1]) - i64::from(start[1]);
        if dx != 0 && dz != 0 {
            return reject(
                codes::ROAD_NOT_AXIS_ALIGNED,
                "road must be horizontal or vertical; diagonals are not supported",
            );
        }
        let length = dx.abs() + dz.abs();
        if length > i64::from(MAX_WORLD_ROAD_LENGTH) {
            return reject(
                protocol_codes::ROAD_TOO_LONG,
                format!(
                    "road length {length} exceeds the v0 maximum of {MAX_WORLD_ROAD_LENGTH} cells"
                ),
            );
        }
        if let Some(code) = self.road_conflict(start, end) {
            return reject(code, "road duplicates or overlaps an existing segment");
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
                self.snapshot()
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

fn reject(code: &'static str, message: impl Into<String>) -> ServerMessage {
    Rejection::new(code, message).into_message()
}

fn reject_road_error(error: RoadError) -> ServerMessage {
    let code = match error {
        RoadError::TooMany => codes::ROAD_STORE_FULL,
        RoadError::IdExhausted => codes::ROAD_ID_EXHAUSTED,
        RoadError::ZeroLength => protocol_codes::ROAD_EMPTY,
        RoadError::TooLong { .. } => protocol_codes::ROAD_TOO_LONG,
        RoadError::CoordinateOutOfRange => protocol_codes::ROAD_OUT_OF_BOUNDS,
        _ => codes::ROAD_REJECTED,
    };
    reject(code, error.to_string())
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

fn serve_authority(listener: TcpListener) -> io::Result<AuthorityReport> {
    let (mut stream, _peer) = listener.accept()?;
    configure_stream(&stream)?;

    let mut world = ServerWorld::new(LOOPBACK_SEED);
    let mut commands = 0u64;
    let mut frames = 0u64;

    loop {
        let payload = read_frame(&mut stream).map_err(frame_to_io)?;
        frames += 1;
        commands += 1;

        let response = match decode_client_command(&payload) {
            Ok(command) => world.apply(&command),
            Err(rejection) => rejection.into_message(),
        };
        write_json_frame(&mut stream, &response).map_err(frame_to_io)?;
        frames += 1;

        if world.is_closed() {
            break;
        }
    }

    Ok(AuthorityReport {
        commands,
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
    let mut reference = ServerWorld::new(LOOPBACK_SEED);
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

fn exchange(
    stream: &mut TcpStream,
    reference: &mut ServerWorld,
    command: &ClientCommand,
) -> io::Result<ServerMessage> {
    let expected = reference.apply(command);
    write_json_frame(stream, command).map_err(frame_to_io)?;
    let actual = read_json_frame::<_, ServerMessage>(stream).map_err(frame_to_io)?;
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
        let mut world = ServerWorld::new(SEED);
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
            } => {
                assert_eq!(session_id, session_id_for_seed(SEED));
                assert_eq!(tick, 0);
                assert_eq!(protocol_version, PROTOCOL_VERSION);
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
            let mut world = ServerWorld::new(SEED);
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
