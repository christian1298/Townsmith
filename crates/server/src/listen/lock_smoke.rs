//! Two-client loopback coverage for advisory blueprint edit locks (v0).
//!
//! Every case drives the real [`run_listen`] accept loop, per-connection reader
//! and writer threads, and the length-prefixed JSON framing over IPv4
//! loopback. A clean `disconnect` is synchronous inside the world mutex before
//! its `goodbye` is queued, so release-on-disconnect and reconnect ordering are
//! request/response points, not readiness races. The only polling is the
//! bounded join in [`join_within`], and no case sleeps on a clock.
//!
//! - Contention: Alice owns the lock; Bob's lock, placement, and unlock are all
//!   rejected without moving the lock table, tick, or world checksum, while
//!   Alice's own placement reaches the library check instead of the lock check.
//! - Unlock: the holder releases the lock; the other player can then take it
//!   and the list shows only that holder.
//! - Lease expiry: a short lease drops the lock on the next authoritative tick,
//!   `blueprint_unlock` reports `blueprint.lock_unknown`, and the other player
//!   can take it.
//! - Disconnect policy: `retain_locks_on_disconnect` keeps the holder's lock
//!   (across a same-name reconnect) until expiry, while the default releases it
//!   immediately and a same-name reconnect keeps the same stable player id.
//! - Checksums: the lock-table checksum is stable and ordered for an unchanged
//!   table, expired-only state leaves the world checksum unchanged, and the
//!   read-only list never advances the tick.
//!
//! Lock and unlock each advance the authoritative tick by one on acceptance
//! (the protocol's `tick` field); the lock table itself is deliberately
//! excluded from the world checksum. These tests pin that behavior rather than
//! changing it.

use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use super::{run_listen, ListenConfig, ListenReport};
use crate::protocol::{
    codes, configure_stream, frame_to_io, read_json_frame, write_json_frame, BlueprintLockView,
    ClientCommand, ServerMessage, LOOPBACK_SEED, PROTOCOL_VERSION, SESSION_IO_TIMEOUT,
};
use crate::world::{
    ServerWorld, DEFAULT_BLUEPRINT_LOCK_LEASE_TICKS, MAX_BLUEPRINT_LOCK_LEASE_TICKS,
    MIN_BLUEPRINT_LOCK_LEASE_TICKS,
};

/// Round-trip content hash accepted by every hello in the harness.
const CONTENT_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// Upper bound for a listener to join every session thread.
const JOIN_DEADLINE: Duration = Duration::from_secs(20);

/// Two distinct package hashes, written without the canonical prefix so the
/// accepted results also prove normalization.
const HASH_A_HEX: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B_HEX: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

const _: () = assert!(HASH_A_HEX.len() == 64);
const _: () = assert!(HASH_B_HEX.len() == 64);

fn canonical(hex: &str) -> String {
    format!("blake3:{hex}")
}

fn hello(name: &str) -> ClientCommand {
    ClientCommand::Hello {
        protocol_version: PROTOCOL_VERSION,
        content_hash: CONTENT_HASH.to_string(),
        player_name: name.to_string(),
    }
}

fn lock(hash: &str) -> ClientCommand {
    ClientCommand::BlueprintLock {
        hash: hash.to_string(),
    }
}

fn unlock(hash: &str) -> ClientCommand {
    ClientCommand::BlueprintUnlock {
        hash: hash.to_string(),
    }
}

fn locks() -> ClientCommand {
    ClientCommand::BlueprintLocks
}

fn place(hash: &str) -> ClientCommand {
    ClientCommand::PlaceBlueprint {
        blueprint_hash: hash.to_string(),
        x: 0.0,
        z: 0.0,
        rotation: 0,
    }
}

fn set_tax() -> ClientCommand {
    ClientCommand::SetTax {
        resident_rate: 12,
        business_rate: 8,
    }
}

fn bind() -> (TcpListener, SocketAddr) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind loopback listener");
    let address = listener.local_addr().expect("listener address");
    (listener, address)
}

fn connect(address: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect_timeout(&address, SESSION_IO_TIMEOUT).expect("connect");
    configure_stream(&stream).expect("configure client stream");
    stream
}

fn exchange(stream: &mut TcpStream, command: &ClientCommand) -> io::Result<ServerMessage> {
    write_json_frame(stream, command).map_err(frame_to_io)?;
    read_json_frame(stream).map_err(frame_to_io)
}

