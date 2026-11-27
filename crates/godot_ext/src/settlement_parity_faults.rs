//! Parity matrix for the bridge `settlements_page` cursor and limit helpers.
//!
//! `settlements_page` is a `#[func]` method and needs a Godot runtime, so this
//! module drives the same pure helpers the method calls
//! ([`settlement_page_args`] and [`settlement_page`]) over real
//! [`SettlementStore`] fixtures. It pins the bridge contract from
//! [[42 - World Placement Bridge]] against the core store order and a local
//! reference implementation of the server's `settlement_list` paging:
//!
//! - the cursor is exclusive, `0` starts at the first id, and a past-end
//!   cursor is terminal;
//! - `limit` is clamped to `1..=256`; the bridge's explicit `0` means every
//!   remaining record (the server's absent `limit` equivalent);
//! - `count` is the page length, `matched` the remainder before the cap, and
//!   `has_more` says whether another page exists;
//! - pages stay ascending by id, cover every record once, and never skip a
//!   record across a full walk;
//! - the full-store checksum is unchanged by reads and by an unrelated local
//!   player roster;
//! - a mutation between pages is visible on the next read while an old cursor
//!   stays safe.
//!
//! The bridge's documented terminal conventions are bridge-local: an empty
//! page returns `next_after = 0` (the server echoes the cursor), and `limit =
//! 0` means all remaining records (the server clamps `Some(0)` to one). Both
//! are pinned explicitly rather than hidden. No engine, filesystem, network,
//! or wall clock is involved.

use core_city::player::{PlayerId, PlayerRegistry};
use core_city::settlement::{SettlementRequest, SettlementRole, SettlementStore};
use std::collections::HashSet;

use super::{settlement_page, settlement_page_args, settlement_payload};

const SEED: u64 = 2026;
const PAGE_CAP: i64 = 256;

/// The bridge response fields this module can build without a Godot runtime.
#[derive(Debug, Clone, PartialEq)]
struct BridgePage {
    ids: Vec<u64>,
    rows: Vec<serde_json::Value>,
    count: usize,
    matched: usize,
    next_after: u64,
    has_more: bool,
    checksum: String,
}

/// The server `settlement_list` fields, recomputed independently.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ServerPage {
    ids: Vec<u64>,
    count: usize,
    matched: usize,
    next_after: u64,
    has_more: bool,
}

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

fn store_checksum(store: &SettlementStore) -> String {
    format!("{:016x}", store.checksum())
}

fn store_ids(store: &SettlementStore) -> Vec<u64> {
    store
        .settlements()
        .iter()
        .map(|settlement| settlement.id)
        .collect()
}

/// A deterministic store of `count` unowned settlements in id order.
fn seeded_store(count: usize) -> SettlementStore {
    let mut store = SettlementStore::new(SEED);
    for index in 0..count {
        let settlement = store
            .found(&request(
                &format!("S{index:04}"),
                "nobody",
                i32::try_from(index).expect("small fixture"),
            ))
            .expect("found deterministic settlement");
        assert_eq!(
            settlement.id,
            u64::try_from(index).expect("fixture index fits u64") + 1,
            "ids are assigned in order"
        );
    }
    assert_eq!(store.count(), count);
    store
}

/// One unowned record, one owned record with two members and tax, one owned
/// record with only its owner. Ids 1–3 stay deterministic.
fn mixed_store() -> SettlementStore {
    let mut store = SettlementStore::new(SEED);
    let owner = player(1);
    let manager = player(2);
    let builder = player(3);

    store
        .found(&request("Unowned", "nobody", 0))
        .expect("found unowned");
    store
        .found_for_player(&request("Owned", "mayor", 1), owner)
        .expect("found owned");
    store
        .invite(owner, 2, manager, SettlementRole::Manager, 1)
        .expect("invite manager");
    store
        .invite(owner, 2, builder, SettlementRole::Builder, 2)
        .expect("invite builder");
    store
        .set_tax_rate(owner, 2, 25)
        .expect("set owned tax rate");
    store
        .found_for_player(&request("Second", "builder", 2), manager)
        .expect("found second owned");
    assert_eq!(store_ids(&store), vec![1, 2, 3]);
    assert_eq!(store.get(2).expect("owned").membership_revision, 3);
    assert_eq!(store.get(2).expect("owned").tax_rate, 25);
    store
}

