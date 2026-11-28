//! Bounded four-player soak against the production listen loop.
//!
//! The one-hour exit soak (§3.10 scenario 1) needs four humans and reference
//! hardware. This test automates the part a machine can own: four real v1 TCP
//! clients share one authoritative world through
//! [`townsmith_server::listen::run_listen`] on an ephemeral loopback port with a
//! temporary save path and `autosave_ticks: 1`, and run a fixed 40-round
//! command tape. That is a bounded soak, not a timing or bandwidth proof.
//!
//! Each round, every online client sends the same fixed command set:
//!
//! 1. `chat` with a round-tagged line, then `server_status`;
//! 2. `set_interest` with a fixed viewport, then `place_road` on a
//!    non-overlapping per-player row.
//!
//! Order is fixed (`alice`, `bravo`, `charlie`, `dave`), so every tick,
//! checksum, and road count is deterministic. The test pins the accepted road
//! count per turn: a status/interest read never advances the tick, and an
//! accepted road advances it by exactly one.
//!
//! At round 20 `dave` disconnects cleanly; rounds 20–22 run with three
//! clients; at round 23 he reconnects with the `session_token` from his
//! `welcome` and `resume`. The resumed `welcome` must be `resumed: true` with
//! the chat watermark he left at, and `chat_since` must return exactly the nine
//! lines posted while he was offline (three clients × three rounds).
//!
//! The test then asserts:
//!
//! - the final live `server_status` tick/checksum equals the independently
//!   loaded save and the [`ListenReport`];
//! - re-saving the loaded world to a second path is byte-identical;
//! - no `.tmp` file is left behind;
//! - the listen report saw five sessions (four players plus one reconnect);
//! - the command count is the pinned, observed value.
//!
//! Waits are response-driven protocol round trips bounded by
//! [`SESSION_IO_TIMEOUT`]. There are no sleeps and no wall-clock assertions, so
//! runtime is machine-dependent and deliberately not asserted.
//!
//! # Deliberate limits
//!
//! - This is a bounded soak, not the one-hour human proof; endurance, memory
//!   growth over an hour, and the reference-hardware timing matrix remain
//!   external.
//! - Every session runs as the process-wide `default_role` (Mayor), the same
//!   v1 gap documented by `four_player_exit.rs`; role-matrix coverage lives in
//!   its companion tests.
//! - It exercises chat, status, interest views, road writes, reconnect
//!   catch-up, autosave, and save/reload determinism. It does not claim
//!   bandwidth, tick-time, or desync-under-real-latency coverage.

use std::fs;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use townsmith_server::listen::{run_listen, ListenConfig};
use townsmith_server::protocol::{
    read_json_frame, write_json_frame, ChatEntry, ClientCommand, ServerMessage, PROTOCOL_VERSION,
    SESSION_IO_TIMEOUT,
};
use townsmith_server::save::temp_path;
use townsmith_server::world::{ClientRole, ServerWorld};

const CONTENT_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// Rounds executed by the fixed tape.
const ROUNDS: usize = 40;
/// `dave` disconnects at the start of this round.
const DISCONNECT_ROUND: usize = 20;
/// `dave` resumes at the start of this round, so he misses rounds 20–22.
const RESUME_ROUND: usize = 23;

/// Fixed interest viewport: the whole test world is inside this circle.
const INTEREST_CENTER: [f64; 2] = [0.0, 0.0];
const INTEREST_RADIUS_M: f64 = 2_048.0;

/// Pinned deterministic outcome, observed on 2026-11-28 and re-verified across
/// consecutive runs. A change means the command tape or authoritative
/// behaviour moved and must be reviewed, not silently re-pinned.
const EXPECTED_FINAL_TICK: u64 = 157;
const EXPECTED_FINAL_CHECKSUM: &str = "a868a06a9f14416d";
const EXPECTED_FINAL_ROADS: u32 = 157;
const EXPECTED_COMMANDS: u64 = 641;
const EXPECTED_SESSIONS: u64 = 5;

static NEXT: AtomicU64 = AtomicU64::new(1);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "townsmith-four-player-soak-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug)]
struct Status {
    tick: u64,
    checksum: String,
    online: u32,
    roads: u32,
}

struct Client {
    stream: TcpStream,
    name: String,
    session_id: String,
    /// Latest chat sequence this connection has observed.
    chat_seq: u64,
}

