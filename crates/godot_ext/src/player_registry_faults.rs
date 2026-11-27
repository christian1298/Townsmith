//! Deterministic matrix for the local bridge player registry (#376).
//!
//! These tests exercise only the crate-root pure helper layer:
//! [`crate::bridge_player_seed`], [`crate::player_registry_sync_seed`],
//! [`crate::players_register_in_store`], [`crate::players_snapshot`], and
//! [`crate::players_find_record`]. They never start Godot, never construct a
//! `TerrainBridge`, and never touch a live world layer. Placement and
//! settlement stores are pure `core_city` values, used to prove that the roster
//! checksum is independent of the other layers.
//!
//! The matrix pins stable error codes (`player.limit`, `player.invalid_name`,
//! `player.seed_mismatch`), the 16-lowercase-hex checksum spelling, id
//! restart-on-seed semantics, and "a rejected call changes nothing".

use core_city::player::{PlayerError, PlayerId, PlayerRegistry, MAX_PLAYERS};
use core_city::{PlacementRequest, PlacementStore, SettlementRequest, SettlementStore};

use crate::{
    bridge_player_error, bridge_player_name, bridge_player_seed, player_registry_sync_seed,
    players_find_record, players_register_in_store, players_snapshot, LocalPlayers,
    BRIDGE_PLAYER_DEFAULT_SEED, BRIDGE_PLAYER_NAME_CHARS,
};

/// Mirror of `TerrainBridge::local_players`: resolve the seed from the open
/// layers first, then sync the roster. A mismatch never reaches the registry.
fn refresh_roster(
    registry: &mut PlayerRegistry,
    layers: [Option<u64>; 4],
) -> Result<(), (&'static str, &'static str)> {
    let seed = bridge_player_seed(layers)?;
    player_registry_sync_seed(registry, seed);
    Ok(())
}

fn empty_registry() -> PlayerRegistry {
    PlayerRegistry::new(BRIDGE_PLAYER_DEFAULT_SEED)
}

fn placement_fixture() -> PlacementRequest {
    PlacementRequest {
        blueprint_id: "local:house".to_string(),
        blueprint_hash: "blake3:matrix".to_string(),
        blueprint_path: "/townsmith/house.tsbp".to_string(),
        name: "House".to_string(),
        bounds_m: [8.0, 6.0, 6.0],
        center_x: 128.0,
        center_z: 128.0,
        rotation_deg: 0,
        material_cost: 100.0,
        resident_capacity: 2,
        job_slots: 0,
        solid_voxels: 100,
        structure: None,
    }
}

#[test]
fn seed_zero_is_the_default_and_the_roster_starts_empty() {
    let local = LocalPlayers::default();
    assert_eq!(local.0.seed, 0);
    assert_eq!(local.0.seed, BRIDGE_PLAYER_DEFAULT_SEED);
    assert!(local.0.is_empty());
    assert_eq!(local.0.checksum(), PlayerRegistry::new(0).checksum());
    assert_eq!(
        bridge_player_seed([None, None, None, None]),
        Ok(BRIDGE_PLAYER_DEFAULT_SEED)
    );
}

#[test]
fn seed_sync_drops_the_roster_and_old_ids_restart_at_one() {
    let mut registry = empty_registry();
    let ada = players_register_in_store(&mut registry, "Ada").expect("ada");
    assert_eq!(ada.id.get(), 1);
    let before = players_snapshot(&registry);

    assert!(player_registry_sync_seed(&mut registry, 7));
    assert_eq!(registry.seed, 7);
    assert!(registry.is_empty());
    assert!(
        registry.get(PlayerId::new(1).expect("id")).is_none(),
        "an id from the old seed cannot be resolved"
    );
    assert!(players_find_record(&registry, "Ada").is_none());
    assert_ne!(players_snapshot(&registry).checksum, before.checksum);

    let again = players_register_in_store(&mut registry, "Ada").expect("ada on seed 7");
    assert_eq!(again.id.get(), 1, "ids restart with the new seed");
    assert_eq!(again.display_name, "Ada");
}

#[test]
fn a_repeated_seed_sync_is_idempotent() {
    let mut registry = empty_registry();
    players_register_in_store(&mut registry, "Grace").expect("grace");
    let roster = players_snapshot(&registry);

    assert!(player_registry_sync_seed(&mut registry, 7));
    assert!(registry.is_empty());
    let empty = players_snapshot(&registry);

    // The second sync to the same seed must not reset again.
    assert!(!player_registry_sync_seed(&mut registry, 7));
    assert_eq!(players_snapshot(&registry), empty);
    let player = players_register_in_store(&mut registry, "Grace").expect("grace on 7");
    let seeded = players_snapshot(&registry);
    assert_eq!(player.id.get(), 1);
    assert!(!player_registry_sync_seed(&mut registry, 7));
    assert_eq!(players_snapshot(&registry), seeded);
    assert_ne!(roster, seeded, "the reset dropped the seed-0 roster");
}

