//! Deterministic D-033 player-registry and settlement-store scale benchmark.
//!
//! Exercises the save-side identity and settlement layer without a server:
//! player registration, settlement founding, membership fill, tax updates,
//! renames, ownership transfers, checksums, JSON serialization, and a
//! parse/validate round trip with a second save.
//!
//! The scene is fully deterministic: ids, names, centres, roles, tax rates, and
//! transfer recipients are derived from the loop index and a fixed seed. No
//! wall-clock value, temp file, network, or renderer is involved. `--smoke`
//! builds the small fixture twice and fails unless both checksums, byte counts,
//! and member totals match. The documented smoke fixture also pins both
//! checksums; size overrides skip the pins and check determinism only.
//!
//! Scale is bounded by the core caps: [`MAX_PLAYERS`] (256) and
//! [`MAX_SETTLEMENTS`] (1,000) per save. The default request is 10,000 of each,
//! so the harness reports both the requested and effective sizes and sets
//! `capped` when the core clamps them. This is data-scale evidence for one
//! save, not a render, network, or 1M gate.

use std::collections::{HashMap, HashSet};
use std::hint::black_box;
use std::process::ExitCode;
use std::time::Instant;

use core_city::{
    PlayerId, PlayerRegistry, Settlement, SettlementRequest, SettlementRole, SettlementStore,
    MAX_PLAYERS, MAX_SETTLEMENTS, MAX_SETTLEMENTS_OWNED_PER_PLAYER, MAX_SETTLEMENT_MEMBERS,
};

const SEED: u64 = 2026;

const DEFAULT_PLAYERS: usize = 10_000;
const DEFAULT_SETTLEMENTS: usize = 10_000;
const DEFAULT_MEMBERS: usize = MAX_SETTLEMENT_MEMBERS;
const DEFAULT_RUNS: usize = 3;

const SMOKE_PLAYERS: usize = 200;
const SMOKE_SETTLEMENTS: usize = 50;
const SMOKE_MEMBERS: usize = 8;
const SMOKE_RUNS: usize = 1;

/// Upper bound for `--runs`; keeps the harness cheap to run by accident.
const MAX_RUNS: usize = 9;

/// Pinned checksums for the default smoke fixture (200 players, 50
/// settlements, 8 members). A change is either an intentional fixture or save
/// format change, or a regression; update both pins together when intentional.
const SMOKE_PLAYERS_CHECKSUM: u64 = 0xb3d8_8928_9837_0243;
const SMOKE_SETTLEMENTS_CHECKSUM: u64 = 0x3fb7_fc64_a225_def9;

/// Fixed read-side scenario: the D-033 per-store cap, one member mix.
const LIST_COUNT: usize = MAX_SETTLEMENTS;
const LIST_MEMBERS_PER_OWNED: usize = MAX_SETTLEMENT_MEMBERS;
/// Player slots used to spread ownership and membership; the owned-record cap
/// stays far below [`MAX_SETTLEMENTS_OWNED_PER_PLAYER`] at this spread.
const LIST_PLAYER_SLOTS: usize = MAX_PLAYERS;
/// Every fifth settlement is unowned, so the mix is 800 owned / 200 unowned.
const LIST_OWNED_STRIDE: usize = 5;
const LIST_PAGE_LIMIT: usize = 256;

/// Pinned smoke values for the fixed settlement-list fixture. Update all three
/// together when the fixture or settlement save format changes intentionally.
const LIST_SMOKE_CHECKSUM: u64 = 0x34d0_aa17_5625_6e95;
const LIST_SMOKE_JSON_BYTES: usize = 1_733_035;
const LIST_SMOKE_JSON_CHECKSUM: u64 = 0x34d0_aa17_5625_6e95;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("SETTLEMENTS_BENCH status=error error={error}");
            ExitCode::FAILURE
        }
    }
}

/// One resolved benchmark configuration.
struct Config {
    smoke: bool,
    players: usize,
    settlements: usize,
    members: usize,
    runs: usize,
    requested_players: usize,
    requested_settlements: usize,
    requested_members: usize,
}

impl Config {
    fn capped(&self) -> bool {
        self.players != self.requested_players
            || self.settlements != self.requested_settlements
            || self.members != self.requested_members
    }
}

/// Per-phase wall times for one build, in milliseconds.
#[derive(Clone, Copy, Default)]
struct Phases {
    register: f64,
    found: f64,
    invite: f64,
    tax: f64,
    rename: f64,
    transfer: f64,
    checksum: f64,
    serialize: f64,
    parse: f64,
}

/// Timed and verified result of one complete build.
struct RunResult {
    phases: Phases,
    members_total: usize,
    players_checksum: u64,
    settlements_checksum: u64,
    save_bytes: usize,
    players_bytes: usize,
    settlements_bytes: usize,
    tax_updates: usize,
    renames: usize,
    transfers: usize,
}

