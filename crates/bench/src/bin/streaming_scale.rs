#![recursion_limit = "256"]

//! Deterministic 100 km² streaming-scale data harness.
//!
//! This is a **data harness**, not a rendering benchmark. It models a 10 km ×
//! 10 km (100 km²) area with a fixed out-and-back flyover route of 41 chunk
//! centres (10.24 km) and a bounded resident set built from three LOD bands:
//!
//! | Band | Chebyshev radius | Mesh step |
//! |---|---:|---:|
//! | near | 1 chunk (256 m) | 1 (full) |
//! | mid | 4 chunks (1.02 km) | 4 |
//! | far | 7 chunks (1.79 km) | 8 |
//!
//! As the centre moves, entering chunks are generated from the fixed seed and
//! meshed with the seam-aware terrain mesher, leaving chunks are evicted, and
//! evicted meshes are cached for reuse on the return leg and on LOD band
//! changes. Generation and meshing are bounded by a per-step chunk budget and
//! a hard resident cap.
//!
//! A second deterministic layer places up to 250,000 compact building records
//! (8 bytes each: position, D-024 tier, height class, footprint) on a
//! near-square grid across the same 100 km² area. A building is resident only
//! while its chunk is resident, so it enters with the chunk and is evicted with
//! it. `--buildings 0` reproduces the terrain-only scene and keeps the original
//! checksum semantics.
//!
//! The harness reports per-step generate/mesh time, evictions, peak resident
//! chunks, peak resident mesh bytes, peak resident buildings and building
//! bytes, total resident bytes, cumulative quads/triangles, total elapsed
//! time, a deterministic terrain checksum over the route, band assignment,
//! chunk heights, and mesh output, and a separate building checksum over the
//! configured grid, resident records, and per-step building counters.
//! `--smoke` uses a 25-chunk resident set and 25,000 buildings and fails on
//! checksum, count, or peak drift.
//!
//! No renderer, GPU, frame pacing, simulation, or save/load is involved. This
//! does not replace the 1M / 30 FPS metropolis gate or its streaming check
//! (docs/68 §8.2 #16). See docs/68 §7.9.
//!
//! Runtime is dominated by full-resolution chunk generation (~14 ms/chunk on
//! the current developer machine), not by meshing. The default scene stays
//! under ~30 s in a release build; a full 100 km² generation would take far
//! longer single-threaded, which is the gap this harness measures.

use std::collections::{BTreeMap, BTreeSet};
use std::hint::black_box;
use std::ops::Range;
use std::time::Instant;

use core_world::{
    terrain_mesh_lod_with_sampler, TerrainMesh, TerrainPos, TerrainVertex, WorldChunk,
    WorldChunkCoord, WorldGenerator, WORLD_CHUNK_METERS,
};

/// Fixed seed for the whole scene.
const SEED: u64 = 0x5354_5245_414D_3130; // "STREAM10"
/// Modelled area: 10 km × 10 km (nominal).
const AREA_KM2: f64 = 100.0;
/// Side of the modelled building area in chunks. 39 × 256 m = 9.984 km, so the
/// nominal 10 km × 10 km / 100 km² area is 99.68 km² of full chunks.
const AREA_CHUNKS: i64 = 39;
/// Area side in metres; exactly `AREA_CHUNKS * WORLD_CHUNK_METERS`.
const AREA_METERS: u32 = AREA_CHUNKS as u32 * WORLD_CHUNK_METERS as u32;
/// Route start chunk, inside the modelled area.
const ROUTE_START: WorldChunkCoord = WorldChunkCoord::new(-20, 0);
/// One-way moves. 20 moves each way = 40 × 256 m = 10.24 km.
const ROUTE_MOVES: usize = 20;

const NEAR_RADIUS: i64 = 1;
const MID_RADIUS: i64 = 4;
const FAR_RADIUS: i64 = 7;
const NEAR_LOD_STEP: usize = 1;
const MID_LOD_STEP: usize = 4;
const FAR_LOD_STEP: usize = 8;

const RESIDENT_CAP: usize = 256;
const CHUNK_BUDGET_PER_STEP: usize = 32;
const MESH_CACHE_CAP: usize = 384;

/// Default compact building records across the modelled area.
const DEFAULT_BUILDINGS: usize = 250_000;
/// Hard upper bound for `--buildings`.
const MAX_BUILDINGS: usize = 250_000;
/// Size of one compact building record, in bytes.
const BUILDING_RECORD_BYTES: usize = std::mem::size_of::<BuildingRecord>();

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
/// Domain tag: "STREAM10".
const CHECKSUM_DOMAIN: u64 = 0x5354_5245_414D_3130;
/// Domain tag: "BUILDI".
const BUILDING_CHECKSUM_DOMAIN: u64 = 0x4255_494C_4449;

const SMOKE_ROUTE_MOVES: usize = 4;
const SMOKE_RESIDENT_CAP: usize = 32;
const SMOKE_CHUNK_BUDGET: usize = 32;
const SMOKE_MESH_CACHE_CAP: usize = 64;
const SMOKE_BUILDINGS: usize = 25_000;

/// Pinned terrain smoke checksum. A change here is an intentional fixture
/// change or a generation/meshing regression.
///
/// The terrain checksum deliberately does not cover the building layer, so
/// `--buildings 0` and the default building layer share it.
const SMOKE_CHECKSUM: u64 = 0xf307_4038_0558_f66d;
/// Pinned smoke building checksum (25,000 buildings on the smoke route).
/// This is new fixture data; it does not move [`SMOKE_CHECKSUM`].
const SMOKE_BUILDING_CHECKSUM: u64 = 0x68f6_2468_d8b4_6b6b;