impl Client {
    fn connect(address: SocketAddr, name: &str) -> Self {
        let stream = TcpStream::connect(address).expect("connect");
        stream
            .set_read_timeout(Some(SESSION_IO_TIMEOUT))
            .expect("read timeout");
        stream
            .set_write_timeout(Some(SESSION_IO_TIMEOUT))
            .expect("write timeout");
        Self {
            stream,
            name: name.to_string(),
            session_id: String::new(),
            chat_seq: 0,
        }
    }

    fn hello(address: SocketAddr, name: &str) -> Self {
        let mut client = Self::connect(address, name);
        client.send(&ClientCommand::Hello {
            protocol_version: PROTOCOL_VERSION,
            content_hash: CONTENT_HASH.to_string(),
            player_name: name.to_string(),
        });
        match client.recv() {
            ServerMessage::Welcome {
                session_id,
                resumed,
                ..
            } => {
                assert!(!resumed, "{name} hello must not be a resume");
                client.session_id = session_id;
            }
            other => panic!("{name} expected welcome, got {other:?}"),
        }
        client
    }

    /// Resume a parked session and consume the resumed welcome plus snapshot.
    ///
    /// Returns the welcome's chat watermark so the caller can request exactly
    /// the lines posted while this player was offline.
    fn resume(address: SocketAddr, name: &str, token: String, last_tick: u64) -> (Self, u64) {
        let mut client = Self::connect(address, name);
        client.send(&ClientCommand::Resume {
            session_id: token,
            last_tick,
        });
        let watermark = match client.recv() {
            ServerMessage::Welcome {
                session_id,
                resumed: true,
                chat_seq,
                ..
            } => {
                client.session_id = session_id;
                chat_seq
            }
            other => panic!("{name} expected a resumed welcome, got {other:?}"),
        };
        client.chat_seq = watermark;
        match client.recv() {
            ServerMessage::Snapshot { tick, checksum, .. } => {
                assert_eq!(checksum.len(), 16);
                assert!(tick >= last_tick);
            }
            other => panic!("{name} expected resume snapshot, got {other:?}"),
        }
        (client, watermark)
    }

    fn send(&mut self, command: &ClientCommand) {
        write_json_frame(&mut self.stream, command).expect("write command");
    }

    fn recv(&mut self) -> ServerMessage {
        let message = read_json_frame::<_, ServerMessage>(&mut self.stream).expect("read frame");
        if let ServerMessage::ChatSent { entry, .. } = &message {
            self.chat_seq = self.chat_seq.max(entry.seq);
        }
        message
    }

    /// Read the next response, skipping and tracking peer chat pushes.
    fn recv_skip_chat(&mut self) -> ServerMessage {
        loop {
            let message = self.recv();
            if !matches!(message, ServerMessage::ChatSent { .. }) {
                return message;
            }
        }
    }

    /// Send one chat line and wait for this client's own echoed entry.
    fn chat(&mut self, text: &str) -> ChatEntry {
        self.send(&ClientCommand::Chat {
            text: text.to_string(),
        });
        loop {
            match self.recv() {
                ServerMessage::ChatSent { entry, .. }
                    if entry.player == self.name && entry.text == text =>
                {
                    return entry;
                }
                // A push for an earlier line from another client.
                ServerMessage::ChatSent { .. } => {}
                other => panic!("{} expected own chat, got {other:?}", self.name),
            }
        }
    }

    fn status(&mut self) -> Status {
        self.send(&ClientCommand::ServerStatus);
        match self.recv_skip_chat() {
            ServerMessage::ServerStatus {
                tick,
                checksum,
                roads,
                online,
                ..
            } => Status {
                tick,
                checksum,
                online,
                roads,
            },
            other => panic!("{} expected server_status, got {other:?}", self.name),
        }
    }

    /// Request a viewport and consume `snapshot` + `interest_view`.
    fn set_interest(&mut self) -> (u64, String) {
        self.send(&ClientCommand::SetInterest {
            center_x: INTEREST_CENTER[0],
            center_z: INTEREST_CENTER[1],
            radius_m: INTEREST_RADIUS_M,
        });
        let (tick, checksum) = loop {
            match self.recv() {
                ServerMessage::Snapshot { tick, checksum, .. } => break (tick, checksum),
                ServerMessage::ChatSent { .. } => {}
                other => panic!("{} expected snapshot, got {other:?}", self.name),
            }
        };
        loop {
            match self.recv() {
                ServerMessage::InterestView {
                    tick: view_tick,
                    center,
                    radius_m,
                    ..
                } => {
                    assert_eq!(view_tick, tick);
                    assert_eq!(center, [0.0_f32, 0.0_f32]);
                    assert_eq!(radius_m, INTEREST_RADIUS_M as f32);
                    break;
                }
                ServerMessage::ChatSent { .. } => {}
                other => panic!("{} expected interest_view, got {other:?}", self.name),
            }
        }
        (tick, checksum)
    }

