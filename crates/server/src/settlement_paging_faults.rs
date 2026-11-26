//! Deterministic edge and fault corpus for `settlement_list` paging.
//!
//! Every case drives the authoritative [`ServerWorld`] (or the transport-only
//! [`Session`] where that boundary matters) and pins the stable response codes
//! and paging invariants: exclusive `after_id`, clamped `limit`, `matched`
//! before the limit, `count` as the page length, `next_after` as the last
//! returned id (or the echoed cursor for an empty page), and `has_more`.
//!
//! Paging is a read: a query must not advance the tick or change the world or
//! settlement-store checksums, and repeating the same query must be stable.
//! No network, filesystem, or wall clock is involved.

use crate::protocol::{
    codes, decode_client_command, ClientCommand, ServerMessage, Session, LOOPBACK_SEED,
    PROTOCOL_VERSION, SETTLEMENT_LIST_PAGE_CAP,
};
use crate::world::{codes as world_codes, ClientRole, ServerWorld};
use core_city::settlement::{SettlementRequest, SettlementStore};

const SEED: u64 = 2026;
const CONTENT_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A decoded `settlement_list` page plus the raw JSON for stability checks.
#[derive(Debug, Clone, PartialEq)]
struct Page {
    tick: u64,
    count: usize,
    matched: u32,
    next_after: u64,
    has_more: bool,
    checksum: String,
    ids: Vec<u64>,
    json: serde_json::Value,
}

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    tick: u64,
    world_checksum: String,
    store_checksum: Option<u64>,
    records_json: String,
}

fn snapshot(world: &ServerWorld) -> Snapshot {
    Snapshot {
        tick: world.tick,
        world_checksum: world.checksum(),
        store_checksum: world.settlements.as_ref().map(SettlementStore::checksum),
        records_json: serde_json::to_string(&world.settlements).expect("settlements json"),
    }
}

fn hello() -> ClientCommand {
    ClientCommand::Hello {
        protocol_version: PROTOCOL_VERSION,
        content_hash: CONTENT_HASH.to_string(),
        player_name: "mayor".to_string(),
    }
}

fn found(name: &str) -> ClientCommand {
    ClientCommand::FoundSettlement {
        name: name.to_string(),
        x: 0,
        z: 0,
        radius_m: 64,
    }
}

/// Submit one `settlement_list` query and assert the response-local invariants.
#[track_caller]
fn page(world: &mut ServerWorld, after_id: Option<u64>, limit: Option<u32>) -> Page {
    let response = world.apply(&ClientCommand::SettlementList { after_id, limit });
    let json = serde_json::to_value(&response).expect("settlement_list serializes");
    let ServerMessage::SettlementList {
        tick,
        count,
        matched,
        next_after,
        has_more,
        checksum,
        settlements,
    } = response
    else {
        panic!("expected settlement_list, got {json}");
    };

    let ids: Vec<u64> = settlements.iter().map(|settlement| settlement.id).collect();
    assert_eq!(count, ids.len(), "count is the page length");
    assert_eq!(count, settlements.len());
    assert!(
        settlements.windows(2).all(|pair| pair[0].id < pair[1].id),
        "a page stays strictly ascending: {ids:?}"
    );
    assert_eq!(
        next_after,
        ids.last().copied().unwrap_or_else(|| after_id.unwrap_or(0)),
        "next_after is the last returned id, or the echoed cursor for an empty page"
    );
    assert_eq!(
        has_more,
        u64::from(matched) > u64::try_from(count).expect("count fits u64"),
        "has_more must mean more matches follow the page"
    );
    if let Some(limit) = limit {
        let effective =
            usize::try_from(limit.clamp(1, SETTLEMENT_LIST_PAGE_CAP)).expect("page cap fits usize");
        assert!(
            count <= effective,
            "page length {count} exceeds clamped limit {effective}"
        );
    }
    assert_eq!(checksum.len(), 16, "checksum is 16 hex digits");
    assert!(
        checksum.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "checksum is hexadecimal: {checksum}"
    );

    Page {
        tick,
        count,
        matched,
        next_after,
        has_more,
        checksum,
        ids,
        json,
    }
}

#[track_caller]
fn assert_page(
    page: &Page,
    count: usize,
    matched: u32,
    next_after: u64,
    has_more: bool,
    ids: &[u64],
) {
    assert_eq!(page.count, count, "page {page:?}");
    assert_eq!(page.matched, matched, "page {page:?}");
    assert_eq!(page.next_after, next_after, "page {page:?}");
    assert_eq!(page.has_more, has_more, "page {page:?}");
    assert_eq!(page.ids, ids, "page {page:?}");
}