/// Scene parameters. Tests use the smoke configuration of the same scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Config {
    seed: u64,
    start: WorldChunkCoord,
    route_moves: usize,
    near_radius: i64,
    mid_radius: i64,
    far_radius: i64,
    near_lod_step: usize,
    mid_lod_step: usize,
    far_lod_step: usize,
    resident_cap: usize,
    chunk_budget_per_step: usize,
    mesh_cache_cap: usize,
    buildings: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            seed: SEED,
            start: ROUTE_START,
            route_moves: ROUTE_MOVES,
            near_radius: NEAR_RADIUS,
            mid_radius: MID_RADIUS,
            far_radius: FAR_RADIUS,
            near_lod_step: NEAR_LOD_STEP,
            mid_lod_step: MID_LOD_STEP,
            far_lod_step: FAR_LOD_STEP,
            resident_cap: RESIDENT_CAP,
            chunk_budget_per_step: CHUNK_BUDGET_PER_STEP,
            mesh_cache_cap: MESH_CACHE_CAP,
            buildings: DEFAULT_BUILDINGS,
        }
    }
}

impl Config {
    const fn smoke() -> Self {
        Self {
            seed: SEED,
            start: ROUTE_START,
            route_moves: SMOKE_ROUTE_MOVES,
            near_radius: 1,
            mid_radius: 2,
            far_radius: 2,
            near_lod_step: 1,
            mid_lod_step: 2,
            far_lod_step: 4,
            resident_cap: SMOKE_RESIDENT_CAP,
            chunk_budget_per_step: SMOKE_CHUNK_BUDGET,
            mesh_cache_cap: SMOKE_MESH_CACHE_CAP,
            buildings: SMOKE_BUILDINGS,
        }
    }

    /// Fixed out-and-back route over the fixed 10 km axis.
    fn route(&self) -> Vec<WorldChunkCoord> {
        let mut route = Vec::with_capacity(self.route_moves * 2 + 1);
        for step in 0..=self.route_moves {
            route.push(WorldChunkCoord::new(
                self.start.x + step as i64,
                self.start.z,
            ));
        }
        for step in (0..self.route_moves).rev() {
            route.push(WorldChunkCoord::new(
                self.start.x + step as i64,
                self.start.z,
            ));
        }
        route
    }

    /// Mesh step for a Chebyshev distance from the centre.
    const fn band_step(&self, distance: i64) -> usize {
        if distance <= self.near_radius {
            self.near_lod_step
        } else if distance <= self.mid_radius {
            self.mid_lod_step
        } else {
            self.far_lod_step
        }
    }

    /// Deterministic 100 km² building layout for this scene.
    fn area(&self) -> AreaLayout {
        let center_x = self.start.x + (self.route_moves as i64) / 2;
        AreaLayout {
            origin: WorldChunkCoord::new(
                center_x - AREA_CHUNKS / 2,
                self.start.z - AREA_CHUNKS / 2,
            ),
            seed: self.seed,
            buildings: self.buildings,
            grid_side: grid_side(self.buildings),
        }
    }
}

/// One compact building record: 8 bytes, no blueprint string and no render
/// data. Positions are in metres from the area origin.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BuildingRecord {
    x_m: u16,
    z_m: u16,
    /// D-024 construction tier, 0..=6 (tent/shed through steel/glass).
    tier: u8,
    /// Stories band, 1..=8.
    height_class: u8,
    footprint_m2: u16,
}

