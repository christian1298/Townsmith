//! 1M-population / 250k-placement save-load data harness.
//!
//! Builds the Phase 5 save shape — 250,000 placements, 250,000 four-resident
//! households (1,000,000 population), and 2,000 named citizens — composes the
//! three core stores into one JSON object, writes it to a real file, reads it
//! back, and re-parses and validates every store against pinned checksums.
//!
//! This measures the core-store save/load path only. It is not the Godot
//! `world_save` wrapper, does not include terrain deltas, construction,
//! economy, roads, utilities, or services, and does not assert disk atomicity.
//! The 30,000 ms save and load targets are the Phase 5 gate values from
//! [[68 - Metropolis Gate]] §5.4.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use core_city::{
    CityRuntime, Home, NamedCitizens, NamedSave, PlacementIndex, PlacementRequest, PlacementStore,
    MAX_HOUSEHOLDS, MAX_MOVE_IN_PER_SECOND, MAX_NAMED_CITIZENS,
};
use serde::{Deserialize, Serialize};

const SEED: u64 = 2026;
const DEFAULT_PLACEMENTS: usize = 250_000;
const SMOKE_PLACEMENTS: usize = 25_000;
const MIN_PLACEMENTS: usize = 1_000;
/// Upper bound for `--placements`. The gate shape is 250,000; the larger cap
/// leaves headroom for a stress run without letting a typo exhaust memory.
const MAX_PLACEMENTS: usize = 500_000;
const RESIDENTS_PER_HOUSEHOLD: u32 = 4;
const NAMED_CITIZENS: usize = 2_000;
/// Phase 5 save/load target, each direction.
const TARGET_MS: f64 = 30_000.0;
const SPACING_M: f32 = 10.0;
const BOUNDS_M: [f32; 3] = [8.0, 6.0, 8.0];
const GROUND_M: u16 = 40;

static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

fn main() {
    let smoke = std::env::args().any(|arg| arg == "--smoke");
    let strict = std::env::args().any(|arg| arg == "--strict");
    let placements = if smoke {
        SMOKE_PLACEMENTS
    } else {
        arg_value("--placements")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(DEFAULT_PLACEMENTS)
    };
    if !(MIN_PLACEMENTS..=MAX_PLACEMENTS).contains(&placements) {
        eprintln!(
            "save_1m: --placements must be {MIN_PLACEMENTS}..={MAX_PLACEMENTS}, got {placements}"
        );
        std::process::exit(2);
    }
    if placements > MAX_HOUSEHOLDS {
        eprintln!("save_1m: {placements} households exceeds MAX_HOUSEHOLDS {MAX_HOUSEHOLDS}");
        std::process::exit(2);
    }
    let json_path = arg_value("--json").map(PathBuf::from);

    let result = if smoke {
        run_smoke(json_path.as_deref())
    } else {
        run_case(placements, json_path.as_deref())
    };
    match result {
        Ok(report) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&report).expect("report json")
            );
            if report["status"] != "ok" {
                eprintln!(
                    "SAVE_1M target miss: {}",
                    report["miss"]
                        .as_array()
                        .map(|misses| misses
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", "))
                        .unwrap_or_default()
                );
                if strict {
                    std::process::exit(1);
                }
            }
        }
        Err(error) => {
            eprintln!("SAVE_1M status=failed error={error}");
            std::process::exit(1);
        }
    }
}

/// Run the smoke fixture twice and require identical counts and checksums.
fn run_smoke(path: Option<&Path>) -> Result<serde_json::Value, String> {
    let mut first = run_case(SMOKE_PLACEMENTS, path)?;
    let second = run_case(SMOKE_PLACEMENTS, path)?;
    for field in ["placements", "households", "population", "named"] {
        if first[field] != second[field] {
            return Err(format!(
                "smoke drift in {field}: {} vs {}",
                first[field], second[field]
            ));
        }
    }
    if first["checksums"] != second["checksums"]
        || first["combined_checksum"] != second["combined_checksum"]
    {
        return Err(format!(
            "smoke checksum drift: {} vs {}",
            first["combined_checksum"], second["combined_checksum"]
        ));
    }
    first["deterministic"] = serde_json::Value::Bool(true);
    Ok(first)
}