/// Build the page exactly as the bridge does, returning the payload shape too.
fn bridge_page_checked(store: &SettlementStore, after_id: u64, limit: Option<usize>) -> BridgePage {
    let page = settlement_page(store.settlements(), after_id, limit);
    let ids: Vec<u64> = page
        .settlements
        .iter()
        .map(|settlement| settlement.id)
        .collect();
    let rows = page
        .settlements
        .iter()
        .map(|settlement| {
            serde_json::to_value(settlement_payload(settlement)).expect("settlement row JSON")
        })
        .collect();
    BridgePage {
        ids,
        rows,
        count: page.settlements.len(),
        matched: page.matched,
        next_after: page.next_after,
        has_more: page.has_more,
        checksum: store_checksum(store),
    }
}

#[track_caller]
fn bridge_page(store: &SettlementStore, after_id: i64, limit: i64) -> BridgePage {
    let (after_id, limit) = settlement_page_args(after_id, limit).expect("valid page arguments");
    bridge_page_checked(store, after_id, limit)
}

#[track_caller]
fn row_by_id(page: &BridgePage, id: u64) -> &serde_json::Value {
    page.rows
        .iter()
        .find(|row| row["id"].as_u64() == Some(id))
        .unwrap_or_else(|| panic!("row {id} is on the page: {:?}", page.ids))
}

fn payload_rows(store: &SettlementStore, ids: &[u64]) -> Vec<serde_json::Value> {
    ids.iter()
        .map(|id| {
            serde_json::to_value(settlement_payload(
                store.get(*id).expect("fixture record exists"),
            ))
            .expect("settlement row JSON")
        })
        .collect()
}

fn assert_unique(ids: &[u64]) {
    let mut seen = HashSet::new();
    for id in ids {
        assert!(seen.insert(*id), "duplicate settlement id {id} in {ids:?}");
    }
}

/// Walk every page with one limit and assert the response-local invariants.
#[track_caller]
fn walk(store: &SettlementStore, limit: i64) -> (Vec<u64>, Vec<usize>) {
    let checksum = store_checksum(store);
    let mut after = 0_i64;
    let mut seen = Vec::new();
    let mut lengths = Vec::new();
    let mut pages = 0_usize;
    loop {
        let page = bridge_page(store, after, limit);
        assert_eq!(
            page.checksum, checksum,
            "a read must keep the full-store checksum"
        );
        let cursor = u64::try_from(after).expect("cursor fits u64");
        let remaining = store
            .settlements()
            .iter()
            .filter(|settlement| settlement.id > cursor)
            .count();
        let cap = if limit == 0 {
            remaining
        } else {
            usize::try_from(limit.min(PAGE_CAP)).expect("page cap fits usize")
        };
        assert_eq!(page.matched, remaining, "matched after {after}");
        assert_eq!(
            page.count,
            cap.min(remaining),
            "count after {after} limit {limit}"
        );
        assert_eq!(page.has_more, page.count < remaining);
        assert_eq!(
            page.next_after,
            page.ids.last().copied().unwrap_or(0),
            "next_after is the last returned id or 0"
        );
        assert!(page.ids.windows(2).all(|pair| pair[0] < pair[1]));
        lengths.push(page.count);
        seen.extend(page.ids);
        pages += 1;
        assert!(pages <= 2_000, "paging did not terminate");
        if !page.has_more {
            break;
        }
        assert!(
            page.next_after > cursor,
            "a non-terminal page must advance the cursor"
        );
        after = i64::try_from(page.next_after).expect("id fits i64");
    }
    (seen, lengths)
}

/// Independent copy of the server `settlement_list` selection rules.
#[track_caller]
fn server_page(store: &SettlementStore, after_id: Option<u64>, limit: Option<u32>) -> ServerPage {
    let after = after_id.unwrap_or(0);
    let matching: Vec<u64> = store
        .settlements()
        .iter()
        .filter(|settlement| settlement.id > after)
        .map(|settlement| settlement.id)
        .collect();
    let matched = matching.len();
    let cap = limit.map(|limit| limit.clamp(1, u32::try_from(PAGE_CAP).expect("cap")) as usize);
    let ids: Vec<u64> = matching
        .into_iter()
        .take(cap.unwrap_or(usize::MAX))
        .collect();
    let count = ids.len();
    ServerPage {
        next_after: ids.last().copied().unwrap_or(after),
        ids,
        count,
        matched,
        has_more: count < matched,
    }
}