fn store_ids(world: &ServerWorld) -> Vec<u64> {
    world
        .settlements
        .as_ref()
        .map(|store| store.settlements.iter().map(|record| record.id).collect())
        .unwrap_or_default()
}

fn store_checksum(world: &ServerWorld) -> Option<u64> {
    world.settlements.as_ref().map(SettlementStore::checksum)
}

/// A hello world with a settlement store seeded directly through the core API.
///
/// This keeps paging tests independent of command payload validation while
/// still exercising the authoritative `apply` path for every query.
fn seeded_store_world(count: usize, role: ClientRole) -> (ServerWorld, Vec<u64>) {
    let mut world = ServerWorld::with_role(SEED, role);
    let mut store = SettlementStore::new(SEED);
    let mut ids = Vec::with_capacity(count);
    for index in 0..count {
        let settlement = store
            .found(&SettlementRequest {
                name: format!("S{index:04}"),
                owner: "nobody".to_string(),
                center: [i32::try_from(index).expect("small seed"), 0],
                radius_m: 64,
            })
            .expect("seed settlement");
        ids.push(settlement.id);
    }
    world.settlements = Some(store);
    assert!(
        matches!(world.apply(&hello()), ServerMessage::Welcome { .. }),
        "seeded world accepts hello"
    );
    (world, ids)
}

/// A hello world with 3–5 settlements founded through the public command.
fn founded_world(names: &[&str]) -> (ServerWorld, Vec<u64>) {
    let mut world = ServerWorld::with_role(SEED, ClientRole::Mayor);
    assert!(
        matches!(world.apply(&hello()), ServerMessage::Welcome { .. }),
        "world accepts hello"
    );
    let mut ids = Vec::with_capacity(names.len());
    for name in names {
        match world.apply(&found(name)) {
            ServerMessage::SettlementFound { settlement, .. } => ids.push(settlement.id),
            other => panic!("expected settlement_found for {name}, got {other:?}"),
        }
    }
    (world, ids)
}

#[test]
fn empty_store_answers_every_cursor_and_limit_without_mutation() {
    let mut world = ServerWorld::with_role(SEED, ClientRole::Mayor);
    assert!(matches!(
        world.apply(&hello()),
        ServerMessage::Welcome { .. }
    ));
    let before = snapshot(&world);
    assert_eq!(before.store_checksum, None, "no store is present");

    for after_id in [None, Some(0), Some(3), Some(u64::MAX)] {
        for limit in [
            None,
            Some(0),
            Some(1),
            Some(SETTLEMENT_LIST_PAGE_CAP),
            Some(SETTLEMENT_LIST_PAGE_CAP + 1),
            Some(u32::MAX),
        ] {
            let page = page(&mut world, after_id, limit);
            assert_page(&page, 0, 0, after_id.unwrap_or(0), false, &[]);
            assert_eq!(page.tick, before.tick);
            assert_eq!(page.checksum, "0000000000000000");
        }
    }
    assert_eq!(snapshot(&world), before, "reads never mutate the world");

    // An explicitly present but empty store reports its own checksum and keeps
    // echoing the cursor instead of falling back to the absent-store zero.
    world.settlements = Some(SettlementStore::new(SEED));
    let expected_checksum = format!("{:016x}", store_checksum(&world).expect("store"));
    for after_id in [None, Some(0), Some(3), Some(u64::MAX)] {
        for limit in [None, Some(0), Some(1), Some(257)] {
            let page = page(&mut world, after_id, limit);
            assert_page(&page, 0, 0, after_id.unwrap_or(0), false, &[]);
            assert_eq!(page.tick, before.tick);
            assert_eq!(page.checksum, expected_checksum);
        }
    }
    assert_eq!(snapshot(&world).tick, before.tick);
    assert_eq!(
        snapshot(&world).store_checksum,
        Some(store_checksum(&world).unwrap())
    );
}