    /// Place one road and return the accepted snapshot tick.
    fn place_road(&mut self, start: [i32; 2], end: [i32; 2]) -> (u64, String) {
        self.send(&ClientCommand::PlaceRoad { start, end });
        match self.recv_skip_chat() {
            ServerMessage::Snapshot { tick, checksum, .. } => (tick, checksum),
            other => panic!("{} expected road snapshot, got {other:?}", self.name),
        }
    }

    /// Read retained chat lines newer than `after`.
    fn chat_since(&mut self, after: u64) -> Vec<ChatEntry> {
        self.send(&ClientCommand::ChatSince { after });
        loop {
            match self.recv() {
                ServerMessage::ChatLog { entries, .. } => {
                    if let Some(last) = entries.last() {
                        self.chat_seq = self.chat_seq.max(last.seq);
                    }
                    return entries;
                }
                ServerMessage::ChatSent { .. } => {}
                other => panic!("{} expected chat_log, got {other:?}", self.name),
            }
        }
    }

    fn disconnect(&mut self) {
        self.send(&ClientCommand::Disconnect);
        assert!(
            matches!(self.recv_skip_chat(), ServerMessage::Goodbye),
            "{} expected goodbye",
            self.name
        );
    }
}

/// Roads accepted before `round` starts.
///
/// Rounds 0–19 have four players, 20–22 have three (dave offline), and 23–39
/// have four again.
fn roads_before(round: usize) -> u32 {
    let (full, bare) = (4_u32, 3_u32);
    if round <= DISCONNECT_ROUND {
        round as u32 * full
    } else if round < RESUME_ROUND {
        (DISCONNECT_ROUND as u32 * full) + (round - DISCONNECT_ROUND) as u32 * bare
    } else {
        (DISCONNECT_ROUND as u32 * full)
            + ((RESUME_ROUND - DISCONNECT_ROUND) as u32 * bare)
            + (round - RESUME_ROUND) as u32 * full
    }
}

/// Fixed per-client road row: one axis-aligned segment per round, separated by
/// an 8 m gap so collinear segments never overlap.
fn road_for(index: u32, round: usize) -> ([i32; 2], [i32; 2]) {
    let x = round as i32 * 16 + index as i32 * 4;
    let z = index as i32 * 4_096;
    ([x, z], [x + 8, z])
}

/// One deterministic turn: chat, status, interest view, road write.
fn run_turn(client: &mut Client, index: u32, round: usize, expected_online: u32) {
    let text = format!("r{round:02}-{}", client.name);
    let entry = client.chat(&text);
    assert_eq!(
        entry.seq, client.chat_seq,
        "{} own chat must be the newest observed line",
        client.name
    );

    let status = client.status();
    assert_eq!(status.checksum.len(), 16);
    assert_eq!(
        status.online, expected_online,
        "{} saw the wrong online count in round {round}",
        client.name
    );
    assert_eq!(
        status.roads,
        roads_before(round) + index,
        "{} saw an unexpected road count before its round-{round} write",
        client.name
    );

    let (interest_tick, interest_checksum) = client.set_interest();
    assert_eq!(
        (interest_tick, interest_checksum),
        (status.tick, status.checksum.clone()),
        "a read-only interest view must not advance tick or checksum"
    );

    let (start, end) = road_for(index, round);
    let (road_tick, road_checksum) = client.place_road(start, end);
    assert_eq!(
        road_tick,
        status.tick + 1,
        "an accepted road must advance the tick by exactly one"
    );
    assert_ne!(
        road_checksum, status.checksum,
        "an accepted road must change the world checksum"
    );
    assert_eq!(road_checksum.len(), 16);
}

