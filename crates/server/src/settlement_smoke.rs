//! Deterministic three-connection settlement loopback smoke.
//!
//! The harness binds one local listener on port 0 and drives the merged D-033
//! settlement protocol through the real authoritative executor. Two primary
//! clients (`alpha` and `bravo`, both **Mayor**) and one admin session
//! (`root`, **Admin**) share a single [`ServerWorld`]; every connection is
//! installed with its role through [`ServerWorld::open_connection`] and
//! [`ServerWorld::set_session_role`] before its first command, so the roles
//! are per-session even though the settlement store is shared.
//!
//! # Sequence
//!
//! 1. `alpha` founds *Harbour* at `(10, -20)` with a 64 m radius.
//! 2. Both clients list settlements; `bravo` reads `settlement_at` at the
//!    centre and far away.
//! 3. `alpha` invites `bravo` as builder, sets the role to member, removes
//!    him, removes him again (`settlement.not_member`), and re-invites him.
//! 4. The builder's rename is `settlement.forbidden`; `alpha` renames the
//!    settlement and transfers ownership to `bravo`; `bravo` renames it.
//! 5. `alpha`'s claim is `settlement.already_owned`; `bravo` sets tax 12 and
//!    rejects tax 101 (`settlement.tax_out_of_range`).
//! 6. `alpha`'s release is `role.forbidden` (Mayor), then the admin session
//!    releases the settlement; `bravo`'s tax is `settlement.unowned`.
//! 7. `bravo` claims the unowned settlement, then stale revision, invalid
//!    role, unknown settlement, and unknown recipient commands reject with
//!    their stable codes; the final membership edit leaves `bravo` as owner.
//!
//! Every accepted `settlement_found` / `settlement_updated` response is
//! checked for its full record, a 16-hex `settlement_checksum`, and a 16-hex
//! `world_checksum`; every mutation must move both checksums, and read-only
//! lists must report the latest settlement checksum. No command advances the
//! tick, so every response is at tick 0.
//!
//! After the sessions close, the server thread verifies the authoritative
//! store, compares the final checksums with the client transcript (proving the
//! read-only tail did not move the world), and round-trips the world through
//! [`ServerWorld::to_save_json_pretty`] and
//! [`ServerWorld::from_save_json`]. The report is deterministic: no sleeps,
//! no wall clock, and a fixed seed.
//!
//! The module is test scaffolding only: it does not change the protocol, the
//! world, or persistence.

use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

use core_city::player::PlayerId;
use core_city::settlement::SettlementRole;

use crate::protocol::{
    configure_stream, decode_client_command, frame_to_io, read_frame, write_json_frame,
    ClientCommand, FrameError, ServerMessage, SettlementMemberView, SettlementView, LOOPBACK_SEED,
    PROTOCOL_VERSION, SESSION_IO_TIMEOUT,
};
use crate::world::{codes, ClientRole, ServerWorld};

/// Round-trip content hash carried by every hello in the smoke.
const CONTENT_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// Display name and deterministic registry id for the founding player.
const ALPHA: &str = "alpha";
const ALPHA_ID: u64 = 1;

/// Display name and deterministic registry id for the second player.
const BRAVO: &str = "bravo";
const BRAVO_ID: u64 = 2;

/// Display name for the admin session; its id is registered third.
const ROOT: &str = "root";

/// Settlement id assigned to the first founded record.
const SETTLEMENT_ID: u64 = 1;

/// Settlement centre in whole world metres.
const CENTER: [i32; 2] = [10, -20];

/// Settlement soft radius in metres.
const RADIUS_M: u32 = 64;

/// Recipient id that is deliberately absent from the player registry.
const UNKNOWN_PLAYER_ID: u64 = 424_242;

/// Role permission bits on the wire: owner/manager, builder, member.
const PERMISSIONS_ALL: u8 = 0b1111;
const PERMISSIONS_BUILD: u8 = 0b0001;
const PERMISSIONS_NONE: u8 = 0b0000;

/// Result of one deterministic settlement loopback run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementSmokeReport {
    /// Server responses asserted by the client transcript (welcomes, reads,
    /// mutations, rejections, and goodbyes).
    pub messages: u64,
    /// Settlement records in the final authoritative store.
    pub settlements: u64,
    /// Members in the final settlement record.
    pub members: u64,
    /// FNV-1a over the ordered transcript events.
    pub checksum: String,
    /// Final authoritative world checksum.
    pub world_checksum: String,
}

