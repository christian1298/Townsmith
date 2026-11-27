//! Admin-role companion to [`four_player_exit`](four_player_exit.rs).
//!
//! The four-player exit harness runs every session as **Mayor** because
//! `run_listen` assigns one `default_role` per process and exposes no
//! per-session role hook. This test closes the command-surface half of that
//! gap: a second, independent `run_listen` world starts with
//! [`ServerWorld::with_role`] set to [`ClientRole::Admin`], an ephemeral
//! loopback port, a temporary save path, and two real v1 TCP clients. It proves
//! that the admin-only commands work over the wire, not that different players
//! can hold different roles (they cannot, in v0).
//!
//! The scripted session is:
//!
//! 1. `alice` and `bravo` complete `hello` (both sessions inherit the
//!    process-wide Admin role).
//! 2. `alice` pauses with admin `time_control`; `bravo` places a road while
//!    paused (Mode B: editing continues), and `alice` resumes.
//! 3. `alice` founds a settlement and force-releases it; `bravo` releases an
//!    unknown id and gets `settlement.unknown` with no state change.
//! 4. `alice` locks a blueprint and `bravo` force-unlocks it; the audit ring
//!    records `blueprint.unlock_forced`.
//! 5. `alice` reads `audit_log` with valid filters, then sends a malformed
//!    filter and gets `audit.filter_invalid` with no tick/checksum change.
//! 6. Both sessions close; the final save reloads to the report checksum with
//!    `paused == false`, one road, one released settlement, and no `.tmp`.
//!
//! # Observability limits (v0)
//!
//! `paused` is not a field of `snapshot` or `server_status`; in protocol v0 the
//! only wire message that carries it is `time_vote_status`. This test therefore
//! asserts the accepted `time_control` responses (which echo an unchanged tick
//! and world checksum) and the reloaded `paused == false` after the final
//! resume, rather than sampling `paused == true` mid-run. The pause flag itself
//! is unit-tested in `world.rs`. The audit ring is per-session, so each client
//! only sees its own commands; the test queries each session's ring
//! deliberately.
//!
//! Everything is response-driven; there are no sleeps. The world is Admin for
//! the whole process, so this is a command-surface test, not per-player role
//! assignment.

use std::fs;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use townsmith_server::listen::{run_listen, ListenConfig};
use townsmith_server::protocol::{
    codes, read_json_frame, write_json_frame, AuditLogEntry, ClientCommand, ServerMessage,
    TimeAction, PROTOCOL_VERSION, SESSION_IO_TIMEOUT,
};
use townsmith_server::save::temp_path;
use townsmith_server::world::{ClientRole, ServerWorld, AUDIT_UNLOCK_FORCED};

const CONTENT_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// Settlement centre; no placement is involved, so any in-range cell works.
const SETTLEMENT_CENTER: [i32; 2] = [10, -20];
const SETTLEMENT_RADIUS_M: u32 = 64;

/// Unknown settlement id for the rejection path.
const MISSING_SETTLEMENT_ID: u64 = 9_999;

/// Pinned deterministic outcome, verified stable across three consecutive runs
/// on 2026-11-27. A change means the scripted admin session moved authoritative
/// state and must be reviewed, not silently re-pinned.
const EXPECTED_FINAL_TICK: u64 = 3;
const EXPECTED_FINAL_CHECKSUM: &str = "5cfb82c3223d51f2";

static NEXT: AtomicU64 = AtomicU64::new(1);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "townsmith-four-player-admin-{label}-{}-{}",
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
}