#[test]
fn cursor_edges_are_exclusive_at_first_between_last_past_and_max() {
    let (mut world, ids) = founded_world(&["Harbour", "Docks", "Mills", "Foundry", "Quarry"]);
    assert_eq!(ids, vec![1, 2, 3, 4, 5]);

    // Create a real gap so a cursor can fall strictly between two record ids.
    world
        .settlements
        .as_mut()
        .expect("store")
        .settlements
        .retain(|record| record.id != 2);
    let ids = store_ids(&world);
    assert_eq!(ids, vec![1, 3, 4, 5]);
    let before = snapshot(&world);

    // No cursor and a zero cursor are the same exclusive lower bound.
    let all = [
        page(&mut world, None, None),
        page(&mut world, Some(0), None),
    ];
    for page in &all {
        assert_page(page, 4, 4, 5, false, &ids);
        assert_eq!(page.tick, before.tick);
        assert_eq!(
            page.checksum,
            format!("{:016x}", before.store_checksum.unwrap())
        );
    }

    // Exact existing ids, a between-ids cursor, the last id, past-last, and max.
    assert_page(&page(&mut world, Some(1), None), 3, 3, 5, false, &[3, 4, 5]);
    assert_page(&page(&mut world, Some(2), None), 3, 3, 5, false, &[3, 4, 5]);
    assert_page(&page(&mut world, Some(3), None), 2, 2, 5, false, &[4, 5]);
    assert_page(&page(&mut world, Some(4), None), 1, 1, 5, false, &[5]);
    assert_page(&page(&mut world, Some(5), None), 0, 0, 5, false, &[]);
    assert_page(&page(&mut world, Some(6), None), 0, 0, 6, false, &[]);
    assert_page(
        &page(&mut world, Some(u64::MAX), None),
        0,
        0,
        u64::MAX,
        false,
        &[],
    );

    // A page limit does not change `matched`: it still counts every record
    // after the cursor, not just the page.
    assert_page(&page(&mut world, Some(1), Some(1)), 1, 3, 3, true, &[3]);
    assert_page(&page(&mut world, Some(2), Some(2)), 2, 3, 4, true, &[3, 4]);
    assert_page(&page(&mut world, Some(3), Some(2)), 2, 2, 5, false, &[4, 5]);
    assert_page(&page(&mut world, Some(4), Some(1)), 1, 1, 5, false, &[5]);
    assert_page(&page(&mut world, Some(5), Some(1)), 0, 0, 5, false, &[]);

    assert_eq!(
        snapshot(&world),
        before,
        "cursor reads never mutate the world"
    );
}

#[test]
fn limit_clamps_to_one_through_the_page_cap() {
    let (mut world, ids) = seeded_store_world(300, ClientRole::Mayor);
    assert_eq!(ids.len(), 300);
    assert_eq!(ids.first(), Some(&1));
    assert_eq!(ids.last(), Some(&300));
    let before = snapshot(&world);

    // `None` keeps the pre-paging all-records behavior; an explicit limit is
    // clamped to 1..=256, so 0 behaves like 1 and 257+ behaves like 256.
    assert_page(&page(&mut world, None, None), 300, 300, 300, false, &ids);
    for (limit, expected_count) in [
        (0, 1),
        (1, 1),
        (2, 2),
        (255, 255),
        (SETTLEMENT_LIST_PAGE_CAP - 1, 255),
        (SETTLEMENT_LIST_PAGE_CAP, 256),
        (SETTLEMENT_LIST_PAGE_CAP + 1, 256),
        (300, 256),
        (u32::MAX, 256),
    ] {
        let page = page(&mut world, None, Some(limit));
        assert_page(
            &page,
            expected_count,
            300,
            u64::try_from(expected_count).unwrap(),
            expected_count < 300,
            &ids[..expected_count],
        );
        assert_eq!(page.tick, before.tick);
        assert_eq!(
            page.checksum,
            format!("{:016x}", before.store_checksum.unwrap())
        );
    }

    // Exact page-size boundaries at the end of the list.
    assert_page(
        &page(&mut world, Some(44), Some(SETTLEMENT_LIST_PAGE_CAP)),
        256,
        256,
        300,
        false,
        &ids[44..],
    );
    assert_page(
        &page(&mut world, Some(43), Some(SETTLEMENT_LIST_PAGE_CAP)),
        256,
        257,
        299,
        true,
        &ids[43..299],
    );

    // One-record pages at the very end, with and without a clamp, then past it.
    for limit in [Some(0), Some(1), Some(256), Some(u32::MAX)] {
        assert_page(
            &page(&mut world, Some(299), limit),
            1,
            1,
            300,
            false,
            &[300],
        );
    }
    assert_page(&page(&mut world, Some(300), Some(1)), 0, 0, 300, false, &[]);
    assert_page(
        &page(&mut world, Some(300), Some(u32::MAX)),
        0,
        0,
        300,
        false,
        &[],
    );

    assert_eq!(
        snapshot(&world),
        before,
        "limit queries never mutate the world"
    );
}