impl BuildingRecord {
    fn new(seed: u64, index: usize, gx: usize, gz: usize, grid_side: usize) -> Self {
        debug_assert!(grid_side > 0);
        let x_m = ((gx as u64 * u64::from(AREA_METERS)) / grid_side as u64) as u16;
        let z_m = ((gz as u64 * u64::from(AREA_METERS)) / grid_side as u64) as u16;
        // Per-index mixing keeps generation independent of chunk visit order.
        let hash =
            mix64(seed ^ 0xB01D_1A6E_5EED ^ (index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        Self {
            x_m,
            z_m,
            tier: (hash % 7) as u8,
            height_class: 1 + ((hash >> 8) % 8) as u8,
            footprint_m2: 48 + ((hash >> 16) % 400) as u16,
        }
    }

    /// Pack the record into one `u64` for checksumming without reading padding.
    const fn packed(self) -> u64 {
        (self.x_m as u64)
            | ((self.z_m as u64) << 16)
            | ((self.tier as u64) << 32)
            | ((self.height_class as u64) << 40)
            | ((self.footprint_m2 as u64) << 48)
    }
}

/// Deterministic near-square building grid over the 100 km² area.
#[derive(Debug, Clone, Copy)]
struct AreaLayout {
    origin: WorldChunkCoord,
    seed: u64,
    buildings: usize,
    grid_side: usize,
}

impl AreaLayout {
    /// Buildings whose chunk is `coord`, in deterministic `(gz, gx)` order.
    fn chunk_buildings(&self, coord: WorldChunkCoord) -> Vec<BuildingRecord> {
        if self.buildings == 0 || self.grid_side == 0 {
            return Vec::new();
        }
        let local_x = coord.x - self.origin.x;
        let local_z = coord.z - self.origin.z;
        if !(0..AREA_CHUNKS).contains(&local_x) || !(0..AREA_CHUNKS).contains(&local_z) {
            return Vec::new();
        }

        let columns = chunk_cell_range(local_x as usize, self.grid_side);
        let rows = chunk_cell_range(local_z as usize, self.grid_side);
        let mut records = Vec::with_capacity(columns.len() * rows.len());
        for gz in rows {
            for gx in columns.clone() {
                let index = gz * self.grid_side + gx;
                if index >= self.buildings {
                    continue;
                }
                records.push(BuildingRecord::new(
                    self.seed,
                    index,
                    gx,
                    gz,
                    self.grid_side,
                ));
            }
        }
        records
    }
}

/// Smallest square grid side that can hold `buildings` records.
fn grid_side(buildings: usize) -> usize {
    let mut side = 0usize;
    while side * side < buildings {
        side += 1;
    }
    side
}

/// Grid columns whose bucket `gx * AREA_CHUNKS / grid_side` is `local_chunk`.
fn chunk_cell_range(local_chunk: usize, grid_side: usize) -> Range<usize> {
    if grid_side == 0 {
        return 0..0;
    }
    let chunks = AREA_CHUNKS as usize;
    let start = (local_chunk * grid_side).div_ceil(chunks);
    let end = ((local_chunk + 1) * grid_side).div_ceil(chunks);
    start..end
}

/// SplitMix64 finalizer; stable, dependency-free mixing.
fn mix64(value: u64) -> u64 {
    let mut x = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// One resident chunk and its current LOD mesh.
#[derive(Debug)]
struct Resident {
    chunk: WorldChunk,
    lod_step: usize,
    mesh: TerrainMesh,
    stats: MeshStats,
    /// Compact records resident with this chunk.
    buildings: Vec<BuildingRecord>,
}

/// Byte and triangle counts for one mesh.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct MeshStats {
    bytes: usize,
    quads: u64,
    triangles: u64,
}

impl MeshStats {
    fn from_mesh(mesh: &TerrainMesh) -> Self {
        Self {
            bytes: mesh.vertices.len() * std::mem::size_of::<TerrainVertex>()
                + mesh.indices.len() * std::mem::size_of::<u32>(),
            quads: mesh.quad_count() as u64,
            triangles: (mesh.indices.len() / 3) as u64,
        }
    }
}

type CacheKey = (WorldChunkCoord, usize);

struct CachedMesh {
    mesh: TerrainMesh,
    stats: MeshStats,
    used: u64,
}

/// Small deterministic LRU cache for evicted meshes.
struct MeshCache {
    entries: BTreeMap<CacheKey, CachedMesh>,
    capacity: usize,
    clock: u64,
    peak_entries: usize,
}

impl MeshCache {
    fn new(capacity: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            capacity,
            clock: 0,
            peak_entries: 0,
        }
    }

    fn take(&mut self, key: &CacheKey) -> Option<(TerrainMesh, MeshStats)> {
        let entry = self.entries.remove(key)?;
        Some((entry.mesh, entry.stats))
    }

    fn insert(&mut self, key: CacheKey, mesh: TerrainMesh, stats: MeshStats) {
        if self.capacity == 0 {
            return;
        }
        self.clock += 1;
        self.entries.insert(
            key,
            CachedMesh {
                mesh,
                stats,
                used: self.clock,
            },
        );
        while self.entries.len() > self.capacity {
            let victim = self
                .entries
                .iter()
                .min_by_key(|(key, entry)| (entry.used, **key))
                .map(|(key, _)| *key);
            if let Some(victim) = victim {
                self.entries.remove(&victim);
            }
        }
        self.peak_entries = self.peak_entries.max(self.entries.len());
    }
}

/// Measured state after the route finishes.
#[derive(Debug)]
struct Outcome {
    config: Config,
    route_centers: usize,
    route_moves: usize,
    route_km: f64,
    chunks_visited: usize,
    chunk_entries: usize,
    chunks_generated: usize,
    peak_resident: usize,
    mesh_bytes_peak: usize,
    resident_bytes_peak: usize,
    resident_buildings: usize,
    resident_buildings_peak: usize,
    resident_building_bytes: usize,
    building_bytes_peak: usize,
    building_bytes_total: usize,
    total_resident_bytes_peak: usize,
    building_entries: usize,
    building_evictions: usize,
    building_grid_side: usize,
    mesh_bytes_total: u64,
    quads: u64,
    triangles: u64,
    evictions: usize,
    budget_skipped: usize,
    cache_hits: usize,
    cache_misses: usize,
    cache_peak_entries: usize,
    generate_ms_total: f64,
    mesh_ms_total: f64,
    building_ms_total: f64,
    step_samples: Vec<f64>,
    total_ms: f64,
    checksum: u64,
    building_checksum: u64,
    status: &'static str,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--smoke") {
        run_smoke();
        return;
    }

    let buildings = match parse_buildings(&args) {
        Ok(buildings) => buildings,
        Err(message) => {
            eprintln!("streaming_scale: {message}");
            eprintln!("usage: streaming_scale [--buildings N] [--smoke]");
            std::process::exit(2);
        }
    };

    let outcome = simulate(Config {
        buildings,
        ..Config::default()
    });
    let report = serde_json::to_string(&report(&outcome)).expect("json");
    println!("{report}");
    eprintln!(
        "STREAMING_SCALE chunks={} peak_resident={} mesh_bytes_peak={} \
         buildings={} resident_buildings_peak={} building_bytes_peak={} \
         total_resident_bytes_peak={} generate_ms_mean={:.2} mesh_ms_mean={:.2} \
         building_ms_mean={:.2} step_ms_p95={:.2} checksum={:016x} \
         building_checksum={:016x} status={}",
        outcome.chunks_visited,
        outcome.peak_resident,
        outcome.mesh_bytes_peak,
        outcome.config.buildings,
        outcome.resident_buildings_peak,
        outcome.building_bytes_peak,
        outcome.total_resident_bytes_peak,
        mean_ms(&outcome.generate_ms_total, outcome.step_samples.len()),
        mean_ms(&outcome.mesh_ms_total, outcome.step_samples.len()),
        mean_ms(&outcome.building_ms_total, outcome.step_samples.len()),
        percentile(&outcome.step_samples, 0.95),
        outcome.checksum,
        outcome.building_checksum,
        outcome.status
    );
}

