//! Deterministic D-042 content-registry parser and selection microbenchmark.
//!
//! Builds one manifest at the entry cap ([`MAX_CONTENT_REGISTRY_ENTRIES`], 256
//! entries: 128 mods and 128 resource packs), parses it once for the read-side
//! phases, and then measures:
//!
//! - [`parse_content_registry_manifest`] per iteration;
//! - [`ContentRegistryManifest::implicit_ids`] per iteration;
//! - [`ContentRegistryManifest::select`] over a fixed 64-ID subset;
//! - [`ContentRegistryManifest::provides`] over 256 exact-ID and 256 namespace
//!   queries.
//!
//! The fixture is fully deterministic: IDs, kinds, shared namespace groups, and
//! canonical `blake3:<64 hex>` checksums are derived from fixed constants and a
//! splitmix64 stream seeded with [`SEED`]. No wall-clock value, file, network,
//! server, or renderer is involved, and the generated JSON stays well under the
//! 1 MiB manifest cap.
//!
//! `--smoke` rebuilds the fixture twice and fails unless the JSON, checksum,
//! entry/kind counts, implied IDs, selection, and provides totals match. The
//! default and smoke fixtures also have pinned values, so a parser or fixture
//! change is visible. Timings are printed separately from the stable summary
//! line and vary between runs; they are not part of the determinism check.
//!
//! This is a microbenchmark for the D-042 parser/selection contract. It is not
//! a server wiring test, a load test, the 1M gate, or a render benchmark.

use std::fmt::Write as _;
use std::hint::black_box;
use std::process::ExitCode;
use std::time::Instant;

use core_city::content_registry::{
    parse_content_registry_manifest, ContentRegistryKind, ContentRegistryManifest,
    MAX_CONTENT_REGISTRY_BYTES, MAX_CONTENT_REGISTRY_ENTRIES,
};

/// Fixed fixture seed. Changing it changes every checksum and the pinned values.
const SEED: u64 = 0x5eed_2026_1127;

/// Mods and resource packs are split evenly at the 256-entry cap.
const MOD_COUNT: usize = MAX_CONTENT_REGISTRY_ENTRIES / 2;
const PACK_COUNT: usize = MAX_CONTENT_REGISTRY_ENTRIES - MOD_COUNT;

/// Shared namespace groups. Each ID is `<namespace>:<entry>`, and every group
/// holds [`ENTRIES_PER_NAMESPACE`] entries, so namespace queries match many
/// entries without duplicate explicit `namespace` claims (which the parser
/// rejects).
const MOD_NAMESPACES: [&str; 8] = [
    "amber", "birch", "cedar", "dune", "elm", "flint", "granite", "harbor",
];
const PACK_NAMESPACES: [&str; 8] = [
    "pack-amber",
    "pack-birch",
    "pack-cedar",
    "pack-dune",
    "pack-elm",
    "pack-flint",
    "pack-granite",
    "pack-harbor",
];
const ENTRIES_PER_NAMESPACE: usize = MOD_COUNT / MOD_NAMESPACES.len();

/// Every fourth fixture ID is selected: 64 of 256.
const SELECT_STRIDE: usize = 4;
const SELECT_COUNT: usize = MAX_CONTENT_REGISTRY_ENTRIES / SELECT_STRIDE;

const DEFAULT_ITERATIONS: usize = 1_000;
const SMOKE_ITERATIONS: usize = 50;
const MAX_ITERATIONS: usize = 100_000;

/// Pinned smoke fixture identity. Update only for an intentional fixture or
/// parser change, and update the docs/15 line with it.
const SMOKE_MANIFEST_CHECKSUM: u64 = 0x7052_67e4_2c8f_0ff3;
const SMOKE_MANIFEST_BYTES: usize = 32_412;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("CONTENT_REGISTRY_BENCH status=error error={error}");
            ExitCode::FAILURE
        }
    }
}

/// One resolved benchmark configuration.
struct Config {
    smoke: bool,
    iterations: usize,
}

/// The deterministic fixture and its identity.
struct Fixture {
    json: String,
    checksum: u64,
    ids: Vec<String>,
    namespace_tokens: Vec<String>,
    mods: usize,
    packs: usize,
}

/// One measured run: total microseconds for each phase.
#[derive(Clone, Copy, Default)]
struct Timings {
    parse_us: f64,
    implicit_ids_us: f64,
    select_us: f64,
    provides_us: f64,
}

