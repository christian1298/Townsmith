//! First automated Phase 6 exit scenario: four instrumented TCP clients
//! against the production listen loop.
//!
//! The test binds an ephemeral loopback port and runs
//! [`townsmith_server::listen::run_listen`] on a background thread with a
//! temporary save path. Four real v1 clients (`alice`, `bravo`, `charlie`,
//! `dave`) then execute one deterministic scripted session:
//!
//! 1. `hello` in a fixed order, which fixes the stable player ids.
//! 2. `alice` founds *Harbour*, invites `bravo` as builder, promotes him to
//!    manager, and `bravo` sets the settlement tax to 12%.
//! 3. `alice` and `bravo` contend one advisory blueprint lock: the second lock
//!    rejects with `blueprint.locked` and leaves tick and checksum unchanged.
//! 4. `alice` unlocks and places the server placement fixture; `bravo` repeats
//!    the exact request and gets the same placement id without a second charge.
//! 5. `alice` chats; all four clients observe the same broadcast line.
//! 6. A D-021 time vote to pause passes 3–1 (three of four online), and the
//!    authoritative world reports `paused: true`.
//! 7. `dave` disconnects and resumes on a fifth connection with the token from
//!    his `welcome`.
//! 8. A read-only `server_status` tail leaves tick and checksum unchanged.
//!
//! The test then asserts that the final [`ListenReport`] tick/checksum equals
//! the last `server_status`, that the save reloads to the same world checksum,
//! that exactly one placement exists, and that no `.tmp` file is left.
//!
//! # Deliberate gaps (v1)
//!
//! - **Session roles are process-wide.** `run_listen` assigns one
//!   `default_role` to every accepted session and exposes no per-session role
//!   hook, so all four sessions run as **Mayor**. Distinct *settlement* roles
//!   (owner/manager) are exercised; protocol-level `role.forbidden` paths and
//!   the admin-only direct `time_control` / `settlement_release` /
//!   force-unlock paths are not. Time control is covered through the
//!   server-authoritative vote instead.
//! - A second Admin-role world and a denied-builder mutation are omitted to
//!   keep one logical change; they belong to the role-matrix tests.
//! - "Edit a blueprint while another places it" (§3.10 scenario 2) is only
//!   covered as lock contention: v1 placement does not require the caller to
//!   hold the advisory lock.
//! - Bandwidth, tick time, and the one-hour soak (§3.10 scenarios 1 and 7)
//!   need reference hardware and remain human/hardware gates.
//!
//! Everything is response-driven: every wait is a protocol round trip bounded
//! by [`SESSION_IO_TIMEOUT`]; there are no sleeps and no wall-clock assertions.

use std::fs;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use townsmith_server::listen::{run_listen, ListenConfig};
use townsmith_server::placement::{write_placement_fixture, PLACEMENT_SMOKE_SEED};
use townsmith_server::protocol::{
    codes, read_json_frame, write_json_frame, ClientCommand, ServerMessage, TimeAction, VoteAction,
    PROTOCOL_VERSION, SESSION_IO_TIMEOUT,
};
use townsmith_server::save::temp_path;
use townsmith_server::world::{ClientRole, ServerWorld};

const CONTENT_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// Settlement centre; far from the placement so the build-permission gate is
/// not involved.
const SETTLEMENT_CENTER: [i32; 2] = [10, -20];
const SETTLEMENT_RADIUS_M: u32 = 64;

/// Placement centre outside the settlement.
const PLACEMENT_CENTER: [f64; 2] = [256.0, 0.0];

/// Pinned deterministic outcome, verified stable across three consecutive runs
/// on 2026-11-27. Changes here mean the scripted session moved authoritative
/// state and must be reviewed, not silently re-pinned.
const EXPECTED_FINAL_TICK: u64 = 3;
const EXPECTED_FINAL_CHECKSUM: &str = "cdbe19ba10ed8ec5";

static NEXT: AtomicU64 = AtomicU64::new(1);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "townsmith-four-player-exit-{label}-{}-{}",
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