/// Connect, complete `hello`, and require a fresh welcome at `expected_tick`.
///
/// Returns the stream and the server-issued session id, which is distinct per
/// connection even when the display name is reused after a reconnect.
fn hello_client(address: SocketAddr, name: &str, expected_tick: u64) -> (TcpStream, String) {
    let mut stream = connect(address);
    let welcome = exchange(&mut stream, &hello(name)).expect("hello exchange");
    match welcome {
        ServerMessage::Welcome {
            session_id,
            tick,
            resumed,
            ..
        } => {
            assert_eq!(tick, expected_tick, "welcome tick for `{name}`");
            assert!(!resumed, "`{name}` must get a fresh welcome");
            (stream, session_id)
        }
        other => panic!("expected welcome for `{name}`, got {other:?}"),
    }
}

fn start_server(
    listener: TcpListener,
    world: ServerWorld,
    limit: u64,
) -> thread::JoinHandle<io::Result<ListenReport>> {
    thread::spawn(move || run_listen(listener, world, ListenConfig::default(), Some(limit)))
}

/// Join the accept loop through a channel so a deadlock fails the test instead
/// of hanging the suite.
fn join_within(server: thread::JoinHandle<io::Result<ListenReport>>) -> ListenReport {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(server.join());
    });
    match receiver.recv_timeout(JOIN_DEADLINE) {
        Ok(Ok(Ok(report))) => report,
        Ok(Ok(Err(error))) => panic!("listen run failed: {error}"),
        Ok(Err(_)) => panic!("listen thread panicked"),
        Err(_) => panic!("listen did not join every session within {JOIN_DEADLINE:?}"),
    }
}

fn world_with(lease_ticks: Option<u64>, retain_locks_on_disconnect: bool) -> ServerWorld {
    let mut world = ServerWorld::new(LOOPBACK_SEED);
    if let Some(lease) = lease_ticks {
        world.set_lock_lease_ticks(lease);
    }
    world.set_retain_locks_on_disconnect(retain_locks_on_disconnect);
    world
}

fn empty_lock_checksum() -> String {
    ServerWorld::new(LOOPBACK_SEED).blueprint_locks_checksum()
}

fn expect_lock(message: ServerMessage) -> (BlueprintLockView, usize, String) {
    match message {
        ServerMessage::BlueprintLockResult {
            lock,
            locks_count,
            checksum,
            ..
        } => (lock, locks_count, checksum),
        other => panic!("expected blueprint_lock_result, got {other:?}"),
    }
}

fn expect_unlock(message: ServerMessage) -> (String, usize, String, bool, Option<u64>) {
    match message {
        ServerMessage::BlueprintUnlockResult {
            hash,
            locks_count,
            checksum,
            forced,
            holder_id,
            ..
        } => (hash, locks_count, checksum, forced, holder_id),
        other => panic!("expected blueprint_unlock_result, got {other:?}"),
    }
}

fn expect_lock_list(message: ServerMessage) -> (u64, usize, String, Vec<BlueprintLockView>) {
    match message {
        ServerMessage::BlueprintLockList {
            tick,
            count,
            checksum,
            locks,
        } => (tick, count, checksum, locks),
        other => panic!("expected blueprint_lock_list, got {other:?}"),
    }
}

fn expect_reject(message: ServerMessage, expected_code: &str) -> String {
    match message {
        ServerMessage::Reject { code, message } => {
            assert_eq!(code, expected_code, "rejection message: {message}");
            message
        }
        other => panic!("expected reject `{expected_code}`, got {other:?}"),
    }
}

fn disconnect(stream: &mut TcpStream) {
    assert_eq!(
        exchange(stream, &ClientCommand::Disconnect).expect("disconnect"),
        ServerMessage::Goodbye
    );
}