fn run() -> Result<(), String> {
    let Some(config) = parse_config()? else {
        return Ok(());
    };

    let mut runs = Vec::with_capacity(config.runs);
    for _ in 0..config.runs {
        runs.push(build_run(&config)?);
    }

    let first = runs.first().ok_or("no runs were executed")?;
    for (index, run) in runs.iter().enumerate().skip(1) {
        check(run.players_checksum == first.players_checksum, || {
            format!(
                "run {index} player checksum 0x{:016x} differs from 0x{:016x}",
                run.players_checksum, first.players_checksum
            )
        })?;
        check(
            run.settlements_checksum == first.settlements_checksum,
            || {
                format!(
                    "run {index} settlement checksum 0x{:016x} differs from 0x{:016x}",
                    run.settlements_checksum, first.settlements_checksum
                )
            },
        )?;
        check(run.members_total == first.members_total, || {
            format!(
                "run {index} member total {} differs from {}",
                run.members_total, first.members_total
            )
        })?;
        check(run.save_bytes == first.save_bytes, || {
            format!(
                "run {index} save size {} differs from {}",
                run.save_bytes, first.save_bytes
            )
        })?;
    }

    if config.smoke {
        let repeat = build_run(&config)?;
        check(
            repeat.players_checksum == first.players_checksum
                && repeat.settlements_checksum == first.settlements_checksum,
            || "smoke checksums are not deterministic across two builds".to_string(),
        )?;
        check(
            repeat.save_bytes == first.save_bytes && repeat.members_total == first.members_total,
            || "smoke save size or member total changed across two builds".to_string(),
        )?;
        if config.requested_players == SMOKE_PLAYERS
            && config.requested_settlements == SMOKE_SETTLEMENTS
            && config.requested_members == SMOKE_MEMBERS
        {
            check(first.players_checksum == SMOKE_PLAYERS_CHECKSUM, || {
                format!(
                        "smoke player checksum 0x{:016x} drifted from pinned 0x{SMOKE_PLAYERS_CHECKSUM:016x}",
                        first.players_checksum
                    )
            })?;
            check(
                first.settlements_checksum == SMOKE_SETTLEMENTS_CHECKSUM,
                || {
                    format!(
                        "smoke settlement checksum 0x{:016x} drifted from pinned 0x{SMOKE_SETTLEMENTS_CHECKSUM:016x}",
                        first.settlements_checksum
                    )
                },
            )?;
        }
    }

    let phases = Phases {
        register: median(&runs.iter().map(|r| r.phases.register).collect::<Vec<_>>()),
        found: median(&runs.iter().map(|r| r.phases.found).collect::<Vec<_>>()),
        invite: median(&runs.iter().map(|r| r.phases.invite).collect::<Vec<_>>()),
        tax: median(&runs.iter().map(|r| r.phases.tax).collect::<Vec<_>>()),
        rename: median(&runs.iter().map(|r| r.phases.rename).collect::<Vec<_>>()),
        transfer: median(&runs.iter().map(|r| r.phases.transfer).collect::<Vec<_>>()),
        checksum: median(&runs.iter().map(|r| r.phases.checksum).collect::<Vec<_>>()),
        serialize: median(&runs.iter().map(|r| r.phases.serialize).collect::<Vec<_>>()),
        parse: median(&runs.iter().map(|r| r.phases.parse).collect::<Vec<_>>()),
    };

    print_summary(&config, first, &phases);

    let json = serde_json::json!({
        "scene": "settlements_v1",
        "smoke": config.smoke,
        "runs": config.runs,
        "players": config.players,
        "requested_players": config.requested_players,
        "settlements": config.settlements,
        "requested_settlements": config.requested_settlements,
        "members_per_settlement": config.members,
        "requested_members_per_settlement": config.requested_members,
        "members_total": first.members_total,
        "register_ms": round4(phases.register),
        "found_ms": round4(phases.found),
        "invite_ms": round4(phases.invite),
        "tax_ms": round4(phases.tax),
        "rename_ms": round4(phases.rename),
        "transfer_ms": round4(phases.transfer),
        "checksum_ms": round4(phases.checksum),
        "serialize_ms": round4(phases.serialize),
        "parse_ms": round4(phases.parse),
        "tax_updates": first.tax_updates,
        "renames": first.renames,
        "transfers": first.transfers,
        "players_checksum": format!("0x{:016x}", first.players_checksum),
        "settlements_checksum": format!("0x{:016x}", first.settlements_checksum),
        "save_bytes": first.save_bytes,
        "players_bytes": first.players_bytes,
        "settlements_bytes": first.settlements_bytes,
        "capped": config.capped(),
        "status": "ok",
    });
    println!("SETTLEMENTS_BENCH {json}");

    // Second phase: the fixed settlement-list read/serialization scene. It is
    // built and measured after the cap/construction benchmark above so the
    // existing output stays first and unchanged.
    let list = run_list_phase(&config)?;
    print_list_summary(&list);
    println!("SETTLEMENT_LIST_BENCH {}", list.output);
    Ok(())
}

fn print_summary(config: &Config, result: &RunResult, phases: &Phases) {
    println!(
        "settlements_bench: players={}{} settlements={}{} members/settlement={} members_total={} runs={}",
        config.players,
        cap_note(config.players, config.requested_players),
        config.settlements,
        cap_note(config.settlements, config.requested_settlements),
        config.members,
        result.members_total,
        config.runs
    );
    if config.capped() {
        println!(
            "  core caps: MAX_PLAYERS={MAX_PLAYERS} MAX_SETTLEMENTS={MAX_SETTLEMENTS} \
             MAX_SETTLEMENT_MEMBERS={MAX_SETTLEMENT_MEMBERS} \
             MAX_SETTLEMENTS_OWNED_PER_PLAYER={MAX_SETTLEMENTS_OWNED_PER_PLAYER}"
        );
    }
    println!("  register  {:>10.4} ms", phases.register);
    println!("  found     {:>10.4} ms", phases.found);
    println!("  invite    {:>10.4} ms", phases.invite);
    println!("  tax       {:>10.4} ms", phases.tax);
    println!("  rename    {:>10.4} ms", phases.rename);
    println!("  transfer  {:>10.4} ms", phases.transfer);
    println!("  checksum  {:>10.4} ms", phases.checksum);
    println!("  serialize {:>10.4} ms", phases.serialize);
    println!("  parse     {:>10.4} ms", phases.parse);
    println!(
        "  players_checksum=0x{:016x} settlements_checksum=0x{:016x} save_bytes={}",
        result.players_checksum, result.settlements_checksum, result.save_bytes
    );
}