fn arg_value(name: &str) -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == name {
            return args.next();
        }
        if let Some(value) = arg.strip_prefix(&format!("{name}=")) {
            return Some(value.to_string());
        }
    }
    None
}

struct Fixture {
    placements: PlacementStore,
    runtime: CityRuntime,
    named: NamedCitizens,
    build_ms: f64,
}

impl Fixture {
    fn checksums(&self) -> [u64; 3] {
        [
            self.placements.checksum(),
            self.runtime.checksum(),
            self.named.checksum(),
        ]
    }
}

/// Build the deterministic fixture: one 8×6×8 house per 10 m grid cell, four
/// residents per household, and the first 500 households' slots named.
fn build_fixture(placements: usize) -> Result<Fixture, String> {
    if !(MIN_PLACEMENTS..=MAX_PLACEMENTS).contains(&placements) {
        return Err(format!(
            "placements must be {MIN_PLACEMENTS}..={MAX_PLACEMENTS}, got {placements}"
        ));
    }
    let side = (placements as f64).sqrt().ceil() as i64;
    let offset = -((side - 1) as f32 * SPACING_M) * 0.5;
    let mut store = PlacementStore::new(SEED);
    let mut index = PlacementIndex::new();
    let build_start = Instant::now();
    for slot in 0..placements {
        let column = (slot as i64) % side;
        let row = (slot as i64) / side;
        let request = PlacementRequest {
            blueprint_id: "base:tenement".to_string(),
            blueprint_hash: "0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
            blueprint_path: "res://content/base/tenement.tsbp".to_string(),
            name: "Tenement".to_string(),
            bounds_m: BOUNDS_M,
            center_x: offset + column as f32 * SPACING_M,
            center_z: offset + row as f32 * SPACING_M,
            rotation_deg: 0,
            material_cost: 60_000.0,
            resident_capacity: RESIDENTS_PER_HOUSEHOLD,
            job_slots: 0,
            solid_voxels: 4_800,
            structure: None,
        };
        store
            .place_indexed(&mut index, &request, |_, _| GROUND_M)
            .map_err(|error| error.to_string())?;
    }
    let build_ms = elapsed_ms(build_start);
    if index.len() != placements {
        return Err(format!(
            "placement index has {} entries, expected {placements}",
            index.len()
        ));
    }

    let mut runtime = CityRuntime::new(SEED);
    runtime.set_move_in_per_second(MAX_MOVE_IN_PER_SECOND);
    let homes: Vec<Home> = store
        .placements
        .iter()
        .map(|placement| Home {
            id: placement.id,
            capacity: placement.resident_capacity,
            center: [placement.center[0], placement.center[2]],
        })
        .collect();
    runtime.sync_homes(&homes);
    // Fill every home directly: the fixture is a save shape, not a move-in
    // simulation. Needs stay at 1.0 so the saved floats are representative.
    for household in runtime.households.values_mut() {
        household.residents = household.capacity;
    }
    runtime.validate().map_err(|error| error.to_string())?;

    let mut named = NamedCitizens::new(SEED);
    named.max = NAMED_CITIZENS.min(MAX_NAMED_CITIZENS);
    named.sync(&runtime.households);
    if named.len() != NAMED_CITIZENS {
        return Err(format!(
            "named pool has {} citizens, expected {NAMED_CITIZENS}",
            named.len()
        ));
    }
    named.validate().map_err(|error| error.to_string())?;

    Ok(Fixture {
        placements: store,
        runtime,
        named,
        build_ms,
    })
}

#[derive(Serialize)]
struct CombinedRef<'a> {
    placements: &'a PlacementStore,
    population: &'a CityRuntime,
    named_citizens: NamedSave,
}

#[derive(Deserialize)]
struct CombinedSave {
    placements: PlacementStore,
    population: CityRuntime,
    named_citizens: NamedSave,
}

struct Loaded {
    serialize_ms: f64,
    file_write_ms: f64,
    file_read_ms: f64,
    parse_ms: f64,
    verify_ms: f64,
    json_bytes: u64,
    checksums: [u64; 3],
    combined_checksum: u64,
}