/// Parse `--buildings N` / `--buildings=N`; 0 keeps the terrain-only scene.
fn parse_buildings(args: &[String]) -> Result<usize, String> {
    let mut buildings = DEFAULT_BUILDINGS;
    let mut index = 0usize;
    while index < args.len() {
        let arg = &args[index];
        if let Some(value) = arg.strip_prefix("--buildings=") {
            buildings = parse_building_count(value)?;
        } else if arg == "--buildings" {
            index += 1;
            let value = args
                .get(index)
                .ok_or_else(|| "--buildings needs a value in 0..=250000".to_string())?;
            buildings = parse_building_count(value)?;
        } else if arg == "--help" || arg == "-h" {
            println!(
                "usage: streaming_scale [--buildings N] [--smoke]\n\
                 \n\
                 --buildings N  compact building records over the 100 km^2 area\n\
                 \x20              (0..=250000, default 250000; 0 = terrain-only)\n\
                 --smoke        two-run deterministic smoke scene (25,000 buildings)"
            );
            std::process::exit(0);
        } else {
            return Err(format!("unknown argument: {arg}"));
        }
        index += 1;
    }
    Ok(buildings)
}

fn parse_building_count(value: &str) -> Result<usize, String> {
    let count: usize = value
        .parse()
        .map_err(|_| format!("invalid building count: {value}"))?;
    if count > MAX_BUILDINGS {
        return Err(format!(
            "building count {count} exceeds the {MAX_BUILDINGS} limit"
        ));
    }
    Ok(count)
}

/// Run the smoke scene twice and fail unless the terrain checksum, building
/// checksum, counts, and peaks are identical and pinned.
fn run_smoke() {
    let config = Config::smoke();
    let first = simulate(config);
    let second = simulate(config);

    let deterministic = first.checksum == second.checksum
        && first.building_checksum == second.building_checksum
        && first.chunks_visited == second.chunks_visited
        && first.chunks_generated == second.chunks_generated
        && first.peak_resident == second.peak_resident
        && first.mesh_bytes_peak == second.mesh_bytes_peak
        && first.resident_buildings_peak == second.resident_buildings_peak
        && first.building_bytes_peak == second.building_bytes_peak
        && first.total_resident_bytes_peak == second.total_resident_bytes_peak
        && first.building_entries == second.building_entries
        && first.quads == second.quads
        && first.evictions == second.evictions;
    let expected_resident = ((config.far_radius * 2 + 1) as usize).pow(2);
    let peak_ok = first.peak_resident == expected_resident;
    let pinned = first.checksum == SMOKE_CHECKSUM;
    let building_pinned = first.building_checksum == SMOKE_BUILDING_CHECKSUM;
    let buildings_ok = first.config.buildings == SMOKE_BUILDINGS
        && first.resident_buildings_peak > 0
        && first.resident_buildings_peak <= first.config.buildings
        && first.building_bytes_peak == first.resident_buildings_peak * BUILDING_RECORD_BYTES
        && first.total_resident_bytes_peak >= first.resident_bytes_peak;
    let ok = deterministic
        && peak_ok
        && pinned
        && building_pinned
        && buildings_ok
        && first.status == "ok";

    println!(
        "STREAMING_SCALE_SMOKE chunks={} peak_resident={} mesh_bytes_peak={} \
         buildings={} resident_buildings_peak={} building_bytes_peak={} \
         generate_ms_mean={:.2} mesh_ms_mean={:.2} checksum={:016x} \
         building_checksum={:016x} deterministic={} peak_ok={} pinned={} \
         building_pinned={} buildings_ok={} status={}",
        first.chunks_visited,
        first.peak_resident,
        first.mesh_bytes_peak,
        first.config.buildings,
        first.resident_buildings_peak,
        first.building_bytes_peak,
        mean_ms(&first.generate_ms_total, first.step_samples.len()),
        mean_ms(&first.mesh_ms_total, first.step_samples.len()),
        first.checksum,
        first.building_checksum,
        deterministic,
        peak_ok,
        pinned,
        building_pinned,
        buildings_ok,
        if ok { "ok" } else { "failed" }
    );
    if !ok {
        std::process::exit(1);
    }
}

/// Deterministic chunks in the far ring around one centre, with their LOD step.
fn desired_chunks(config: &Config, center: WorldChunkCoord) -> Vec<(WorldChunkCoord, usize)> {
    let side = (config.far_radius * 2 + 1) as usize;
    let mut desired = Vec::with_capacity(side * side);
    for dz in -config.far_radius..=config.far_radius {
        for dx in -config.far_radius..=config.far_radius {
            let coord = WorldChunkCoord::new(center.x + dx, center.z + dz);
            desired.push((coord, config.band_step(coord.distance_chebyshev(center))));
        }
    }
    desired
}

fn build_mesh(generator: &WorldGenerator, chunk: &WorldChunk, step: usize) -> TerrainMesh {
    terrain_mesh_lod_with_sampler(chunk, step, |x, z| {
        generator.height_at(TerrainPos::new(x, z))
    })
}