/// Run the deterministic three-connection settlement loopback over TCP.
///
/// # Errors
///
/// Returns an error when the listener, a socket, or an assertion in the
/// harness fails. No wall-clock sleeps are used; every ordering point is a
/// request/response pair, so a passing run is reproducible.
pub fn run_settlement_smoke() -> io::Result<SettlementSmokeReport> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let address = listener.local_addr()?;

    let server = thread::spawn(move || serve_settlement(listener));
    let client_result = run_settlement_client(address);
    let server_result = server
        .join()
        .map_err(|_| io::Error::other("settlement loopback server thread panicked"))?;

    let client = client_result?;
    let server = server_result?;

    if client.final_settlement_checksum != server.settlement_checksum {
        return Err(io::Error::other(format!(
            "settlement checksum diverged: client {} != server {}",
            client.final_settlement_checksum, server.settlement_checksum
        )));
    }
    // The last mutation is followed by read-only lists, `settlement_at`
    // reads, and disconnects. Equal final checksums prove that tail did not
    // move authoritative state.
    if client.final_world_checksum != server.world_checksum {
        return Err(io::Error::other(format!(
            "world checksum diverged: client {} != server {}",
            client.final_world_checksum, server.world_checksum
        )));
    }

    Ok(SettlementSmokeReport {
        messages: client.messages,
        settlements: server.settlements,
        members: server.members,
        checksum: client.checksum,
        world_checksum: server.world_checksum,
    })
}

struct SettlementServerOutcome {
    settlements: u64,
    members: u64,
    settlement_checksum: String,
    world_checksum: String,
}

fn serve_settlement(listener: TcpListener) -> io::Result<SettlementServerOutcome> {
    let world = Arc::new(Mutex::new(ServerWorld::with_role(
        LOOPBACK_SEED,
        ClientRole::Viewer,
    )));
    // The client connects and completes each hello before opening the next
    // connection, so accept order is deterministic.
    let roles = [ClientRole::Mayor, ClientRole::Mayor, ClientRole::Admin];
    let mut handles = Vec::with_capacity(roles.len());

    for role in roles {
        let (stream, _peer) = listener.accept()?;
        let connection_id = {
            let mut world = world
                .lock()
                .map_err(|_| io::Error::other("settlement world mutex poisoned"))?;
            let connection_id = world.open_connection();
            world.set_session_role(connection_id, role);
            connection_id
        };
        let shared = Arc::clone(&world);
        handles.push(thread::spawn(move || {
            serve_settlement_stream(stream, shared, connection_id)
        }));
    }
    for handle in handles {
        handle
            .join()
            .map_err(|_| io::Error::other("settlement session thread panicked"))??;
    }

    let world = world
        .lock()
        .map_err(|_| io::Error::other("settlement world mutex poisoned"))?;
    if world.tick != 0 {
        return Err(io::Error::other(format!(
            "settlement smoke moved the tick to {}",
            world.tick
        )));
    }
    if world.players.count() != 3 {
        return Err(io::Error::other(format!(
            "settlement smoke expected 3 registered players, got {}",
            world.players.count()
        )));
    }
    let bravo = PlayerId::new(BRAVO_ID).map_err(|error| io::Error::other(error.to_string()))?;
    if world
        .players
        .get(bravo)
        .map(|player| player.display_name.as_str())
        != Some(BRAVO)
    {
        return Err(io::Error::other(
            "settlement smoke expected bravo to hold player id 2",
        ));
    }

    let Some(store) = world.settlements.as_ref() else {
        return Err(io::Error::other(
            "settlement smoke left no settlement store",
        ));
    };
    if store.seed != world.seed {
        return Err(io::Error::other(format!(
            "settlement store seed {} does not match world seed {}",
            store.seed, world.seed
        )));
    }
    if store.count() != 1 {
        return Err(io::Error::other(format!(
            "settlement smoke expected 1 settlement, got {}",
            store.count()
        )));
    }
    let settlement = store.get(SETTLEMENT_ID).ok_or_else(|| {
        io::Error::other(format!(
            "settlement smoke expected settlement {SETTLEMENT_ID}"
        ))
    })?;
    if settlement.name != "Bravos Rest"
        || settlement.owner_id != Some(bravo)
        || !settlement.is_owned()
        || settlement.center != CENTER
        || settlement.radius_m != RADIUS_M
        || settlement.membership_revision != 12
        || settlement.tax_rate != 12
        || settlement.members.len() != 1
        || settlement.role_of(bravo) != Some(SettlementRole::Owner)
    {
        return Err(io::Error::other(format!(
            "settlement smoke final record is unexpected: {settlement:?}"
        )));
    }

    let settlement_checksum = format!("{:016x}", store.checksum());
    let world_checksum = world.checksum();

    // Save/load round-trip of the resulting store. The JSON path exercises the
    // same validation and optional settlement serialization as the file path,
    // without a filesystem dependency.
    let json = world
        .to_save_json_pretty()
        .map_err(|error| io::Error::other(format!("settlement save failed: {error}")))?;
    let loaded = ServerWorld::from_save_json(&json)
        .map_err(|error| io::Error::other(format!("settlement load failed: {error}")))?;
    if loaded.tick != world.tick || loaded.checksum() != world_checksum {
        return Err(io::Error::other(
            "settlement save/load changed the tick or world checksum",
        ));
    }
    let Some(loaded_store) = loaded.settlements.as_ref() else {
        return Err(io::Error::other(
            "settlement save/load dropped the settlement store",
        ));
    };
    if loaded_store.checksum() != store.checksum()
        || loaded_store.get(SETTLEMENT_ID) != Some(settlement)
    {
        return Err(io::Error::other(
            "settlement save/load changed the settlement record",
        ));
    }

    Ok(SettlementServerOutcome {
        settlements: u64::try_from(store.count()).unwrap_or(u64::MAX),
        members: u64::try_from(settlement.members.len()).unwrap_or(u64::MAX),
        settlement_checksum,
        world_checksum,
    })
}