fn server_cursor(after_id: i64) -> Option<u64> {
    if after_id == 0 {
        None
    } else {
        Some(u64::try_from(after_id).expect("fixture cursor fits u64"))
    }
}

fn server_limit(limit: i64) -> Option<u32> {
    if limit == 0 {
        None
    } else {
        Some(u32::try_from(limit).expect("fixture limit fits u32"))
    }
}

#[test]
fn page_arguments_match_the_documented_contract() {
    assert_eq!(settlement_page_args(0, 0), Ok((0, None)));
    assert_eq!(settlement_page_args(0, 1), Ok((0, Some(1))));
    assert_eq!(settlement_page_args(8, 8), Ok((8, Some(8))));
    assert_eq!(settlement_page_args(23, 256), Ok((23, Some(256))));
    assert_eq!(settlement_page_args(0, 257), Ok((0, Some(256))));
    assert_eq!(settlement_page_args(0, 1_000), Ok((0, Some(256))));
    assert_eq!(
        settlement_page_args(i64::MAX, i64::MAX),
        Ok((i64::MAX as u64, Some(256)))
    );

    for (after_id, limit) in [(-1, 0), (0, -1), (-1, -1), (i64::MIN, 8), (8, i64::MIN)] {
        let (code, _) = settlement_page_args(after_id, limit).expect_err("invalid page arguments");
        assert_eq!(code, "invalid_page", "after_id={after_id} limit={limit}");
    }
}

#[test]
fn empty_store_is_terminal_for_every_cursor_and_limit() {
    let store = SettlementStore::new(SEED);
    let checksum = store_checksum(&store);
    for after_id in [0, 1, 7, i64::MAX] {
        for limit in [0, 1, 8, 256, 257] {
            let page = bridge_page(&store, after_id, limit);
            assert!(page.ids.is_empty(), "after_id={after_id} limit={limit}");
            assert!(page.rows.is_empty());
            assert_eq!(page.count, 0);
            assert_eq!(page.matched, 0);
            assert_eq!(page.next_after, 0, "the bridge terminal cursor is 0");
            assert!(!page.has_more);
            assert_eq!(page.checksum, checksum);
        }
    }
}

#[test]
fn single_settlement_reaches_a_terminal_page() {
    let store = seeded_store(1);
    let first = bridge_page(&store, 0, 8);
    assert_eq!(first.ids, vec![1]);
    assert_eq!(first.count, 1);
    assert_eq!(first.matched, 1);
    assert_eq!(first.next_after, 1);
    assert!(!first.has_more);

    let all = bridge_page(&store, 0, 0);
    assert_eq!(all.ids, vec![1]);
    assert_eq!(all.count, 1);
    assert_eq!(all.matched, 1);
    assert_eq!(all.next_after, 1);
    assert!(!all.has_more);

    for after_id in [1, 2, i64::MAX] {
        let terminal = bridge_page(&store, after_id, 8);
        assert!(terminal.ids.is_empty(), "after_id={after_id}");
        assert_eq!(terminal.matched, 0);
        assert_eq!(terminal.next_after, 0);
        assert!(!terminal.has_more);
    }
}

#[test]
fn twenty_three_settlements_walk_without_duplicates_or_skips() {
    let store = seeded_store(23);
    let expected = store_ids(&store);
    assert_eq!(expected, (1..=23).collect::<Vec<u64>>());

    let cases: [(i64, Vec<usize>); 5] = [
        (0, vec![23]),
        (1, vec![1; 23]),
        (8, vec![8, 8, 7]),
        (256, vec![23]),
        (257, vec![23]),
    ];
    for (limit, expected_lengths) in cases {
        let (seen, lengths) = walk(&store, limit);
        assert_eq!(seen, expected, "limit={limit}");
        assert_unique(&seen);
        assert_eq!(lengths, expected_lengths, "limit={limit}");
        assert_eq!(lengths.iter().sum::<usize>(), 23);
    }
}