/// Alice's accepted lock and Bob's rejected attempts leave one table entry,
/// one tick, and Alice's own placement reaching the library check.
#[test]
fn foreign_lock_blocks_lock_place_and_unlock_without_mutating_the_world() {
    let (listener, address) = bind();
    let server = start_server(listener, world_with(None, false), 2);

    let (mut alice, alice_session) = hello_client(address, "alice", 0);
    let (mut bob, bob_session) = hello_client(address, "bob", 0);
    assert_ne!(
        alice_session, bob_session,
        "each connection must receive a distinct session id"
    );

    let (alice_lock, count, first_checksum) =
        expect_lock(exchange(&mut alice, &lock(HASH_A_HEX)).expect("alice lock"));
    assert_eq!(alice_lock.hash, canonical(HASH_A_HEX));
    assert_eq!(alice_lock.player, "alice");
    assert_eq!(alice_lock.tick, 1);
    assert_eq!(
        alice_lock.expires_tick, DEFAULT_BLUEPRINT_LOCK_LEASE_TICKS,
        "the lease is computed from the pre-acceptance tick"
    );
    assert_eq!(count, 1);
    assert_ne!(first_checksum, empty_lock_checksum());
    let alice_id = alice_lock.player_id;

    // Bob's lock names the holder and leaves the table untouched.
    let message = expect_reject(
        exchange(&mut bob, &lock(HASH_A_HEX)).expect("bob lock"),
        codes::BLUEPRINT_LOCKED,
    );
    assert!(
        message.contains(&format!("player #{alice_id}")),
        "the rejection must name the holder id: {message}"
    );

    // The foreign lock is checked before library resolution, so a server with
    // no library still reports the lock, not a missing library.
    let message = expect_reject(
        exchange(&mut bob, &place(HASH_A_HEX)).expect("bob place"),
        codes::BLUEPRINT_LOCKED,
    );
    assert!(
        message.contains(&format!("player #{alice_id}")),
        "the placement rejection must name the holder id: {message}"
    );

    // Neither rejected command advances the tick or changes the lock table.
    let (tick, count, checksum, listed) =
        expect_lock_list(exchange(&mut bob, &locks()).expect("bob list"));
    assert_eq!(tick, 1, "rejected lock/place must not advance the tick");
    assert_eq!(count, 1);
    assert_eq!(checksum, first_checksum);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].player_id, alice_id);
    assert_eq!(listed[0].player, "alice");

    // Alice's own placement is not lock-rejected; it proceeds to the library
    // check (this server configures none) and fails there instead.
    let message = expect_reject(
        exchange(&mut alice, &place(HASH_A_HEX)).expect("alice place"),
        codes::PLACEMENT_LIBRARY_UNAVAILABLE,
    );
    assert!(
        message.contains("no blueprint library"),
        "alice must reach the library check: {message}"
    );

    let (tick, count, checksum, _) =
        expect_lock_list(exchange(&mut alice, &locks()).expect("alice list"));
    assert_eq!(tick, 1, "a failed placement must not advance the tick");
    assert_eq!(count, 1);
    assert_eq!(checksum, first_checksum);

    disconnect(&mut alice);
    disconnect(&mut bob);

    let report = join_within(server);
    assert_eq!(report.sessions, 2);
    assert_eq!(report.commands, 10);
    assert_eq!(report.saves, 0);
    assert_eq!(report.tick, 1, "only the accepted lock advances the tick");

    let mut reference = ServerWorld::new(LOOPBACK_SEED);
    reference.apply(&hello("alice"));
    reference.apply(&lock(HASH_A_HEX));
    assert_eq!(
        report.checksum,
        reference.checksum(),
        "a foreign lock/place rejection and the lock table must not move the world checksum"
    );
}

