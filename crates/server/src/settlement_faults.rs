//! Deterministic rejection corpus for the authoritative settlement commands.
//!
//! Every case asserts the stable code and that a rejected command leaves the
//! tick, world checksum, settlement-store checksum, and serialized store
//! untouched. No network or wall clock is involved.

use crate::protocol::{ClientCommand, ServerMessage, PROTOCOL_VERSION};
use crate::world::ClientRole;
use crate::world::ServerWorld;
use core_city::settlement::{SettlementRequest, SettlementStore};

const SEED: u64 = 2026;
const CONTENT_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    tick: u64,
    world_checksum: String,
    settlement_checksum: Option<u64>,
    records_json: String,
}

fn snapshot(world: &ServerWorld) -> Snapshot {
    Snapshot {
        tick: world.tick,
        world_checksum: world.checksum(),
        settlement_checksum: world.settlements.as_ref().map(SettlementStore::checksum),
        records_json: serde_json::to_string(&world.settlements).expect("settlements json"),
    }
}

fn hello(name: &str) -> ClientCommand {
    ClientCommand::Hello {
        protocol_version: PROTOCOL_VERSION,
        content_hash: CONTENT_HASH.to_string(),
        player_name: name.to_string(),
    }
}

fn world_with_role(role: ClientRole) -> ServerWorld {
    let mut world = ServerWorld::with_role(SEED, role);
    // Pre-seed the store so a rejected `found_settlement` cannot flip `None`
    // to `Some` and move the world checksum for an unrelated reason.
    world.settlements = Some(SettlementStore::new(SEED));
    let response = world.apply(&hello("mayor"));
    assert!(
        matches!(response, ServerMessage::Welcome { .. }),
        "{response:?}"
    );
    world
}

fn admin_world() -> ServerWorld {
    world_with_role(ClientRole::Admin)
}

fn found(world: &mut ServerWorld, name: &str) -> u64 {
    let response = world.apply(&ClientCommand::FoundSettlement {
        name: name.to_string(),
        x: 0,
        z: 0,
        radius_m: 64,
    });
    match response {
        ServerMessage::SettlementFound { settlement, .. } => settlement.id,
        other => panic!("expected settlement_found, got {other:?}"),
    }
}

fn register(world: &mut ServerWorld, name: &str) -> u64 {
    world
        .players
        .register(name)
        .expect("register player")
        .id
        .get()
}

fn assert_rejected(world: &mut ServerWorld, command: ClientCommand, code: &str) {
    let before = snapshot(world);
    let response = world.apply(&command);
    match response {
        ServerMessage::Reject { code: actual, .. } => assert_eq!(actual, code, "{command:?}"),
        other => panic!("expected reject {code}, got {other:?} for {command:?}"),
    }
    assert_eq!(
        snapshot(world),
        before,
        "rejection mutated world for {command:?}"
    );
}

fn unowned(world: &mut ServerWorld) -> u64 {
    let settlement = world
        .settlements
        .as_mut()
        .expect("store")
        .found(&SettlementRequest {
            name: "Unowned".to_string(),
            owner: "nobody".to_string(),
            center: [0, 0],
            radius_m: 64,
        })
        .expect("unowned settlement");
    settlement.id
}

#[test]
fn found_rejects_bad_names_radius_and_duplicates() {
    let mut world = admin_world();
    assert_rejected(
        &mut world,
        ClientCommand::FoundSettlement {
            name: "   ".to_string(),
            x: 0,
            z: 0,
            radius_m: 64,
        },
        "empty_name",
    );
    assert_rejected(
        &mut world,
        ClientCommand::FoundSettlement {
            name: "Harbor".to_string(),
            x: 0,
            z: 0,
            radius_m: 15,
        },
        "invalid_radius",
    );
    assert_rejected(
        &mut world,
        ClientCommand::FoundSettlement {
            name: "Harbor".to_string(),
            x: 0,
            z: 0,
            radius_m: 2001,
        },
        "invalid_radius",
    );
    assert_rejected(
        &mut world,
        ClientCommand::FoundSettlement {
            name: "Harbor".to_string(),
            x: 1_000_001,
            z: 0,
            radius_m: 64,
        },
        "coordinate_out_of_range",
    );
    found(&mut world, "Harbor");
    assert_rejected(
        &mut world,
        ClientCommand::FoundSettlement {
            name: " harbor ".to_string(),
            x: 0,
            z: 0,
            radius_m: 64,
        },
        "duplicate_name",
    );
}

#[test]
fn found_requires_the_mayor_role() {
    let mut world = world_with_role(ClientRole::Viewer);
    assert_rejected(
        &mut world,
        ClientCommand::FoundSettlement {
            name: "Harbor".to_string(),
            x: 0,
            z: 0,
            radius_m: 64,
        },
        "role.forbidden",
    );
}

#[test]
fn list_and_at_are_viewer_reads() {
    let mut world = world_with_role(ClientRole::Viewer);
    assert!(matches!(
        world.apply(&ClientCommand::SettlementList {
            after_id: None,
            limit: None
        }),
        ServerMessage::SettlementList { .. }
    ));
    assert!(matches!(
        world.apply(&ClientCommand::SettlementAt {
            x: 1_000_000,
            z: -1_000_000
        }),
        ServerMessage::SettlementAt { .. }
    ));
}