fn run_case(placements: usize, path: Option<&Path>) -> Result<serde_json::Value, String> {
    let fixture = build_fixture(placements)?;
    let expected = fixture.checksums();
    let expected_combined = combined_checksum(expected);
    let json_path = path.map_or_else(|| temp_save_path(placements), Path::to_path_buf);
    let temporary = path.is_none();

    let result = round_trip(&fixture, expected, expected_combined, &json_path);
    if temporary {
        let _ = fs::remove_file(&json_path);
    }
    let loaded = result?;

    let households = fixture.runtime.households.len();
    let population = fixture.runtime.population();
    let named = fixture.named.len();
    let total_save_ms = loaded.serialize_ms + loaded.file_write_ms;
    let total_load_ms = loaded.file_read_ms + loaded.parse_ms + loaded.verify_ms;
    let save_within_target = total_save_ms <= TARGET_MS;
    let load_within_target = total_load_ms <= TARGET_MS;
    let mut misses = Vec::new();
    if !save_within_target {
        misses.push(format!(
            "save {:.3} ms exceeds {:.0} ms",
            total_save_ms, TARGET_MS
        ));
    }
    if !load_within_target {
        misses.push(format!(
            "load {:.3} ms exceeds {:.0} ms",
            total_load_ms, TARGET_MS
        ));
    }

    let mut report = serde_json::json!({
        "scene": "save_1m",
        "seed": SEED,
        "placements": fixture.placements.placements.len(),
        "households": households,
        "population": population,
        "named": named,
        "residents_per_household": RESIDENTS_PER_HOUSEHOLD,
        "grid_side": (placements as f64).sqrt().ceil() as i64,
        "spacing_m": SPACING_M,
        "bounds_m": BOUNDS_M,
        "build_ms": round_ms(fixture.build_ms),
        "serialize_ms": round_ms(loaded.serialize_ms),
        "file_write_ms": round_ms(loaded.file_write_ms),
        "file_read_ms": round_ms(loaded.file_read_ms),
        "parse_ms": round_ms(loaded.parse_ms),
        "verify_ms": round_ms(loaded.verify_ms),
        "total_save_ms": round_ms(total_save_ms),
        "total_load_ms": round_ms(total_load_ms),
        "target_ms": TARGET_MS,
        "save_within_target": save_within_target,
        "load_within_target": load_within_target,
        "json_bytes": loaded.json_bytes,
        "json_path": json_path.to_string_lossy(),
        "checksums": {
            "placements": format!("{:016x}", loaded.checksums[0]),
            "population": format!("{:016x}", loaded.checksums[1]),
            "named_citizens": format!("{:016x}", loaded.checksums[2]),
        },
        "combined_checksum": format!("{:016x}", loaded.combined_checksum),
        "status": if misses.is_empty() { "ok" } else { "miss" },
    });
    if !misses.is_empty() {
        report["miss"] = serde_json::json!(misses);
    }
    if let Some(mb) = peak_rss_mb() {
        report["peak_rss_mb"] = serde_json::json!(mb);
    }
    Ok(report)
}