fn run() -> Result<(), String> {
    let Some(config) = parse_config()? else {
        return Ok(());
    };

    let fixture = build_fixture();
    verify_fixture(&fixture, &config)?;

    let manifest = parse_content_registry_manifest(&fixture.json)
        .map_err(|error| format!("fixture manifest did not parse: {error}"))?;
    let selection = select_tokens(&fixture);
    verify_parsed(&manifest, &selection)?;

    let timings = measure(&config, &fixture, &manifest, &selection)?;
    print_summary(&config, &fixture, &timings);
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
    println!("usage: content_registry [--smoke] [--iterations N]");
    println!(
        "  default: {} iterations at the {MAX_CONTENT_REGISTRY_ENTRIES}-entry cap",
        DEFAULT_ITERATIONS
    );
    println!(
        "  smoke:   {SMOKE_ITERATIONS} iterations plus a two-build determinism check and pinned counts"
    );
    println!(
        "  measures parse_content_registry_manifest, implicit_ids, select(64), \
         provides(256 exact + 256 namespace)"
    );
}

/// Build the fixture twice for smoke (determinism check), once otherwise.
fn build_fixture() -> Fixture {
    let mut json = String::with_capacity(48 * 1024);
    let mut ids = Vec::with_capacity(MAX_CONTENT_REGISTRY_ENTRIES);
    let mut namespace_tokens = Vec::with_capacity(MAX_CONTENT_REGISTRY_ENTRIES);
    let mut mods = 0usize;
    let mut packs = 0usize;
    let mut index = 0usize;

    json.push_str("{\"format_version\":1,\"ids\":[");

    for namespace in MOD_NAMESPACES {
        for slot in 0..ENTRIES_PER_NAMESPACE {
            let id = format!("{namespace}:mod{slot:02}");
            push_entry(&mut json, index, &id, "mod");
            ids.push(id);
            namespace_tokens.push(namespace.to_string());
            mods += 1;
            index += 1;
        }
    }

    for namespace in PACK_NAMESPACES {
        for slot in 0..ENTRIES_PER_NAMESPACE {
            let id = format!("{namespace}:res{slot:02}");
            push_entry(&mut json, index, &id, "resource_pack");
            ids.push(id);
            namespace_tokens.push(namespace.to_string());
            packs += 1;
            index += 1;
        }
    }

    json.push_str("]}");

    Fixture {
        checksum: fnv1a64(json.as_bytes()),
        json,
        ids,
        namespace_tokens,
        mods,
        packs,
    }
}

/// Append one canonical entry in a fixed key order.
fn push_entry(json: &mut String, index: usize, id: &str, kind: &str) {
    if index > 0 {
        json.push(',');
    }
    write!(
        json,
        "{{\"id\":\"{id}\",\"kind\":\"{kind}\",\"checksum\":\"blake3:{}\"}}",
        checksum_hex(index)
    )
    .expect("writing JSON to a String cannot fail");
}