struct Client {
    stream: TcpStream,
    name: String,
    session_id: String,
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
        }
    }

    fn hello(address: SocketAddr, name: &str) -> Self {
        let mut client = Self::connect(address, name);
        client.send(&ClientCommand::Hello {
            protocol_version: PROTOCOL_VERSION,
            content_hash: CONTENT_HASH.to_string(),
            player_name: name.to_string(),
        });
        let message = client.recv_skip_chat();
        match message {
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

    fn send(&mut self, command: &ClientCommand) {
        write_json_frame(&mut self.stream, command).expect("write command");
    }

    fn recv(&mut self) -> ServerMessage {
        read_json_frame::<_, ServerMessage>(&mut self.stream).expect("read frame")
    }

    /// Read the next response, skipping broadcast chat pushes.
    fn recv_skip_chat(&mut self) -> ServerMessage {
        loop {
            let message = self.recv();
            if !matches!(message, ServerMessage::ChatSent { .. }) {
                return message;
            }
        }
    }

    fn expect_chat(&mut self) -> String {
        match self.recv() {
            ServerMessage::ChatSent { entry, .. } => entry.text,
            other => panic!("{} expected chat push, got {other:?}", self.name),
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

fn settlement_of(message: ServerMessage) -> townsmith_server::protocol::SettlementView {
    match message {
        ServerMessage::SettlementFound { settlement, .. }
        | ServerMessage::SettlementUpdated { settlement, .. } => settlement,
        other => panic!("expected settlement record, got {other:?}"),
    }
}

fn status_of(message: ServerMessage) -> (u64, String, u32) {
    match message {
        ServerMessage::ServerStatus {
            tick,
            checksum,
            placements,
            ..
        } => (tick, checksum, placements),
        other => panic!("expected server_status, got {other:?}"),
    }
}

fn player_id(client: &mut Client, name: &str) -> u64 {
    client.send(&ClientCommand::PlayerList {
        after_id: None,
        limit: None,
    });
    match client.recv_skip_chat() {
        ServerMessage::PlayerList { players, .. } => {
            players
                .iter()
                .find(|player| player.name == name)
                .unwrap_or_else(|| panic!("player {name} is not in the roster"))
                .id
        }
        other => panic!("expected player_list, got {other:?}"),
    }
}

#[test]
fn four_players_scripted_exit_scenario() {
    let library = TempDir::new("library");
    let save_dir = TempDir::new("save");
    let save = save_dir.path().join("world.json");
    let hash = write_placement_fixture(library.path()).expect("write placement fixture");

    let mut world = ServerWorld::with_role(PLACEMENT_SMOKE_SEED, ClientRole::Mayor);
    world
        .set_blueprint_library(library.path().to_path_buf())
        .expect("configure library");

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
    let address = listener.local_addr().expect("local address");
    let config = ListenConfig {
        save_path: Some(save.clone()),
        autosave_ticks: 0,
        shutdown_save: false,
    };
    let server = thread::spawn(move || run_listen(listener, world, config, Some(5)));

    // 1. Deterministic hello order fixes the stable player ids.
    let mut alice = Client::hello(address, "alice");
    let mut bravo = Client::hello(address, "bravo");
    let mut charlie = Client::hello(address, "charlie");
    let mut dave = Client::hello(address, "dave");
    let bravo_id = player_id(&mut alice, "bravo");

    // 2. Settlement flow: found, invite, promote, tax.
    alice.send(&ClientCommand::FoundSettlement {
        name: "Harbour".to_string(),
        x: SETTLEMENT_CENTER[0],
        z: SETTLEMENT_CENTER[1],
        radius_m: SETTLEMENT_RADIUS_M,
    });
    let founded = settlement_of(alice.recv_skip_chat());
    assert_eq!(founded.membership_revision, 1);
    assert_eq!(founded.owner, "alice");
    let settlement_id = founded.id;

    alice.send(&ClientCommand::SettlementInvite {
        settlement_id,
        player_id: bravo_id,
        role: "builder".to_string(),
        expected_revision: 1,
    });
    let invited = settlement_of(alice.recv_skip_chat());
    assert_eq!(invited.membership_revision, 2);
    assert!(invited
        .members
        .iter()
        .any(|member| member.player == bravo_id && member.role == "builder"));

    alice.send(&ClientCommand::SettlementSetRole {
        settlement_id,
        player_id: bravo_id,
        role: "manager".to_string(),
        expected_revision: 2,
    });
    let promoted = settlement_of(alice.recv_skip_chat());
    assert_eq!(promoted.membership_revision, 3);
    assert!(promoted
        .members
        .iter()
        .any(|member| member.player == bravo_id && member.role == "manager"));

    bravo.send(&ClientCommand::SettlementSetTax {
        settlement_id,
        rate: 12,
    });
    let taxed = settlement_of(bravo.recv_skip_chat());
    assert_eq!(taxed.tax_rate, 12);
    assert_eq!(taxed.membership_revision, 3, "tax is data-only");

    // 3. Two players contend one advisory lock; the loser changes nothing.
    alice.send(&ClientCommand::BlueprintLock { hash: hash.clone() });
    let locked = match alice.recv_skip_chat() {
        ServerMessage::BlueprintLockResult {
            lock, locks_count, ..
        } => {
            assert_eq!(lock.hash, hash);
            assert_eq!(lock.player, "alice");
            assert_eq!(locks_count, 1);
            lock
        }
        other => panic!("expected blueprint_lock_result, got {other:?}"),
    };

    charlie.send(&ClientCommand::ServerStatus);
    let (tick_before, checksum_before, placements_before) = status_of(charlie.recv_skip_chat());
    assert_eq!(placements_before, 0);

    bravo.send(&ClientCommand::BlueprintLock { hash: hash.clone() });
    match bravo.recv_skip_chat() {
        ServerMessage::Reject { code, .. } => assert_eq!(code, codes::BLUEPRINT_LOCKED),
        other => panic!("expected blueprint.locked rejection, got {other:?}"),
    }

    charlie.send(&ClientCommand::ServerStatus);
    let (tick_after, checksum_after, _) = status_of(charlie.recv_skip_chat());
    assert_eq!(
        (tick_after, checksum_after.as_str()),
        (tick_before, checksum_before.as_str()),
        "a rejected lock must not advance tick or checksum"
    );
    assert_eq!(
        locked.tick, tick_before,
        "the accepted lock is the tick the baseline status observed"
    );

    alice.send(&ClientCommand::BlueprintUnlock { hash: hash.clone() });
    match alice.recv_skip_chat() {
        ServerMessage::BlueprintUnlockResult {
            hash: released,
            locks_count,
            ..
        } => {
            assert_eq!(released, hash);
            assert_eq!(locks_count, 0);
        }
        other => panic!("expected blueprint_unlock_result, got {other:?}"),
    }

    // 4. One authoritative placement; a duplicate request is idempotent.
    alice.send(&ClientCommand::PlaceBlueprint {
        blueprint_hash: hash.clone(),
        x: PLACEMENT_CENTER[0],
        z: PLACEMENT_CENTER[1],
        rotation: 0,
    });
    let (placement_id, place_tick, placement_checksum) = match alice.recv_skip_chat() {
        ServerMessage::PlaceBlueprintResult {
            placement_id,
            tick,
            world_checksum,
            placement_checksum,
            ..
        } => {
            assert_eq!(placement_checksum.len(), 16);
            (placement_id, tick, world_checksum)
        }
        other => panic!("expected place_blueprint_result, got {other:?}"),
    };

    bravo.send(&ClientCommand::PlaceBlueprint {
        blueprint_hash: hash.clone(),
        x: PLACEMENT_CENTER[0],
        z: PLACEMENT_CENTER[1],
        rotation: 0,
    });
    match bravo.recv_skip_chat() {
        ServerMessage::PlaceBlueprintResult {
            placement_id: duplicate_id,
            tick,
            world_checksum,
            ..
        } => {
            assert_eq!(duplicate_id, placement_id, "duplicate must reuse the id");
            assert_eq!(tick, place_tick, "duplicate must not advance the tick");
            assert_eq!(
                world_checksum, placement_checksum,
                "duplicate must not change the world checksum"
            );
        }
        other => panic!("expected idempotent place_blueprint_result, got {other:?}"),
    }

    // 5. One chat line is observed by the sender and all three peers.
    alice.send(&ClientCommand::Chat {
        text: "harbour is open".to_string(),
    });
    assert_eq!(alice.expect_chat(), "harbour is open");
    assert_eq!(bravo.expect_chat(), "harbour is open");
    assert_eq!(charlie.expect_chat(), "harbour is open");
    assert_eq!(dave.expect_chat(), "harbour is open");

    // 6. D-021 vote: alice calls pause, bravo and charlie vote yes (3 of 4).
    alice.send(&ClientCommand::TimeVote {
        action: VoteAction::Call,
        time: Some(TimeAction::Pause),
    });
    match alice.recv_skip_chat() {
        ServerMessage::TimeVoteStatus {
            state,
            online,
            paused,
            ..
        } => {
            assert_eq!(state, "open");
            assert_eq!(online, 4);
            assert!(!paused);
        }
        other => panic!("expected open time_vote_status, got {other:?}"),
    }

    bravo.send(&ClientCommand::TimeVote {
        action: VoteAction::Yes,
        time: None,
    });
    match bravo.recv_skip_chat() {
        ServerMessage::TimeVoteStatus { state, yes, .. } => {
            assert_eq!(state, "open");
            assert_eq!(yes, 2);
        }
        other => panic!("expected open time_vote_status, got {other:?}"),
    }

    charlie.send(&ClientCommand::TimeVote {
        action: VoteAction::Yes,
        time: None,
    });
    match charlie.recv_skip_chat() {
        ServerMessage::TimeVoteStatus {
            state,
            yes,
            no,
            paused,
            ..
        } => {
            assert_eq!(state, "passed");
            assert_eq!((yes, no), (3, 0));
            assert!(paused, "a passed pause vote must pause the world");
        }
        other => panic!("expected passed time_vote_status, got {other:?}"),
    }

    // 7. dave disconnects and resumes on a fifth connection.
    let dave_token = dave.session_id.clone();
    dave.disconnect();
    drop(dave);

    let mut resumed = Client::connect(address, "dave");
    resumed.send(&ClientCommand::Resume {
        session_id: dave_token,
        last_tick: 0,
    });
    match resumed.recv_skip_chat() {
        ServerMessage::Welcome { resumed: true, .. } => {}
        other => panic!("expected resumed welcome, got {other:?}"),
    }
    match resumed.recv_skip_chat() {
        ServerMessage::Snapshot { checksum, .. } => assert_eq!(checksum.len(), 16),
        other => panic!("expected resume snapshot, got {other:?}"),
    }

    // 8. Read-only tail leaves tick and checksum unchanged.
    resumed.send(&ClientCommand::ServerStatus);
    let (tail_tick, tail_checksum, tail_placements) = status_of(resumed.recv_skip_chat());
    assert_eq!(tail_placements, 1, "exactly one placement must exist");

    resumed.send(&ClientCommand::ServerStatus);
    let (again_tick, again_checksum, _) = status_of(resumed.recv_skip_chat());
    assert_eq!(
        (tail_tick, tail_checksum.as_str()),
        (again_tick, again_checksum.as_str()),
        "read-only commands must not advance tick or checksum"
    );

    resumed.disconnect();
    drop(resumed);

    // Final read from a still-connected session, then a clean close for all.
    alice.send(&ClientCommand::ServerStatus);
    let (final_tick, final_checksum, final_placements) = status_of(alice.recv_skip_chat());
    assert_eq!(final_placements, 1);
    println!("FOUR_PLAYER_EXIT tick={final_tick} checksum={final_checksum}");
    alice.disconnect();
    bravo.disconnect();
    charlie.disconnect();

    let report = server
        .join()
        .expect("listen thread panicked")
        .expect("listen returned an error");

    assert_eq!(report.sessions, 5, "four players plus one reconnect");
    assert_eq!(
        (report.tick, report.checksum.as_str()),
        (EXPECTED_FINAL_TICK, EXPECTED_FINAL_CHECKSUM),
        "the scripted session diverged from its pinned tick/checksum"
    );
    assert_eq!(report.tick, final_tick, "final report tick diverged");
    assert_eq!(
        report.checksum, final_checksum,
        "final report checksum diverged"
    );
    assert!(
        report.saves > 0,
        "the configured save path was never written"
    );

    // Save/reload round-trip: the file parses and hashes identically.
    let loaded = ServerWorld::load_from(&save).expect("load final save");
    assert!(!loaded.report.recovered_from_backup);
    assert_eq!(
        loaded.world.checksum(),
        report.checksum,
        "reloaded world checksum diverged"
    );
    assert_eq!(
        loaded
            .world
            .placements
            .as_ref()
            .map_or(0, |store| store.placements.len()),
        1,
        "reloaded world must hold exactly one placement"
    );
    assert!(!temp_path(&save).exists(), "save left a .tmp file behind");
}