fn serve_settlement_stream(
    stream: TcpStream,
    world: Arc<Mutex<ServerWorld>>,
    connection_id: u64,
) -> io::Result<()> {
    configure_stream(&stream)?;
    let mut stream = stream;

    loop {
        let payload = match read_frame(&mut stream) {
            Ok(payload) => payload,
            Err(FrameError::Truncated { .. }) | Err(FrameError::Io(_)) => break,
            Err(error) => return Err(io::Error::other(error.to_string())),
        };

        let (responses, closed) = match decode_client_command(&payload) {
            Ok(command) => {
                let mut world = world
                    .lock()
                    .map_err(|_| io::Error::other("settlement world mutex poisoned"))?;
                let responses = world.apply_session(connection_id, &command);
                let closed = world.session_closed(connection_id);
                (responses, closed)
            }
            Err(rejection) => (vec![rejection.into_message()], false),
        };
        for response in &responses {
            write_json_frame(&mut stream, response).map_err(frame_to_io)?;
        }

        if closed {
            break;
        }
    }

    world
        .lock()
        .map_err(|_| io::Error::other("settlement world mutex poisoned"))?
        .close_connection(connection_id);

    Ok(())
}

struct SettlementClient {
    stream: TcpStream,
}

impl SettlementClient {
    fn connect(
        address: SocketAddr,
        player: &str,
        transcript: &mut SettlementTranscript,
    ) -> io::Result<Self> {
        let stream = TcpStream::connect_timeout(&address, SESSION_IO_TIMEOUT)?;
        configure_stream(&stream)?;
        let mut client = Self { stream };
        let welcome = client.send(&ClientCommand::Hello {
            protocol_version: PROTOCOL_VERSION,
            content_hash: CONTENT_HASH.to_string(),
            player_name: player.to_string(),
        })?;
        transcript.welcome(welcome)?;
        Ok(client)
    }

    fn send(&mut self, command: &ClientCommand) -> io::Result<ServerMessage> {
        write_json_frame(&mut self.stream, command).map_err(frame_to_io)?;
        crate::protocol::read_json_frame(&mut self.stream).map_err(frame_to_io)
    }

    fn goodbye(mut self, transcript: &mut SettlementTranscript) -> io::Result<()> {
        let response = self.send(&ClientCommand::Disconnect)?;
        transcript.goodbye(response)
    }
}