/// Canonical 64 lowercase hex characters from a fixed seed and entry index.
fn checksum_hex(index: usize) -> String {
    let mut state = SEED.wrapping_add((index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15));
    let mut hex = String::with_capacity(64);
    for _ in 0..4 {
        write!(hex, "{:016x}", splitmix64(&mut state))
            .expect("writing hex to a String cannot fail");
    }
    hex
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn select_tokens(fixture: &Fixture) -> Vec<&str> {
    fixture
        .ids
        .iter()
        .step_by(SELECT_STRIDE)
        .map(String::as_str)
        .collect()
}

fn verify_fixture(fixture: &Fixture, config: &Config) -> Result<(), String> {
    check(
        fixture.ids.len() == MAX_CONTENT_REGISTRY_ENTRIES
            && fixture.mods == MOD_COUNT
            && fixture.packs == PACK_COUNT,
        || {
            format!(
                "fixture has {} ids ({} mods, {} packs), expected {} ({} + {})",
                fixture.ids.len(),
                fixture.mods,
                fixture.packs,
                MAX_CONTENT_REGISTRY_ENTRIES,
                MOD_COUNT,
                PACK_COUNT
            )
        },
    )?;
    check(fixture.json.len() <= MAX_CONTENT_REGISTRY_BYTES, || {
        format!(
            "fixture manifest is {} bytes, above the {MAX_CONTENT_REGISTRY_BYTES}-byte cap",
            fixture.json.len()
        )
    })?;

    let mut unique = fixture.ids.clone();
    unique.sort();
    unique.dedup();
    check(unique.len() == fixture.ids.len(), || {
        "fixture IDs are not unique".to_string()
    })?;

    if config.smoke {
        let repeat = build_fixture();
        check(
            repeat.json == fixture.json && repeat.checksum == fixture.checksum,
            || "smoke fixture is not deterministic across two builds".to_string(),
        )?;
        check(repeat.ids == fixture.ids, || {
            "smoke fixture ID list changed across two builds".to_string()
        })?;
        check(fixture.checksum == SMOKE_MANIFEST_CHECKSUM, || {
            format!(
                "smoke manifest checksum 0x{:016x} drifted from pinned 0x{SMOKE_MANIFEST_CHECKSUM:016x}",
                fixture.checksum
            )
        })?;
        check(fixture.json.len() == SMOKE_MANIFEST_BYTES, || {
            format!(
                "smoke manifest size {} drifted from pinned {SMOKE_MANIFEST_BYTES}",
                fixture.json.len()
            )
        })?;
    }

    Ok(())
}

fn verify_parsed(manifest: &ContentRegistryManifest, selection: &[&str]) -> Result<(), String> {
    check(manifest.ids.len() == MAX_CONTENT_REGISTRY_ENTRIES, || {
        format!(
            "parsed {} entries, expected {MAX_CONTENT_REGISTRY_ENTRIES}",
            manifest.ids.len()
        )
    })?;
    check(
        manifest
            .ids
            .iter()
            .filter(|entry| entry.kind == ContentRegistryKind::Mod)
            .count()
            == MOD_COUNT,
        || "parsed mod count differs from the fixture".to_string(),
    )?;
    check(
        manifest
            .ids
            .iter()
            .filter(|entry| entry.kind == ContentRegistryKind::ResourcePack)
            .count()
            == PACK_COUNT,
        || "parsed resource-pack count differs from the fixture".to_string(),
    )?;
    check(
        manifest.ids.iter().all(|entry| {
            entry
                .checksum
                .as_deref()
                .is_some_and(core_city::content_registry::is_canonical_content_checksum)
        }),
        || "fixture produced a non-canonical checksum".to_string(),
    )?;
    check(
        manifest.implicit_ids().len() == MAX_CONTENT_REGISTRY_ENTRIES + 1,
        || "implicit_ids did not return base plus every entry".to_string(),
    )?;
    check(selection.len() == SELECT_COUNT, || {
        format!(
            "selection token count {} differs from {SELECT_COUNT}",
            selection.len()
        )
    })?;
    let selected = manifest
        .select(selection)
        .map_err(|error| format!("fixture selection failed: {error}"))?;
    check(
        selected.len() == SELECT_COUNT + 1
            && selected
                .iter()
                .any(|id| id == core_city::content_registry::BASE_CONTENT_ID),
        || {
            format!(
                "selection returned {} ids, expected base plus {SELECT_COUNT}",
                selected.len()
            )
        },
    )?;
    Ok(())
}

fn measure(
    config: &Config,
    fixture: &Fixture,
    manifest: &ContentRegistryManifest,
    selection: &[&str],
) -> Result<Timings, String> {
    let iterations = config.iterations;

    let start = Instant::now();
    for _ in 0..iterations {
        let parsed = parse_content_registry_manifest(black_box(&fixture.json))
            .expect("fixture manifest parses");
        black_box(parsed.ids.len());
    }
    let parse_us = elapsed_us(start);

    let start = Instant::now();
    for _ in 0..iterations {
        let ids = manifest.implicit_ids();
        black_box(ids.len());
        black_box(&ids);
    }
    let implicit_ids_us = elapsed_us(start);

    let start = Instant::now();
    for _ in 0..iterations {
        let selected = manifest
            .select(black_box(selection))
            .expect("fixture selection succeeds");
        black_box(selected.len());
        black_box(&selected);
    }
    let select_us = elapsed_us(start);

    let mut exact_hits = 0usize;
    let mut namespace_hits = 0usize;
    let start = Instant::now();
    for _ in 0..iterations {
        exact_hits = 0;
        namespace_hits = 0;
        for token in &fixture.ids {
            if manifest.provides(black_box(token)) {
                exact_hits += 1;
            }
        }
        for token in &fixture.namespace_tokens {
            if manifest.provides(black_box(token)) {
                namespace_hits += 1;
            }
        }
        black_box(exact_hits);
        black_box(namespace_hits);
    }
    let provides_us = elapsed_us(start);

    check(
        exact_hits == MAX_CONTENT_REGISTRY_ENTRIES
            && namespace_hits == MAX_CONTENT_REGISTRY_ENTRIES,
        || {
            format!(
                "provides matched {exact_hits} exact and {namespace_hits} namespace queries, \
                 expected {MAX_CONTENT_REGISTRY_ENTRIES} each"
            )
        },
    )?;
    check(!manifest.provides("not-in-the-fixture"), || {
        "provides matched an unknown ID".to_string()
    })?;

    Ok(Timings {
        parse_us,
        implicit_ids_us,
        select_us,
        provides_us,
    })
}

fn print_summary(config: &Config, fixture: &Fixture, timings: &Timings) {
    let total_us =
        timings.parse_us + timings.implicit_ids_us + timings.select_us + timings.provides_us;

    // Stable line: deterministic fixture identity and iteration count only.
    println!(
        "content_registry_bench: scenario=content_registry_256 entries={} mods={} packs={} \
         implicit={} selected={} provides_exact={} provides_namespace={} manifest_bytes={} \
         manifest_checksum=0x{:016x} iterations={} status=ok",
        fixture.ids.len(),
        fixture.mods,
        fixture.packs,
        MAX_CONTENT_REGISTRY_ENTRIES + 1,
        SELECT_COUNT,
        MAX_CONTENT_REGISTRY_ENTRIES,
        MAX_CONTENT_REGISTRY_ENTRIES,
        fixture.json.len(),
        fixture.checksum,
        config.iterations
    );

    println!(
        "  parse          {:>12.3} us total ({:>10.1} ns/op)",
        timings.parse_us,
        ns_per_op(timings.parse_us, config.iterations)
    );
    println!(
        "  implicit_ids   {:>12.3} us total ({:>10.1} ns/op)",
        timings.implicit_ids_us,
        ns_per_op(timings.implicit_ids_us, config.iterations)
    );
    println!(
        "  select         {:>12.3} us total ({:>10.1} ns/op, {} ids)",
        timings.select_us,
        ns_per_op(timings.select_us, config.iterations),
        SELECT_COUNT
    );
    println!(
        "  provides       {:>12.3} us total ({:>10.1} ns/op, {} calls)",
        timings.provides_us,
        ns_per_op(timings.provides_us, config.iterations),
        MAX_CONTENT_REGISTRY_ENTRIES * 2
    );

    let json = serde_json::json!({
        "scenario": "content_registry_256",
        "smoke": config.smoke,
        "iterations": config.iterations,
        "manifest_bytes": fixture.json.len(),
        "manifest_checksum": format!("0x{:016x}", fixture.checksum),
        "checksum_algorithm": "fnv1a64",
        "entries": fixture.ids.len(),
        "mods": fixture.mods,
        "packs": fixture.packs,
        "implicit_ids": MAX_CONTENT_REGISTRY_ENTRIES + 1,
        "selected": SELECT_COUNT,
        "provides_exact_queries": MAX_CONTENT_REGISTRY_ENTRIES,
        "provides_namespace_queries": MAX_CONTENT_REGISTRY_ENTRIES,
        "phases": {
            "parse": {
                "total_us": round3(timings.parse_us),
                "ns_per_op": round1(ns_per_op(timings.parse_us, config.iterations)),
            },
            "implicit_ids": {
                "total_us": round3(timings.implicit_ids_us),
                "ns_per_op": round1(ns_per_op(timings.implicit_ids_us, config.iterations)),
            },
            "select": {
                "total_us": round3(timings.select_us),
                "ns_per_op": round1(ns_per_op(timings.select_us, config.iterations)),
            },
            "provides": {
                "total_us": round3(timings.provides_us),
                "ns_per_op": round1(ns_per_op(timings.provides_us, config.iterations)),
            },
        },
        "total_us": round3(total_us),
        "status": "ok",
    });
    println!("CONTENT_REGISTRY_BENCH {json}");
}

fn elapsed_us(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1_000_000.0
}

fn ns_per_op(total_us: f64, iterations: usize) -> f64 {
    total_us * 1_000.0 / iterations as f64
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn round3(value: f64) -> f64 {
    (value * 1_000.0).round() / 1_000.0
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |mut hash, &byte| {
            hash ^= u64::from(byte);
            hash.wrapping_mul(0x100_0000_01b3)
        })
}

fn check(condition: bool, message: impl FnOnce() -> String) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message())
    }
}