#[test]
fn the_257th_registration_is_player_limit_without_partial_mutation() {
    let mut registry = empty_registry();
    for index in 0..MAX_PLAYERS {
        let player = players_register_in_store(&mut registry, &format!("Player {index}"))
            .expect("under the limit");
        assert_eq!(player.id.get(), u64::try_from(index).expect("index") + 1);
    }
    let before = players_snapshot(&registry);
    assert_eq!(before.count, MAX_PLAYERS);
    assert_eq!(before.players.len(), MAX_PLAYERS);

    let (code, _) =
        players_register_in_store(&mut registry, "Player 256").expect_err("full registry");
    assert_eq!(code, "player.limit");
    assert_eq!(
        players_snapshot(&registry),
        before,
        "a rejected registration mutates nothing"
    );

    let existing =
        players_register_in_store(&mut registry, "player 0").expect("duplicate at the cap");
    assert_eq!(existing.id.get(), 1);
    assert_eq!(existing.display_name, "Player 0");
    assert_eq!(players_snapshot(&registry), before);
}

#[test]
fn case_insensitive_ids_are_idempotent_and_keep_the_first_display_name() {
    let mut registry = empty_registry();
    let first = players_register_in_store(&mut registry, "Alice").expect("alice");
    let checksum = registry.checksum();
    for variant in ["alice", " ALICE ", "aLiCe"] {
        let again = players_register_in_store(&mut registry, variant).expect(variant);
        assert_eq!(again.id, first.id, "variant {variant:?}");
        assert_eq!(again.display_name, "Alice", "variant {variant:?}");
        assert_eq!(registry.checksum(), checksum, "variant {variant:?}");
    }
    assert_eq!(registry.count(), 1);
}

#[test]
fn invalid_names_return_player_invalid_name_and_leave_the_roster_unchanged() {
    let mut registry = empty_registry();
    players_register_in_store(&mut registry, "Ada").expect("ada");
    let before = players_snapshot(&registry);

    let overlong = "x".repeat(BRIDGE_PLAYER_NAME_CHARS + 1);
    let too_many_chars_non_ascii = "é".repeat(BRIDGE_PLAYER_NAME_CHARS + 1);
    let cases = [
        "",
        "   ",
        "\t\n",
        "a\u{0}b",
        "a\nb",
        "\u{7f}",
        "a\u{85}b",
        overlong.as_str(),
        too_many_chars_non_ascii.as_str(),
    ];
    for name in cases {
        let (code, _) = players_register_in_store(&mut registry, name).expect_err(name);
        assert_eq!(code, "player.invalid_name", "name {name:?}");
        assert_eq!(
            players_snapshot(&registry),
            before,
            "name {name:?} left the roster unchanged"
        );
    }

    // The limit is characters, not bytes. Non-ASCII labels are accepted and
    // matched case-insensitively; only empty/control/overlong names are not.
    let longest = "é".repeat(BRIDGE_PLAYER_NAME_CHARS);
    let accepted =
        players_register_in_store(&mut registry, &longest).expect("32 non-ASCII characters");
    assert_eq!(accepted.display_name, longest);
    assert_eq!(accepted.id.get(), 2);
    assert!(players_find_record(&registry, &longest.to_uppercase()).is_some());
}

#[test]
fn players_list_is_sorted_stable_and_exposes_labels_not_paths() {
    let mut registry = empty_registry();
    for name in ["Zoe", "Ada", "Mia"] {
        players_register_in_store(&mut registry, name).expect(name);
    }
    let label = "../../etc/passwd";
    players_register_in_store(&mut registry, label).expect("label");

    let first = players_snapshot(&registry);
    let ids: Vec<u64> = first.players.iter().map(|player| player.id).collect();
    assert_eq!(ids, vec![1, 2, 3, 4]);
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));

    let names: Vec<&str> = first
        .players
        .iter()
        .map(|player| player.name.as_str())
        .collect();
    assert_eq!(names, vec!["Zoe", "Ada", "Mia", label]);
    assert_eq!(
        first.players[3].name, label,
        "a name is a verbatim label, never a resolved path"
    );

    assert_eq!(players_snapshot(&registry), first, "listing is read-only");
    assert_eq!(first.checksum, format!("{:016x}", registry.checksum()));
    assert_eq!(first.checksum.len(), 16);
    assert!(first
        .checksum
        .chars()
        .all(|value| value.is_ascii_digit() || ('a'..='f').contains(&value)));
}