#[test]
fn four_player_bounded_soak() {
    let save_dir = TempDir::new("save");
    let save = save_dir.path().join("world.json");
    let second = save_dir.path().join("world-again.json");

    let world =
        ServerWorld::with_role(townsmith_server::protocol::LOOPBACK_SEED, ClientRole::Mayor);

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
    let address = listener.local_addr().expect("local address");
    let config = ListenConfig {
        save_path: Some(save.clone()),
        autosave_ticks: 1,
        shutdown_save: false,
    };
    let server =
        thread::spawn(move || run_listen(listener, world, config, Some(EXPECTED_SESSIONS)));

    // Four fixed players in a fixed order; ids and checksums stay stable.
    let mut alice = Client::hello(address, "alice");
    let mut bravo = Client::hello(address, "bravo");
    let mut charlie = Client::hello(address, "charlie");
    let mut dave = Some(Client::hello(address, "dave"));

    // Chat lines posted while dave is parked, in authoritative order.
    let offline_texts: Vec<String> = (DISCONNECT_ROUND..RESUME_ROUND)
        .flat_map(|round| ["alice", "bravo", "charlie"].map(|name| format!("r{round:02}-{name}")))
        .collect();
    assert_eq!(offline_texts.len(), 9);

    let mut dave_token = String::new();
    let mut resume_tick = 0_u64;
    for round in 0..ROUNDS {
        if round == DISCONNECT_ROUND {
            let mut leaving = dave.take().expect("dave is still connected");
            assert_eq!(
                leaving.chat_seq, 80,
                "dave must have observed all 20 rounds of four-player chat"
            );
            dave_token = leaving.session_id.clone();
            resume_tick = leaving.status().tick;
            leaving.disconnect();
            drop(leaving);
        }

        if round == RESUME_ROUND {
            let (mut resumed, watermark) =
                Client::resume(address, "dave", dave_token.clone(), resume_tick);
            assert_eq!(
                watermark, 89,
                "the resumed welcome must carry the chat watermark after 9 offline lines"
            );
            let catch_up = resumed.chat_since(80);
            let texts: Vec<String> = catch_up.iter().map(|entry| entry.text.clone()).collect();
            assert_eq!(
                texts, offline_texts,
                "resume catch-up must return exactly the offline lines"
            );
            assert_eq!(resumed.chat_seq, 89);
            dave = Some(resumed);
        }

        let online = if (DISCONNECT_ROUND..RESUME_ROUND).contains(&round) {
            3
        } else {
            4
        };
        run_turn(&mut alice, 0, round, online);
        run_turn(&mut bravo, 1, round, online);
        run_turn(&mut charlie, 2, round, online);
        if let Some(dave) = dave.as_mut() {
            run_turn(dave, 3, round, online);
        }
    }

    // Final live read, then close every session cleanly.
    let final_status = alice.status();
    assert_eq!(final_status.online, 4);
    assert_eq!(final_status.roads, EXPECTED_FINAL_ROADS);
    let (final_tick, final_checksum) = (final_status.tick, final_status.checksum);

    alice.disconnect();
    bravo.disconnect();
    charlie.disconnect();
    dave.take().expect("dave resumed").disconnect();

    let report = server
        .join()
        .expect("listen thread panicked")
        .expect("listen returned an error");

    assert_eq!(
        report.sessions, EXPECTED_SESSIONS,
        "four players plus one reconnect"
    );
    assert_eq!(
        report.commands, EXPECTED_COMMANDS,
        "the fixed tape produced an unexpected command count"
    );
    assert_eq!(
        (report.tick, report.checksum.as_str()),
        (EXPECTED_FINAL_TICK, EXPECTED_FINAL_CHECKSUM),
        "the soak tape diverged from its pinned outcome"
    );
    assert_eq!(
        (report.tick, report.checksum.as_str()),
        (final_tick, final_checksum.as_str()),
        "the final live status diverged from the listen report"
    );
    assert!(
        report.saves > 0,
        "the configured save path was never written"
    );

    // Independent reload must reproduce the live tick, checksum, and roads.
    let loaded = ServerWorld::load_from(&save).expect("load final save");
    assert!(!loaded.report.recovered_from_backup);
    assert_eq!(loaded.world.tick, final_tick);
    assert_eq!(loaded.world.checksum(), final_checksum);
    assert_eq!(
        loaded.world.roads.roads.len(),
        EXPECTED_FINAL_ROADS as usize
    );
    assert_eq!(loaded.world.players.count(), 4);
    assert!(!temp_path(&save).exists(), "save left a .tmp file behind");

    // A second save from the loaded world must be byte-identical.
    loaded.world.save_to(&second).expect("second save");
    assert_eq!(
        fs::read(&save).expect("read first save"),
        fs::read(&second).expect("read second save"),
        "re-saving the loaded world must be byte-identical"
    );
    assert!(!temp_path(&second).exists(), "second save left a .tmp file");

    println!(
        "FOUR_PLAYER_SOAK tick={} checksum={} roads={} plans={} sessions={} commands={} saves={}",
        report.tick,
        report.checksum,
        final_status.roads,
        EXPECTED_COMMANDS,
        report.sessions,
        report.commands,
        report.saves
    );
}