#[test]
fn invite_rejects_unknown_ids_and_bad_roles() {
    let mut world = admin_world();
    let id = found(&mut world, "Harbor");
    let bravo = register(&mut world, "bravo");
    assert_rejected(
        &mut world,
        ClientCommand::SettlementInvite {
            settlement_id: 999,
            player_id: bravo,
            role: "builder".to_string(),
            expected_revision: 1,
        },
        "settlement.unknown",
    );
    assert_rejected(
        &mut world,
        ClientCommand::SettlementInvite {
            settlement_id: id,
            player_id: 0,
            role: "builder".to_string(),
            expected_revision: 1,
        },
        "settlement.identity",
    );
    assert_rejected(
        &mut world,
        ClientCommand::SettlementInvite {
            settlement_id: id,
            player_id: bravo,
            role: "owner".to_string(),
            expected_revision: 1,
        },
        "invalid_record",
    );
    assert_rejected(
        &mut world,
        ClientCommand::SettlementInvite {
            settlement_id: id,
            player_id: bravo,
            role: "emperor".to_string(),
            expected_revision: 1,
        },
        "settlement.role_invalid",
    );
}

#[test]
fn membership_cannot_target_or_demote_the_owner() {
    let mut world = admin_world();
    let id = found(&mut world, "Harbor");
    assert_rejected(
        &mut world,
        ClientCommand::SettlementSetRole {
            settlement_id: id,
            player_id: 1,
            role: "member".to_string(),
            expected_revision: 1,
        },
        "settlement.owner_required",
    );
    assert_rejected(
        &mut world,
        ClientCommand::SettlementRemoveMember {
            settlement_id: id,
            player_id: 1,
            expected_revision: 1,
        },
        "settlement.owner_required",
    );
}

#[test]
fn claim_rejects_owned_and_stale_revisions() {
    let mut world = admin_world();
    let id = found(&mut world, "Harbor");
    assert_rejected(
        &mut world,
        ClientCommand::SettlementClaim {
            settlement_id: id,
            expected_revision: 1,
        },
        "settlement.already_owned",
    );
    assert_rejected(
        &mut world,
        ClientCommand::SettlementClaim {
            settlement_id: id,
            expected_revision: 0,
        },
        "settlement.revision_conflict",
    );
}

#[test]
fn rename_rejects_empty_duplicate_and_stale() {
    let mut world = admin_world();
    let id = found(&mut world, "Harbor");
    found(&mut world, "Riverside");
    assert_rejected(
        &mut world,
        ClientCommand::SettlementRename {
            settlement_id: id,
            name: "  ".to_string(),
            expected_revision: 1,
        },
        "empty_name",
    );
    assert_rejected(
        &mut world,
        ClientCommand::SettlementRename {
            settlement_id: id,
            name: "riverside".to_string(),
            expected_revision: 1,
        },
        "duplicate_name",
    );
    assert_rejected(
        &mut world,
        ClientCommand::SettlementRename {
            settlement_id: id,
            name: "Harbor".to_string(),
            expected_revision: 0,
        },
        "settlement.revision_conflict",
    );
}

#[test]
fn transfer_rejects_unknown_player_and_stale() {
    let mut world = admin_world();
    let id = found(&mut world, "Harbor");
    let bravo = register(&mut world, "bravo");
    assert_rejected(
        &mut world,
        ClientCommand::SettlementTransfer {
            settlement_id: id,
            player_id: 999,
            expected_revision: 1,
        },
        "settlement.unknown_player",
    );
    assert_rejected(
        &mut world,
        ClientCommand::SettlementTransfer {
            settlement_id: id,
            player_id: bravo,
            expected_revision: 0,
        },
        "settlement.revision_conflict",
    );
}

#[test]
fn release_requires_admin_and_an_owned_record() {
    let mut mayor = world_with_role(ClientRole::Mayor);
    let id = found(&mut mayor, "Harbor");
    assert_rejected(
        &mut mayor,
        ClientCommand::SettlementRelease {
            settlement_id: id,
            expected_revision: 1,
        },
        "role.forbidden",
    );

    let mut admin = admin_world();
    let unowned_id = unowned(&mut admin);
    assert_rejected(
        &mut admin,
        ClientCommand::SettlementRelease {
            settlement_id: unowned_id,
            expected_revision: 0,
        },
        "settlement.unowned",
    );
}

#[test]
fn tax_rejects_range_permission_and_unowned() {
    let mut world = admin_world();
    let id = found(&mut world, "Harbor");
    assert_rejected(
        &mut world,
        ClientCommand::SettlementSetTax {
            settlement_id: id,
            rate: 101,
        },
        "settlement.tax_out_of_range",
    );
    assert_rejected(
        &mut world,
        ClientCommand::SettlementSetTax {
            settlement_id: 999,
            rate: 50,
        },
        "settlement.unknown",
    );

    let mut unowned_world = admin_world();
    let unowned_id = unowned(&mut unowned_world);
    assert_rejected(
        &mut unowned_world,
        ClientCommand::SettlementSetTax {
            settlement_id: unowned_id,
            rate: 50,
        },
        "settlement.unowned",
    );
}