fn simulate(config: Config) -> Outcome {
    let scene_start = Instant::now();
    let route = config.route();
    let generator = WorldGenerator::new(config.seed);
    let area = config.area();

    let mut checksum = FNV_OFFSET;
    checksum = fnv_u64(checksum, CHECKSUM_DOMAIN);
    checksum = fnv_u64(checksum, config.seed);
    checksum = fnv_u64(checksum, config.near_radius as u64);
    checksum = fnv_u64(checksum, config.mid_radius as u64);
    checksum = fnv_u64(checksum, config.far_radius as u64);
    checksum = fnv_u64(checksum, config.near_lod_step as u64);
    checksum = fnv_u64(checksum, config.mid_lod_step as u64);
    checksum = fnv_u64(checksum, config.far_lod_step as u64);
    checksum = fnv_u64(checksum, route.len() as u64);
    for coord in &route {
        checksum = hash_coord(checksum, *coord);
    }

    let mut building_checksum = FNV_OFFSET;
    building_checksum = fnv_u64(building_checksum, BUILDING_CHECKSUM_DOMAIN);
    building_checksum = fnv_u64(building_checksum, config.seed);
    building_checksum = fnv_u64(building_checksum, area.buildings as u64);
    building_checksum = fnv_u64(building_checksum, area.grid_side as u64);
    building_checksum = fnv_u64(building_checksum, AREA_CHUNKS as u64);
    building_checksum = hash_coord(building_checksum, area.origin);

    let mut resident: BTreeMap<WorldChunkCoord, Resident> = BTreeMap::new();
    let mut cache = MeshCache::new(config.mesh_cache_cap);
    let mut visited: BTreeSet<WorldChunkCoord> = BTreeSet::new();

    let mut peak_resident = 0usize;
    let mut mesh_bytes_peak = 0usize;
    let mut resident_bytes_peak = 0usize;
    let mut resident_buildings = 0usize;
    let mut resident_buildings_peak = 0usize;
    let mut building_bytes_peak = 0usize;
    let mut total_resident_bytes_peak = 0usize;
    let mut building_entries = 0usize;
    let mut building_evictions = 0usize;
    let mut mesh_bytes_total = 0u64;
    let mut quads = 0u64;
    let mut triangles = 0u64;
    let mut evictions = 0usize;
    let mut budget_skipped = 0usize;
    let mut cache_hits = 0usize;
    let mut cache_misses = 0usize;
    let mut chunk_entries = 0usize;
    let mut chunks_generated = 0usize;
    let mut generate_ms_total = 0.0f64;
    let mut mesh_ms_total = 0.0f64;
    let mut building_ms_total = 0.0f64;
    let mut step_samples = Vec::with_capacity(route.len());

    for (step_index, &center) in route.iter().enumerate() {
        let step_start = Instant::now();
        checksum = fnv_u64(checksum, step_index as u64);
        checksum = hash_coord(checksum, center);

        let desired = desired_chunks(&config, center);
        let desired_set: BTreeSet<WorldChunkCoord> =
            desired.iter().map(|(coord, _)| *coord).collect();

        // Evict chunks that left the far ring.
        let leaving: Vec<WorldChunkCoord> = resident
            .keys()
            .copied()
            .filter(|coord| !desired_set.contains(coord))
            .collect();
        let mut evicted = 0usize;
        let mut step_building_evictions = 0usize;
        for coord in leaving {
            if let Some(entry) = resident.remove(&coord) {
                step_building_evictions += entry.buildings.len();
                cache.insert((coord, entry.lod_step), entry.mesh, entry.stats);
                evicted += 1;
            }
        }

        // Collect LOD band changes before touching the map.
        let mut promotions = Vec::new();
        for (coord, lod_step) in &desired {
            if let Some(entry) = resident.get(coord) {
                if entry.lod_step != *lod_step {
                    promotions.push((*coord, *lod_step));
                }
            }
        }

        let mut generated = 0usize;
        let mut mesh_builds = 0usize;
        let mut step_hits = 0usize;
        let mut step_misses = 0usize;
        let mut step_building_entries = 0usize;
        let mut generate_step_ms = 0.0f64;
        let mut mesh_step_ms = 0.0f64;
        let mut building_step_ms = 0.0f64;

        for (coord, new_step) in promotions {
            let entry = resident.get_mut(&coord).expect("resident chunk");
            let old_step = entry.lod_step;
            let old_mesh = std::mem::take(&mut entry.mesh);
            cache.insert((coord, old_step), old_mesh, entry.stats);

            let (mesh, stats) = match cache.take(&(coord, new_step)) {
                Some((mesh, stats)) => {
                    step_hits += 1;
                    (mesh, stats)
                }
                None => {
                    step_misses += 1;
                    let mesh_start = Instant::now();
                    let mesh = black_box(build_mesh(&generator, &entry.chunk, new_step));
                    mesh_step_ms += ms(mesh_start.elapsed());
                    let stats = MeshStats::from_mesh(&mesh);
                    mesh_bytes_total += stats.bytes as u64;
                    quads += stats.quads;
                    triangles += stats.triangles;
                    (mesh, stats)
                }
            };

            entry.mesh = mesh;
            entry.stats = stats;
            entry.lod_step = new_step;
            mesh_builds += 1;
        }

        // Generate and mesh new chunks in a deterministic ring order, bounded
        // by the per-step chunk budget and the resident cap.
        for (coord, lod_step) in &desired {
            if resident.contains_key(coord) {
                continue;
            }
            if generated >= config.chunk_budget_per_step || resident.len() >= config.resident_cap {
                budget_skipped += 1;
                continue;
            }

            let generate_start = Instant::now();
            let chunk = generator.generate(*coord).expect("chunk generation");
            generate_step_ms += ms(generate_start.elapsed());
            generated += 1;
            chunks_generated += 1;
            checksum = hash_chunk(checksum, *coord, *lod_step, &chunk);

            let building_start = Instant::now();
            let buildings = area.chunk_buildings(*coord);
            building_step_ms += ms(building_start.elapsed());
            step_building_entries += buildings.len();

            let (mesh, stats) = match cache.take(&(*coord, *lod_step)) {
                Some((mesh, stats)) => {
                    step_hits += 1;
                    (mesh, stats)
                }
                None => {
                    step_misses += 1;
                    let mesh_start = Instant::now();
                    let mesh = black_box(build_mesh(&generator, &chunk, *lod_step));
                    mesh_step_ms += ms(mesh_start.elapsed());
                    let stats = MeshStats::from_mesh(&mesh);
                    mesh_bytes_total += stats.bytes as u64;
                    quads += stats.quads;
                    triangles += stats.triangles;
                    (mesh, stats)
                }
            };

            resident.insert(
                *coord,
                Resident {
                    chunk,
                    lod_step: *lod_step,
                    mesh,
                    stats,
                    buildings,
                },
            );
            visited.insert(*coord);
            chunk_entries += 1;
            mesh_builds += 1;
        }

        let resident_mesh_bytes: usize = resident.values().map(|entry| entry.stats.bytes).sum();
        let resident_terrain_bytes: usize = resident
            .values()
            .map(|entry| entry.chunk.memory_bytes() + entry.stats.bytes)
            .sum();
        resident_buildings = resident
            .values()
            .map(|entry| entry.buildings.len())
            .sum::<usize>();
        let resident_building_bytes = resident_buildings * BUILDING_RECORD_BYTES;
        let resident_total_bytes = resident_terrain_bytes + resident_building_bytes;

        peak_resident = peak_resident.max(resident.len());
        mesh_bytes_peak = mesh_bytes_peak.max(resident_mesh_bytes);
        resident_bytes_peak = resident_bytes_peak.max(resident_terrain_bytes);
        resident_buildings_peak = resident_buildings_peak.max(resident_buildings);
        building_bytes_peak = building_bytes_peak.max(resident_building_bytes);
        total_resident_bytes_peak = total_resident_bytes_peak.max(resident_total_bytes);
        evictions += evicted;
        building_entries += step_building_entries;
        building_evictions += step_building_evictions;
        cache_hits += step_hits;
        cache_misses += step_misses;
        generate_ms_total += generate_step_ms;
        mesh_ms_total += mesh_step_ms;
        building_ms_total += building_step_ms;

        // Building checksum: configured grid, resident records (chunk order,
        // then `(gz, gx)` order inside each chunk), and per-step counters.
        building_checksum = fnv_u64(building_checksum, step_index as u64);
        building_checksum = fnv_u64(building_checksum, step_building_entries as u64);
        building_checksum = fnv_u64(building_checksum, step_building_evictions as u64);
        building_checksum = fnv_u64(building_checksum, resident_buildings as u64);
        for (coord, entry) in &resident {
            if entry.buildings.is_empty() {
                continue;
            }
            building_checksum = hash_coord(building_checksum, *coord);
            building_checksum = fnv_u64(building_checksum, entry.buildings.len() as u64);
            for record in &entry.buildings {
                building_checksum = fnv_u64(building_checksum, record.packed());
            }
        }

        checksum = fnv_u64(checksum, evicted as u64);
        checksum = fnv_u64(checksum, generated as u64);
        checksum = fnv_u64(checksum, mesh_builds as u64);
        checksum = fnv_u64(checksum, step_hits as u64);
        checksum = fnv_u64(checksum, step_misses as u64);
        checksum = fnv_u64(checksum, resident.len() as u64);
        checksum = fnv_u64(checksum, resident_mesh_bytes as u64);
        checksum = fnv_u64(checksum, quads);

        step_samples.push(ms(step_start.elapsed()));
    }

    let status = if budget_skipped == 0 && peak_resident <= config.resident_cap {
        "ok"
    } else {
        "capped"
    };

    Outcome {
        config,
        route_centers: route.len(),
        route_moves: route.len().saturating_sub(1),
        route_km: route.len().saturating_sub(1) as f64 * WORLD_CHUNK_METERS as f64 / 1000.0,
        chunks_visited: visited.len(),
        chunk_entries,
        chunks_generated,
        peak_resident,
        mesh_bytes_peak,
        resident_bytes_peak,
        resident_buildings,
        resident_buildings_peak,
        resident_building_bytes: resident_buildings * BUILDING_RECORD_BYTES,
        building_bytes_peak,
        building_bytes_total: building_entries * BUILDING_RECORD_BYTES,
        total_resident_bytes_peak,
        building_entries,
        building_evictions,
        building_grid_side: area.grid_side,
        mesh_bytes_total,
        quads,
        triangles,
        evictions,
        budget_skipped,
        cache_hits,
        cache_misses,
        cache_peak_entries: cache.peak_entries,
        generate_ms_total,
        mesh_ms_total,
        building_ms_total,
        step_samples,
        total_ms: ms(scene_start.elapsed()),
        checksum,
        building_checksum,
        status,
    }
}