fn cap_note(effective: usize, requested: usize) -> String {
    if effective == requested {
        String::new()
    } else {
        format!(" (requested {requested})")
    }
}

fn parse_config() -> Result<Option<Config>, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_usage();
        return Ok(None);
    }
    for arg in &args {
        let known = matches!(
            arg.as_str(),
            "--smoke" | "--players" | "--settlements" | "--members" | "--runs"
        );
        if arg.starts_with("--") && !known {
            return Err(format!("unknown option {arg}"));
        }
    }

    let smoke = args.iter().any(|arg| arg == "--smoke");
    let (players, settlements, members, runs) = if smoke {
        (SMOKE_PLAYERS, SMOKE_SETTLEMENTS, SMOKE_MEMBERS, SMOKE_RUNS)
    } else {
        (
            DEFAULT_PLAYERS,
            DEFAULT_SETTLEMENTS,
            DEFAULT_MEMBERS,
            DEFAULT_RUNS,
        )
    };

    let requested_players = option(&args, "--players")?.unwrap_or(players);
    let requested_settlements = option(&args, "--settlements")?.unwrap_or(settlements);
    let requested_members = option(&args, "--members")?.unwrap_or(members);
    let requested_runs = option(&args, "--runs")?.unwrap_or(runs);

    let effective_players = requested_players.clamp(1, MAX_PLAYERS);
    let owned_slots = effective_players.saturating_mul(MAX_SETTLEMENTS_OWNED_PER_PLAYER);
    let effective_settlements = requested_settlements.clamp(1, MAX_SETTLEMENTS.min(owned_slots));
    let effective_members = requested_members.clamp(1, MAX_SETTLEMENT_MEMBERS);
    let effective_runs = requested_runs.clamp(1, MAX_RUNS);

    Ok(Some(Config {
        smoke,
        players: effective_players,
        settlements: effective_settlements,
        members: effective_members,
        runs: effective_runs,
        requested_players,
        requested_settlements,
        requested_members,
    }))
}

fn option(args: &[String], name: &str) -> Result<Option<usize>, String> {
    let Some(index) = args.iter().position(|arg| arg == name) else {
        return Ok(None);
    };
    let raw = args
        .get(index + 1)
        .ok_or_else(|| format!("{name} needs a value"))?;
    let value = raw
        .parse::<usize>()
        .map_err(|_| format!("{name} value {raw:?} is not a non-negative integer"))?;
    Ok(Some(value))
}

fn print_usage() {
    println!(
        "usage: settlements [--smoke] [--players N] [--settlements N] [--members N] [--runs N]"
    );
    println!(
        "  default: {} players, {} settlements, {} members, {} runs (median)",
        DEFAULT_PLAYERS, DEFAULT_SETTLEMENTS, DEFAULT_MEMBERS, DEFAULT_RUNS
    );
    println!(
        "  smoke:   {} players, {} settlements, {} members, {} run + determinism rebuild",
        SMOKE_PLAYERS, SMOKE_SETTLEMENTS, SMOKE_MEMBERS, SMOKE_RUNS
    );
    println!(
        "  effective sizes are clamped to the D-033 save caps (players {MAX_PLAYERS}, \
         settlements {MAX_SETTLEMENTS}, members/settlement {MAX_SETTLEMENT_MEMBERS})"
    );
}

