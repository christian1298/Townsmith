//! Fault matrix for the local settlement mutation helpers.
//!
//! These are pure-helper tests: no Godot engine, no TCP, and no bridge
//! instance. Every rejection must leave the store byte-for-byte unchanged and
//! every success must return the record that is actually stored.

use core_city::player::PlayerId;
use core_city::settlement::{Settlement, SettlementRequest, SettlementRole, SettlementStore};

use super::{
    settlement_claim_in_store, settlement_invite_in_store, settlement_mutation_args,
    settlement_player_arg, settlement_release_in_store, settlement_remove_member_in_store,
    settlement_rename_args, settlement_rename_in_store, settlement_role_arg,
    settlement_set_role_in_store, settlement_set_tax_in_store, settlement_transfer_in_store,
};

const SEED: u64 = 2026;

fn player(id: u64) -> PlayerId {
    PlayerId::new(id).expect("valid player id")
}

fn request(name: &str, owner: &str, x: i32) -> SettlementRequest {
    SettlementRequest {
        name: name.to_string(),
        owner: owner.to_string(),
        center: [x, 0],
        radius_m: 64,
    }
}

fn owned_store() -> SettlementStore {
    let mut store = SettlementStore::new(SEED);
    store
        .found_for_player(&request("Harbor", "Mayor", 0), player(1))
        .expect("found Harbor");
    store
}

fn unowned_store() -> SettlementStore {
    let mut store = SettlementStore::new(SEED);
    store
        .found(&request("Harbor", "Mayor", 0))
        .expect("found unowned Harbor");
    store
}

