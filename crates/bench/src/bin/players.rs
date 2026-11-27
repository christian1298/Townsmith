//! Deterministic [`PlayerRegistry`] microbenchmark at the 256-player cap.
//!
//! Builds the same fixture every run: 256 names `player-000`..`player-255`,
//! seeded with a fixed seed, so ids are 1..=256 and the display order is stable.
//! Each iteration measures:
//!
//! - 256 registrations from an empty registry, plus 256 case-insensitive
//!   re-register lookups that must be rejected as duplicate names;
//! - one `players()` list copy (256 [`Player`] clones);
//! - 256 exact display-name finds, 256 upper-case finds, and 256 misses;
//! - one [`PlayerRegistry::checksum`] (JSON serialization + FNV-1a-64);
//! - a 4 x 64 page projection by id: 256 `get` lookups over four pages.
//!
//! No wall-clock value is part of any checksum. The registry checksum covers the
//! saved registry; the name-set checksum covers the fixture names only, and the
//! workload checksum folds the observable counts each iteration so a silently
//! wrong lookup is visible even when timings look normal.
//!
//! `--smoke` builds the fixture twice, requires the same name set and registry
//! checksum, and pins the registry and name-set checksums. Default and smoke
//! iterations are small on purpose: this is a CPU-only microbenchmark for the
//! D-033 player address book, not a server, network, 1M, or render gate.

use std::hint::black_box;
use std::process::ExitCode;
use std::time::Instant;

use core_city::{PlayerError, PlayerId, PlayerRegistry, MAX_PLAYERS};

/// Fixed fixture seed. Changing it changes every id-derived value and pin.
const SEED: u64 = 0x5eed_2026_1127;

/// Fixture size: exactly the registry cap.
const NAMES: usize = MAX_PLAYERS;

/// Page projection shape: 4 pages of 64 players, looked up by id.
const PAGE_SIZE: usize = 64;
const PAGES: usize = NAMES / PAGE_SIZE;

const DEFAULT_ITERATIONS: usize = 100;
const SMOKE_ITERATIONS: usize = 10;
const MAX_ITERATIONS: usize = 100_000;

/// Pinned smoke fixture identity. Update both, and the docs/15 line, only for
/// an intentional fixture or registry save-format change.
const SMOKE_NAME_SET_CHECKSUM: u64 = 0xf6e7_c362_5f24_883f;
const SMOKE_REGISTRY_CHECKSUM: u64 = 0x488b_df83_0fd5_01ee;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("PLAYERS_BENCH status=error error={error}");
            ExitCode::FAILURE
        }
    }
}

/// One resolved benchmark configuration.
struct Config {
    smoke: bool,
    iterations: usize,
}

/// The deterministic name fixture and its identity.
struct Fixture {
    names: Vec<String>,
    upper_names: Vec<String>,
    misses: Vec<String>,
    ids: Vec<PlayerId>,
    name_set_checksum: u64,
}

/// Total nanoseconds per phase across every iteration.
#[derive(Clone, Copy, Default)]
struct Timings {
    register_ns: f64,
    list_ns: f64,
    find_ns: f64,
    checksum_ns: f64,
    page_ns: f64,
}

/// Deterministic observations from one measured iteration.
#[derive(Clone, Copy)]
struct Observed {
    count: usize,
    rejected: usize,
    list_len: usize,
    list_name_bytes: usize,
    exact_hits: usize,
    case_hits: usize,
    miss_hits: usize,
    registry_checksum: u64,
    page_found: usize,
    page_id_sum: u64,
}

fn run() -> Result<(), String> {
    let Some(config) = parse_config()? else {
        return Ok(());
    };

    let fixture = build_fixture();
    let reference = build_registry(&fixture);
    verify_fixture(&fixture, &reference, &config)?;

    let (timings, observed) = measure(&config, &fixture)?;
    print_summary(&config, &fixture, &timings, &observed);
    Ok(())
}

fn parse_config() -> Result<Option<Config>, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_usage();
        return Ok(None);
    }
    for arg in &args {
        let known = matches!(arg.as_str(), "--smoke" | "--iterations");
        if arg.starts_with("--") && !known {
            return Err(format!("unknown option {arg}"));
        }
    }

    let smoke = args.iter().any(|arg| arg == "--smoke");
    let default = if smoke {
        SMOKE_ITERATIONS
    } else {
        DEFAULT_ITERATIONS
    };
    let iterations = option(&args, "--iterations")?.unwrap_or(default);
    if iterations == 0 {
        return Err("--iterations must be positive".to_string());
    }
    if iterations > MAX_ITERATIONS {
        return Err(format!("--iterations must be at most {MAX_ITERATIONS}"));
    }

    Ok(Some(Config { smoke, iterations }))
}