fn build_run(config: &Config) -> Result<RunResult, String> {
    let players = config.players;
    let settlements = config.settlements;
    let member_target = config.members.clamp(1, players);

    // Phase 1: player registration.
    let mut registry = PlayerRegistry::new(SEED);
    let mut ids: Vec<PlayerId> = Vec::with_capacity(players);
    let start = Instant::now();
    for index in 0..players {
        let player = registry
            .register(&format!("Player {index:05}"))
            .map_err(|error| format!("register player {index}: {error}"))?;
        ids.push(player.id);
    }
    let register_ms = elapsed_ms(start);

    check(registry.count() == players, || {
        format!(
            "registered {} players, expected {players}",
            registry.count()
        )
    })?;
    let mut seen = HashSet::with_capacity(players);
    for (index, player) in registry.players().iter().enumerate() {
        check(player.id.get() == index as u64 + 1, || {
            format!(
                "player id {} is not the expected {}",
                player.id,
                index as u64 + 1
            )
        })?;
        check(seen.insert(player.id), || {
            format!("duplicate player id {}", player.id)
        })?;
        check(!player.display_name.is_empty(), || {
            format!("player {} has an empty name", player.id)
        })?;
    }
    check(registry.next_id == players as u64 + 1, || {
        format!(
            "player next_id {} is not {}",
            registry.next_id,
            players as u64 + 1
        )
    })?;

    // Phase 2: settlement founding, one owner per settlement in id order.
    let mut store = SettlementStore::new(SEED);
    let mut settlement_ids: Vec<u64> = Vec::with_capacity(settlements);
    let mut owners: Vec<PlayerId> = Vec::with_capacity(settlements);
    let start = Instant::now();
    for index in 0..settlements {
        let owner = ids[index % players];
        let owner_name = registry
            .get(owner)
            .ok_or_else(|| format!("registered owner {owner} is missing"))?
            .display_name
            .clone();
        let request = SettlementRequest {
            name: settlement_name(index),
            owner: owner_name,
            center: [(index % 512) as i32 * 64, (index / 512) as i32 * 64],
            radius_m: 64 + (index % 29) as u32 * 64,
        };
        let founded = store
            .found_for_player(&request, owner)
            .map_err(|error| format!("found settlement {index}: {error}"))?;
        settlement_ids.push(founded.id);
        owners.push(owner);
    }
    let found_ms = elapsed_ms(start);

    check(store.count() == settlements, || {
        format!(
            "founded {} settlements, expected {settlements}",
            store.count()
        )
    })?;
    let mut settlement_seen = HashSet::with_capacity(settlements);
    for (index, settlement) in store.settlements().iter().enumerate() {
        check(settlement.id == index as u64 + 1, || {
            format!(
                "settlement id {} is not the expected {}",
                settlement.id,
                index as u64 + 1
            )
        })?;
        check(settlement_seen.insert(settlement.id), || {
            format!("duplicate settlement id {}", settlement.id)
        })?;
        check(settlement.owner_id == Some(owners[index]), || {
            format!("settlement {} owner is not the founder", settlement.id)
        })?;
        check(settlement.members.len() == 1, || {
            format!(
                "new settlement {} has {} members",
                settlement.id,
                settlement.members.len()
            )
        })?;
        check(
            settlement.members[0].player == owners[index]
                && settlement.members[0].role == SettlementRole::Owner,
            || {
                format!(
                    "settlement {} founder is not its owner member",
                    settlement.id
                )
            },
        )?;
        check(settlement.membership_revision == 1, || {
            format!("settlement {} revision started above 1", settlement.id)
        })?;
    }

    // Phase 3: membership fill. Invites are revision-checked on every edit.
    let mut revisions: Vec<u32> = store
        .settlements()
        .iter()
        .map(|settlement| settlement.membership_revision)
        .collect();
    let mut members_total = 0usize;
    let start = Instant::now();
    for index in 0..settlements {
        let owner = owners[index];
        let id = settlement_ids[index];
        let mut members: HashSet<PlayerId> = HashSet::with_capacity(member_target);
        members.insert(owner);
        let mut revision = revisions[index];
        let mut slot = 0usize;
        let mut attempts = 0usize;
        while members.len() < member_target {
            attempts += 1;
            check(attempts <= players, || {
                format!("settlement {id}: could not find {member_target} distinct members")
            })?;
            let candidate = ids[(index * 7 + 3 + slot) % players];
            slot += 1;
            if !members.insert(candidate) {
                continue;
            }
            let role = role_for(slot);
            let updated = store
                .invite(owner, id, candidate, role, revision)
                .map_err(|error| format!("invite player {candidate} to {id}: {error}"))?;
            revision += 1;
            check(updated.membership_revision == revision, || {
                format!(
                    "settlement {id} revision {} is not {revision} after invite",
                    updated.membership_revision
                )
            })?;
        }
        revisions[index] = revision;
        members_total += members.len();
    }
    let invite_ms = elapsed_ms(start);

    check(members_total == settlements * member_target, || {
        format!(
            "member total {members_total} is not {}",
            settlements * member_target
        )
    })?;
    let mut verified_total = 0usize;
    for (index, settlement) in store.settlements().iter().enumerate() {
        check(settlement.members.len() == member_target, || {
            format!(
                "settlement {} has {} members, expected {member_target}",
                settlement.id,
                settlement.members.len()
            )
        })?;
        check(
            settlement
                .members
                .windows(2)
                .all(|pair| pair[0].player < pair[1].player),
            || format!("settlement {} members are not sorted", settlement.id),
        )?;
        check(settlement.membership_revision == revisions[index], || {
            format!(
                "settlement {} revision {} is not the tracked {}",
                settlement.id, settlement.membership_revision, revisions[index]
            )
        })?;
        let owner = settlement
            .owner_id
            .ok_or_else(|| format!("settlement {} lost its owner", settlement.id))?;
        check(owner == owners[index], || {
            format!("settlement {} owner changed", settlement.id)
        })?;
        let owner_roles = settlement
            .members
            .iter()
            .filter(|member| member.role.is_owner())
            .count();
        check(owner_roles == 1, || {
            format!("settlement {} has {owner_roles} owner roles", settlement.id)
        })?;
        check(
            settlement.role_of(owner) == Some(SettlementRole::Owner),
            || format!("settlement {} owner is missing from members", settlement.id),
        )?;
        verified_total += settlement.members.len();
    }
    check(verified_total == members_total, || {
        "verified member total differs from the fill total".to_string()
    })?;

    // Phase 4: tax updates. Tax is data-only and must not bump the revision.
    let start = Instant::now();
    for index in 0..settlements {
        let rate = tax_rate_for(index);
        let updated = store
            .set_tax_rate(owners[index], settlement_ids[index], rate)
            .map_err(|error| format!("set tax on settlement {}: {error}", settlement_ids[index]))?;
        check(
            updated.tax_rate == rate && updated.membership_revision == revisions[index],
            || {
                format!(
                    "settlement {} tax update changed rate/revision unexpectedly",
                    settlement_ids[index]
                )
            },
        )?;
    }
    let tax_ms = elapsed_ms(start);

    for (index, settlement) in store.settlements().iter().enumerate() {
        check(settlement.tax_rate == tax_rate_for(index), || {
            format!("settlement {} tax rate did not stick", settlement.id)
        })?;
        check(settlement.membership_revision == revisions[index], || {
            format!(
                "settlement {} revision changed during tax updates",
                settlement.id
            )
        })?;
    }

    // Phase 5: renames. Every rename bumps the revision by exactly one.
    let start = Instant::now();
    for index in 0..settlements {
        let name = renamed_name(index);
        let expected = revisions[index] + 1;
        let updated = store
            .rename(
                owners[index],
                settlement_ids[index],
                &name,
                revisions[index],
            )
            .map_err(|error| format!("rename settlement {}: {error}", settlement_ids[index]))?;
        check(
            updated.name == name && updated.membership_revision == expected,
            || {
                format!(
                    "settlement {} rename left name or revision stale",
                    settlement_ids[index]
                )
            },
        )?;
        revisions[index] = expected;
    }
    let rename_ms = elapsed_ms(start);

    for (index, settlement) in store.settlements().iter().enumerate() {
        check(settlement.name == renamed_name(index), || {
            format!("settlement {} name did not stick", settlement.id)
        })?;
        check(settlement.membership_revision == revisions[index], || {
            format!(
                "settlement {} revision is not monotonic after rename",
                settlement.id
            )
        })?;
    }

    // Phase 6: ownership transfers to an existing member with a spare
    // ownership slot. A full member list cannot accept a non-member recipient,
    // so the recipient always comes from the settlement's own members.
    let mut owned: HashMap<PlayerId, usize> = HashMap::with_capacity(players);
    for owner in &owners {
        *owned.entry(*owner).or_insert(0) += 1;
    }
    let mut recipients: Vec<Option<PlayerId>> = vec![None; settlements];
    let mut transfers = 0usize;
    let start = Instant::now();
    if players >= 2 {
        for index in 0..settlements {
            let previous = owners[index];
            let settlement = store
                .get(settlement_ids[index])
                .ok_or_else(|| format!("settlement {} disappeared", settlement_ids[index]))?;
            let member_count = settlement.members.len();
            let mut recipient = None;
            for offset in 0..member_count {
                let member = settlement.members[(index + offset) % member_count].player;
                if member == previous {
                    continue;
                }
                if owned.get(&member).copied().unwrap_or(0) < MAX_SETTLEMENTS_OWNED_PER_PLAYER {
                    recipient = Some(member);
                    break;
                }
            }
            let Some(recipient) = recipient else {
                // Only happens with a one-member settlement; leave it untouched.
                continue;
            };
            let updated = store
                .transfer(previous, settlement_ids[index], recipient, revisions[index])
                .map_err(|error| {
                    format!("transfer settlement {}: {error}", settlement_ids[index])
                })?;
            revisions[index] += 1;
            check(updated.membership_revision == revisions[index], || {
                format!(
                    "settlement {} revision is not monotonic after transfer",
                    settlement_ids[index]
                )
            })?;
            *owned.get_mut(&previous).expect("previous owner counted") -= 1;
            *owned.entry(recipient).or_insert(0) += 1;
            recipients[index] = Some(recipient);
            transfers += 1;
        }
    }
    let transfer_ms = elapsed_ms(start);

    let mut final_owned: HashMap<PlayerId, usize> = HashMap::with_capacity(players);
    for (index, settlement) in store.settlements().iter().enumerate() {
        let Some(recipient) = recipients[index] else {
            check(settlement.membership_revision == revisions[index], || {
                format!(
                    "settlement {} revision changed without a transfer",
                    settlement.id
                )
            })?;
            continue;
        };
        check(settlement.owner_id == Some(recipient), || {
            format!("settlement {} did not transfer ownership", settlement.id)
        })?;
        check(
            settlement.role_of(recipient) == Some(SettlementRole::Owner),
            || {
                format!(
                    "settlement {} recipient is not the owner role",
                    settlement.id
                )
            },
        )?;
        check(
            settlement.role_of(owners[index]) == Some(SettlementRole::Manager),
            || {
                format!(
                    "settlement {} previous owner was not demoted",
                    settlement.id
                )
            },
        )?;
        check(settlement.membership_revision == revisions[index], || {
            format!(
                "settlement {} revision is stale after transfer",
                settlement.id
            )
        })?;
        let count = final_owned.entry(recipient).or_insert(0);
        *count += 1;
        check(*count <= MAX_SETTLEMENTS_OWNED_PER_PLAYER, || {
            format!("player {recipient} exceeds the ownership cap")
        })?;
    }

    // Phase 7: checksums, including an in-memory stability repeat.
    let start = Instant::now();
    let players_checksum = registry.checksum();
    let settlements_checksum = store.checksum();
    let checksum_repeat = store.checksum();
    let players_repeat = registry.checksum();
    let checksum_ms = elapsed_ms(start);
    check(checksum_repeat == settlements_checksum, || {
        "settlement checksum is not stable in memory".to_string()
    })?;
    check(players_repeat == players_checksum, || {
        "player checksum is not stable in memory".to_string()
    })?;

    // Phase 8: in-memory JSON serialization; no temp files are written.
    let start = Instant::now();
    let settlements_json = serde_json::to_vec(&store)
        .map_err(|error| format!("serialize settlement store: {error}"))?;
    let players_json = serde_json::to_vec(&registry)
        .map_err(|error| format!("serialize player registry: {error}"))?;
    let serialize_ms = elapsed_ms(start);
    check(
        !settlements_json.is_empty() && !players_json.is_empty(),
        || "serialization produced an empty payload".to_string(),
    )?;

    // Phase 9: parse, validate, and save again; the second save must be
    // byte-identical to the first so checksums stay stable across a reload.
    let start = Instant::now();
    let decoded_store: SettlementStore = serde_json::from_slice(&settlements_json)
        .map_err(|error| format!("parse settlement store: {error}"))?;
    decoded_store
        .validate_loaded()
        .map_err(|error| format!("validate settlement store: {error}"))?;
    let decoded_players: PlayerRegistry = serde_json::from_slice(&players_json)
        .map_err(|error| format!("parse player registry: {error}"))?;
    decoded_players
        .validate_loaded()
        .map_err(|error| format!("validate player registry: {error}"))?;
    let resaved_store = serde_json::to_vec(&decoded_store)
        .map_err(|error| format!("re-serialize settlement store: {error}"))?;
    let resaved_players = serde_json::to_vec(&decoded_players)
        .map_err(|error| format!("re-serialize player registry: {error}"))?;
    let parse_ms = elapsed_ms(start);

    check(decoded_store == store, || {
        "round-tripped settlement store differs from the source".to_string()
    })?;
    check(decoded_players == registry, || {
        "round-tripped player registry differs from the source".to_string()
    })?;
    check(
        decoded_store.checksum() == settlements_checksum
            && decoded_players.checksum() == players_checksum,
        || "checksums changed after a JSON round trip".to_string(),
    )?;
    check(
        resaved_store == settlements_json && resaved_players == players_json,
        || "second save is not byte-identical to the first".to_string(),
    )?;
    check(
        decoded_store.count() == settlements && decoded_players.count() == players,
        || "round-tripped counts changed".to_string(),
    )?;

    let save_bytes = settlements_json.len() + players_json.len();
    black_box(&decoded_store);

    Ok(RunResult {
        phases: Phases {
            register: register_ms,
            found: found_ms,
            invite: invite_ms,
            tax: tax_ms,
            rename: rename_ms,
            transfer: transfer_ms,
            checksum: checksum_ms,
            serialize: serialize_ms,
            parse: parse_ms,
        },
        members_total,
        players_checksum,
        settlements_checksum,
        save_bytes,
        players_bytes: players_json.len(),
        settlements_bytes: settlements_json.len(),
        tax_updates: settlements,
        renames: settlements,
        transfers,
    })
}