#[test]
fn one_thousand_settlements_walk_in_bounded_pages() {
    let store = seeded_store(1_000);
    let expected: Vec<u64> = (1..=1_000).collect();
    assert_eq!(store_ids(&store), expected);

    for (limit, page_count) in [(0, 1), (1, 1_000), (8, 125), (256, 4)] {
        let (seen, lengths) = walk(&store, limit);
        assert_eq!(seen, expected, "limit={limit}");
        assert_unique(&seen);
        assert_eq!(lengths.len(), page_count, "limit={limit}");
        assert_eq!(lengths.iter().sum::<usize>(), 1_000);
        if limit > 0 {
            let full = usize::try_from(limit.min(PAGE_CAP)).expect("page cap fits usize");
            assert!(
                lengths[..lengths.len() - 1].iter().all(|len| *len == full),
                "every non-terminal page is full at limit={limit}: {lengths:?}"
            );
        }
    }
}

#[test]
fn exact_multiples_end_on_a_full_terminal_page() {
    for (count, limit) in [(8, 8), (16, 8), (24, 8), (256, 256), (512, 256)] {
        let store = seeded_store(count);
        let (seen, lengths) = walk(&store, limit);
        let limit_usize = usize::try_from(limit).expect("limit fits usize");
        let count_i64 = i64::try_from(count).expect("fixture count fits i64");
        assert_eq!(seen.len(), count, "count={count} limit={limit}");
        assert_eq!(
            lengths.len(),
            usize::try_from(count_i64 / limit).expect("page count fits usize"),
            "count={count} limit={limit}"
        );
        assert!(
            lengths.iter().all(|len| *len == limit_usize),
            "exact multiples stay full: {lengths:?}"
        );

        let terminal = bridge_page(
            &store,
            i64::try_from(count).expect("fixture count fits i64"),
            limit,
        );
        assert!(terminal.ids.is_empty());
        assert_eq!(terminal.count, 0);
        assert_eq!(terminal.matched, 0);
        assert_eq!(terminal.next_after, 0);
        assert!(!terminal.has_more);
    }
}

#[test]
fn matched_is_reported_before_the_page_cap() {
    let store = seeded_store(23);
    for (after_id, matched) in [
        (0, 23),
        (1, 22),
        (7, 16),
        (8, 15),
        (20, 3),
        (22, 1),
        (23, 0),
        (24, 0),
        (i64::MAX, 0),
    ] {
        let page = bridge_page(&store, after_id, 8);
        assert_eq!(page.matched, matched, "after_id={after_id}");
        assert_eq!(page.count, matched.min(8), "after_id={after_id}");
        assert_eq!(page.has_more, matched > 8, "after_id={after_id}");
        if matched == 0 {
            assert_eq!(page.next_after, 0, "after_id={after_id}");
        } else {
            assert_eq!(
                page.next_after,
                u64::try_from(after_id).expect("cursor")
                    + u64::try_from(page.count).expect("count"),
                "after_id={after_id}"
            );
        }
    }
}

#[test]
fn bridge_pages_match_the_core_order_and_server_reference() {
    let store = mixed_store();
    let core_ids = store_ids(&store);
    assert_eq!(core_ids, vec![1, 2, 3]);

    for after_id in [0, 1, 2, 3, 4, i64::MAX] {
        for limit in [0, 1, 8, 256, 257] {
            let page = bridge_page(&store, after_id, limit);
            let cursor = u64::try_from(after_id).expect("fixture cursor fits u64");
            let remaining: Vec<u64> = core_ids.iter().copied().filter(|id| *id > cursor).collect();
            let cap = if limit == 0 {
                remaining.len()
            } else {
                usize::try_from(limit.min(PAGE_CAP)).expect("page cap fits usize")
            };
            let expected: Vec<u64> = remaining.iter().copied().take(cap).collect();
            assert_eq!(
                page.ids, expected,
                "core order after_id={after_id} limit={limit}"
            );

            let server = server_page(&store, server_cursor(after_id), server_limit(limit));
            assert_eq!(page.ids, server.ids);
            assert_eq!(page.count, server.count);
            assert_eq!(page.matched, server.matched);
            assert_eq!(page.has_more, server.has_more);
            if page.count == 0 {
                // Documented bridge/server difference: the bridge ends a walk
                // with 0; the server echoes the request cursor.
                assert_eq!(page.next_after, 0);
                assert_eq!(server.next_after, server_cursor(after_id).unwrap_or(0));
            } else {
                assert_eq!(page.next_after, server.next_after);
            }
        }
    }

    let all = bridge_page(&store, 0, 0);
    let server_all = server_page(&store, None, None);
    assert_eq!(all.ids, server_all.ids);
    assert_eq!(all.count, server_all.count);
    assert_eq!(all.matched, server_all.matched);
    assert_eq!(all.next_after, server_all.next_after);
    assert!(!all.has_more);
}