/// The holder releases the lock; the other player can then acquire it, and the
/// list shows only the new holder.
#[test]
fn unlock_is_holder_scoped_and_the_other_player_can_then_lock() {
    let (listener, address) = bind();
    let server = start_server(listener, world_with(None, false), 2);

    let (mut alice, _) = hello_client(address, "alice", 0);
    let (mut bob, _) = hello_client(address, "bob", 0);

    let (alice_lock, _, alice_checksum) =
        expect_lock(exchange(&mut alice, &lock(HASH_A_HEX)).expect("alice lock"));
    let alice_id = alice_lock.player_id;

    let message = expect_reject(
        exchange(&mut bob, &unlock(HASH_A_HEX)).expect("bob unlock"),
        codes::BLUEPRINT_LOCKED,
    );
    assert!(
        message.contains(&format!("player #{alice_id}")),
        "the foreign unlock rejection must name the holder: {message}"
    );

    // The rejected unlock changes nothing.
    let (tick, count, checksum, _) =
        expect_lock_list(exchange(&mut bob, &locks()).expect("bob list"));
    assert_eq!((tick, count), (1, 1));
    assert_eq!(
        checksum, alice_checksum,
        "the rejected unlock must not move the lock table"
    );

    let (hash, count, checksum, forced, holder_id) =
        expect_unlock(exchange(&mut alice, &unlock(HASH_A_HEX)).expect("alice unlock"));
    assert_eq!(hash, canonical(HASH_A_HEX));
    assert_eq!(count, 0);
    assert_eq!(checksum, empty_lock_checksum());
    assert!(!forced);
    assert_eq!(holder_id, None);

    let (bob_lock, count, bob_checksum) =
        expect_lock(exchange(&mut bob, &lock(HASH_A_HEX)).expect("bob lock"));
    assert_eq!(bob_lock.player, "bob");
    assert_ne!(
        bob_lock.player_id, alice_id,
        "the two players must have distinct stable ids"
    );
    assert_eq!(bob_lock.tick, 3);
    assert_eq!(count, 1);

    let (tick, count, first_checksum, listed) =
        expect_lock_list(exchange(&mut alice, &locks()).expect("alice list"));
    assert_eq!(tick, 3);
    assert_eq!(count, 1);
    assert_eq!(listed, vec![bob_lock.clone()]);
    assert_eq!(first_checksum, bob_checksum);
    assert_eq!(listed[0].player, "bob");

    // Repeated reads from either client are byte-identical and never tick.
    for (stream, name) in [(&mut alice, "alice"), (&mut bob, "bob")] {
        let (tick, count, checksum, listed) =
            expect_lock_list(exchange(stream, &locks()).expect("repeat list"));
        assert_eq!(tick, 3, "{name} list must not advance the tick");
        assert_eq!(count, 1);
        assert_eq!(checksum, first_checksum);
        assert_eq!(listed, vec![bob_lock.clone()]);
    }

    disconnect(&mut alice);
    disconnect(&mut bob);

    let report = join_within(server);
    assert_eq!(report.tick, 3, "lock, unlock, and lock each advance once");
    assert_eq!(report.saves, 0);

    let mut reference = ServerWorld::new(LOOPBACK_SEED);
    reference.apply(&hello("reference"));
    reference.apply(&lock(HASH_A_HEX));
    reference.apply(&unlock(HASH_A_HEX));
    reference.apply(&lock(HASH_A_HEX));
    assert_eq!(
        report.checksum,
        reference.checksum(),
        "the lock table is excluded from the world checksum"
    );
}