fn settlement_name(index: usize) -> String {
    format!("Settlement {index:05}")
}

fn renamed_name(index: usize) -> String {
    format!("Town {index:05}")
}

/// One measured read pass over the fixed 1,000-settlement fixture.
struct ListSample {
    list_copy_ms: f64,
    json_ms: f64,
    checksum_ms: f64,
    paged_ms: f64,
    count: usize,
    owned: usize,
    unowned: usize,
    members: usize,
    pages: usize,
    paged_records: usize,
    checksum: u64,
    json_bytes: usize,
    json_checksum: u64,
    json: Vec<u8>,
}

/// Median read timings and verified fixture identity for the fixed scene.
struct ListResult {
    count: usize,
    owned: usize,
    unowned: usize,
    members: usize,
    runs: usize,
    list_copy_ms: f64,
    json_ms: f64,
    checksum_ms: f64,
    paged_ms: f64,
    page_limit: usize,
    pages: usize,
    paged_records: usize,
    checksum: u64,
    json_bytes: usize,
    json_checksum: u64,
    output: String,
}

/// Build and measure the fixed 1,000-settlement list scene.
///
/// The fixture is rebuilt for every timed pass so each sample starts from the
/// same owned/unowned and membership shape. Only the read-side operations are
/// timed; construction is setup and is not part of this scenario.
fn run_list_phase(config: &Config) -> Result<ListResult, String> {
    let mut runs = Vec::with_capacity(config.runs);
    for _ in 0..config.runs {
        let store = build_list_fixture()?;
        runs.push(measure_list(&store)?);
    }

    let first = runs
        .first()
        .ok_or("no settlement-list runs were executed")?;
    for (index, run) in runs.iter().enumerate().skip(1) {
        check(run.checksum == first.checksum, || {
            format!(
                "list run {index} checksum 0x{:016x} differs from 0x{:016x}",
                run.checksum, first.checksum
            )
        })?;
        check(run.json_bytes == first.json_bytes, || {
            format!(
                "list run {index} JSON size {} differs from {}",
                run.json_bytes, first.json_bytes
            )
        })?;
        check(run.json_checksum == first.json_checksum, || {
            format!(
                "list run {index} JSON checksum 0x{:016x} differs from 0x{:016x}",
                run.json_checksum, first.json_checksum
            )
        })?;
        check(run.json == first.json, || {
            format!("list run {index} JSON bytes differ from run 0")
        })?;
        check(
            run.members == first.members
                && run.owned == first.owned
                && run.unowned == first.unowned,
            || format!("list run {index} fixture shape differs from run 0"),
        )?;
        check(
            run.pages == first.pages && run.paged_records == first.paged_records,
            || format!("list run {index} paged shape differs from run 0"),
        )?;
    }

    if config.smoke {
        let store = build_list_fixture()?;
        let repeat = measure_list(&store)?;
        check(
            repeat.checksum == first.checksum
                && repeat.json_bytes == first.json_bytes
                && repeat.json_checksum == first.json_checksum
                && repeat.json == first.json,
            || "smoke settlement-list fixture is not deterministic across two builds".to_string(),
        )?;
        check(first.checksum == LIST_SMOKE_CHECKSUM, || {
            format!(
                "smoke list checksum 0x{:016x} drifted from pinned 0x{LIST_SMOKE_CHECKSUM:016x}",
                first.checksum
            )
        })?;
        check(first.json_bytes == LIST_SMOKE_JSON_BYTES, || {
            format!(
                "smoke list JSON size {} drifted from pinned {LIST_SMOKE_JSON_BYTES}",
                first.json_bytes
            )
        })?;
        check(first.json_checksum == LIST_SMOKE_JSON_CHECKSUM, || {
            format!(
                "smoke list JSON checksum 0x{:016x} drifted from pinned 0x{LIST_SMOKE_JSON_CHECKSUM:016x}",
                first.json_checksum
            )
        })?;
    }

    let mut result = ListResult {
        count: first.count,
        owned: first.owned,
        unowned: first.unowned,
        members: first.members,
        runs: config.runs,
        list_copy_ms: median(&runs.iter().map(|run| run.list_copy_ms).collect::<Vec<_>>()),
        json_ms: median(&runs.iter().map(|run| run.json_ms).collect::<Vec<_>>()),
        checksum_ms: median(&runs.iter().map(|run| run.checksum_ms).collect::<Vec<_>>()),
        paged_ms: median(&runs.iter().map(|run| run.paged_ms).collect::<Vec<_>>()),
        page_limit: LIST_PAGE_LIMIT,
        pages: first.pages,
        paged_records: first.paged_records,
        checksum: first.checksum,
        json_bytes: first.json_bytes,
        json_checksum: first.json_checksum,
        output: String::new(),
    };

    let output = serde_json::json!({
        "scenario": "settlement_list_1000",
        "count": result.count,
        "owned": result.owned,
        "unowned": result.unowned,
        "members": result.members,
        "members_per_owned": LIST_MEMBERS_PER_OWNED,
        "runs": result.runs,
        "median_ms": {
            "list_copy": round4(result.list_copy_ms),
            "json": round4(result.json_ms),
            "checksum": round4(result.checksum_ms),
            "paged": round4(result.paged_ms),
        },
        "json_bytes": result.json_bytes,
        "json_checksum": format!("0x{:016x}", result.json_checksum),
        "checksum": format!("0x{:016x}", result.checksum),
        "page_limit": result.page_limit,
        "pages": result.pages,
        "paged_records": result.paged_records,
        "paging": "client_slice_over_settlements",
        "status": "ok",
    });
    result.output = output.to_string();
    Ok(result)
}