impl Client {
    fn hello(address: SocketAddr, name: &str) -> Self {
        let stream = TcpStream::connect(address).expect("connect");
        stream
            .set_read_timeout(Some(SESSION_IO_TIMEOUT))
            .expect("read timeout");
        stream
            .set_write_timeout(Some(SESSION_IO_TIMEOUT))
            .expect("write timeout");
        let mut client = Self {
            stream,
            name: name.to_string(),
        };
        client.send(&ClientCommand::Hello {
            protocol_version: PROTOCOL_VERSION,
            content_hash: CONTENT_HASH.to_string(),
            player_name: name.to_string(),
        });
        match client.recv_skip_chat() {
            ServerMessage::Welcome { resumed, .. } => {
                assert!(!resumed, "{name} hello must not be a resume");
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

    fn recv_skip_chat(&mut self) -> ServerMessage {
        loop {
            let message = self.recv();
            if !matches!(message, ServerMessage::ChatSent { .. }) {
                return message;
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

/// `(tick, checksum, roads, settlements, placements, online)`
fn status_of(message: ServerMessage) -> (u64, String, u32, u32, u32, u32) {
    match message {
        ServerMessage::ServerStatus {
            tick,
            checksum,
            roads,
            settlements,
            placements,
            online,
            ..
        } => (tick, checksum, roads, settlements, placements, online),
        other => panic!("expected server_status, got {other:?}"),
    }
}

fn snapshot_tick(message: ServerMessage) -> (u64, String) {
    match message {
        ServerMessage::Snapshot { tick, checksum, .. } => (tick, checksum),
        other => panic!("expected snapshot, got {other:?}"),
    }
}

fn audit_entries(message: ServerMessage) -> (u32, String, Vec<AuditLogEntry>) {
    match message {
        ServerMessage::AuditLog {
            matched,
            checksum,
            entries,
            ..
        } => (matched, checksum, entries),
        other => panic!("expected audit_log, got {other:?}"),
    }
}

#[test]
fn four_player_admin_command_surface() {
    let save_dir = TempDir::new("save");
    let save = save_dir.path().join("world.json");

    let world =
        ServerWorld::with_role(townsmith_server::protocol::LOOPBACK_SEED, ClientRole::Admin);

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
    let address = listener.local_addr().expect("local address");
    let config = ListenConfig {
        save_path: Some(save.clone()),
        autosave_ticks: 0,
        shutdown_save: false,
    };
    let server = thread::spawn(move || run_listen(listener, world, config, Some(2)));

    let mut alice = Client::hello(address, "alice");
    let mut bravo = Client::hello(address, "bravo");
    let bravo_id = player_id(&mut alice, "bravo");
    let alice_id = player_id(&mut bravo, "alice");

    // Baseline: two online sessions and an empty world.
    alice.send(&ClientCommand::ServerStatus);
    let (baseline_tick, baseline_checksum, baseline_roads, baseline_settlements, _, online) =
        status_of(alice.recv_skip_chat());
    assert_eq!(online, 2);
    assert_eq!(baseline_settlements, 0);

    // 1. Admin time control: pause is accepted, editing continues while
    //    paused, and resume is accepted. Neither pause nor resume advances the
    //    tick (the paused flag is deliberately outside the world checksum).
    alice.send(&ClientCommand::TimeControl {
        action: TimeAction::Pause,
    });
    let (paused_tick, paused_checksum) = snapshot_tick(alice.recv_skip_chat());
    assert_eq!(
        (paused_tick, paused_checksum.as_str()),
        (baseline_tick, baseline_checksum.as_str()),
        "an admin pause must not advance the tick or change the world checksum"
    );

    bravo.send(&ClientCommand::PlaceRoad {
        start: [0, 0],
        end: [4, 0],
    });
    let (edited_tick, _) = snapshot_tick(bravo.recv_skip_chat());
    assert_eq!(
        edited_tick,
        paused_tick + 1,
        "a road edit while paused must be accepted and advance the tick"
    );
    bravo.send(&ClientCommand::ServerStatus);
    let (_, _, roads_after_edit, settlements_after_edit, _, _) = status_of(bravo.recv_skip_chat());
    assert!(
        roads_after_edit > baseline_roads,
        "the accepted road edit must add road segments"
    );
    assert_eq!(settlements_after_edit, 0);

    alice.send(&ClientCommand::TimeControl {
        action: TimeAction::Resume,
    });
    let (resumed_tick, _) = snapshot_tick(alice.recv_skip_chat());
    assert_eq!(
        resumed_tick, edited_tick,
        "an admin resume must not advance the tick"
    );

    // 2. Settlement release: found as alice, force-release at revision 1.
    alice.send(&ClientCommand::FoundSettlement {
        name: "Admin Bay".to_string(),
        x: SETTLEMENT_CENTER[0],
        z: SETTLEMENT_CENTER[1],
        radius_m: SETTLEMENT_RADIUS_M,
    });
    let (settlement_id, found_revision) = match alice.recv_skip_chat() {
        ServerMessage::SettlementFound { settlement, .. } => {
            assert_eq!(settlement.owner, "alice");
            assert!(settlement.owned);
            (settlement.id, settlement.membership_revision)
        }
        other => panic!("expected settlement_found, got {other:?}"),
    };
    assert_eq!(found_revision, 1);

    alice.send(&ClientCommand::SettlementRelease {
        settlement_id,
        expected_revision: found_revision,
    });
    match alice.recv_skip_chat() {
        ServerMessage::SettlementUpdated {
            settlement,
            world_checksum,
            ..
        } => {
            assert!(!settlement.owned, "force release must clear ownership");
            assert_eq!(settlement.owner_id, None);
            assert!(settlement.members.is_empty());
            assert_eq!(settlement.membership_revision, found_revision + 1);
            assert_eq!(world_checksum.len(), 16);
        }
        other => panic!("expected settlement_updated, got {other:?}"),
    }

    // 3. Validation still runs under Admin: an unknown settlement rejects and
    //    leaves the authoritative state untouched.
    alice.send(&ClientCommand::ServerStatus);
    let (before_reject_tick, before_reject_checksum, before_roads, before_settlements, _, _) =
        status_of(alice.recv_skip_chat());

    bravo.send(&ClientCommand::SettlementRelease {
        settlement_id: MISSING_SETTLEMENT_ID,
        expected_revision: 1,
    });
    match bravo.recv_skip_chat() {
        ServerMessage::Reject { code, .. } => {
            assert_eq!(code, townsmith_server::world::codes::SETTLEMENT_UNKNOWN)
        }
        other => panic!("expected settlement.unknown rejection, got {other:?}"),
    }

    // 4. Malformed audit filter rejects too, with the same no-change guarantee.
    alice.send(&ClientCommand::AuditLog {
        limit: Some(4),
        tag: Some("BAD TAG".to_string()),
        accepted: None,
        role: None,
    });
    match alice.recv_skip_chat() {
        ServerMessage::Reject { code, .. } => assert_eq!(code, codes::AUDIT_FILTER_INVALID),
        other => panic!("expected audit.filter_invalid rejection, got {other:?}"),
    }

    alice.send(&ClientCommand::ServerStatus);
    let (after_reject_tick, after_reject_checksum, after_roads, after_settlements, _, _) =
        status_of(alice.recv_skip_chat());
    assert_eq!(
        (
            after_reject_tick,
            after_reject_checksum.as_str(),
            after_roads,
            after_settlements
        ),
        (
            before_reject_tick,
            before_reject_checksum.as_str(),
            before_roads,
            before_settlements
        ),
        "rejected admin commands must not mutate world state"
    );
    assert_eq!(
        roads_after_edit, after_roads,
        "no rejected command may add road segments"
    );
    assert_eq!(after_settlements, 1, "one released settlement is stored");

    // 5. Audit ring: the accepted admin commands are recorded with the admin
    //    role, and the malformed query is recorded as a rejection.
    alice.send(&ClientCommand::AuditLog {
        limit: Some(8),
        tag: Some("time_control".to_string()),
        accepted: Some(true),
        role: Some("admin".to_string()),
    });
    let (matched, audit_checksum, entries) = audit_entries(alice.recv_skip_chat());
    assert_eq!(matched, 2, "pause and resume are both audited");
    assert_eq!(audit_checksum.len(), 16);
    assert_eq!(entries.len(), 2);
    for entry in &entries {
        assert_eq!(entry.tag, "time_control");
        assert_eq!(entry.role, "admin");
        assert!(entry.accepted);
        assert_eq!(entry.code, townsmith_server::world::AUDIT_ACCEPTED);
    }

    alice.send(&ClientCommand::AuditLog {
        limit: Some(4),
        tag: Some("settlement_release".to_string()),
        accepted: Some(true),
        role: None,
    });
    let (matched, _, entries) = audit_entries(alice.recv_skip_chat());
    assert_eq!(matched, 1);
    assert_eq!(entries[0].tag, "settlement_release");
    assert!(entries[0].accepted);

    alice.send(&ClientCommand::AuditLog {
        limit: Some(4),
        tag: None,
        accepted: Some(false),
        role: None,
    });
    let (matched, _, entries) = audit_entries(alice.recv_skip_chat());
    assert!(matched >= 1, "the malformed filter rejection is audited");
    assert!(entries
        .iter()
        .any(|entry| entry.tag == "audit_log" && entry.code == codes::AUDIT_FILTER_INVALID));

    // 6. Force unlock: alice holds the lock, bravo (same Admin role, different
    //    player identity) unlocks it. No fixture is needed because a lock is
    //    advisory intent and the hash only has to be well formed.
    let bare = "ab".repeat(32);
    let hash = format!("blake3:{bare}");
    alice.send(&ClientCommand::BlueprintLock { hash: bare.clone() });
    match alice.recv_skip_chat() {
        ServerMessage::BlueprintLockResult { lock, .. } => {
            assert_eq!(lock.hash, hash);
            assert_eq!(lock.player, "alice");
        }
        other => panic!("expected blueprint_lock_result, got {other:?}"),
    }

    bravo.send(&ClientCommand::BlueprintUnlock { hash: bare });
    match bravo.recv_skip_chat() {
        ServerMessage::BlueprintUnlockResult {
            hash: released,
            forced,
            holder_id,
            locks_count,
            ..
        } => {
            assert_eq!(released, hash);
            assert!(forced, "an admin must force-unlock a foreign lock");
            assert_eq!(holder_id, Some(alice_id));
            assert_eq!(locks_count, 0);
        }
        other => panic!("expected blueprint_unlock_result, got {other:?}"),
    }
    assert_ne!(alice_id, bravo_id);

    bravo.send(&ClientCommand::AuditLog {
        limit: Some(4),
        tag: Some("blueprint_unlock".to_string()),
        accepted: None,
        role: None,
    });
    let (matched, _, entries) = audit_entries(bravo.recv_skip_chat());
    assert_eq!(matched, 1);
    assert_eq!(entries[0].code, AUDIT_UNLOCK_FORCED);
    assert!(entries[0].accepted);
    assert_eq!(entries[0].role, "admin");

    // 7. Read-only tail, clean close, and the final listen report.
    alice.send(&ClientCommand::ServerStatus);
    let (final_tick, final_checksum, final_roads, final_settlements, final_placements, _) =
        status_of(alice.recv_skip_chat());
    assert_eq!(final_roads, roads_after_edit);
    assert_eq!(final_settlements, 1);
    assert_eq!(final_placements, 0);
    assert!(
        final_tick > after_reject_tick,
        "accepted locks advance the tick"
    );

    alice.disconnect();
    bravo.disconnect();

    let report = server
        .join()
        .expect("listen thread panicked")
        .expect("listen returned an error");

    assert_eq!(report.sessions, 2, "two admin sessions");
    assert_eq!(
        (report.tick, report.checksum.as_str()),
        (EXPECTED_FINAL_TICK, EXPECTED_FINAL_CHECKSUM),
        "the scripted admin session diverged from its pinned tick/checksum"
    );
    assert_eq!(
        (report.tick, report.checksum.as_str()),
        (final_tick, final_checksum.as_str()),
        "final report diverged from the last server_status"
    );
    assert!(
        report.saves > 0,
        "the configured save path was never written"
    );
    println!(
        "FOUR_PLAYER_ADMIN tick={} checksum={} roads={} settlements={}",
        report.tick, report.checksum, final_roads, final_settlements
    );

    // Save/reload: same checksum, one road, one released settlement, and the
    // admin resume persisted `paused: false`.
    let loaded = ServerWorld::load_from(&save).expect("load final save");
    assert!(!loaded.report.recovered_from_backup);
    assert_eq!(loaded.world.checksum(), report.checksum);
    assert!(!loaded.world.paused, "the final admin resume must persist");
    assert_eq!(
        loaded
            .world
            .settlements
            .as_ref()
            .map_or(0, |store| store.count()),
        1
    );
    assert!(!temp_path(&save).exists(), "save left a .tmp file behind");
}