struct SettlementClientOutcome {
    messages: u64,
    checksum: String,
    final_settlement_checksum: String,
    final_world_checksum: String,
}

#[derive(Default)]
struct SettlementTranscript {
    messages: u64,
    events: Vec<String>,
    last_settlement_checksum: String,
    last_world_checksum: String,
}

impl SettlementTranscript {
    fn welcome(&mut self, message: ServerMessage) -> io::Result<()> {
        match message {
            ServerMessage::Welcome {
                tick: 0,
                protocol_version,
                resumed: false,
                ..
            } if protocol_version == PROTOCOL_VERSION => {}
            other => {
                return Err(io::Error::other(format!(
                    "settlement hello expected a fresh welcome at tick 0, got {other:?}"
                )))
            }
        }
        self.record("welcome");
        Ok(())
    }

    fn mutation(
        &mut self,
        tag: &str,
        message: ServerMessage,
        expected: &SettlementView,
    ) -> io::Result<()> {
        let (settlement, settlement_checksum, world_checksum) = match message {
            ServerMessage::SettlementFound {
                tick: 0,
                settlement,
                settlement_checksum,
                world_checksum,
            }
            | ServerMessage::SettlementUpdated {
                tick: 0,
                settlement,
                settlement_checksum,
                world_checksum,
            } => (settlement, settlement_checksum, world_checksum),
            other => {
                return Err(io::Error::other(format!(
                    "settlement `{tag}` expected settlement_found/settlement_updated at tick 0, got {other:?}"
                )))
            }
        };
        if settlement != *expected {
            return Err(io::Error::other(format!(
                "settlement `{tag}` expected {expected:?}, got {settlement:?}"
            )));
        }
        expect_checksum("settlement_checksum", &settlement_checksum)?;
        expect_checksum("world_checksum", &world_checksum)?;
        if !self.last_settlement_checksum.is_empty() {
            if settlement_checksum == self.last_settlement_checksum {
                return Err(io::Error::other(format!(
                    "settlement `{tag}` did not change the settlement checksum {settlement_checksum}"
                )));
            }
            if world_checksum == self.last_world_checksum {
                return Err(io::Error::other(format!(
                    "settlement `{tag}` did not change the world checksum {world_checksum}"
                )));
            }
        }
        self.last_settlement_checksum = settlement_checksum.clone();
        self.last_world_checksum = world_checksum.clone();
        self.record(&format!(
            "{tag}:{settlement_checksum}:{world_checksum}:{}:{}",
            settlement.membership_revision, settlement.tax_rate
        ));
        Ok(())
    }

    fn list(&mut self, message: ServerMessage, expected: &SettlementView) -> io::Result<()> {
        let (count, checksum, settlements) = match message {
            ServerMessage::SettlementList {
                tick: 0,
                count,
                checksum,
                settlements,
                ..
            } => (count, checksum, settlements),
            other => {
                return Err(io::Error::other(format!(
                    "settlement_list expected a read at tick 0, got {other:?}"
                )))
            }
        };
        if count != 1 || settlements.as_slice() != std::slice::from_ref(expected) {
            return Err(io::Error::other(format!(
                "settlement_list expected one record {expected:?}, got {settlements:?}"
            )));
        }
        if checksum != self.last_settlement_checksum {
            return Err(io::Error::other(format!(
                "settlement_list checksum {checksum} != latest mutation {}",
                self.last_settlement_checksum
            )));
        }
        self.record(&format!("list:{checksum}:{count}"));
        Ok(())
    }

    fn at(&mut self, message: ServerMessage, expected: Option<&SettlementView>) -> io::Result<()> {
        let (found, settlement) = match message {
            ServerMessage::SettlementAt {
                tick: 0,
                found,
                settlement,
            } => (found, settlement),
            other => {
                return Err(io::Error::other(format!(
                    "settlement_at expected a read at tick 0, got {other:?}"
                )))
            }
        };
        if found != expected.is_some() || settlement.as_ref() != expected {
            return Err(io::Error::other(format!(
                "settlement_at expected found={} {expected:?}, got found={found} {settlement:?}",
                expected.is_some()
            )));
        }
        self.record(&format!(
            "at:{found}:{}",
            settlement
                .as_ref()
                .map_or("none", |settlement| settlement.name.as_str())
        ));
        Ok(())
    }