#[test]
fn zero_limit_is_all_remaining_and_server_zero_clamps_to_one() {
    let store = seeded_store(23);
    let page = bridge_page(&store, 8, 0);
    assert_eq!(page.count, 15);
    assert_eq!(page.matched, 15);
    assert_eq!(page.next_after, 23);
    assert!(!page.has_more);
    assert_eq!(page.ids, (9..=23).collect::<Vec<u64>>());

    // The server treats `Some(0)` as one record, not as "all remaining".
    let server = server_page(&store, Some(8), Some(0));
    assert_eq!(server.count, 1);
    assert_eq!(server.matched, 15);
    assert_eq!(server.next_after, 9);
    assert!(server.has_more);
}

#[test]
fn rows_are_a_stable_function_of_id_order() {
    let store = mixed_store();
    let ids = store_ids(&store);
    let expected_rows = payload_rows(&store, &ids);

    let page = bridge_page(&store, 0, 0);
    assert_eq!(page.ids, ids);
    assert_eq!(page.rows, expected_rows);
    assert_eq!(bridge_page(&store, 0, 0).rows, page.rows);

    for split in [1, 2, 256] {
        let first = bridge_page(&store, 0, split);
        let rest = bridge_page(
            &store,
            i64::try_from(first.next_after).expect("cursor fits i64"),
            0,
        );
        let combined: Vec<serde_json::Value> =
            first.rows.iter().chain(rest.rows.iter()).cloned().collect();
        assert_eq!(combined, expected_rows, "split={split}");
    }

    let unowned = row_by_id(&page, 1);
    assert_eq!(unowned["owned"].as_bool(), Some(false));
    assert!(unowned["owner_id"].is_null());
    assert_eq!(unowned["members"].as_array().expect("members").len(), 0);
    assert_eq!(unowned["membership_revision"], 0);
    assert_eq!(unowned["tax_rate"], 0);

    let owned = row_by_id(&page, 2);
    assert_eq!(owned["owned"].as_bool(), Some(true));
    assert_eq!(owned["owner_id"].as_u64(), Some(1));
    assert_eq!(owned["membership_revision"], 3);
    assert_eq!(owned["tax_rate"], 25);
    let members = owned["members"].as_array().expect("members");
    let member_ids: Vec<u64> = members
        .iter()
        .map(|member| member["player"].as_u64().expect("member id"))
        .collect();
    let roles: Vec<&str> = members
        .iter()
        .map(|member| member["role"].as_str().expect("member role"))
        .collect();
    let bits: Vec<u64> = members
        .iter()
        .map(|member| member["permissions"].as_u64().expect("member bits"))
        .collect();
    assert_eq!(member_ids, vec![1, 2, 3]);
    assert_eq!(roles, vec!["owner", "manager", "builder"]);
    assert_eq!(bits, vec![15, 15, 1]);

    let second = row_by_id(&page, 3);
    assert_eq!(second["owned"].as_bool(), Some(true));
    assert_eq!(second["owner_id"].as_u64(), Some(2));
    assert_eq!(second["membership_revision"], 1);
    assert_eq!(second["tax_rate"], 0);
}

