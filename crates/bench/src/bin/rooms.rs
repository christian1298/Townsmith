//! Deterministic two-story house room-detection benchmark.
//!
//! Uses the shared acceptance fixture from `core_rooms::demo`: an 8 m x 6 m x
//! 6 m house shell with two floors, four rooms, doors, windows, and functional
//! props inside the default 5 x 3 x 5 chunk volume (16 m x 9.6 m x 16 m), then
//! runs room detection repeatedly. Run with `--release` for the reported
//! numbers.

use std::hint::black_box;
use std::time::Instant;

use core_rooms::demo::{acceptance_house, acceptance_props, acceptance_room_materials};
use core_rooms::RoomStats;
use serde_json::json;

const WARMUP_ITERATIONS: usize = 3;
const MEASURED_ITERATIONS: usize = 20;

fn median(samples: &mut [f64]) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = samples.len() / 2;
    if samples.len().is_multiple_of(2) {
        (samples[mid - 1] + samples[mid]) / 2.0
    } else {
        samples[mid]
    }
}

/// FNV-1a over canonical [`RoomStats`] JSON.
fn checksum(stats: &RoomStats) -> u64 {
    let bytes = serde_json::to_vec(stats).expect("RoomStats serializes");
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn main() {
    let volume = acceptance_house();
    let materials = acceptance_room_materials();
    let props = acceptance_props();

    for _ in 0..WARMUP_ITERATIONS {
        black_box(core_rooms::detect_rooms(&volume, &materials, &props));
    }

    let mut samples = Vec::with_capacity(MEASURED_ITERATIONS);
    let mut stats = None;
    for _ in 0..MEASURED_ITERATIONS {
        let start = Instant::now();
        let measured = black_box(core_rooms::detect_rooms(&volume, &materials, &props));
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
        stats = Some(measured);
    }

    let stats = stats.expect("at least one measured iteration");
    let median_ms = median(&mut samples);
    let min_ms = samples.first().copied().unwrap_or_default();
    let max_ms = samples.last().copied().unwrap_or_default();
    let bounds = volume.config().bounds_m();

    let output = json!({
        "scene": "rooms_two_story_house",
        "bounds_m": bounds,
        "solid_voxels": stats.solid_voxels,
        "material_kinds": stats.materials.len(),
        "materials": stats.materials,
        "room_count": stats.rooms.len(),
        "rooms": stats.rooms,
        "resident_capacity": stats.resident_capacity,
        "job_slots": stats.job_slots,
        "class": stats.class,
        "iterations": MEASURED_ITERATIONS,
        "warmup": WARMUP_ITERATIONS,
        "detection_ms": median_ms,
        "min_detection_ms": min_ms,
        "max_detection_ms": max_ms,
        "checksum": format!("{:016x}", checksum(&stats)),
    });

    println!(
        "{}",
        serde_json::to_string(&output).expect("benchmark output serializes")
    );
}