    fn reject(&mut self, message: ServerMessage, code: &str) -> io::Result<()> {
        match message {
            ServerMessage::Reject {
                code: actual_code, ..
            } if actual_code == code => {}
            other => {
                return Err(io::Error::other(format!(
                    "settlement expected rejection `{code}`, got {other:?}"
                )))
            }
        }
        self.record(&format!("reject:{code}"));
        Ok(())
    }

    fn goodbye(&mut self, message: ServerMessage) -> io::Result<()> {
        match message {
            ServerMessage::Goodbye => {}
            other => {
                return Err(io::Error::other(format!(
                    "settlement expected goodbye, got {other:?}"
                )))
            }
        }
        self.record("goodbye");
        Ok(())
    }

    fn record(&mut self, event: &str) {
        self.messages = self.messages.saturating_add(1);
        self.events.push(event.to_string());
    }

    fn checksum(&self) -> String {
        let mut hash = FNV_OFFSET;
        for event in &self.events {
            hash = fnv_update(hash, event.as_bytes());
            hash = fnv_update(hash, &[0x1f]);
        }
        format!("{hash:016x}")
    }
}

fn run_settlement_client(address: SocketAddr) -> io::Result<SettlementClientOutcome> {
    let mut transcript = SettlementTranscript::default();
    let mut alpha = SettlementClient::connect(address, ALPHA, &mut transcript)?;
    let mut bravo = SettlementClient::connect(address, BRAVO, &mut transcript)?;
    let mut root = SettlementClient::connect(address, ROOT, &mut transcript)?;

    // 1. Alpha founds the settlement. The first owned record upgrades the store
    //    to v2 and gives alpha the owner role at revision 1.
    let mut expected = base_view();
    transcript.mutation(
        "found",
        alpha.send(&ClientCommand::FoundSettlement {
            name: "Harbour".to_string(),
            x: CENTER[0],
            z: CENTER[1],
            radius_m: RADIUS_M,
        })?,
        &expected,
    )?;

    // 2. Both clients read the list; bravo reads the containing cell and a far
    //    cell. None of these advance the tick or move the checksums.
    transcript.list(
        alpha.send(&ClientCommand::SettlementList {
            after_id: None,
            limit: None,
        })?,
        &expected,
    )?;
    transcript.list(
        bravo.send(&ClientCommand::SettlementList {
            after_id: None,
            limit: None,
        })?,
        &expected,
    )?;
    transcript.at(
        bravo.send(&ClientCommand::SettlementAt {
            x: CENTER[0],
            z: CENTER[1],
        })?,
        Some(&expected),
    )?;
    transcript.at(
        bravo.send(&ClientCommand::SettlementAt { x: 5_000, z: 5_000 })?,
        None,
    )?;

    // 3. Invite, demote, remove, reject a second remove, then re-invite.
    expected.members.push(member(BRAVO_ID, "builder"));
    expected.membership_revision = 2;
    transcript.mutation(
        "invite",
        alpha.send(&ClientCommand::SettlementInvite {
            settlement_id: SETTLEMENT_ID,
            player_id: BRAVO_ID,
            role: "builder".to_string(),
            expected_revision: 1,
        })?,
        &expected,
    )?;

    expected.members[1] = member(BRAVO_ID, "member");
    expected.membership_revision = 3;
    transcript.mutation(
        "set_role",
        alpha.send(&ClientCommand::SettlementSetRole {
            settlement_id: SETTLEMENT_ID,
            player_id: BRAVO_ID,
            role: "member".to_string(),
            expected_revision: 2,
        })?,
        &expected,
    )?;

    expected.members.pop();
    expected.membership_revision = 4;
    transcript.mutation(
        "remove",
        alpha.send(&ClientCommand::SettlementRemoveMember {
            settlement_id: SETTLEMENT_ID,
            player_id: BRAVO_ID,
            expected_revision: 3,
        })?,
        &expected,
    )?;
    transcript.reject(
        alpha.send(&ClientCommand::SettlementRemoveMember {
            settlement_id: SETTLEMENT_ID,
            player_id: BRAVO_ID,
            expected_revision: 4,
        })?,
        codes::SETTLEMENT_NOT_MEMBER,
    )?;

    expected.members.push(member(BRAVO_ID, "builder"));
    expected.membership_revision = 5;
    transcript.mutation(
        "reinvite",
        alpha.send(&ClientCommand::SettlementInvite {
            settlement_id: SETTLEMENT_ID,
            player_id: BRAVO_ID,
            role: "builder".to_string(),
            expected_revision: 4,
        })?,
        &expected,
    )?;

    // 4. A builder lacks `manage`: the rename rejects at the core permission
    //    check after the revision matches.
    transcript.reject(
        bravo.send(&ClientCommand::SettlementRename {
            settlement_id: SETTLEMENT_ID,
            name: "Bravos Rest".to_string(),
            expected_revision: 5,
        })?,
        codes::SETTLEMENT_FORBIDDEN,
    )?;

    expected.name = "Portsmith".to_string();
    expected.membership_revision = 6;
    transcript.mutation(
        "rename",
        alpha.send(&ClientCommand::SettlementRename {
            settlement_id: SETTLEMENT_ID,
            name: "Portsmith".to_string(),
            expected_revision: 5,
        })?,
        &expected,
    )?;

    expected.owner_id = Some(BRAVO_ID);
    expected.members[0] = member(ALPHA_ID, "manager");
    expected.members[1] = member(BRAVO_ID, "owner");
    expected.membership_revision = 7;
    transcript.mutation(
        "transfer",
        alpha.send(&ClientCommand::SettlementTransfer {
            settlement_id: SETTLEMENT_ID,
            player_id: BRAVO_ID,
            expected_revision: 6,
        })?,
        &expected,
    )?;

    expected.name = "Bravos Rest".to_string();
    expected.membership_revision = 8;
    transcript.mutation(
        "rename",
        bravo.send(&ClientCommand::SettlementRename {
            settlement_id: SETTLEMENT_ID,
            name: "Bravos Rest".to_string(),
            expected_revision: 7,
        })?,
        &expected,
    )?;

    // 5. Claiming an owned record and an out-of-range tax rate.
    transcript.reject(
        alpha.send(&ClientCommand::SettlementClaim {
            settlement_id: SETTLEMENT_ID,
            expected_revision: 8,
        })?,
        codes::SETTLEMENT_ALREADY_OWNED,
    )?;

    expected.tax_rate = 12;
    transcript.mutation(
        "tax",
        bravo.send(&ClientCommand::SettlementSetTax {
            settlement_id: SETTLEMENT_ID,
            rate: 12,
        })?,
        &expected,
    )?;
    transcript.reject(
        bravo.send(&ClientCommand::SettlementSetTax {
            settlement_id: SETTLEMENT_ID,
            rate: 101,
        })?,
        codes::SETTLEMENT_TAX_OUT_OF_RANGE,
    )?;

    // 6. Release is above the Mayor ceiling; the admin session performs the
    //    Q-113 recovery. The released record keeps its tax and legacy label.
    transcript.reject(
        alpha.send(&ClientCommand::SettlementRelease {
            settlement_id: SETTLEMENT_ID,
            expected_revision: 8,
        })?,
        codes::FORBIDDEN,
    )?;

    expected.owner_id = None;
    expected.owned = false;
    expected.members.clear();
    expected.membership_revision = 9;
    transcript.mutation(
        "release",
        root.send(&ClientCommand::SettlementRelease {
            settlement_id: SETTLEMENT_ID,
            expected_revision: 8,
        })?,
        &expected,
    )?;
    transcript.reject(
        bravo.send(&ClientCommand::SettlementSetTax {
            settlement_id: SETTLEMENT_ID,
            rate: 7,
        })?,
        codes::SETTLEMENT_UNOWNED,
    )?;

    // 7. Bravo claims the released settlement, then the remaining stable
    //    rejection paths and a final membership edit.
    expected.owner_id = Some(BRAVO_ID);
    expected.owned = true;
    expected.members.push(member(BRAVO_ID, "owner"));
    expected.membership_revision = 10;
    transcript.mutation(
        "claim",
        bravo.send(&ClientCommand::SettlementClaim {
            settlement_id: SETTLEMENT_ID,
            expected_revision: 9,
        })?,
        &expected,
    )?;

    transcript.reject(
        alpha.send(&ClientCommand::SettlementInvite {
            settlement_id: 999,
            player_id: BRAVO_ID,
            role: "builder".to_string(),
            expected_revision: 10,
        })?,
        codes::SETTLEMENT_UNKNOWN,
    )?;
    transcript.reject(
        bravo.send(&ClientCommand::SettlementInvite {
            settlement_id: SETTLEMENT_ID,
            player_id: ALPHA_ID,
            role: "builder".to_string(),
            expected_revision: 9,
        })?,
        codes::SETTLEMENT_REVISION_CONFLICT,
    )?;
    transcript.reject(
        bravo.send(&ClientCommand::SettlementInvite {
            settlement_id: SETTLEMENT_ID,
            player_id: ALPHA_ID,
            role: "superuser".to_string(),
            expected_revision: 10,
        })?,
        codes::SETTLEMENT_ROLE_INVALID,
    )?;
    transcript.reject(
        bravo.send(&ClientCommand::SettlementTransfer {
            settlement_id: SETTLEMENT_ID,
            player_id: UNKNOWN_PLAYER_ID,
            expected_revision: 10,
        })?,
        codes::SETTLEMENT_UNKNOWN_PLAYER,
    )?;

    expected.members.insert(0, member(ALPHA_ID, "member"));
    expected.membership_revision = 11;
    transcript.mutation(
        "invite",
        bravo.send(&ClientCommand::SettlementInvite {
            settlement_id: SETTLEMENT_ID,
            player_id: ALPHA_ID,
            role: "member".to_string(),
            expected_revision: 10,
        })?,
        &expected,
    )?;

    expected.members.remove(0);
    expected.membership_revision = 12;
    transcript.mutation(
        "remove",
        bravo.send(&ClientCommand::SettlementRemoveMember {
            settlement_id: SETTLEMENT_ID,
            player_id: ALPHA_ID,
            expected_revision: 11,
        })?,
        &expected,
    )?;
    transcript.reject(
        bravo.send(&ClientCommand::SettlementRemoveMember {
            settlement_id: SETTLEMENT_ID,
            player_id: ALPHA_ID,
            expected_revision: 12,
        })?,
        codes::SETTLEMENT_NOT_MEMBER,
    )?;

    // The read-only tail after the last mutation pins the checksums.
    transcript.list(
        bravo.send(&ClientCommand::SettlementList {
            after_id: None,
            limit: None,
        })?,
        &expected,
    )?;
    transcript.at(
        bravo.send(&ClientCommand::SettlementAt {
            x: CENTER[0],
            z: CENTER[1],
        })?,
        Some(&expected),
    )?;

    alpha.goodbye(&mut transcript)?;
    bravo.goodbye(&mut transcript)?;
    root.goodbye(&mut transcript)?;

    Ok(SettlementClientOutcome {
        messages: transcript.messages,
        checksum: transcript.checksum(),
        final_settlement_checksum: transcript.last_settlement_checksum,
        final_world_checksum: transcript.last_world_checksum,
    })
}