fn round_trip(
    fixture: &Fixture,
    expected: [u64; 3],
    expected_combined: u64,
    json_path: &Path,
) -> Result<Loaded, String> {
    let envelope = CombinedRef {
        placements: &fixture.placements,
        population: &fixture.runtime,
        named_citizens: fixture.named.to_save(),
    };
    let serialize_start = Instant::now();
    let json = serde_json::to_string(&envelope).map_err(|error| error.to_string())?;
    let serialize_ms = elapsed_ms(serialize_start);
    let json_bytes = json.len() as u64;

    let write_start = Instant::now();
    {
        let mut file = File::create(json_path)
            .map_err(|error| format!("create {}: {error}", json_path.display()))?;
        file.write_all(json.as_bytes())
            .map_err(|error| format!("write {}: {error}", json_path.display()))?;
        file.flush()
            .map_err(|error| format!("flush {}: {error}", json_path.display()))?;
    }
    let file_write_ms = elapsed_ms(write_start);
    // Release the serialized string before the load allocates its copy.
    drop(json);

    let read_start = Instant::now();
    let encoded = fs::read_to_string(json_path)
        .map_err(|error| format!("read {}: {error}", json_path.display()))?;
    let file_read_ms = elapsed_ms(read_start);

    let parse_start = Instant::now();
    let parsed: CombinedSave =
        serde_json::from_str(&encoded).map_err(|error| format!("parse combined json: {error}"))?;
    let parse_ms = elapsed_ms(parse_start);
    drop(encoded);

    let verify_start = Instant::now();
    parsed
        .placements
        .validate_loaded()
        .map_err(|error| format!("placements: {error}"))?;
    parsed
        .population
        .validate()
        .map_err(|error| format!("population: {error}"))?;
    let named =
        NamedCitizens::from_save(parsed.named_citizens).map_err(|error| error.to_string())?;
    if parsed.placements.placements.len() != fixture.placements.placements.len() {
        return Err(format!(
            "placement count changed: {} != {}",
            parsed.placements.placements.len(),
            fixture.placements.placements.len()
        ));
    }
    if parsed.population.households.len() != fixture.runtime.households.len() {
        return Err(format!(
            "household count changed: {} != {}",
            parsed.population.households.len(),
            fixture.runtime.households.len()
        ));
    }
    if parsed.population.population() != fixture.runtime.population() {
        return Err(format!(
            "population changed: {} != {}",
            parsed.population.population(),
            fixture.runtime.population()
        ));
    }
    if named.len() != fixture.named.len() {
        return Err(format!(
            "named count changed: {} != {}",
            named.len(),
            fixture.named.len()
        ));
    }
    let actual = [
        parsed.placements.checksum(),
        parsed.population.checksum(),
        named.checksum(),
    ];
    if actual != expected {
        return Err(format!(
            "per-store checksum changed: {actual:?} != {expected:?}"
        ));
    }
    let combined = combined_checksum(actual);
    if combined != expected_combined {
        return Err(format!(
            "combined checksum changed: {combined:016x} != {expected_combined:016x}"
        ));
    }
    let verify_ms = elapsed_ms(verify_start);

    Ok(Loaded {
        serialize_ms,
        file_write_ms,
        file_read_ms,
        parse_ms,
        verify_ms,
        json_bytes,
        checksums: actual,
        combined_checksum: combined,
    })
}

fn combined_checksum(checksums: [u64; 3]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for value in checksums {
        for byte in value.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

fn temp_save_path(placements: usize) -> PathBuf {
    let serial = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "townsmith_save_1m_{}_{placements}_{serial}.json",
        std::process::id()
    ))
}

/// Peak resident set in MiB on Linux, best effort.
fn peak_rss_mb() -> Option<f64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmHWM:"))?;
    let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some((kb / 1024.0 * 100.0).round() / 100.0)
}

fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1_000.0
}

fn round_ms(value: f64) -> f64 {
    (value * 1_000.0).round() / 1_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_fixture_has_the_expected_counts_and_checksums() {
        let fixture = build_fixture(SMOKE_PLACEMENTS).expect("fixture");
        assert_eq!(fixture.placements.placements.len(), SMOKE_PLACEMENTS);
        assert_eq!(fixture.runtime.households.len(), SMOKE_PLACEMENTS);
        assert_eq!(
            fixture.runtime.population(),
            u64::try_from(SMOKE_PLACEMENTS).expect("count") * u64::from(RESIDENTS_PER_HOUSEHOLD)
        );
        assert_eq!(fixture.named.len(), NAMED_CITIZENS);
        assert_eq!(fixture.placements.next_id, SMOKE_PLACEMENTS as u64 + 1);
        assert_eq!(
            fixture.checksums(),
            [
                0x2282_a192_60d4_58a3,
                0xba8b_5730_dc27_7d66,
                0xccf4_ddd6_5c95_1f76
            ]
        );
    }

    #[test]
    fn smoke_save_load_round_trip_is_deterministic() {
        let first = run_case(SMOKE_PLACEMENTS, None).expect("first run");
        let second = run_case(SMOKE_PLACEMENTS, None).expect("second run");
        assert_eq!(first["status"], "ok");
        assert_eq!(first["placements"], SMOKE_PLACEMENTS);
        assert_eq!(first["population"], SMOKE_PLACEMENTS * 4);
        assert_eq!(first["named"], NAMED_CITIZENS);
        assert!(first["json_bytes"].as_u64().expect("bytes") > 0);
        assert_eq!(first["checksums"], second["checksums"]);
        assert_eq!(first["combined_checksum"], second["combined_checksum"]);
    }
}