fn report(outcome: &Outcome) -> serde_json::Value {
    let config = outcome.config;
    let area = config.area();
    let steps = outcome.step_samples.len();
    let building_area_km2 = (AREA_METERS as f64 / 1000.0) * (AREA_METERS as f64 / 1000.0);
    serde_json::json!({
        "scene": "streaming_scale",
        "format_version": 2,
        "render": false,
        "note": "Deterministic streaming data harness: 10 km x 10 km nominal terrain area, 10.24 km out-and-back route, near/mid/far resident bands, terrain generation and LOD meshing, plus a deterministic compact building-record layer over a 39 x 39 chunk area. No renderer, GPU, frame pacing, or simulation; not the 1M / 30 FPS metropolis gate (docs/68 section 8.2 checks 16-17).",
        "seed": config.seed,
        "area_km2": AREA_KM2,
        "route": "ten_km_out_and_back",
        "route_chunks": outcome.route_centers,
        "route_moves": outcome.route_moves,
        "route_km": outcome.route_km,
        "chunk_size_m": WORLD_CHUNK_METERS,
        "near_radius_chunks": config.near_radius,
        "mid_radius_chunks": config.mid_radius,
        "far_radius_chunks": config.far_radius,
        "near_lod_step": config.near_lod_step,
        "mid_lod_step": config.mid_lod_step,
        "far_lod_step": config.far_lod_step,
        "resident_cap": config.resident_cap,
        "chunk_budget_per_step": config.chunk_budget_per_step,
        "mesh_cache_cap": config.mesh_cache_cap,
        "chunks_visited": outcome.chunks_visited,
        "chunk_entries": outcome.chunk_entries,
        "chunks_generated": outcome.chunks_generated,
        "peak_resident": outcome.peak_resident,
        "mesh_bytes_peak": outcome.mesh_bytes_peak,
        "resident_bytes_peak": outcome.resident_bytes_peak,
        "mesh_bytes_total": outcome.mesh_bytes_total,
        "quads": outcome.quads,
        "triangles": outcome.triangles,
        "evictions": outcome.evictions,
        "budget_skipped": outcome.budget_skipped,
        "mesh_cache_hits": outcome.cache_hits,
        "mesh_cache_misses": outcome.cache_misses,
        "mesh_cache_peak_entries": outcome.cache_peak_entries,
        "generate_ms_mean": mean_ms(&outcome.generate_ms_total, steps),
        "mesh_ms_mean": mean_ms(&outcome.mesh_ms_total, steps),
        "building_ms_mean": mean_ms(&outcome.building_ms_total, steps),
        "generate_ms_total": outcome.generate_ms_total,
        "mesh_ms_total": outcome.mesh_ms_total,
        "building_ms_total": outcome.building_ms_total,
        "step_ms_median": percentile(&outcome.step_samples, 0.50),
        "step_ms_p95": percentile(&outcome.step_samples, 0.95),
        "step_ms_max": max_sample(&outcome.step_samples),
        "elapsed_ms": outcome.total_ms,
        "buildings": config.buildings,
        "building_grid_side": outcome.building_grid_side,
        "building_grid_chunks": AREA_CHUNKS,
        "building_record_bytes": BUILDING_RECORD_BYTES,
        "building_area_m": AREA_METERS,
        "building_area_km2": building_area_km2,
        "building_area_origin_x": area.origin.x,
        "building_area_origin_z": area.origin.z,
        "resident_buildings": outcome.resident_buildings,
        "resident_buildings_peak": outcome.resident_buildings_peak,
        "resident_building_bytes": outcome.resident_building_bytes,
        "building_bytes_peak": outcome.building_bytes_peak,
        "building_bytes_total": outcome.building_bytes_total,
        "total_resident_bytes_peak": outcome.total_resident_bytes_peak,
        "building_entries": outcome.building_entries,
        "building_evictions": outcome.building_evictions,
        "checksum": format!("{:016x}", outcome.checksum),
        "checksum_scope": "route + band assignment + generated chunk heights/biomes + mesh quads + per-step resident/cache counters",
        "building_checksum": format!("{:016x}", outcome.building_checksum),
        "building_checksum_scope": "configured building grid/origin + per-step entry/eviction/resident counters + resident records in chunk and (gz, gx) order",
        "status": outcome.status,
    })
}