#[test]
fn players_find_is_exact_case_insensitive_and_safe_on_a_miss() {
    let mut registry = empty_registry();
    players_register_in_store(&mut registry, "Ada Lovelace").expect("ada");
    let before = players_snapshot(&registry);

    for needle in ["Ada Lovelace", "ada lovelace", "  ADA LOVELACE  "] {
        let hit = players_find_record(&registry, needle).expect(needle);
        assert_eq!(hit.id, 1);
        assert_eq!(hit.name, "Ada Lovelace");
    }
    for needle in ["Ada", "", "   ", "a\u{0}b", "Grace"] {
        assert!(
            players_find_record(&registry, needle).is_none(),
            "needle {needle:?}"
        );
    }
    assert_eq!(
        players_snapshot(&registry),
        before,
        "a find never registers or mutates"
    );
}

#[test]
fn repeated_register_and_list_calls_keep_the_checksum_stable_without_duplicates() {
    let mut registry = empty_registry();
    for name in ["Ada", "Grace", "Alan"] {
        players_register_in_store(&mut registry, name).expect(name);
    }
    let baseline = players_snapshot(&registry);

    for _ in 0..128 {
        for name in ["Ada", "GRACE", " alan "] {
            players_register_in_store(&mut registry, name).expect(name);
        }
        assert_eq!(players_snapshot(&registry), baseline);
    }

    let ids: std::collections::BTreeSet<u64> =
        baseline.players.iter().map(|player| player.id).collect();
    assert_eq!(ids.len(), 3, "no duplicate ids");
    assert_eq!(
        baseline.players.len(),
        ids.len(),
        "one record per id in the listing"
    );
}

#[test]
fn settlement_and_placement_mutations_leave_the_roster_checksum_unchanged() {
    let mut registry = empty_registry();
    players_register_in_store(&mut registry, "Ada").expect("ada");
    let roster = players_snapshot(&registry);

    let mut placements = PlacementStore::new(BRIDGE_PLAYER_DEFAULT_SEED);
    placements
        .place(&placement_fixture(), |_, _| 32)
        .expect("placement");
    assert_eq!(
        refresh_roster(&mut registry, [Some(placements.seed), None, None, None]),
        Ok(())
    );
    assert_eq!(players_snapshot(&registry), roster);

    let mut settlements = SettlementStore::new(BRIDGE_PLAYER_DEFAULT_SEED);
    settlements
        .found(&SettlementRequest {
            name: "Riverside".to_string(),
            owner: "player".to_string(),
            center: [10, 20],
            radius_m: 64,
        })
        .expect("settlement");
    assert_eq!(
        refresh_roster(
            &mut registry,
            [Some(placements.seed), Some(settlements.seed), None, None]
        ),
        Ok(())
    );
    assert_eq!(players_snapshot(&registry), roster);

    assert!(placements.remove(1));
    placements.clear();
    settlements.clear();
    assert_eq!(
        refresh_roster(
            &mut registry,
            [
                Some(placements.seed),
                Some(settlements.seed),
                Some(BRIDGE_PLAYER_DEFAULT_SEED),
                Some(BRIDGE_PLAYER_DEFAULT_SEED)
            ]
        ),
        Ok(())
    );
    assert_eq!(
        players_snapshot(&registry),
        roster,
        "other layers never feed the roster checksum"
    );
}

#[test]
fn a_seed_mismatch_is_reported_before_any_roster_change() {
    let mut registry = empty_registry();
    players_register_in_store(&mut registry, "Ada").expect("ada");
    let before = players_snapshot(&registry);

    let (code, _) =
        refresh_roster(&mut registry, [Some(0), Some(7), None, None]).expect_err("mismatch");
    assert_eq!(code, "player.seed_mismatch");
    assert_eq!(
        players_snapshot(&registry),
        before,
        "the mismatch is resolved before the registry is touched"
    );

    let (code, _) = bridge_player_seed([None, Some(0), Some(0), Some(7)]).expect_err("mismatch");
    assert_eq!(code, "player.seed_mismatch");
}

#[test]
fn core_player_errors_map_to_the_bridge_code_vocabulary() {
    let cases = [
        (PlayerError::TooMany, "player.limit"),
        (PlayerError::IdExhausted, "player.limit"),
        (PlayerError::EmptyName, "player.invalid_name"),
        (
            PlayerError::NameTooLong {
                limit: BRIDGE_PLAYER_NAME_CHARS,
            },
            "player.invalid_name",
        ),
        (PlayerError::DuplicateName, "player.invalid_name"),
    ];
    for (error, expected) in cases {
        assert_eq!(bridge_player_error(&error).0, expected, "{error:?}");
    }

    // The bridge validator runs first, so name errors never reach the mapping.
    assert_eq!(
        bridge_player_name("   ").expect_err("blank").0,
        "player.invalid_name"
    );
    assert!(bridge_player_name("  Ada  ").is_ok());
}