fn assert_rejected<F>(store: &mut SettlementStore, code: &str, call: F)
where
    F: FnOnce(&mut SettlementStore) -> Result<Settlement, (&'static str, String)>,
{
    let before = store.clone();
    let error = call(store).expect_err("expected rejection");
    assert_eq!(error.0, code, "unexpected code: {}", error.1);
    assert_eq!(*store, before, "rejection mutated the store");
}

#[test]
fn argument_parsing_rejects_negative_and_overflowing_values() {
    assert_eq!(
        settlement_rename_args(-1, 0).expect_err("negative id").0,
        "invalid_id"
    );
    assert_eq!(
        settlement_rename_args(1, -1)
            .expect_err("negative revision")
            .0,
        "invalid_revision"
    );
    assert_eq!(
        settlement_mutation_args(1, i64::from(u32::MAX) + 1)
            .expect_err("overflowing revision")
            .0,
        "invalid_revision"
    );
    assert_eq!(
        settlement_player_arg(0).expect_err("zero player").0,
        "invalid_player_id"
    );
    assert_eq!(
        settlement_player_arg(-1).expect_err("negative player").0,
        "invalid_player_id"
    );
    assert_eq!(
        settlement_player_arg(i64::MAX).expect("max player id"),
        player(u64::try_from(i64::MAX).expect("positive"))
    );
    assert_eq!(
        settlement_role_arg("owner").expect_err("owner role").0,
        "settlement.role_invalid"
    );
    assert_eq!(
        settlement_role_arg("emperor").expect_err("unknown role").0,
        "settlement.role_invalid"
    );
    assert_eq!(
        settlement_role_arg("MANAGER").expect("case-insensitive"),
        SettlementRole::Manager
    );
}

#[test]
fn unknown_ids_reject_every_mutation_without_touching_the_store() {
    let mut store = owned_store();
    let missing = 99;
    assert_rejected(&mut store, "settlement.unknown", |store| {
        settlement_rename_in_store(store, missing, "New", 1)
    });
    assert_rejected(&mut store, "settlement.unknown", |store| {
        settlement_claim_in_store(store, missing, player(2), 1)
    });
    assert_rejected(&mut store, "settlement.unknown", |store| {
        settlement_invite_in_store(store, missing, player(2), "member", 1)
    });
    assert_rejected(&mut store, "settlement.unknown", |store| {
        settlement_set_role_in_store(store, missing, player(2), "manager", 1)
    });
    assert_rejected(&mut store, "settlement.unknown", |store| {
        settlement_remove_member_in_store(store, missing, player(2), 1)
    });
    assert_rejected(&mut store, "settlement.unknown", |store| {
        settlement_transfer_in_store(store, missing, player(2), 1)
    });
    assert_rejected(&mut store, "settlement.unknown", |store| {
        settlement_release_in_store(store, missing, 1)
    });
    assert_rejected(&mut store, "settlement.unknown", |store| {
        settlement_set_tax_in_store(store, missing, 5)
    });
}

#[test]
fn stale_revisions_reject_without_mutation() {
    let mut store = owned_store();
    let stale = 2;
    assert_rejected(&mut store, "settlement.revision_conflict", |store| {
        settlement_rename_in_store(store, 1, "New", stale)
    });
    assert_rejected(&mut store, "settlement.revision_conflict", |store| {
        settlement_invite_in_store(store, 1, player(2), "member", stale)
    });
    assert_rejected(&mut store, "settlement.revision_conflict", |store| {
        settlement_set_role_in_store(store, 1, player(2), "manager", stale)
    });
    assert_rejected(&mut store, "settlement.revision_conflict", |store| {
        settlement_remove_member_in_store(store, 1, player(2), stale)
    });
    assert_rejected(&mut store, "settlement.revision_conflict", |store| {
        settlement_transfer_in_store(store, 1, player(2), stale)
    });
    assert_rejected(&mut store, "settlement.revision_conflict", |store| {
        settlement_release_in_store(store, 1, stale)
    });
    assert_rejected(&mut store, "settlement.revision_conflict", |store| {
        settlement_claim_in_store(store, 1, player(2), stale)
    });
}

#[test]
fn unowned_records_reject_owner_gated_actions() {
    let mut store = unowned_store();
    assert_rejected(&mut store, "settlement.unowned", |store| {
        settlement_rename_in_store(store, 1, "New", 0)
    });
    assert_rejected(&mut store, "settlement.unowned", |store| {
        settlement_invite_in_store(store, 1, player(2), "member", 0)
    });
    assert_rejected(&mut store, "settlement.unowned", |store| {
        settlement_transfer_in_store(store, 1, player(2), 0)
    });
    assert_rejected(&mut store, "settlement.unowned", |store| {
        settlement_set_tax_in_store(store, 1, 5)
    });
    assert_rejected(&mut store, "settlement.unowned", |store| {
        settlement_remove_member_in_store(store, 1, player(2), 0)
    });
}

#[test]
fn owner_cannot_be_invited_removed_or_demoted() {
    let mut store = owned_store();
    assert_rejected(&mut store, "settlement.owner_required", |store| {
        settlement_remove_member_in_store(store, 1, player(1), 1)
    });
    assert_rejected(&mut store, "settlement.owner_required", |store| {
        settlement_set_role_in_store(store, 1, player(1), "manager", 1)
    });
    assert_rejected(&mut store, "settlement.role_invalid", |store| {
        settlement_invite_in_store(store, 1, player(2), "owner", 1)
    });
    assert_rejected(&mut store, "invalid_record", |store| {
        settlement_invite_in_store(store, 1, player(1), "manager", 1)
    });
}

#[test]
fn rename_validates_name_and_duplicates_before_revision() {
    let mut store = owned_store();
    store
        .found_for_player(&request("Riverside", "Mayor", 100), player(1))
        .expect("second settlement");

    assert_rejected(&mut store, "empty_name", |store| {
        settlement_rename_in_store(store, 1, "   ", 1)
    });
    assert_rejected(&mut store, "name_too_long", |store| {
        settlement_rename_in_store(store, 1, &"x".repeat(49), 1)
    });
    assert_rejected(&mut store, "invalid_record", |store| {
        settlement_rename_in_store(store, 1, "bad\nname", 1)
    });
    assert_rejected(&mut store, "duplicate_name", |store| {
        settlement_rename_in_store(store, 1, "riverside", 1)
    });
    assert_rejected(&mut store, "settlement.revision_conflict", |store| {
        settlement_rename_in_store(store, 1, "New", 99)
    });

    let before = store.get(1).expect("record").clone();
    let updated = settlement_rename_in_store(&mut store, 1, "Harbor", 1).expect("same name");
    assert_eq!(updated.membership_revision, before.membership_revision + 1);
    assert_eq!(store.get(1), Some(&updated));
}

#[test]
fn tax_is_bounded_and_does_not_bump_revision() {
    let mut store = owned_store();
    assert_rejected(&mut store, "settlement.tax_out_of_range", |store| {
        settlement_set_tax_in_store(store, 1, 101)
    });
    assert_rejected(&mut store, "settlement.tax_out_of_range", |store| {
        settlement_set_tax_in_store(store, 1, 65535)
    });

    let before = store.get(1).expect("record").clone();
    let updated = settlement_set_tax_in_store(&mut store, 1, 100).expect("tax");
    assert_eq!(updated.tax_rate, 100);
    assert_eq!(updated.membership_revision, before.membership_revision);
    assert_eq!(store.get(1), Some(&updated));
}

#[test]
fn claim_rejects_owned_and_claims_unowned_once() {
    let mut owned = owned_store();
    assert_rejected(&mut owned, "settlement.already_owned", |store| {
        settlement_claim_in_store(store, 1, player(2), 1)
    });

    let mut unowned = unowned_store();
    let claimed = settlement_claim_in_store(&mut unowned, 1, player(2), 0).expect("claim");
    assert_eq!(claimed.owner_id, Some(player(2)));
    assert_eq!(claimed.membership_revision, 1);
    assert_rejected(&mut unowned, "settlement.already_owned", |store| {
        settlement_claim_in_store(store, 1, player(3), 1)
    });
}

#[test]
fn invite_role_and_remove_bump_one_revision_per_success() {
    let mut store = owned_store();
    let start = store.get(1).expect("record").membership_revision;

    let invited =
        settlement_invite_in_store(&mut store, 1, player(2), "builder", start).expect("invite");
    assert_eq!(invited.membership_revision, start + 1);

    let promoted = settlement_set_role_in_store(&mut store, 1, player(2), "manager", start + 1)
        .expect("promote");
    assert_eq!(promoted.membership_revision, start + 2);

    let same = settlement_set_role_in_store(&mut store, 1, player(2), "manager", start + 2)
        .expect("same role");
    assert_eq!(
        same.membership_revision,
        start + 2,
        "no-op role keeps revision"
    );

    let removed =
        settlement_remove_member_in_store(&mut store, 1, player(2), start + 2).expect("remove");
    assert_eq!(removed.membership_revision, start + 3);
    assert_rejected(&mut store, "settlement.not_member", |store| {
        settlement_remove_member_in_store(store, 1, player(2), start + 3)
    });
}

#[test]
fn transfer_moves_owner_and_demotes_previous_owner() {
    let mut store = owned_store();
    store
        .invite(player(1), 1, player(2), SettlementRole::Builder, 1)
        .expect("invite");
    let updated = settlement_transfer_in_store(&mut store, 1, player(2), 2).expect("transfer");
    assert_eq!(updated.owner_id, Some(player(2)));
    assert_eq!(updated.membership_revision, 3);
    assert_eq!(
        updated
            .members
            .iter()
            .find(|member| member.player == player(1))
            .expect("old owner")
            .role,
        SettlementRole::Manager
    );

    let self_transfer = settlement_transfer_in_store(&mut store, 1, player(2), 3);
    if let Ok(record) = self_transfer {
        assert_eq!(record.owner_id, Some(player(2)));
    } else {
        assert_eq!(
            self_transfer.expect_err("self").0,
            "settlement.already_owned"
        );
    }
}

#[test]
fn release_clears_membership_and_rejects_second_release() {
    let mut store = owned_store();
    let released = settlement_release_in_store(&mut store, 1, 1).expect("release");
    assert!(!released.is_owned());
    assert!(released.members.is_empty());
    assert_eq!(released.membership_revision, 2);
    assert_rejected(&mut store, "settlement.unowned", |store| {
        settlement_release_in_store(store, 1, 2)
    });
}

#[test]
fn success_returns_the_stored_record_and_checksum() {
    let mut store = owned_store();
    let before = store.checksum();
    let updated =
        settlement_invite_in_store(&mut store, 1, player(2), "member", 1).expect("invite");
    assert_eq!(store.get(1), Some(&updated));
    assert_ne!(store.checksum(), before);
}