fn hash_coord(mut state: u64, coord: WorldChunkCoord) -> u64 {
    state = fnv_u64(state, coord.x as u64);
    fnv_u64(state, coord.z as u64)
}

fn hash_chunk(mut state: u64, coord: WorldChunkCoord, lod_step: usize, chunk: &WorldChunk) -> u64 {
    state = hash_coord(state, coord);
    state = fnv_u64(state, lod_step as u64);
    for &height in chunk.heights() {
        for byte in height.to_le_bytes() {
            state ^= u64::from(byte);
            state = state.wrapping_mul(FNV_PRIME);
        }
    }
    for &biome in chunk.biomes() {
        state ^= biome as u64;
        state = state.wrapping_mul(FNV_PRIME);
    }
    state
}

fn fnv_u64(mut state: u64, value: u64) -> u64 {
    for byte in value.to_le_bytes() {
        state ^= u64::from(byte);
        state = state.wrapping_mul(FNV_PRIME);
    }
    state
}

fn mean_ms(total: &f64, steps: usize) -> f64 {
    if steps == 0 {
        0.0
    } else {
        total / steps as f64
    }
}

fn percentile(samples: &[f64], fraction: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted[index]
}

fn max_sample(samples: &[f64]) -> f64 {
    samples.iter().copied().fold(0.0_f64, f64::max)
}