fn option(args: &[String], name: &str) -> Result<Option<usize>, String> {
    let Some(index) = args.iter().position(|arg| arg == name) else {
        return Ok(None);
    };
    let raw = args
        .get(index + 1)
        .ok_or_else(|| format!("{name} needs a value"))?;
    raw.parse::<usize>()
        .map(Some)
        .map_err(|_| format!("{name} value {raw:?} is not a non-negative integer"))
}

fn print_usage() {
    println!("usage: players [--smoke] [--iterations N]");
    println!("  default: {DEFAULT_ITERATIONS} iterations at the {MAX_PLAYERS}-player cap");
    println!(
        "  smoke:   {SMOKE_ITERATIONS} iterations plus a two-build determinism check and pinned checksums"
    );
    println!(
        "  measures 256 registers + 256 duplicate lookups, list copy, 512 name finds + 256 misses, \
         checksum, and a {PAGES} x {PAGE_SIZE} page projection by id"
    );
}

/// Build the fixture. In smoke mode the caller builds it twice via
/// [`run`]'s reference check; every value here is a pure function of the index.
fn build_fixture() -> Fixture {
    let names: Vec<String> = (0..NAMES)
        .map(|index| format!("player-{index:03}"))
        .collect();
    let upper_names: Vec<String> = names.iter().map(|name| name.to_uppercase()).collect();
    let misses: Vec<String> = (0..NAMES)
        .map(|index| format!("missing-{index:03}"))
        .collect();
    let ids: Vec<PlayerId> = (1..=u64::try_from(NAMES).expect("names fit u64"))
        .map(|value| PlayerId::new(value).expect("fixture id is in range"))
        .collect();

    let mut name_set_checksum = FNV_OFFSET;
    for name in &names {
        for byte in name.bytes() {
            name_set_checksum = fnv_step(name_set_checksum, byte);
        }
        name_set_checksum = fnv_step(name_set_checksum, b'\n');
    }

    Fixture {
        names,
        upper_names,
        misses,
        ids,
        name_set_checksum,
    }
}

fn build_registry(fixture: &Fixture) -> PlayerRegistry {
    let mut registry = PlayerRegistry::new(SEED);
    for name in &fixture.names {
        registry.register(name).expect("fixture registration");
    }
    registry
}

/// Always-on structural checks plus the smoke-only pinned identity.
fn verify_fixture(
    fixture: &Fixture,
    reference: &PlayerRegistry,
    config: &Config,
) -> Result<(), String> {
    if reference.count() != NAMES {
        return Err(format!(
            "fixture registered {} players, expected {NAMES}",
            reference.count()
        ));
    }
    if reference.players().len() != fixture.ids.len() {
        return Err("fixture id count does not match the registry".to_string());
    }
    for (player, expected) in reference.players().iter().zip(&fixture.ids) {
        if player.id != *expected {
            return Err(format!(
                "fixture id {} does not match expected {expected}",
                player.id
            ));
        }
    }
    for page in 0..PAGES {
        for offset in 0..PAGE_SIZE {
            let player = &reference.players()[page * PAGE_SIZE + offset];
            let id = fixture.ids[page * PAGE_SIZE + offset];
            if player.id != id {
                return Err(format!("page {page} offset {offset} has id {}", player.id));
            }
        }
    }
    if !config.smoke {
        return Ok(());
    }

    if fixture.name_set_checksum != SMOKE_NAME_SET_CHECKSUM {
        return Err(format!(
            "name-set checksum mismatch: expected 0x{SMOKE_NAME_SET_CHECKSUM:016x}, \
             got 0x{:016x}",
            fixture.name_set_checksum
        ));
    }
    if reference.checksum() != SMOKE_REGISTRY_CHECKSUM {
        return Err(format!(
            "registry checksum mismatch: expected 0x{SMOKE_REGISTRY_CHECKSUM:016x}, \
             got 0x{:016x}",
            reference.checksum()
        ));
    }

    // Second build: the fixture and registry must be byte-for-byte identical.
    let again = build_fixture();
    let second = build_registry(&again);
    if again.names != fixture.names
        || again.name_set_checksum != fixture.name_set_checksum
        || second.checksum() != reference.checksum()
    {
        return Err("second fixture build is not deterministic".to_string());
    }
    Ok(())
}