fn print_list_summary(result: &ListResult) {
    println!(
        "settlement_list_bench: scenario=settlement_list_1000 count={} owned={} unowned={} \
         members={} runs={} page_limit={} pages={}",
        result.count,
        result.owned,
        result.unowned,
        result.members,
        result.runs,
        result.page_limit,
        result.pages
    );
    println!("  list_copy {:>10.4} ms", result.list_copy_ms);
    println!(
        "  json      {:>10.4} ms ({} bytes, json_checksum=0x{:016x})",
        result.json_ms, result.json_bytes, result.json_checksum
    );
    println!(
        "  checksum  {:>10.4} ms (0x{:016x})",
        result.checksum_ms, result.checksum
    );
    println!(
        "  paged     {:>10.4} ms ({} records in {} pages)",
        result.paged_ms, result.paged_records, result.pages
    );
}

/// Build the fixed list fixture: 1,000 settlements, 200 unowned, 800 owned with
/// 64 members each (51,200 member records), fixed seed, ids, and names.
fn build_list_fixture() -> Result<SettlementStore, String> {
    let mut registry = PlayerRegistry::new(SEED);
    let mut ids = Vec::with_capacity(LIST_PLAYER_SLOTS);
    for index in 0..LIST_PLAYER_SLOTS {
        let player = registry
            .register(&format!("Player {index:03}"))
            .map_err(|error| format!("register list player {index}: {error}"))?;
        ids.push(player.id);
    }

    let mut store = SettlementStore::new(SEED);
    for index in 0..LIST_COUNT {
        let owned = index % LIST_OWNED_STRIDE != 0;
        let owner = ids[index % LIST_PLAYER_SLOTS];
        let request = SettlementRequest {
            name: settlement_name(index),
            owner: registry
                .get(owner)
                .ok_or_else(|| format!("list owner {owner} is missing"))?
                .display_name
                .clone(),
            center: [(index % 64) as i32 * 128, (index / 64) as i32 * 128],
            radius_m: 64 + (index % 29) as u32 * 64,
        };
        if owned {
            let founded = store
                .found_for_player(&request, owner)
                .map_err(|error| format!("found owned list settlement {index}: {error}"))?;
            check(founded.id == index as u64 + 1, || {
                format!("owned list settlement {index} id is {}", founded.id)
            })?;
            let mut revision = founded.membership_revision;
            let mut members: HashSet<PlayerId> = HashSet::with_capacity(LIST_MEMBERS_PER_OWNED);
            members.insert(owner);
            let mut slot = 1usize;
            let mut attempts = 0usize;
            while members.len() < LIST_MEMBERS_PER_OWNED {
                attempts += 1;
                check(attempts <= LIST_PLAYER_SLOTS, || {
                    format!(
                        "list settlement {index}: could not fill \
                         {LIST_MEMBERS_PER_OWNED} distinct members"
                    )
                })?;
                let candidate = ids[(index * 7 + 3 + attempts) % LIST_PLAYER_SLOTS];
                if !members.insert(candidate) {
                    continue;
                }
                let updated = store
                    .invite(owner, founded.id, candidate, role_for(slot), revision)
                    .map_err(|error| {
                        format!("invite player {candidate} to list settlement {index}: {error}")
                    })?;
                revision = updated.membership_revision;
                slot += 1;
            }
            check(revision == LIST_MEMBERS_PER_OWNED as u32, || {
                format!(
                    "list settlement {index} revision {revision} is not \
                     {LIST_MEMBERS_PER_OWNED}"
                )
            })?;
        } else {
            let founded = store
                .found(&request)
                .map_err(|error| format!("found unowned list settlement {index}: {error}"))?;
            check(
                founded.id == index as u64 + 1 && founded.members.is_empty(),
                || {
                    format!(
                        "unowned list settlement {index} has id {} or members",
                        founded.id
                    )
                },
            )?;
        }
    }

    store
        .validate_loaded()
        .map_err(|error| format!("validate list fixture: {error}"))?;

    let mut owned_count = 0usize;
    let mut member_count = 0usize;
    let mut previous_id = 0u64;
    for settlement in store.settlements() {
        check(settlement.id > previous_id, || {
            format!(
                "list settlement ids are not strictly ascending at {}",
                settlement.id
            )
        })?;
        previous_id = settlement.id;
        if settlement.is_owned() {
            owned_count += 1;
            check(
                settlement.members.len() == LIST_MEMBERS_PER_OWNED
                    && settlement.membership_revision == LIST_MEMBERS_PER_OWNED as u32,
                || {
                    format!(
                        "list settlement {} has {} members and revision {}, expected {}",
                        settlement.id,
                        settlement.members.len(),
                        settlement.membership_revision,
                        LIST_MEMBERS_PER_OWNED
                    )
                },
            )?;
            check(
                settlement
                    .owner_id
                    .is_some_and(|owner| settlement.role_of(owner) == Some(SettlementRole::Owner)),
                || format!("list settlement {} owner role is missing", settlement.id),
            )?;
        } else {
            check(settlement.members.is_empty(), || {
                format!("unowned list settlement {} carries members", settlement.id)
            })?;
        }
        member_count += settlement.members.len();
    }

    let expected_owned = LIST_COUNT - LIST_COUNT.div_ceil(LIST_OWNED_STRIDE);
    let expected_unowned = LIST_COUNT.div_ceil(LIST_OWNED_STRIDE);
    check(store.count() == LIST_COUNT, || {
        format!(
            "list fixture has {} settlements, expected {LIST_COUNT}",
            store.count()
        )
    })?;
    check(owned_count == expected_owned, || {
        format!("list fixture has {owned_count} owned, expected {expected_owned}")
    })?;
    check(store.count() - owned_count == expected_unowned, || {
        format!(
            "list fixture has {} unowned, expected {expected_unowned}",
            store.count() - owned_count
        )
    })?;
    check(
        member_count == expected_owned * LIST_MEMBERS_PER_OWNED,
        || {
            format!(
                "list fixture has {member_count} member records, expected {}",
                expected_owned * LIST_MEMBERS_PER_OWNED
            )
        },
    )?;

    Ok(store)
}