#[test]
fn checksum_is_stable_across_pages_and_ignores_the_local_roster() {
    let store = mixed_store();
    let before = store.checksum();

    let mut registry = PlayerRegistry::new(SEED);
    let mayor = registry.register("Mayor").expect("register mayor");
    registry.register("Manager").expect("register manager");
    registry.register("Builder").expect("register builder");
    assert_eq!(mayor.id, player(1));
    let registry_checksum = registry.checksum();
    registry
        .rename(mayor.id, "Mayor Renamed")
        .expect("rename roster label");
    assert_ne!(registry.checksum(), registry_checksum);

    // The store and every page are blind to that roster.
    assert_eq!(store.checksum(), before);
    for limit in [0, 1, 8, 256, 257] {
        let (seen, _) = walk(&store, limit);
        assert_eq!(seen, store_ids(&store));
        assert_eq!(store.checksum(), before);
    }
    assert_eq!(walk(&store, 8).0, store_ids(&store));

    let page = bridge_page(&store, 0, 0);
    assert_eq!(page.checksum, store_checksum(&store));
    for row in &page.rows {
        assert!(
            row.get("owner_name").is_none(),
            "names are resolved outside the settlement row"
        );
    }
    let owned = row_by_id(&page, 2);
    assert_eq!(owned["owner_id"].as_u64(), Some(1));
    // The legacy display label is the stored label, not a roster lookup.
    assert_eq!(owned["owner"].as_str(), Some("mayor"));
}

#[test]
fn mutating_between_pages_is_visible_and_keeps_the_old_cursor_safe() {
    let mut store = seeded_store(23);
    let before = store_checksum(&store);
    let first = bridge_page(&store, 0, 8);
    assert_eq!(first.ids, (1..=8).collect::<Vec<u64>>());
    assert_eq!(first.matched, 23);
    assert_eq!(first.next_after, 8);
    assert!(first.has_more);
    assert_eq!(first.checksum, before);

    let owner = player(1);
    store.claim(owner, 10, 0).expect("claim 10");
    store
        .rename(owner, 10, "Renamed", 1)
        .expect("rename 10 after claim");
    store.claim(owner, 12, 0).expect("claim 12");
    store
        .set_tax_rate(owner, 12, 40)
        .expect("tax 12 after claim");
    store
        .found(&request("Newest", "nobody", 23))
        .expect("append id 24");

    let after = store_checksum(&store);
    assert_ne!(after, before, "mutations move the store checksum");

    // The old cursor is still the same window; no record is skipped or
    // duplicated, and the changed rows show their new state.
    let second = bridge_page(&store, 8, 8);
    assert_eq!(second.ids, (9..=16).collect::<Vec<u64>>());
    assert_eq!(second.matched, 16);
    assert_eq!(second.next_after, 16);
    assert!(second.has_more);
    assert_eq!(second.checksum, after);

    let renamed = row_by_id(&second, 10);
    assert_eq!(renamed["name"].as_str(), Some("Renamed"));
    assert_eq!(renamed["owned"].as_bool(), Some(true));
    assert_eq!(renamed["owner_id"].as_u64(), Some(1));
    assert_eq!(renamed["membership_revision"], 2);

    let taxed = row_by_id(&second, 12);
    assert_eq!(taxed["owned"].as_bool(), Some(true));
    assert_eq!(taxed["tax_rate"], 40);
    assert_eq!(taxed["membership_revision"], 1);

    let third = bridge_page(&store, 16, 8);
    assert_eq!(third.ids, (17..=24).collect::<Vec<u64>>());
    assert_eq!(third.matched, 8);
    assert_eq!(third.next_after, 24);
    assert!(!third.has_more);
    assert_eq!(
        row_by_id(&third, 24)["name"].as_str(),
        Some("Newest"),
        "an appended record is visible to the next page"
    );

    let mut seen = first.ids.clone();
    seen.extend(second.ids);
    seen.extend(third.ids);
    assert_eq!(seen, store_ids(&store));
    assert_unique(&seen);

    let refreshed = bridge_page(&store, 0, 8);
    assert_eq!(refreshed.ids, first.ids);
    assert_eq!(refreshed.checksum, after);

    let terminal = bridge_page(&store, 24, 8);
    assert!(terminal.ids.is_empty());
    assert_eq!(terminal.next_after, 0);
    assert!(!terminal.has_more);
}

#[test]
fn paging_is_read_only_for_every_cursor_and_limit() {
    let store = mixed_store();
    let snapshot = store.clone();
    let next_id = store.next_id;
    for after_id in [0, 1, 2, 3, 4, 23, i64::MAX] {
        for limit in [0, 1, 8, 256, 257] {
            let page = bridge_page(&store, after_id, limit);
            assert_eq!(page.checksum, store_checksum(&store));
        }
    }
    assert_eq!(store, snapshot, "reads never mutate the store");
    assert_eq!(store.next_id, next_id);
}
