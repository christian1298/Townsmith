//! 250,000-placement record harness for the Phase 5 building-count target.
//!
//! Builds a deterministic flat grid of placements through the indexed
//! placement path, then serializes and parses the placement store. This is a
//! record/data harness, not the 1M render gate: it does not build meshes,
//! construction sites, households, or a renderer.

use std::time::Instant;

use core_city::{PlacementIndex, PlacementRequest, PlacementStore};

const SEED: u64 = 2026;
const DEFAULT_PLACEMENTS: usize = 250_000;
const MIN_PLACEMENTS: usize = 1_000;
const MAX_PLACEMENTS: usize = 250_000;
const SMOKE_PLACEMENTS: usize = 25_000;
const SPACING_M: f32 = 10.0;
const BOUNDS_M: [f32; 3] = [8.0, 6.0, 8.0];
const GROUND_M: u16 = 40;
const HOUSE_CAPACITY: u32 = 10;
const WORKPLACE_SLOTS: u32 = 10;

fn main() {
    let smoke = std::env::args().any(|arg| arg == "--smoke");
    let placements = if smoke {
        SMOKE_PLACEMENTS
    } else {
        arg_value("--placements")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(DEFAULT_PLACEMENTS)
    };
    if !(MIN_PLACEMENTS..=MAX_PLACEMENTS).contains(&placements) {
        eprintln!(
            "placements: --placements must be {MIN_PLACEMENTS}..={MAX_PLACEMENTS}, got {placements}"
        );
        std::process::exit(2);
    }

    let first = build(placements);
    if smoke {
        let second = build(placements);
        if first.checksum != second.checksum
            || first.houses != second.houses
            || first.workplaces != second.workplaces
        {
            eprintln!(
                "placements smoke failed: {:016x}/{} vs {:016x}/{}",
                first.checksum, first.houses, second.checksum, second.houses
            );
            std::process::exit(1);
        }
    }

    let (houses, workplaces) = split(placements);
    let result = serde_json::json!({
        "scene": "placements",
        "seed": SEED,
        "placements": first.store.placements.len(),
        "houses": houses,
        "workplaces": workplaces,
        "index_entries": first.index.len(),
        "grid_side": side(placements),
        "spacing_m": SPACING_M,
        "bounds_m": BOUNDS_M,
        "build_ms": first.build_ms,
        "per_placement_us": first.build_ms * 1_000.0 / placements as f64,
        "serialized_bytes": first.serialized_bytes,
        "serialize_ms": first.serialize_ms,
        "parse_ms": first.parse_ms,
        "checksum": format!("{:016x}", first.checksum),
        "deterministic": smoke,
        "status": "ok",
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&result).expect("pretty json")
    );
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

fn side(placements: usize) -> i64 {
    (placements as f64).sqrt().ceil() as i64
}

fn split(placements: usize) -> (usize, usize) {
    let houses = placements * 5 / 6;
    (houses, placements - houses)
}

struct Built {
    store: PlacementStore,
    index: PlacementIndex,
    checksum: u64,
    build_ms: f64,
    serialize_ms: f64,
    parse_ms: f64,
    serialized_bytes: usize,
    houses: usize,
    workplaces: usize,
}

fn build(placements: usize) -> Built {
    let (houses, workplaces) = split(placements);
    let side = side(placements);
    let offset = -((side - 1) as f32 * SPACING_M) * 0.5;
    let mut store = PlacementStore::new(SEED);
    let mut index = PlacementIndex::new();
    let build_start = Instant::now();
    for slot in 0..placements {
        let column = (slot as i64) % side;
        let row = (slot as i64) / side;
        let house = slot < houses;
        let request = PlacementRequest {
            blueprint_id: if house {
                "base:tenement".to_string()
            } else {
                "base:workshop".to_string()
            },
            blueprint_hash: "0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
            blueprint_path: if house {
                "res://content/base/tenement.tsbp".to_string()
            } else {
                "res://content/base/workshop.tsbp".to_string()
            },
            name: if house {
                "Tenement".to_string()
            } else {
                "Workshop".to_string()
            },
            bounds_m: BOUNDS_M,
            center_x: offset + column as f32 * SPACING_M,
            center_z: offset + row as f32 * SPACING_M,
            rotation_deg: 0,
            material_cost: if house { 60_000.0 } else { 72_000.0 },
            resident_capacity: if house { HOUSE_CAPACITY } else { 0 },
            job_slots: if house { 0 } else { WORKPLACE_SLOTS },
            solid_voxels: if house { 4_800 } else { 5_760 },
            structure: None,
        };
        store
            .place_indexed(&mut index, &request, |_, _| GROUND_M)
            .expect("deterministic placement");
    }
    let build_ms = ms(build_start.elapsed());

    let serialize_start = Instant::now();
    let json = serde_json::to_string(&store).expect("placement json");
    let serialize_ms = ms(serialize_start.elapsed());
    let serialized_bytes = json.len();

    let parse_start = Instant::now();
    let parsed = PlacementStore::from_json(&json).expect("placement parse");
    let parse_ms = ms(parse_start.elapsed());
    assert_eq!(parsed.placements.len(), placements);
    let checksum = parsed.checksum();

    Built {
        store,
        index,
        checksum,
        build_ms,
        serialize_ms,
        parse_ms,
        serialized_bytes,
        houses,
        workplaces,
    }
}

fn ms(duration: std::time::Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_is_five_sixths_houses() {
        assert_eq!(split(6), (5, 1));
        assert_eq!(split(120), (100, 20));
        assert_eq!(split(25_000), (20_833, 4_167));
    }

    #[test]
    fn small_build_is_deterministic_and_indexed() {
        let first = build(1_000);
        let second = build(1_000);
        assert_eq!(first.checksum, second.checksum);
        assert_eq!(first.store.placements.len(), 1_000);
        assert_eq!(first.index.len(), 1_000);
        assert_eq!(first.houses, 833);
        assert_eq!(first.workplaces, 167);
        assert!(first.serialized_bytes > 0);
    }
}