/// Time the four read-side operations once over an already-built fixture.
fn measure_list(store: &SettlementStore) -> Result<ListSample, String> {
    let start = Instant::now();
    let copy = store.settlements().to_vec();
    let list_copy_ms = elapsed_ms(start);
    check(copy.len() == store.count(), || {
        "list copy length differs from the store count".to_string()
    })?;
    check(copy == store.settlements(), || {
        "list copy differs from the store slice".to_string()
    })?;
    black_box(&copy);
    drop(copy);

    let start = Instant::now();
    let json =
        serde_json::to_vec(store).map_err(|error| format!("serialize settlement list: {error}"))?;
    let json_ms = elapsed_ms(start);
    check(!json.is_empty(), || {
        "settlement-list serialization produced an empty payload".to_string()
    })?;

    let start = Instant::now();
    let checksum = store.checksum();
    let checksum_ms = elapsed_ms(start);

    let mut page = Vec::with_capacity(LIST_PAGE_LIMIT);
    let start = Instant::now();
    let (paged_records, pages) = paged_read(store.settlements(), LIST_PAGE_LIMIT, &mut page)?;
    let paged_ms = elapsed_ms(start);
    check(paged_records == store.count(), || {
        format!(
            "paged read returned {paged_records} records, expected {}",
            store.count()
        )
    })?;
    check(pages == store.count().div_ceil(LIST_PAGE_LIMIT), || {
        format!(
            "paged read used {pages} pages, expected {}",
            store.count().div_ceil(LIST_PAGE_LIMIT)
        )
    })?;
    black_box(&page);
    drop(page);

    let owned = store
        .settlements()
        .iter()
        .filter(|settlement| settlement.is_owned())
        .count();
    let members: usize = store
        .settlements()
        .iter()
        .map(|settlement| settlement.members.len())
        .sum();

    Ok(ListSample {
        list_copy_ms,
        json_ms,
        checksum_ms,
        paged_ms,
        count: store.count(),
        owned,
        unowned: store.count() - owned,
        members,
        pages,
        paged_records,
        checksum,
        json_bytes: json.len(),
        json_checksum: fnv1a64(&json),
        json,
    })
}