/// A short lease expires on the next authoritative tick and frees contention.
#[test]
fn lease_expiry_drops_the_lock_without_moving_the_world_checksum() {
    assert_eq!(MIN_BLUEPRINT_LOCK_LEASE_TICKS, 1);
    const _: () = assert!(MAX_BLUEPRINT_LOCK_LEASE_TICKS >= 2);

    let (listener, address) = bind();
    let server = start_server(listener, world_with(Some(2), false), 2);

    let (mut alice, _) = hello_client(address, "alice", 0);
    let (mut bob, _) = hello_client(address, "bob", 0);

    let (alice_lock, count, _) =
        expect_lock(exchange(&mut alice, &lock(HASH_A_HEX)).expect("alice lock"));
    assert_eq!(alice_lock.tick, 1);
    assert_eq!(alice_lock.expires_tick, 2);
    assert_eq!(count, 1);
    let alice_id = alice_lock.player_id;

    expect_reject(
        exchange(&mut bob, &lock(HASH_A_HEX)).expect("bob lock while leased"),
        codes::BLUEPRINT_LOCKED,
    );

    let (tick, count, _, _) = expect_lock_list(exchange(&mut bob, &locks()).expect("list live"));
    assert_eq!((tick, count), (1, 1), "the lease is live at tick 1");

    // `set_tax` is the harness's deterministic tick advance; it also changes
    // the economy, so the reference world below applies the same command.
    let snapshot = exchange(&mut bob, &set_tax()).expect("advance tick");
    let snapshot_checksum = match snapshot {
        ServerMessage::Snapshot { tick, checksum, .. } => {
            assert_eq!(tick, 2);
            checksum
        }
        other => panic!("expected snapshot, got {other:?}"),
    };

    let (tick, count, checksum, listed) =
        expect_lock_list(exchange(&mut bob, &locks()).expect("list expired"));
    assert_eq!(tick, 2);
    assert_eq!(count, 0);
    assert_eq!(listed, Vec::new());
    assert_eq!(
        checksum,
        empty_lock_checksum(),
        "expired locks are omitted from the table checksum"
    );

    expect_reject(
        exchange(&mut alice, &unlock(HASH_A_HEX)).expect("unlock expired"),
        codes::BLUEPRINT_LOCK_UNKNOWN,
    );

    let (bob_lock, count, _) =
        expect_lock(exchange(&mut bob, &lock(HASH_A_HEX)).expect("bob takes expired hash"));
    assert_eq!(bob_lock.player, "bob");
    assert_ne!(bob_lock.player_id, alice_id);
    assert_eq!(bob_lock.tick, 3);
    assert_eq!(count, 1);

    let (tick, count, _, listed) =
        expect_lock_list(exchange(&mut alice, &locks()).expect("list after takeover"));
    assert_eq!((tick, count), (3, 1));
    assert_eq!(listed[0].player_id, bob_lock.player_id);

    disconnect(&mut alice);
    disconnect(&mut bob);
    let report = join_within(server);
    assert_eq!(report.tick, 3);
    assert_eq!(report.saves, 0);

    // Expired-only state is invisible to the world checksum: the same tick and
    // economy without any lock record produces the same checksum.
    let mut expired_reference = ServerWorld::new(LOOPBACK_SEED);
    expired_reference.set_lock_lease_ticks(2);
    expired_reference.apply(&hello("reference"));
    expired_reference.apply(&lock(HASH_A_HEX));
    expired_reference.apply(&set_tax());
    assert_eq!(expired_reference.tick, 2);
    assert_eq!(
        expired_reference.blueprint_locks_checksum(),
        empty_lock_checksum()
    );
    assert_eq!(snapshot_checksum, expired_reference.checksum());

    let mut no_lock_reference = ServerWorld::new(LOOPBACK_SEED);
    no_lock_reference.apply(&hello("reference"));
    no_lock_reference.apply(&set_tax());
    no_lock_reference.apply(&set_tax());
    assert_eq!(no_lock_reference.tick, 2);
    assert_eq!(
        expired_reference.checksum(),
        no_lock_reference.checksum(),
        "an expired lock record must not move the world checksum"
    );

    let mut reference = ServerWorld::new(LOOPBACK_SEED);
    reference.set_lock_lease_ticks(2);
    reference.apply(&hello("reference"));
    reference.apply(&lock(HASH_A_HEX));
    reference.apply(&set_tax());
    reference.apply(&lock(HASH_A_HEX));
    assert_eq!(
        report.checksum,
        reference.checksum(),
        "the final live lock table is excluded from the world checksum"
    );
}