fn ms(duration: std::time::Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_fixture_covers_ten_km_and_100_km2() {
        let config = Config::default();
        let route = config.route();
        assert_eq!(route.len(), 41);
        assert_eq!(route.first(), Some(&ROUTE_START));
        assert_eq!(route.last(), Some(&ROUTE_START));
        assert_eq!(
            (route.len() - 1) * WORLD_CHUNK_METERS as usize,
            10_240,
            "route must be 10.24 km"
        );
        assert_eq!(AREA_KM2, 100.0);
        assert_eq!(config.resident_cap, RESIDENT_CAP);
    }

    #[test]
    fn route_is_contiguous_and_deterministic() {
        let config = Config::default();
        let first = config.route();
        let second = config.route();
        assert_eq!(first, second);
        assert!(first
            .windows(2)
            .all(|pair| pair[0].distance_chebyshev(pair[1]) == 1));
        assert!(first.iter().all(|coord| coord.z == ROUTE_START.z));
        assert!(first
            .iter()
            .all(|coord| coord.x >= ROUTE_START.x && coord.x <= ROUTE_START.x + 20));
    }

    #[test]
    fn bands_map_distance_to_lod_steps() {
        let config = Config::default();
        assert_eq!(config.band_step(0), NEAR_LOD_STEP);
        assert_eq!(config.band_step(NEAR_RADIUS), NEAR_LOD_STEP);
        assert_eq!(config.band_step(NEAR_RADIUS + 1), MID_LOD_STEP);
        assert_eq!(config.band_step(MID_RADIUS), MID_LOD_STEP);
        assert_eq!(config.band_step(MID_RADIUS + 1), FAR_LOD_STEP);
        assert_eq!(config.band_step(FAR_RADIUS), FAR_LOD_STEP);
    }

    #[test]
    fn smoke_scene_is_bounded_and_reuses_meshes() {
        let outcome = simulate(Config::smoke());
        assert_eq!(outcome.status, "ok");
        assert_eq!(
            outcome.peak_resident,
            ((Config::smoke().far_radius * 2 + 1) as usize).pow(2)
        );
        assert!(outcome.peak_resident <= SMOKE_RESIDENT_CAP);
        assert!(outcome.evictions > 0);
        assert!(
            outcome.cache_hits > 0,
            "the return leg should hit cached meshes"
        );
        assert_eq!(outcome.budget_skipped, 0);
    }

    #[test]
    fn smoke_scene_is_deterministic() {
        let first = simulate(Config::smoke());
        let second = simulate(Config::smoke());
        assert_eq!(first.checksum, second.checksum);
        assert_eq!(first.chunks_visited, second.chunks_visited);
        assert_eq!(first.chunks_generated, second.chunks_generated);
        assert_eq!(first.peak_resident, second.peak_resident);
        assert_eq!(first.mesh_bytes_peak, second.mesh_bytes_peak);
        assert_eq!(first.quads, second.quads);
        assert_eq!(first.triangles, second.triangles);
        assert_eq!(first.evictions, second.evictions);
    }

    #[test]
    fn seed_changes_the_checksum() {
        let first = simulate(Config::smoke());
        let second = simulate(Config {
            seed: SEED ^ 0xFFFF,
            ..Config::smoke()
        });
        assert_ne!(first.checksum, second.checksum);
    }

    #[test]
    fn building_grid_and_record_shape_are_stable() {
        assert_eq!(BUILDING_RECORD_BYTES, 8);
        assert_eq!(grid_side(0), 0);
        assert_eq!(grid_side(25_000), 159);
        assert_eq!(grid_side(250_000), 500);
        let config = Config::default();
        let area = config.area();
        assert_eq!(area.grid_side, 500);
        assert_eq!(config.buildings, 250_000);
        assert_eq!(area.origin.z, ROUTE_START.z - AREA_CHUNKS / 2);
    }

    #[test]
    fn building_chunks_cover_every_record_once_and_in_order() {
        let area = Config::smoke().area();
        let mut total = 0usize;
        let mut sorted: Vec<u64> = Vec::new();
        for gz in 0..AREA_CHUNKS {
            for gx in 0..AREA_CHUNKS {
                let coord = WorldChunkCoord {
                    x: area.origin.x + gx,
                    z: area.origin.z + gz,
                };
                let buildings = area.chunk_buildings(coord);
                total += buildings.len();
                sorted.extend(buildings.iter().map(|record| record.packed()));
                assert!(buildings
                    .windows(2)
                    .all(|pair| { (pair[0].z_m, pair[0].x_m) <= (pair[1].z_m, pair[1].x_m) }));
            }
        }
        assert_eq!(total, 25_000);
        sorted.sort_unstable();
        assert_eq!(sorted.len(), 25_000);
        assert!(area
            .chunk_buildings(WorldChunkCoord {
                x: ROUTE_START.x + AREA_CHUNKS,
                z: ROUTE_START.z,
            })
            .is_empty());
    }

    #[test]
    fn building_records_are_deterministic_and_seed_sensitive() {
        let first = simulate(Config::smoke());
        let second = simulate(Config::smoke());
        assert_eq!(first.building_checksum, second.building_checksum);
        assert_eq!(
            first.resident_buildings_peak,
            second.resident_buildings_peak
        );
        assert_eq!(first.building_bytes_peak, second.building_bytes_peak);
        let other = simulate(Config {
            seed: SEED ^ 0x1234,
            ..Config::smoke()
        });
        assert_ne!(first.building_checksum, other.building_checksum);
    }

    #[test]
    fn zero_buildings_keep_the_terrain_checksum_and_drop_the_building_layer() {
        let base = simulate(Config::smoke());
        let empty = simulate(Config {
            buildings: 0,
            ..Config::smoke()
        });
        assert_eq!(base.checksum, empty.checksum);
        assert_eq!(empty.resident_buildings_peak, 0);
        assert_eq!(empty.building_bytes_peak, 0);
        assert_eq!(empty.building_bytes_total, 0);
        assert_eq!(empty.building_entries, 0);
        assert_eq!(empty.building_evictions, 0);
        assert!(base.resident_buildings > 0);
        assert!(base.resident_buildings <= base.resident_buildings_peak);
        assert!(base.building_entries > 0);
        assert!(base.building_evictions > 0);
        assert_eq!(
            base.resident_buildings_peak * BUILDING_RECORD_BYTES,
            base.building_bytes_peak
        );
    }

    #[test]
    fn building_count_parser_accepts_the_cap_and_rejects_over_it() {
        assert_eq!(parse_building_count("0"), Ok(0));
        assert_eq!(parse_building_count("250000"), Ok(250_000));
        assert!(parse_building_count("250001").is_err());
        assert!(parse_building_count("-1").is_err());
        assert!(parse_building_count("x").is_err());
    }

    #[test]
    fn chunk_budget_cap_is_deterministic() {
        let config = Config {
            chunk_budget_per_step: 1,
            ..Config::smoke()
        };
        let first = simulate(config);
        let second = simulate(config);
        assert_eq!(first.checksum, second.checksum);
        assert_eq!(first.status, "capped");
        assert!(first.budget_skipped > 0);
        assert!(first.peak_resident <= config.resident_cap);
    }
}