fn base_view() -> SettlementView {
    SettlementView {
        id: SETTLEMENT_ID,
        name: "Harbour".to_string(),
        owner: ALPHA.to_string(),
        owner_id: Some(ALPHA_ID),
        owned: true,
        center: CENTER,
        radius_m: RADIUS_M,
        members: vec![member(ALPHA_ID, "owner")],
        membership_revision: 1,
        tax_rate: 0,
    }
}

fn member(player: u64, role: &str) -> SettlementMemberView {
    let permissions = match role {
        "owner" | "manager" => PERMISSIONS_ALL,
        "builder" => PERMISSIONS_BUILD,
        _ => PERMISSIONS_NONE,
    };
    SettlementMemberView {
        player,
        role: role.to_string(),
        permissions,
    }
}

fn expect_checksum(field: &str, value: &str) -> io::Result<()> {
    let valid = value.len() == 16
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
    if valid {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "settlement {field} `{value}` is not 16 lowercase hex digits"
        )))
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settlement_loopback_is_deterministic() {
        let first = run_settlement_smoke().expect("first settlement loopback");
        let second = run_settlement_smoke().expect("second settlement loopback");

        assert_eq!(first.messages, 36);
        assert_eq!(first.settlements, 1);
        assert_eq!(first.members, 1);
        assert_eq!(first.checksum.len(), 16);
        assert!(first
            .checksum
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')));
        assert_eq!(first.world_checksum.len(), 16);
        assert_eq!(first, second, "two runs must produce the same report");
    }
}