/// With retention enabled a disconnected holder keeps the lock across a
/// same-name reconnect until the lease expires.
#[test]
fn retain_on_disconnect_keeps_the_lock_until_lease_expiry() {
    let (listener, address) = bind();
    // Three connections: alice, bob, and alice's same-name reconnect.
    let server = start_server(listener, world_with(Some(2), true), 3);

    let (mut alice, alice_session) = hello_client(address, "alice", 0);
    let (mut bob, _) = hello_client(address, "bob", 0);

    let (alice_lock, _, _) =
        expect_lock(exchange(&mut alice, &lock(HASH_A_HEX)).expect("alice lock"));
    let alice_id = alice_lock.player_id;
    assert_eq!(alice_lock.expires_tick, 2);

    // `disconnect` applies its release policy inside the world mutex before
    // `goodbye` is queued, so the following read is ordered, not a poll.
    disconnect(&mut alice);

    let (tick, count, checksum, listed) =
        expect_lock_list(exchange(&mut bob, &locks()).expect("retained list"));
    assert_eq!(tick, 1);
    assert_eq!(
        count, 1,
        "retain=true must keep the disconnected holder's lock"
    );
    assert_eq!(listed[0].player_id, alice_id);
    assert_ne!(checksum, empty_lock_checksum());

    expect_reject(
        exchange(&mut bob, &lock(HASH_A_HEX)).expect("bob lock retained"),
        codes::BLUEPRINT_LOCKED,
    );

    // A new connection with the same display name resolves the same stable
    // player identity, so the retained lock is still the holder's own.
    let (mut alice_again, alice_again_session) = hello_client(address, "alice", 1);
    assert_ne!(
        alice_session, alice_again_session,
        "each connection has its own session id"
    );
    let (relisted_tick, count, _, listed) =
        expect_lock_list(exchange(&mut alice_again, &locks()).expect("alice list"));
    assert_eq!(relisted_tick, 1);
    assert_eq!(count, 1);
    assert_eq!(listed[0].player_id, alice_id);

    let (refreshed, count, _) =
        expect_lock(exchange(&mut alice_again, &lock(HASH_A_HEX)).expect("alice re-lock"));
    assert_eq!(
        refreshed.player_id, alice_id,
        "a same-name reconnect must keep the stable player id"
    );
    assert_eq!(refreshed.expires_tick, 3);
    assert_eq!(count, 1);
    disconnect(&mut alice_again);

    // The refreshed lock (expires at 3) survives the second disconnect too.
    let (tick, count, _, _) =
        expect_lock_list(exchange(&mut bob, &locks()).expect("list after second disconnect"));
    assert_eq!((tick, count), (2, 1));

    let snapshot = exchange(&mut bob, &set_tax()).expect("advance to expiry");
    match snapshot {
        ServerMessage::Snapshot { tick, .. } => assert_eq!(tick, 3),
        other => panic!("expected snapshot, got {other:?}"),
    }

    let (tick, count, checksum, listed) =
        expect_lock_list(exchange(&mut bob, &locks()).expect("lease expired"));
    assert_eq!((tick, count), (3, 0));
    assert_eq!(listed, Vec::new());
    assert_eq!(checksum, empty_lock_checksum());

    expect_reject(
        exchange(&mut bob, &unlock(HASH_A_HEX)).expect("unlock after expiry"),
        codes::BLUEPRINT_LOCK_UNKNOWN,
    );

    let (bob_lock, count, _) =
        expect_lock(exchange(&mut bob, &lock(HASH_A_HEX)).expect("bob takes expired lock"));
    assert_eq!(bob_lock.player, "bob");
    assert_eq!(bob_lock.tick, 4);
    assert_eq!(count, 1);

    disconnect(&mut bob);
    let report = join_within(server);
    assert_eq!(report.sessions, 3);
    assert_eq!(report.tick, 4);
    assert_eq!(report.saves, 0);

    let mut reference = ServerWorld::new(LOOPBACK_SEED);
    reference.set_lock_lease_ticks(2);
    reference.apply(&hello("reference"));
    reference.apply(&lock(HASH_A_HEX));
    reference.apply(&lock(HASH_A_HEX));
    reference.apply(&set_tax());
    reference.apply(&lock(HASH_A_HEX));
    assert_eq!(report.checksum, reference.checksum());
}

/// The default policy releases a disconnected holder's lock immediately; a
/// same-name reconnect keeps the same stable player id but must re-acquire.
#[test]
fn disconnect_releases_immediately_when_retention_is_off() {
    let (listener, address) = bind();
    let server = start_server(listener, world_with(None, false), 3);

    let (mut alice, _) = hello_client(address, "alice", 0);
    let (mut bob, _) = hello_client(address, "bob", 0);

    let (alice_lock, _, _) =
        expect_lock(exchange(&mut alice, &lock(HASH_A_HEX)).expect("alice lock"));
    let alice_id = alice_lock.player_id;

    disconnect(&mut alice);

    // The release happens before `goodbye`, so this list is deterministic.
    let (tick, count, checksum, listed) =
        expect_lock_list(exchange(&mut bob, &locks()).expect("list after release"));
    assert_eq!(tick, 1);
    assert_eq!(count, 0, "retain=false must release on disconnect");
    assert_eq!(listed, Vec::new());
    assert_eq!(checksum, empty_lock_checksum());

    let (mut alice_again, _) = hello_client(address, "alice", 1);

    // The reconnect has the same stable identity, but the old lock is gone: it
    // can acquire a different hash while Bob takes the released one.
    let (alice_second, count, _) = expect_lock(
        exchange(&mut alice_again, &lock(HASH_B_HEX)).expect("alice re-lock another hash"),
    );
    assert_eq!(
        alice_second.player_id, alice_id,
        "the same display name must resolve the same stable player id"
    );
    assert_eq!(count, 1);

    let (bob_lock, count, _) =
        expect_lock(exchange(&mut bob, &lock(HASH_A_HEX)).expect("bob takes released hash"));
    assert_eq!(bob_lock.player, "bob");
    assert_ne!(bob_lock.player_id, alice_id);
    assert_eq!(count, 2);

    let (tick, count, checksum, listed) =
        expect_lock_list(exchange(&mut bob, &locks()).expect("final list"));
    assert_eq!(tick, 3);
    assert_eq!(count, 2);
    assert_eq!(listed[0].hash, canonical(HASH_A_HEX));
    assert_eq!(listed[0].player_id, bob_lock.player_id);
    assert_eq!(listed[1].hash, canonical(HASH_B_HEX));
    assert_eq!(listed[1].player_id, alice_id);
    let (tick_again, count_again, checksum_again, listed_again) =
        expect_lock_list(exchange(&mut bob, &locks()).expect("repeated list"));
    assert_eq!((tick_again, count_again), (tick, count));
    assert_eq!(checksum_again, checksum, "list checksum must be stable");
    assert_eq!(listed_again, listed, "list order must be stable");

    expect_reject(
        exchange(&mut alice_again, &unlock(HASH_A_HEX)).expect("alice foreign unlock"),
        codes::BLUEPRINT_LOCKED,
    );

    disconnect(&mut alice_again);
    disconnect(&mut bob);
    let report = join_within(server);
    assert_eq!(report.sessions, 3);
    assert_eq!(report.tick, 3);
    assert_eq!(report.saves, 0);

    let mut reference = ServerWorld::new(LOOPBACK_SEED);
    reference.apply(&hello("reference"));
    reference.apply(&lock(HASH_A_HEX));
    reference.apply(&lock(HASH_B_HEX));
    reference.apply(&lock(HASH_A_HEX));
    assert_eq!(report.checksum, reference.checksum());
}

