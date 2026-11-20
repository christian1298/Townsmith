//! Deterministic two-story house room-detection benchmark.
//!
//! Builds an 8 m x 6 m x 6 m house shell with two floors, four rooms, doors,
//! windows, and functional props inside the default 5 x 3 x 5 chunk volume
//! (16 m x 9.6 m x 16 m), then runs room detection repeatedly. Run with
//! `--release` for the reported numbers.

use std::hint::black_box;
use std::time::Instant;

use core_rooms::{detect_rooms, PropInput, PropKind, RoomMaterials, RoomStats};
use core_voxel::{MaterialId, VolumePos, VoxelVolume, AIR};
use serde_json::json;

const FLOOR: MaterialId = 1;
const WALL: MaterialId = 2;
const CEILING: MaterialId = 3;
const DOOR: MaterialId = 4;
const WINDOW: MaterialId = 5;

const WARMUP_ITERATIONS: usize = 3;
const MEASURED_ITERATIONS: usize = 20;

fn pos(x: u32, y: u32, z: u32) -> VolumePos {
    VolumePos::new(x, y, z)
}

fn build_house(volume: &mut VoxelVolume) {
    // Ground slab and outer shell: 8 m x 6 m footprint, 6 m tall.
    volume
        .fill_box(pos(40, 0, 40), pos(120, 2, 100), FLOOR)
        .unwrap();
    volume
        .fill_box(pos(40, 2, 40), pos(120, 60, 100), WALL)
        .unwrap();
    volume
        .fill_box(pos(42, 2, 42), pos(118, 58, 98), AIR)
        .unwrap();

    // First-floor slab and roof.
    volume
        .fill_box(pos(42, 30, 42), pos(118, 32, 98), CEILING)
        .unwrap();
    volume
        .fill_box(pos(40, 58, 40), pos(120, 60, 100), CEILING)
        .unwrap();

    // Ground-floor partition (kitchen / bathroom) with a door block.
    volume
        .fill_box(pos(78, 2, 42), pos(80, 30, 98), WALL)
        .unwrap();
    volume
        .fill_box(pos(78, 2, 60), pos(80, 20, 64), DOOR)
        .unwrap();

    // Upper-floor partition (two bedrooms) with a door block.
    volume
        .fill_box(pos(78, 32, 42), pos(80, 58, 98), WALL)
        .unwrap();
    volume
        .fill_box(pos(78, 32, 60), pos(80, 50, 64), DOOR)
        .unwrap();

    // Exterior door and one window per room on the south wall.
    volume
        .fill_box(pos(60, 2, 40), pos(68, 20, 42), DOOR)
        .unwrap();
    volume
        .fill_box(pos(48, 10, 40), pos(56, 20, 42), WINDOW)
        .unwrap();
    volume
        .fill_box(pos(96, 10, 40), pos(104, 20, 42), WINDOW)
        .unwrap();
    volume
        .fill_box(pos(48, 40, 40), pos(56, 50, 42), WINDOW)
        .unwrap();
    volume
        .fill_box(pos(96, 40, 40), pos(104, 50, 42), WINDOW)
        .unwrap();
}

fn props() -> Vec<PropInput> {
    vec![
        PropInput::new("base:stove", PropKind::Stove, [5.5, 1.0, 6.0]),
        PropInput::new("base:sink", PropKind::Sink, [5.0, 1.0, 7.0]),
        PropInput::new("base:toilet", PropKind::Toilet, [9.0, 1.0, 6.0]),
        PropInput::new("base:shower", PropKind::Shower, [10.0, 1.0, 6.0]),
        PropInput::new("base:bed", PropKind::Bed, [5.0, 4.0, 6.0]),
        PropInput::new("base:desk", PropKind::Desk, [6.5, 4.0, 6.0]),
        PropInput::new("base:bed", PropKind::Bed, [9.0, 4.0, 6.0]),
        PropInput::new("base:desk", PropKind::Desk, [10.5, 4.0, 6.0]),
    ]
}

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
    let mut volume = VoxelVolume::default();
    build_house(&mut volume);

    let materials = RoomMaterials::new(WALL, FLOOR, CEILING, DOOR, WINDOW);
    let props = props();

    for _ in 0..WARMUP_ITERATIONS {
        black_box(detect_rooms(&volume, &materials, &props));
    }

    let mut samples = Vec::with_capacity(MEASURED_ITERATIONS);
    let mut stats = None;
    for _ in 0..MEASURED_ITERATIONS {
        let start = Instant::now();
        let measured = black_box(detect_rooms(&volume, &materials, &props));
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