fn measure(config: &Config, fixture: &Fixture) -> Result<(Timings, Observed), String> {
    let iterations = config.iterations;
    let mut timings = Timings::default();
    let mut expected: Option<u64> = None;
    let mut last = None;

    for _ in 0..iterations {
        let start = Instant::now();
        let mut registry = PlayerRegistry::new(SEED);
        for name in &fixture.names {
            registry
                .register(black_box(name))
                .expect("fixture registration");
        }
        let mut rejected = 0usize;
        for upper in &fixture.upper_names {
            match registry.register(black_box(upper)) {
                Err(PlayerError::DuplicateName) => rejected += 1,
                other => return Err(format!("duplicate lookup returned {other:?}")),
            }
        }
        timings.register_ns += elapsed_ns(start);

        let start = Instant::now();
        let list = registry.players().to_vec();
        black_box(&list);
        timings.list_ns += elapsed_ns(start);
        let list_len = list.len();
        let list_name_bytes: usize = list.iter().map(|player| player.display_name.len()).sum();

        let start = Instant::now();
        let mut exact_hits = 0usize;
        for name in &fixture.names {
            exact_hits += registry.find_by_display_name(black_box(name)).len();
        }
        let mut case_hits = 0usize;
        for upper in &fixture.upper_names {
            case_hits += registry.find_by_display_name(black_box(upper)).len();
        }
        let mut miss_hits = 0usize;
        for miss in &fixture.misses {
            miss_hits += registry.find_by_display_name(black_box(miss)).len();
        }
        timings.find_ns += elapsed_ns(start);

        let start = Instant::now();
        let registry_checksum = registry.checksum();
        black_box(registry_checksum);
        timings.checksum_ns += elapsed_ns(start);

        let start = Instant::now();
        let mut page_found = 0usize;
        let mut page_id_sum = 0u64;
        for page in 0..PAGES {
            for offset in 0..PAGE_SIZE {
                let id = fixture.ids[page * PAGE_SIZE + offset];
                if let Some(player) = registry.get(black_box(id)) {
                    page_found += 1;
                    page_id_sum = page_id_sum.wrapping_add(player.id.get());
                    if player.id != id {
                        return Err(format!("page lookup for {id} returned {}", player.id));
                    }
                }
            }
        }
        timings.page_ns += elapsed_ns(start);

        if registry.count() != NAMES
            || rejected != NAMES
            || list_len != NAMES
            || exact_hits != NAMES
            || case_hits != NAMES
            || miss_hits != 0
            || page_found != NAMES
            || page_id_sum != u64::try_from(NAMES * (NAMES + 1) / 2).expect("id sum fits u64")
        {
            return Err(format!(
                "iteration invariant failed: count={} rejected={rejected} list={list_len} \
                 exact={exact_hits} case={case_hits} misses={miss_hits} page={page_found} \
                 id_sum={page_id_sum}",
                registry.count()
            ));
        }

        let observed = Observed {
            count: registry.count(),
            rejected,
            list_len,
            list_name_bytes,
            exact_hits,
            case_hits,
            miss_hits,
            registry_checksum,
            page_found,
            page_id_sum,
        };
        let checksum = observed_checksum(&observed);
        match expected {
            Some(value) if value != checksum => {
                return Err(format!(
                    "iteration checksum changed: expected 0x{value:016x}, got 0x{checksum:016x}"
                ));
            }
            Some(_) => {}
            None => expected = Some(checksum),
        }
        last = Some(observed);
    }

    last.map(|observed| (timings, observed))
        .ok_or_else(|| "no iterations measured".to_string())
}

fn elapsed_ns(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1_000_000_000.0
}