/// Walk one cursor through a fresh world and pin every cross-page invariant.
///
/// A fresh world per case keeps each walk inside the deterministic 40-credit
/// command burst, so the test exercises paging rather than the rate bucket.
#[track_caller]
fn assert_walk(
    world: &mut ServerWorld,
    ids: &[u64],
    start: Option<u64>,
    page_size: u32,
    before: &Snapshot,
) {
    let start_cursor = start.unwrap_or(0);
    let expected: Vec<u64> = ids
        .iter()
        .copied()
        .filter(|id| *id > start_cursor)
        .collect();
    let effective = usize::try_from(page_size.clamp(1, SETTLEMENT_LIST_PAGE_CAP)).unwrap();
    let mut cursor = start;
    let mut walked = Vec::with_capacity(expected.len());
    let mut queries = 0usize;

    loop {
        queries += 1;
        assert!(
            queries <= expected.len() + 2,
            "page walk must terminate: start {start:?} size {page_size}"
        );
        let page = page(world, cursor, Some(page_size));
        assert_eq!(page.tick, before.tick);
        assert_eq!(
            page.checksum,
            format!("{:016x}", before.store_checksum.unwrap())
        );
        assert_eq!(
            page.matched,
            u32::try_from(expected.len() - walked.len()).unwrap(),
            "matched counts every match after the cursor"
        );
        if let Some(last) = walked.last() {
            assert!(
                page.ids.first().is_none_or(|first| first > last),
                "no duplicate or out-of-order id across pages: {walked:?} then {:?}",
                page.ids
            );
        }
        walked.extend(page.ids.iter().copied());

        if page.has_more {
            assert_eq!(
                page.count, effective,
                "a non-final page is exactly the clamped page size"
            );
        } else {
            assert_eq!(page.matched as usize, page.count);
            assert_eq!(page.ids, expected[walked.len() - page.count..]);
        }
        cursor = Some(page.next_after);
        if !page.has_more {
            break;
        }
    }

    assert_eq!(walked, expected, "walk start {start:?} size {page_size}");
    assert_eq!(
        cursor,
        expected.last().copied().or(start),
        "final cursor is the last id, or the start when empty"
    );
}

#[test]
fn page_walk_concatenates_to_the_full_list_without_gaps_or_duplicates() {
    // Small pages across the whole list: every id appears once, in order.
    for page_size in [1, 2, 7, 20, SETTLEMENT_LIST_PAGE_CAP] {
        for start in [None, Some(0), Some(1), Some(19), Some(20)] {
            let (mut world, ids) = seeded_store_world(20, ClientRole::Mayor);
            assert_eq!(ids.len(), 20);
            let before = snapshot(&world);
            assert_walk(&mut world, &ids, start, page_size, &before);
            assert_eq!(snapshot(&world), before, "walks never mutate the world");
        }
    }

    // Large lists across the 256 cap and a clamped over-cap request.
    for page_size in [
        SETTLEMENT_LIST_PAGE_CAP,
        SETTLEMENT_LIST_PAGE_CAP + 1,
        1_000,
    ] {
        for start in [None, Some(0), Some(1), Some(299), Some(300)] {
            let (mut world, ids) = seeded_store_world(300, ClientRole::Mayor);
            assert_eq!(ids.len(), 300);
            let before = snapshot(&world);
            assert_walk(&mut world, &ids, start, page_size, &before);
            assert_eq!(snapshot(&world), before, "walks never mutate the world");
        }
    }
}

#[test]
fn paging_is_read_only_and_repeatable() {
    let (mut world, _ids) = seeded_store_world(300, ClientRole::Mayor);
    let before = snapshot(&world);

    let queries = [
        (None, None),
        (Some(0), Some(1)),
        (Some(1), None),
        (Some(150), Some(7)),
        (Some(299), Some(256)),
        (Some(300), Some(1)),
        (Some(u64::MAX), Some(u32::MAX)),
    ];

    for (after_id, limit) in queries {
        let first = page(&mut world, after_id, limit);
        let second = page(&mut world, after_id, limit);
        assert_eq!(first, second, "identical queries are stable");
        assert_eq!(first.tick, before.tick, "paging never advances the tick");
        assert_eq!(
            first.checksum,
            format!("{:016x}", before.store_checksum.unwrap()),
            "the store checksum is a pure function of the store"
        );
        assert_eq!(snapshot(&world), before, "paging never mutates the world");
    }

    assert_eq!(world.tick, before.tick);
    assert_eq!(world.checksum(), before.world_checksum);
    assert_eq!(store_checksum(&world), before.store_checksum);
    assert_eq!(store_ids(&world).len(), 300);
}