/// The lock-table checksum is stable and sorted for an unchanged table, and
/// read-only list traffic does not advance the tick.
#[test]
fn lock_checksum_is_stable_sorted_and_list_is_read_only() {
    let (listener, address) = bind();
    let server = start_server(listener, world_with(None, false), 2);

    let (mut alice, _) = hello_client(address, "alice", 0);
    let (mut bob, _) = hello_client(address, "bob", 0);

    // Insert B before A: the list must still be ascending by canonical hash.
    let (_, _, checksum_b) = expect_lock(exchange(&mut alice, &lock(HASH_B_HEX)).expect("lock b"));
    let (_, count, checksum_ab) =
        expect_lock(exchange(&mut alice, &lock(HASH_A_HEX)).expect("lock a"));
    assert_eq!(count, 2);
    assert_ne!(checksum_b, checksum_ab);

    let (tick, count, checksum, listed) =
        expect_lock_list(exchange(&mut bob, &locks()).expect("bob list"));
    assert_eq!(tick, 2);
    assert_eq!(count, 2);
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].hash, canonical(HASH_A_HEX));
    assert_eq!(listed[1].hash, canonical(HASH_B_HEX));
    assert_eq!(checksum, checksum_ab);

    // Failed foreign attempts are side-effect free for the table checksum.
    expect_reject(
        exchange(&mut bob, &lock(HASH_B_HEX)).expect("bob foreign lock"),
        codes::BLUEPRINT_LOCKED,
    );
    expect_reject(
        exchange(&mut bob, &unlock(HASH_A_HEX)).expect("bob foreign unlock"),
        codes::BLUEPRINT_LOCKED,
    );
    expect_reject(
        exchange(&mut bob, &place(HASH_B_HEX)).expect("bob foreign place"),
        codes::BLUEPRINT_LOCKED,
    );

    for (stream, name) in [(&mut alice, "alice"), (&mut bob, "bob")] {
        let (tick, count, repeated, listed) =
            expect_lock_list(exchange(stream, &locks()).expect("stable list"));
        assert_eq!(tick, 2, "{name} list must not advance the tick");
        assert_eq!(count, 2);
        assert_eq!(repeated, checksum_ab, "{name} checksum must be stable");
        assert_eq!(listed[0].hash, canonical(HASH_A_HEX));
        assert_eq!(listed[1].hash, canonical(HASH_B_HEX));
    }

    disconnect(&mut alice);
    disconnect(&mut bob);
    let report = join_within(server);
    assert_eq!(report.tick, 2, "only the two locks advance the tick");
    assert_eq!(report.saves, 0);

    let mut reference = ServerWorld::new(LOOPBACK_SEED);
    reference.apply(&hello("reference"));
    reference.apply(&lock(HASH_B_HEX));
    reference.apply(&lock(HASH_A_HEX));
    assert_eq!(report.checksum, reference.checksum());
}