fn print_summary(config: &Config, fixture: &Fixture, timings: &Timings, observed: &Observed) {
    let iterations = config.iterations;
    let total_ns = timings.register_ns
        + timings.list_ns
        + timings.find_ns
        + timings.checksum_ns
        + timings.page_ns;
    let register_calls = iterations * NAMES * 2;
    let find_calls = iterations * NAMES * 3;

    // Stable line: deterministic fixture identity and iteration count only.
    println!(
        "players_bench: scenario=players_256 count={} pages={} page_size={} \
         name_set_checksum=0x{:016x} registry_checksum=0x{:016x} iterations={} status=ok",
        fixture.names.len(),
        PAGES,
        PAGE_SIZE,
        fixture.name_set_checksum,
        observed.registry_checksum,
        iterations
    );

    println!(
        "  register       {:>12.3} us total ({:>10.1} ns/op, {} calls)",
        timings.register_ns / 1_000.0,
        timings.register_ns / register_calls as f64,
        register_calls
    );
    println!(
        "  list           {:>12.3} us total ({:>10.1} ns/op, {} copies)",
        timings.list_ns / 1_000.0,
        timings.list_ns / iterations as f64,
        iterations
    );
    println!(
        "  find           {:>12.3} us total ({:>10.1} ns/op, {} queries)",
        timings.find_ns / 1_000.0,
        timings.find_ns / find_calls as f64,
        find_calls
    );
    println!(
        "  checksum       {:>12.3} us total ({:>10.1} ns/op)",
        timings.checksum_ns / 1_000.0,
        timings.checksum_ns / iterations as f64
    );
    println!(
        "  page           {:>12.3} us total ({:>10.1} ns/op, {} lookups)",
        timings.page_ns / 1_000.0,
        timings.page_ns / (iterations * NAMES) as f64,
        iterations * NAMES
    );
    println!(
        "  total          {:>12.3} us total ({:>10.1} ns/op)",
        total_ns / 1_000.0,
        total_ns / iterations as f64
    );

    let json = serde_json::json!({
        "scenario": "players_256",
        "smoke": config.smoke,
        "iterations": iterations,
        "count": observed.count,
        "pages": PAGES,
        "page_size": PAGE_SIZE,
        "name_set_checksum": format!("0x{:016x}", fixture.name_set_checksum),
        "registry_checksum": format!("0x{:016x}", observed.registry_checksum),
        "workload_checksum": format!("0x{:016x}", observed_checksum(observed)),
        "checksum_algorithm": "fnv1a64",
        "observations": {
            "duplicate_rejections": observed.rejected,
            "list_players": observed.list_len,
            "list_name_bytes": observed.list_name_bytes,
            "find_exact_hits": observed.exact_hits,
            "find_case_insensitive_hits": observed.case_hits,
            "find_misses": observed.miss_hits,
            "page_players": observed.page_found,
            "page_id_sum": observed.page_id_sum
        },
        "phases": {
            "register": {
                "calls": register_calls,
                "total_ns": round1(timings.register_ns),
                "ns_per_op": round1(timings.register_ns / register_calls as f64)
            },
            "list": {
                "calls": iterations,
                "total_ns": round1(timings.list_ns),
                "ns_per_op": round1(timings.list_ns / iterations as f64)
            },
            "find": {
                "calls": find_calls,
                "total_ns": round1(timings.find_ns),
                "ns_per_op": round1(timings.find_ns / find_calls as f64)
            },
            "checksum": {
                "calls": iterations,
                "total_ns": round1(timings.checksum_ns),
                "ns_per_op": round1(timings.checksum_ns / iterations as f64)
            },
            "page": {
                "calls": iterations * NAMES,
                "total_ns": round1(timings.page_ns),
                "ns_per_op": round1(timings.page_ns / (iterations * NAMES) as f64)
            },
            "total": {
                "calls": iterations,
                "total_ns": round1(total_ns),
                "ns_per_op": round1(total_ns / iterations as f64)
            }
        }
    });
    println!("{json}");
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv_step(hash: u64, byte: u8) -> u64 {
    (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME)
}

/// FNV-1a-64 over the observed counts, little-endian per field.
fn observed_checksum(observed: &Observed) -> u64 {
    let fields = [
        observed.count as u64,
        observed.rejected as u64,
        observed.list_len as u64,
        observed.list_name_bytes as u64,
        observed.exact_hits as u64,
        observed.case_hits as u64,
        observed.miss_hits as u64,
        observed.registry_checksum,
        observed.page_found as u64,
        observed.page_id_sum,
    ];
    let mut hash = FNV_OFFSET;
    for field in fields {
        for byte in field.to_le_bytes() {
            hash = fnv_step(hash, byte);
        }
    }
    hash
}