#[test]
fn viewer_reads_while_unauthenticated_and_transport_only_principals_are_rejected() {
    // Viewer is the lowest server role, so it is explicitly allowed to read.
    let (mut viewer, ids) = seeded_store_world(1, ClientRole::Viewer);
    assert_eq!(ids, vec![1]);
    let before = snapshot(&viewer);
    assert_page(&page(&mut viewer, None, Some(1)), 1, 1, 1, false, &[1]);
    assert_page(&page(&mut viewer, Some(1), Some(1)), 0, 0, 1, false, &[]);
    assert_eq!(snapshot(&viewer), before);

    // Mutations above the Viewer floor stay rejected without changing state.
    let response = viewer.apply(&found("Harbour"));
    assert!(
        matches!(response, ServerMessage::Reject { ref code, .. } if code.as_str() == world_codes::FORBIDDEN),
        "Viewer mutation must reject with role.forbidden: {response:?}"
    );
    assert_eq!(snapshot(&viewer), before);

    // No role sits below Viewer; the unauthenticated pre-hello principal is
    // the lower gate and is rejected before the read runs.
    let mut unopened = ServerWorld::with_role(SEED, ClientRole::Viewer);
    let before = snapshot(&unopened);
    let response = unopened.apply(&ClientCommand::SettlementList {
        after_id: Some(0),
        limit: Some(1),
    });
    assert!(
        matches!(response, ServerMessage::Reject { ref code, .. } if code.as_str() == world_codes::NOT_HELLO),
        "pre-hello reads reject with hello.required: {response:?}"
    );
    assert_eq!(snapshot(&unopened), before);

    // The transport-only session has no settlement store and must not pretend
    // to page one, even though it accepts hello.
    let mut session = Session::new(LOOPBACK_SEED);
    assert!(
        matches!(
            session.handle(hello()).as_slice(),
            [ServerMessage::Welcome { .. }]
        ),
        "transport session accepts hello"
    );
    let tick = session.tick();
    for (after_id, limit) in [
        (None, None),
        (Some(0), Some(1)),
        (Some(u64::MAX), Some(257)),
    ] {
        let replies = session.handle(ClientCommand::SettlementList { after_id, limit });
        assert!(
            matches!(
                replies.as_slice(),
                [ServerMessage::Reject { code, .. }]
                    if code.as_str() == codes::SETTLEMENT_UNAVAILABLE
            ),
            "transport-only paging must reject as settlement.unavailable: {replies:?}"
        );
        assert_eq!(session.tick(), tick, "rejections never advance the tick");
    }
}

#[test]
fn undecodable_paging_fields_are_rejected_before_apply() {
    // Negative, fractional, stringly, and out-of-range cursor/limit values are
    // decoded as stable invalid-field rejections.
    let invalid_fields = [
        r#"{"type":"settlement_list","after_id":-1}"#,
        r#"{"type":"settlement_list","after_id":1.5}"#,
        r#"{"type":"settlement_list","after_id":"1"}"#,
        r#"{"type":"settlement_list","after_id":18446744073709551616}"#,
        r#"{"type":"settlement_list","limit":-1}"#,
        r#"{"type":"settlement_list","limit":1.5}"#,
        r#"{"type":"settlement_list","limit":"1"}"#,
        r#"{"type":"settlement_list","limit":4294967296}"#,
    ];
    for payload in invalid_fields {
        let error = decode_client_command(payload.as_bytes())
            .expect_err("invalid paging field must not decode");
        assert_eq!(
            error.code,
            codes::COMMAND_INVALID_FIELDS,
            "payload {payload}: {error:?}"
        );
    }

    for payload in [b"{".as_slice(), b"not json".as_slice()] {
        let error = decode_client_command(payload).expect_err("malformed JSON must not decode");
        assert_eq!(error.code, codes::COMMAND_INVALID_JSON, "{error:?}");
    }

    // Valid extremes decode and stay a stable empty page; applying them does
    // not touch the store.
    let decoded = decode_client_command(
        br#"{"type":"settlement_list","after_id":18446744073709551615,"limit":4294967295}"#,
    )
    .expect("u64::MAX / u32::MAX are valid wire values");
    assert_eq!(
        decoded,
        ClientCommand::SettlementList {
            after_id: Some(u64::MAX),
            limit: Some(u32::MAX),
        }
    );
    let (mut world, _ids) = seeded_store_world(3, ClientRole::Mayor);
    let before = snapshot(&world);
    let response = world.apply(&decoded);
    let ServerMessage::SettlementList {
        count,
        matched,
        next_after,
        has_more,
        ..
    } = response
    else {
        panic!("expected settlement_list, got {response:?}");
    };
    assert_eq!(
        (count, matched, next_after, has_more),
        (0, 0, u64::MAX, false)
    );
    assert_eq!(snapshot(&world), before);
}