/// Client-side `after_id`/`limit` walk over the id-sorted full slice.
///
/// Core exposes no cursor query, so this projects pages from
/// [`SettlementStore::settlements`] without a production API change. It
/// measures page slicing/copy cost, not a server-side query.
fn paged_read(
    all: &[Settlement],
    limit: usize,
    page: &mut Vec<Settlement>,
) -> Result<(usize, usize), String> {
    check(limit > 0, || "page limit must be positive".to_string())?;
    let mut after = 0u64;
    let mut records = 0usize;
    let mut pages = 0usize;
    loop {
        let start = all.partition_point(|settlement| settlement.id <= after);
        if start >= all.len() {
            break;
        }
        let end = (start + limit).min(all.len());
        page.clear();
        page.extend_from_slice(&all[start..end]);
        check(page[0].id > after, || {
            "paged read did not advance past after_id".to_string()
        })?;
        check(page.windows(2).all(|pair| pair[0].id < pair[1].id), || {
            "paged read returned records out of id order".to_string()
        })?;
        records += page.len();
        pages += 1;
        after = page[page.len() - 1].id;
        if end >= all.len() {
            break;
        }
    }
    Ok((records, pages))
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |mut hash, &byte| {
            hash ^= u64::from(byte);
            hash.wrapping_mul(0x100_0000_01b3)
        })
}

fn tax_rate_for(index: usize) -> u16 {
    ((index as u64 * 37 + 11) % 101) as u16
}

fn role_for(slot: usize) -> SettlementRole {
    match slot % 3 {
        0 => SettlementRole::Manager,
        1 => SettlementRole::Builder,
        _ => SettlementRole::Member,
    }
}

fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn median(samples: &[f64]) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    match sorted.len() {
        0 => 0.0,
        len if len % 2 == 1 => sorted[len / 2],
        len => (sorted[len / 2 - 1] + sorted[len / 2]) * 0.5,
    }
}

fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

fn check(condition: bool, message: impl FnOnce() -> String) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message())
    }
}
